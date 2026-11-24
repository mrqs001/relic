//! A single-owner, owner-only session. Atomic writes happen on Tokio's blocking pool.
use crate::Error;
use fs2::FileExt;
use grammers_session::types::{
    ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState,
};
use grammers_session::{BoxFuture, Session, SessionData};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_SESSION_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct StoredSession {
    version: u32,
    home_dc: i32,
    dc_options: Vec<DcOption>,
    peer_infos: Vec<PeerInfo>,
    updates_state: UpdatesState,
}
impl From<&SessionData> for StoredSession {
    fn from(data: &SessionData) -> Self {
        Self {
            version: 1,
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
            peer_infos: data.peer_infos.values().cloned().collect(),
            updates_state: data.updates_state.clone(),
        }
    }
}

pub(crate) struct PrivateSession {
    data: Arc<Mutex<SessionData>>,
    path: PathBuf,
    writes: Arc<Mutex<()>>,
    _lock: File,
}

fn private_open(path: &Path, create_new: bool) -> Result<File, Error> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(Error::SessionUnavailable);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| Error::SessionUnavailable)?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| Error::SessionUnavailable)?;
    Ok(file)
}

fn save(path: &Path, data: &SessionData) -> Result<(), Error> {
    let bytes = zeroize::Zeroizing::new(
        serde_json::to_vec(&StoredSession::from(data)).map_err(|_| Error::SessionUnavailable)?,
    );
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return Err(Error::SessionUnavailable);
    }
    let temp = path.with_extension("pending");
    // The independent lock guarantees only this owner can remove its stale interrupted write.
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Error::SessionUnavailable),
    }
    let mut file = private_open(&temp, true)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| Error::SessionUnavailable)?;
    fs::rename(&temp, path).map_err(|_| Error::SessionUnavailable)?;
    File::open(path.parent().ok_or(Error::SessionUnavailable)?)
        .and_then(|file| file.sync_all())
        .map_err(|_| Error::SessionUnavailable)
}

impl PrivateSession {
    pub async fn open(path: PathBuf) -> Result<Arc<Self>, Error> {
        tokio::task::spawn_blocking(move || {
            let parent = path.parent().ok_or(Error::Configuration)?;
            if parent.as_os_str().is_empty() {
                return Err(Error::Configuration);
            }
            if fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(Error::SessionUnavailable);
            }
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(parent)
                .map_err(|_| Error::SessionUnavailable)?;
            // Existing parent permissions belong to the caller; session files
            // themselves are always owner-only. Never chmod an arbitrary parent.
            let lock = private_open(&path.with_extension("lock"), false)?;
            lock.try_lock_exclusive().map_err(|_| Error::SessionBusy)?;
            let data = if path.exists() {
                let file = private_open(&path, false)?;
                let length = file
                    .metadata()
                    .map_err(|_| Error::SessionUnavailable)?
                    .len();
                if length == 0 || length > MAX_SESSION_BYTES {
                    return Err(Error::SessionCorrupt);
                }
                let mut bytes = zeroize::Zeroizing::new(Vec::new());
                file.take(MAX_SESSION_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::SessionUnavailable)?;
                let stored: StoredSession =
                    serde_json::from_slice(&bytes).map_err(|_| Error::SessionCorrupt)?;
                if stored.version != 1 {
                    return Err(Error::SessionCorrupt);
                }
                SessionData {
                    home_dc: stored.home_dc,
                    dc_options: stored.dc_options.into_iter().map(|v| (v.id, v)).collect(),
                    peer_infos: stored.peer_infos.into_iter().map(|v| (v.id(), v)).collect(),
                    updates_state: stored.updates_state,
                }
            } else {
                SessionData::default()
            };
            Ok(Arc::new(Self {
                data: Arc::new(Mutex::new(data)),
                path,
                writes: Arc::new(Mutex::new(())),
                _lock: lock,
            }))
        })
        .await
        .map_err(|_| Error::SessionUnavailable)?
    }

    pub fn account_id(&self) -> Option<String> {
        self.data
            .lock()
            .ok()?
            .peer_infos
            .values()
            .find_map(|info| match info {
                PeerInfo::User {
                    id,
                    is_self: Some(true),
                    ..
                } => Some(id.to_string()),
                _ => None,
            })
    }

    fn mutate(
        &self,
        change: impl FnOnce(&mut SessionData) + Send + 'static,
    ) -> BoxFuture<'_, Result<(), Error>> {
        let data = Arc::clone(&self.data);
        let path = self.path.clone();
        let writes = Arc::clone(&self.writes);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _write = writes.lock().map_err(|_| Error::SessionUnavailable)?;
                let current = data.lock().map_err(|_| Error::SessionUnavailable)?;
                let mut candidate = SessionData {
                    home_dc: current.home_dc,
                    dc_options: current.dc_options.clone(),
                    peer_infos: current.peer_infos.clone(),
                    updates_state: current.updates_state.clone(),
                };
                change(&mut candidate);
                let unchanged = candidate.home_dc == current.home_dc
                    && candidate.dc_options == current.dc_options
                    && candidate.peer_infos == current.peer_infos
                    && candidate.updates_state == current.updates_state;
                drop(current);
                if unchanged {
                    return Ok(());
                }
                // Disk IO stays serialized, but synchronous session readers only
                // lock the last successfully persisted state for a short copy.
                save(&path, &candidate)?;
                *data.lock().map_err(|_| Error::SessionUnavailable)? = candidate;
                Ok(())
            })
            .await
            .map_err(|_| Error::SessionUnavailable)?
        })
    }
}

