use crate::{session::PrivateSession, *};
use chrono::{SecondsFormat, Utc};
use grammers_client::{
    Client, SignInError,
    client::{ClientConfiguration, LoginToken, NoRetries, PasswordToken, UpdatesConfiguration},
    peer::Peer,
    update::Update,
};
use grammers_mtsender::{InvocationError, SenderPool, SenderPoolHandle};
use grammers_session::{
    Session,
    types::{PeerId, PeerKind, PeerRef, UpdateState},
    updates::{MessageBox, State, UpdatesLike},
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

const QUEUE_LIMIT: usize = 128;

struct Connection {
    client: Client,
    handle: SenderPoolHandle,
    runner: JoinHandle<()>,
    worker: Option<JoinHandle<()>>,
    receiver: Option<mpsc::UnboundedReceiver<UpdatesLike>>,
    updates: Option<mpsc::Receiver<Result<Update, Error>>>,
    overflow: Arc<AtomicBool>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.handle.quit();
        self.runner.abort();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

/// One active session owner. Methods are deliberately exclusive for account-wide pacing.
pub struct TelegramClient {
    config: Config,
    session: Arc<PrivateSession>,
    connection: Option<Connection>,
    login: Option<LoginToken>,
    password: Option<PasswordToken>,
    state: AuthState,
    sequence: u64,
    pending: Option<(u64, State)>,
}

fn rpc_error(error: InvocationError) -> Error {
    match error {
        InvocationError::Rpc(error) => match error.name.as_str() {
            "FLOOD_WAIT" | "FLOOD_PREMIUM_WAIT" | "SLOWMODE_WAIT" => {
                Error::FloodWait(error.value.unwrap_or(60))
            }
            "PHONE_NUMBER_INVALID" | "PHONE_NUMBER_BANNED" => Error::InvalidPhone,
            "PHONE_CODE_INVALID" | "PHONE_CODE_EMPTY" => Error::InvalidCode,
            "PHONE_CODE_EXPIRED" | "PHONE_CODE_HASH_EMPTY" => Error::CodeExpired,
            "PASSWORD_HASH_INVALID" => Error::InvalidPassword,
            "CHANNEL_PRIVATE"
            | "CHAT_ADMIN_REQUIRED"
            | "USER_BANNED_IN_CHANNEL"
            | "CHANNEL_INVALID" => Error::AccessDenied,
            "USERNAME_INVALID" | "USERNAME_NOT_OCCUPIED" => Error::InvalidTarget,
            _ if error.code == 401 => Error::AuthenticationRequired,
            _ if error.code == 403 => Error::AccessDenied,
            _ if error.code == 420 => Error::FloodWait(error.value.unwrap_or(60)),
            _ => Error::Upstream,
        },
        InvocationError::Io(_) | InvocationError::Dropped => Error::Network,
        _ => Error::Upstream,
    }
}
async fn bounded<T>(
    duration: Duration,
    future: impl Future<Output = Result<T, InvocationError>>,
) -> Result<T, Error> {
    timeout(duration, future)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(rpc_error)
}
fn public_handle(target: &str) -> Result<String, Error> {
    let target = target.trim();
    let name = target
        .strip_prefix("https://t.me/")
        .unwrap_or(target)
        .trim_start_matches('@');
    if !(5..=32).contains(&name.len())
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_')
    {
        return Err(Error::InvalidTarget);
    }
    Ok(name.to_ascii_lowercase())
}

impl TelegramClient {
    /// Opens and locks the private session without DNS, sockets or upstream validation.
    pub async fn open(config: Config) -> Result<Self, Error> {
        if config.api_id <= 0
            || config.api_hash.len() != 32
            || !config.api_hash.bytes().all(|v| v.is_ascii_hexdigit())
            || !(Duration::from_secs(1)..=Duration::from_secs(60)).contains(&config.request_timeout)
        {
            return Err(Error::Configuration);
        }
        let session = PrivateSession::open(config.session_path.clone()).await?;
        Ok(Self {
            config,
            session,
            connection: None,
            login: None,
            password: None,
            state: AuthState::Disconnected,
            sequence: 0,
            pending: None,
        })
    }
    pub fn status(&self) -> Status {
        let account_id = self.session.account_id();
        Status {
            state: self.state.clone(),
            session_authorized: account_id.is_some(),
            account_id,
        }
    }
    fn start_transport(&mut self) {
        if self.connection.is_some() {
            return;
        }
        let SenderPool {
            runner,
            handle,
            updates,
        } = SenderPool::new(Arc::clone(&self.session), self.config.api_id);
        let client = Client::with_configuration(
            handle.clone(),
            ClientConfiguration {
                retry_policy: Box::new(NoRetries),
                auto_cache_peers: true,
            },
        );
        let runner = tokio::spawn(runner.run());
        self.connection = Some(Connection {
            client,
            handle: handle.thin,
            runner,
            worker: None,
            receiver: Some(updates),
            updates: None,
            overflow: Arc::new(AtomicBool::new(false)),
        });
    }
    async fn start_updates(&mut self) -> Result<(), Error> {
        let connection = self.connection.as_mut().ok_or(Error::NotConnected)?;
        if connection.updates.is_some() {
            return Ok(());
        }
        let receiver = connection.receiver.take().ok_or(Error::NotConnected)?;
        let mut stream = connection
            .client
            .stream_updates(
                receiver,
                UpdatesConfiguration {
                    catch_up: true,
                    update_queue_limit: None,
                },
            )
            .await
            .map_err(|_| Error::SessionUnavailable)?;
        let (sender, receiver) = mpsc::channel(QUEUE_LIMIT);
        let overflow = Arc::clone(&connection.overflow);
        let handle = connection.handle.clone();
        connection.worker = Some(tokio::spawn(async move {
            loop {
                let result = stream.next().await.map_err(rpc_error);
                let failed = result.is_err();
                match sender.try_send(result) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        overflow.store(true, Ordering::Release);
                        handle.quit();
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
                if failed {
                    handle.quit();
                    break;
                }
            }
        }));
        connection.updates = Some(receiver);
        Ok(())
    }

    fn client(&self) -> Result<&Client, Error> {
        let connection = self.connection.as_ref().ok_or(Error::NotConnected)?;
        if connection.overflow.load(Ordering::Acquire) {
            return Err(Error::UpdateOverflow);
        }
        Ok(&connection.client)
    }
    fn account_id(&self) -> Result<String, Error> {
        if self.state != AuthState::Connected {
            return Err(Error::AuthenticationRequired);
        }
        self.session
            .account_id()
            .ok_or(Error::AuthenticationRequired)
    }
    pub async fn connect(&mut self) -> Result<Status, Error> {
        self.start_transport();
        if bounded(self.config.request_timeout, self.client()?.is_authorized()).await? {
            self.state = AuthState::Connected;
            self.start_updates().await?;
            Ok(self.status())
        } else {
            self.state = AuthState::Disconnected;
            Err(Error::AuthenticationRequired)
        }
    }
    pub async fn request_code(&mut self, phone: &str) -> Result<Status, Error> {
        if !(8..=17).contains(&phone.len())
            || !phone.starts_with('+')
            || !phone[1..].bytes().all(|v| v.is_ascii_digit())
        {
            return Err(Error::InvalidPhone);
        }
        self.start_transport();
        let token = bounded(
            self.config.request_timeout,
            self.client()?
                .request_login_code(phone, &self.config.api_hash),
        )
        .await?;
        self.login = Some(token);
        self.password = None;
        self.state = AuthState::CodeRequired;
        Ok(self.status())
    }
    pub async fn submit_code(&mut self, code: &str) -> Result<Status, Error> {
        if !(3..=12).contains(&code.len()) || !code.bytes().all(|v| v.is_ascii_digit()) {
            return Err(Error::InvalidCode);
        }
        let token = self.login.as_ref().ok_or(Error::CodeExpired)?;
        let result = timeout(
            self.config.request_timeout,
            self.client()?.sign_in(token, code),
        )
        .await
        .map_err(|_| Error::Timeout)?;
        match result {
            Ok(_) => {
                self.login = None;
                self.state = AuthState::Connected;
                self.start_updates().await?;
                Ok(self.status())
            }
            Err(SignInError::PasswordRequired(token)) => {
                self.login = None;
                self.password = Some(token);
                self.state = AuthState::PasswordRequired;
                Ok(self.status())
            }
            Err(SignInError::InvalidCode) => Err(Error::InvalidCode),
            Err(SignInError::SignUpRequired) => Err(Error::SignUpRequired),
            Err(SignInError::Other(error)) => Err(rpc_error(error)),
            Err(_) => Err(Error::Upstream),
        }
    }
    pub async fn submit_password(&mut self, password: &str) -> Result<Status, Error> {
        if password.is_empty() || password.len() > 1024 {
            return Err(Error::InvalidPassword);
        }
        let token = self.password.take().ok_or(Error::AuthenticationRequired)?;
        let secret = zeroize::Zeroizing::new(password.as_bytes().to_vec());
        let result = timeout(
            self.config.request_timeout,
            self.client()?.check_password(token, &*secret),
        )
        .await
        .map_err(|_| Error::Timeout)?;
        match result {
            Ok(_) => {
                self.state = AuthState::Connected;
                self.start_updates().await?;
                Ok(self.status())
            }
            Err(SignInError::InvalidPassword(token)) => {
                self.password = Some(token);
                Err(Error::InvalidPassword)
            }
            Err(SignInError::Other(error)) => Err(rpc_error(error)),
            Err(_) => Err(Error::Upstream),
        }
    }
    pub fn disconnect(&mut self) {
        self.connection = None;
        self.login = None;
        self.password = None;
        self.pending = None;
        self.state = AuthState::Disconnected;
    }

    pub async fn resolve_channel(&mut self, target: &str) -> Result<ChannelIdentity, Error> {
        self.account_id()?;
        let handle = public_handle(target)?;
        let peer = bounded(
            self.config.request_timeout,
            self.client()?.resolve_username(&handle),
        )
        .await?
        .ok_or(Error::PeerUnavailable)?;
        let kind = match &peer {
            Peer::User(_) => return Err(Error::UnsupportedPeer),
            Peer::Group(_) => "group",
            Peer::Channel(_) => "channel",
        };
        Ok(ChannelIdentity {
            peer_id: peer.id().bot_api_dialog_id_unchecked().to_string(),
            peer_type: kind.into(),
            title: peer.name().unwrap_or(&handle).into(),
            username: peer.username().map(str::to_owned),
        })
    }
    async fn peer(&self, peer_id: &str) -> Result<PeerRef, Error> {
        self.account_id()?;
        let id = peer_id
            .parse::<i64>()
            .ok()
            .and_then(PeerId::from_bot_api_dialog_id)
            .ok_or(Error::InvalidTarget)?;
        if id.kind() == PeerKind::User {
            return Err(Error::UnsupportedPeer);
        }
        self.session
            .peer_ref(id)
            .await?
            .ok_or(Error::PeerUnavailable)
    }
    pub async fn history(
        &mut self,
        peer_id: &str,
        after_id: i32,
        before_id: Option<i32>,
        limit: usize,
    ) -> Result<HistoryPage, Error> {
        if !(1..=100).contains(&limit) || after_id < 0 || before_id.is_some_and(|v| v <= after_id) {
            return Err(Error::InvalidBounds);
        }
        let peer = self.peer(peer_id).await?;
        let account = self.account_id()?;
        let client = self.client()?;
        let mut iterator = client
            .iter_messages(peer)
            .offset_id(before_id.unwrap_or(0))
            .limit(limit);
        let timeout_duration = self.config.request_timeout;
        timeout(timeout_duration, async {
            let mut messages = Vec::new();
            let mut drained = false;
            let mut last = None;
            for _ in 0..limit {
                let Some(message) = iterator.next().await.map_err(rpc_error)? else {
                    drained = true;
                    break;
                };
                if message.id() <= after_id {
                    drained = true;
                    break;
                }
                if last.is_some_and(|id| message.id() >= id)
                    || before_id.is_some_and(|id| message.id() >= id)
                {
                    return Err(Error::Upstream);
                }
                last = Some(message.id());
                messages.push(normalize_message(&account, &message)?);
            }
            let high = messages.first().and_then(|m| m.message_id.parse().ok());
            Ok(HistoryPage {
                messages,
                resume_before: if drained { None } else { last },
                interval_drained: drained,
                candidate_high_water: high,
            })
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
    pub async fn exact_messages(
        &mut self,
        peer_id: &str,
        ids: &[i32],
    ) -> Result<ExactMessages, Error> {
        if ids.is_empty() || ids.len() > 100 || ids.iter().any(|id| *id <= 0) {
            return Err(Error::InvalidBounds);
        }
        let peer = self.peer(peer_id).await?;
        let account = self.account_id()?;
        let found = bounded(
            self.config.request_timeout,
            self.client()?.get_messages_by_id(peer, ids),
        )
        .await?;
        let mut messages = Vec::new();
        let mut unavailable_ids = Vec::new();
        for (id, message) in ids.iter().zip(found) {
            match message {
                Some(message)
                    if !matches!(&message.raw, grammers_tl_types::enums::Message::Empty(_)) =>
                {
                    messages.push(normalize_message(&account, &message)?)
                }
                _ => unavailable_ids.push(id.to_string()),
            }
        }
        Ok(ExactMessages {
            messages,
            unavailable_ids,
        })
    }
    pub async fn next_update(&mut self, wait: Duration) -> Result<Option<UpdateDelivery>, Error> {
        if wait > Duration::from_secs(30) {
            return Err(Error::InvalidBounds);
        }
        if self.pending.is_some() {
            return Err(Error::PendingAcknowledgement);
        }
        let account = self.account_id()?;
        self.client()?;
        self.start_updates().await?;
        let connection = self.connection.as_mut().ok_or(Error::NotConnected)?;
        let update = match timeout(wait, connection.updates.as_mut().unwrap().recv()).await {
            Ok(Some(result)) => result?,
            Ok(None) => return Err(Error::Network),
            Err(_) => return Ok(None),
        };
        if connection.overflow.load(Ordering::Acquire) {
            return Err(Error::UpdateOverflow);
        }
        let state = update.state().clone();
        let event = match update {
            Update::NewMessage(message) if message.peer_id().kind() != PeerKind::User => {
                MessageEvent::New {
                    message: normalize_message(&account, &message)?,
                }
            }
            Update::MessageEdited(message) if message.peer_id().kind() != PeerKind::User => {
                MessageEvent::Edited {
                    message: normalize_message(&account, &message)?,
                }
            }
            Update::MessageDeleted(deleted) => match deleted.channel_id() {
                Some(channel) => MessageEvent::Deleted {
                    account_id: account,
                    peer_id: PeerId::channel_unchecked(channel)
                        .bot_api_dialog_id_unchecked()
                        .to_string(),
                    message_ids: deleted.messages().iter().map(ToString::to_string).collect(),
                    acquired_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                },
                None => MessageEvent::UnscopedDeletion {
                    message_ids: deleted.messages().iter().map(ToString::to_string).collect(),
                },
            },
            _ => MessageEvent::Ignored,
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::InvalidAcknowledgement)?;
        self.pending = Some((self.sequence, state));
        Ok(Some(UpdateDelivery {
            sequence: self.sequence,
            event,
        }))
    }
    /// Call only after the delivered event is durably handled. Buffered later updates are excluded.
    pub async fn acknowledge_update(&mut self, sequence: u64) -> Result<(), Error> {
        let (expected, state) = self.pending.as_ref().ok_or(Error::InvalidAcknowledgement)?;
        if *expected != sequence {
            return Err(Error::InvalidAcknowledgement);
        }
        let mut stored = self.session.updates_state().await?;
        stored.date = state.date;
        stored.seq = state.seq;
        match state.message_box {
            Some(MessageBox::Common { pts }) => stored.pts = pts,
            Some(MessageBox::Secondary { qts }) => stored.qts = qts,
            Some(MessageBox::Channel { channel_id, pts }) => {
                stored.channels.retain(|c| c.id != channel_id);
                stored.channels.push(grammers_session::types::ChannelState {
                    id: channel_id,
                    pts,
                });
            }
            None => {}
        }
        self.session
            .set_update_state(UpdateState::All(stored))
            .await?;
        self.pending = None;
        Ok(())
    }
    /// Resolves the exact message again to refresh temporary file references. Caller publishes
    /// bytes atomically into its content-addressed store after validating the received media type.
    pub async fn media(
        &mut self,
        peer_id: &str,
        message_id: i32,
        max_bytes: usize,
    ) -> Result<MediaBytes, Error> {
        if message_id <= 0 || max_bytes == 0 || max_bytes > 32 * 1024 * 1024 {
            return Err(Error::InvalidBounds);
        }
        let mut bytes = Vec::new();
        let receipt = timeout(
            self.config.request_timeout,
            self.media_to(peer_id, message_id, max_bytes, &mut bytes),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        Ok(MediaBytes {
            bytes,
            sha256: receipt.sha256,
            mime_type: receipt.mime_type,
            origin: receipt.origin,
        })
    }

    /// One exact-message download, streamed with backpressure. Dropping this
    /// future stops the download; the caller owns removal of incomplete staging.
    pub async fn media_to<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        peer_id: &str,
        message_id: i32,
        max_bytes: usize,
        sink: &mut W,
    ) -> Result<MediaDownload, Error> {
        self.media_to_with_progress(peer_id, message_id, max_bytes, sink, |_, _| {})
            .await
    }

    /// Progress reports successfully written bytes and the freshly resolved
    /// source length, when known. It never implies validation or publication.
    pub async fn media_to_with_progress<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        peer_id: &str,
        message_id: i32,
        max_bytes: usize,
        sink: &mut W,
        mut progress: impl FnMut(usize, Option<usize>),
    ) -> Result<MediaDownload, Error> {
        use tokio::io::AsyncWriteExt;
        if message_id <= 0 || max_bytes == 0 || max_bytes > MAX_STREAM_MEDIA_BYTES {
            return Err(Error::InvalidBounds);
        }
        let peer = self.peer(peer_id).await?;
        let account = self.account_id()?;
        let client = self.client()?;
        let chunk_deadline = self.config.request_timeout;
        timeout(Duration::from_secs(600), async {
            let found = timeout(
                chunk_deadline,
                client.get_messages_by_id(peer, &[message_id]),
            )
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(rpc_error)?;
            let message = found
                .into_iter()
                .next()
                .flatten()
                .ok_or(Error::MediaUnavailable)?;
            let origin = normalize_message(&account, &message)?;
            let media = message.media().ok_or(Error::MediaUnavailable)?;
            if !matches!(
                &media,
                grammers_client::media::Media::Photo(_)
                    | grammers_client::media::Media::Document(_)
                    | grammers_client::media::Media::Sticker(_)
            ) {
                return Err(Error::UnsupportedMedia);
            }
            if media.size().is_some_and(|size| size > max_bytes) {
                return Err(Error::MediaTooLarge);
            }
            let mut iterator = client.iter_download(&media).chunk_size(512 * 1024);
            let mut receipt = DownloadSink::new(sink, max_bytes);
            progress(0, media.size());
            while let Some(chunk) = timeout(chunk_deadline, iterator.next())
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(rpc_error)?
            {
                receipt.write(&chunk).await?;
                progress(receipt.bytes, media.size());
            }
            if receipt.bytes == 0 || media.size().is_some_and(|size| size != receipt.bytes) {
                return Err(Error::MediaUnavailable);
            }
            receipt
                .sink
                .flush()
                .await
                .map_err(|_| Error::MediaStorage)?;
            Ok(MediaDownload {
                bytes: receipt.bytes,
                sha256: format!("{:x}", receipt.hash.finalize()),
                prefix: receipt.prefix,
                mime_type: origin
                    .media
                    .as_ref()
                    .and_then(|media| media.mime_type.clone()),
                origin,
            })
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

struct DownloadSink<'a, W> {
    sink: &'a mut W,
    max: usize,
    bytes: usize,
    hash: Sha256,
    prefix: Vec<u8>,
}
impl<'a, W: tokio::io::AsyncWrite + Unpin> DownloadSink<'a, W> {
    fn new(sink: &'a mut W, max: usize) -> Self {
        Self {
            sink,
            max,
            bytes: 0,
            hash: Sha256::new(),
            prefix: Vec::new(),
        }
    }
    async fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        use tokio::io::AsyncWriteExt;
        if chunk.len() > self.max.saturating_sub(self.bytes) {
            return Err(Error::MediaTooLarge);
        }
        self.sink
            .write_all(chunk)
            .await
            .map_err(|_| Error::MediaStorage)?;
        self.prefix.extend_from_slice(
            &chunk[..chunk.len().min(64usize.saturating_sub(self.prefix.len()))],
        );
        self.hash.update(chunk);
        self.bytes += chunk.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn streaming_media_enforces_actual_bytes_and_backpressure() {
        let mut output = Vec::new();
        let mut sink = DownloadSink::new(&mut output, 10);
        sink.write(b"12345678").await.unwrap();
        assert_eq!(sink.write(b"abc").await.unwrap_err(), Error::MediaTooLarge);
        sink.write(b"90").await.unwrap();
        assert_eq!(sink.bytes, 10);
        assert_eq!(sink.prefix, b"1234567890");
        assert_eq!(
            format!("{:x}", sink.hash.finalize()),
            format!("{:x}", Sha256::digest(b"1234567890"))
        );
        assert_eq!(output, b"1234567890");
        let (mut writer, reader) = tokio::io::duplex(8);
        let mut sink = DownloadSink::new(&mut writer, 100);
        assert!(
            timeout(Duration::from_millis(20), sink.write(&[1; 20]))
                .await
                .is_err()
        );
        drop(reader);
        assert_eq!(sink.write(b"1").await.unwrap_err(), Error::MediaStorage);
    }
    #[test]
    fn target_validation_never_accepts_private_invites_or_user_content_urls() {
        assert_eq!(
            public_handle("https://t.me/Fictional_Channel").unwrap(),
            "fictional_channel"
        );
        for target in [
            "https://t.me/+secret",
            "https://t.me/channel/42",
            "https://t.me/channel?token=secret",
            "@x",
            "../private",
        ] {
            assert!(public_handle(target).is_err());
        }
    }
    #[test]
    fn rpc_errors_are_redacted_and_flood_wait_is_account_scoped() {
        let error = grammers_mtsender::RpcError {
            code: 420,
            name: "FLOOD_WAIT".into(),
            value: Some(432),
            caused_by: None,
        };
        assert_eq!(
            rpc_error(InvocationError::Rpc(error)),
            Error::FloodWait(432)
        );
        let error = grammers_mtsender::RpcError {
            code: 400,
            name: "secret phone or payload".into(),
            value: None,
            caused_by: None,
        };
        assert_eq!(
            rpc_error(InvocationError::Rpc(error)).to_string(),
            "Telegram rejected the request"
        );
    }
    #[tokio::test]
    async fn session_open_and_status_need_no_network() {
        let directory = tempfile::tempdir().unwrap();
        let client = TelegramClient::open(Config {
            api_id: 1,
            api_hash: "00000000000000000000000000000000".to_owned().into(),
            session_path: directory.path().join("relic.session"),
            request_timeout: Duration::from_secs(1),
        })
        .await
        .unwrap();
        assert_eq!(client.status().state, AuthState::Disconnected);
        assert!(!client.status().session_authorized);
        assert!(client.connection.is_none());
    }

    #[tokio::test]
    async fn acknowledgement_is_ordered_and_preserves_other_channel_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut client = TelegramClient::open(Config {
            api_id: 1,
            api_hash: "00000000000000000000000000000000".to_owned().into(),
            session_path: directory.path().join("relic.session"),
            request_timeout: Duration::from_secs(1),
        })
        .await
        .unwrap();
        client
            .session
            .set_update_state(UpdateState::Channel { id: 10, pts: 7 })
            .await
            .unwrap();
        client
            .session
            .set_update_state(UpdateState::Channel { id: 20, pts: 90 })
            .await
            .unwrap();
        client.pending = Some((
            5,
            State {
                date: 100,
                seq: 4,
                message_box: Some(MessageBox::Channel {
                    channel_id: 10,
                    pts: 8,
                }),
            },
        ));
        assert_eq!(
            client.acknowledge_update(6).await,
            Err(Error::InvalidAcknowledgement)
        );
        assert_eq!(
            client
                .session
                .updates_state()
                .await
                .unwrap()
                .channels
                .iter()
                .find(|c| c.id == 10)
                .unwrap()
                .pts,
            7
        );
        client.acknowledge_update(5).await.unwrap();
        let state = client.session.updates_state().await.unwrap();
        assert_eq!(state.channels.iter().find(|c| c.id == 10).unwrap().pts, 8);
        assert_eq!(state.channels.iter().find(|c| c.id == 20).unwrap().pts, 90);
        assert_eq!(
            client.acknowledge_update(5).await,
            Err(Error::InvalidAcknowledgement)
        );
    }
}