impl Session for PrivateSession {
    type Error = Error;
    fn home_dc_id(&self) -> Result<i32, Error> {
        Ok(self
            .data
            .lock()
            .map_err(|_| Error::SessionUnavailable)?
            .home_dc)
    }
    fn set_home_dc_id(&self, id: i32) -> BoxFuture<'_, Result<(), Error>> {
        self.mutate(move |data| data.home_dc = id)
    }
    fn dc_option(&self, id: i32) -> Result<Option<DcOption>, Error> {
        Ok(self
            .data
            .lock()
            .map_err(|_| Error::SessionUnavailable)?
            .dc_options
            .get(&id)
            .cloned())
    }
    fn set_dc_option(&self, value: &DcOption) -> BoxFuture<'_, Result<(), Error>> {
        let value = value.clone();
        self.mutate(move |data| {
            data.dc_options.insert(value.id, value);
        })
    }
    fn peer(&self, id: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Error>> {
        Box::pin(async move {
            let data = self.data.lock().map_err(|_| Error::SessionUnavailable)?;
            if id == PeerId::self_user() {
                Ok(data
                    .peer_infos
                    .values()
                    .find(|peer| {
                        matches!(
                            peer,
                            PeerInfo::User {
                                is_self: Some(true),
                                ..
                            }
                        )
                    })
                    .cloned())
            } else {
                Ok(data.peer_infos.get(&id).cloned())
            }
        })
    }
    fn cache_peer(&self, peer: &PeerInfo) -> BoxFuture<'_, Result<(), Error>> {
        let unchanged = self
            .data
            .lock()
            .ok()
            .and_then(|data| data.peer_infos.get(&peer.id()).cloned())
            .is_some_and(|mut old| {
                let before = old.clone();
                old.extend_info(peer);
                old == before
            });
        if unchanged {
            return Box::pin(async { Ok(()) });
        }
        let peer = peer.clone();
        self.mutate(move |data| {
            data.peer_infos
                .entry(peer.id())
                .and_modify(|v| {
                    v.extend_info(&peer);
                })
                .or_insert(peer);
        })
    }
    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Error>> {
        Box::pin(async move {
            Ok(self
                .data
                .lock()
                .map_err(|_| Error::SessionUnavailable)?
                .updates_state
                .clone())
        })
    }
    fn set_update_state(&self, state: UpdateState) -> BoxFuture<'_, Result<(), Error>> {
        self.mutate(move |data| match state {
            UpdateState::All(state) => data.updates_state = state,
            UpdateState::Primary { pts, date, seq } => {
                data.updates_state.pts = pts;
                data.updates_state.date = date;
                data.updates_state.seq = seq;
            }
            UpdateState::Secondary { qts } => data.updates_state.qts = qts,
            UpdateState::Channel { id, pts } => {
                data.updates_state.channels.retain(|v| v.id != id);
                data.updates_state.channels.push(ChannelState { id, pts });
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn atomic_private_session_retains_state_and_enforces_one_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private/relic.session");
        let session = PrivateSession::open(path.clone()).await.unwrap();
        assert!(matches!(
            PrivateSession::open(path.clone()).await,
            Err(Error::SessionBusy)
        ));
        session
            .set_update_state(UpdateState::Primary {
                pts: 41,
                date: 12,
                seq: 9,
            })
            .await
            .unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(session);
        assert_eq!(
            PrivateSession::open(path)
                .await
                .unwrap()
                .updates_state()
                .await
                .unwrap()
                .pts,
            41
        );
    }
    #[tokio::test]
    async fn corrupt_session_is_never_silently_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("relic.session");
        fs::write(&path, b"not a session").unwrap();
        assert!(matches!(
            PrivateSession::open(path.clone()).await,
            Err(Error::SessionCorrupt)
        ));
        assert_eq!(fs::read(path).unwrap(), b"not a session");
    }

    #[tokio::test]
    async fn failed_write_does_not_advance_memory_or_durable_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("relic.session");
        let session = PrivateSession::open(path.clone()).await.unwrap();
        session
            .set_update_state(UpdateState::Primary {
                pts: 10,
                date: 10,
                seq: 1,
            })
            .await
            .unwrap();
        fs::create_dir(path.with_extension("pending")).unwrap();
        assert!(
            session
                .set_update_state(UpdateState::Primary {
                    pts: 99,
                    date: 99,
                    seq: 9
                })
                .await
                .is_err()
        );
        assert_eq!(session.updates_state().await.unwrap().pts, 10);
        drop(session);
        assert_eq!(
            PrivateSession::open(path)
                .await
                .unwrap()
                .updates_state()
                .await
                .unwrap()
                .pts,
            10
        );
    }

    #[tokio::test]
    async fn self_peer_lookup_uses_the_authenticated_numeric_identity() {
        let directory = tempfile::tempdir().unwrap();
        let session = PrivateSession::open(directory.path().join("relic.session"))
            .await
            .unwrap();
        session
            .cache_peer(&PeerInfo::User {
                id: 1234,
                auth: None,
                bot: Some(false),
                is_self: Some(true),
            })
            .await
            .unwrap();
        assert_eq!(session.account_id().as_deref(), Some("1234"));
        assert!(matches!(
            session.peer(PeerId::self_user()).await.unwrap(),
            Some(PeerInfo::User {
                id: 1234,
                is_self: Some(true),
                ..
            })
        ));
    }
}
