# Relic native Telegram adapter

This independent Rust crate uses the maintained [grammers client](https://docs.rs/grammers-client/0.10.0/grammers_client/) for Telegram user-account MTProto. It never loads `.env`, reads another repository, imports an old session, joins a channel, sends a message, or starts a backfill by itself. Application scheduling, configured channel selection, durable evidence storage, account-wide backoff and source eligibility belong to the Relic application.

`cargo test --manifest-path integrations/telegram/Cargo.toml` performs offline tests and compiles the actual MTProto APIs. No test contacts Telegram. Dependencies are pinned to grammers 0.10.0. `glass_pumpkin =2.0.0-rc0` is pinned because its rc1 release changes the prime-checking API incompatibly with grammers 0.10.0.

## Connection and authentication

`TelegramClient::open(Config)` locks a newly selected private session path but performs no DNS or network request. `status()` only reads local state: `session_authorized` means authorization was retained, not that Telegram currently accepts it. Config contains API ID/hash, an explicit session file path and a 1–60 second request timeout. Config, session and login tokens do not implement Debug or Serialize.

- `connect()` explicitly starts networking and checks the retained authorization.
- `request_code(phone)` accepts an international `+` phone number and returns `code_required`.
- `submit_code(code)` returns `connected` or `password_required`.
- `submit_password(password)` completes optional two-factor authentication.
- `disconnect()` stops the connection and forgets ephemeral login state without revoking the owner's upstream session. Reopen/connect uses the same private authorization.

Codes/passwords are not persisted. Password copies are zeroized. The private session is an atomic versioned file, limited to 16 MiB, written on Tokio's blocking pool, with `0600` file and `0700` containing-directory permissions on Unix. A separate exclusive lock prevents two owners; malformed sessions fail without replacement. It contains private authorization keys, peer access hashes and acknowledged Telegram update state. Do not export it with evidence, include it in ordinary backups, or enable dependency wire/trace logging: third-party debug logs may contain request fields. No logger is installed by this crate.

## Evidence contract

`resolve_channel(handle)` accepts a public handle or exact `https://t.me/handle`, returning signed decimal `peer_id`, type, title and optional current username. Numeric peer identity persists independently of handles; access hashes stay in the private session. User/private one-to-one peers are rejected. Resolving a public channel does not join it. Accounts that are not members may require periodic history catch-up instead of passive updates.

`history(peer_id, after_id, before_id, limit)` requests at most 100 records in a bounded interval. IDs are ordered newest to oldest. Continue with `resume_before` until `interval_drained`; accumulate `candidate_high_water` across pages. Only commit the candidate after the entire interval and its evidence are durably acquired. An exactly full final page needs another request to establish exhaustion. First import/backfill positions are separate application policies. Cancellation, timeout or an error never returns a successful empty page or changes the application's watermark.

`exact_messages(peer_id, ids)` accepts 1–100 known IDs for paced revision sweeps. `unavailable_ids` comes only from successful authorized lookup. Access errors do not become deletions. Message identity is `(telegram, account_id, peer_id, message_id)`; IDs are decimal strings at the serialization boundary. Source time, edit time and acquisition time are separate. Native coordinates are retained when present, never inferred by the adapter.

Messages retain text (including empty media-only content), sender, reply/forward relationships, grouped media identity, media references and separate volatile counters. `raw` is a retained, redacted TL JSON representation: access hashes, expiring file references and authorization fields are removed recursively. It is explicitly not a byte-exact Telegram wire packet. Formatting entities, message actions and content fields remain in that representation. The content fingerprint excludes authorization/file references, counts, reactions and view state; counter-only changes do not create content revisions. A present media reference starts as `not_requested`, not archived.

## Update delivery and bounds

After authentication one background worker receives new, edited and deleted messages into a bounded queue of 128 pending deliveries. On overflow it stops the connection and returns `UpdateOverflow`; the application must mark collection partial and reconnect from acknowledged state. The worker preserves the library's current response batch rather than using grammers' lossy internal queue. That transient native response and the transport's in-flight receive buffers are additional to the 128-delivery bound; sustained end-to-end heap/frame measurements remain an application acceptance gate.

`next_update(wait)` is cancellation-safe at the application queue and accepts a wait up to 30 seconds. It yields one sequence/event and requires `acknowledge_update(sequence)` before returning the next. The caller persists the event first; acknowledgement saves only that event's state, never the library's ahead-of-consumption buffer state. Disconnect/crash therefore replays unacknowledged data. Ordinary private conversations become `ignored`. A deletion without a channel identity becomes `unscoped_deletion` and must never be applied across channels. The application selects enabled registered peers and records its handling before acknowledging every event, including ignored events. Flood-wait errors contain seconds and must block the whole account, not just the current channel.

Use live updates, periodic above-watermark catch-up and paced exact-ID sweeps together. A connected socket alone cannot prove historical completeness, coverage of channels outside account membership, or absence of missed edits/deletions. No live account compatibility check has been performed for this crate.

## Media

`media(peer_id, message_id, max_bytes)` resolves the exact message again, refreshing temporary file references, and downloads supported photo/document/sticker bytes with a strict maximum of 32 MiB and the original request deadline. It returns bytes, checksum, declared MIME type and the originating normalized revision. The application validates/sniffs media type and atomically publishes a content-addressed object before exposing a Relic-controlled URL. A failed media request leaves retained text evidence readable. Unsupported media remains explicit; no browser URL or provider embed is emitted.

`media_to(peer_id, message_id, max_bytes, sink)` is the larger streaming path: up to 512 MiB, 512 KiB transport chunks, sink backpressure, a ten-minute total deadline and the configured timeout per message/chunk request. It returns byte count, SHA-256, a 64-byte signature prefix, declared MIME and exact origin. Known length and actual received bytes are both checked. Dropping the future cancels; the caller owns cleanup of incomplete files. Relic uses private RAII staging files and validates the full retained file before publishing. Offline sink tests cover byte overflow, checksum, write failure and cancellation under backpressure.

`media_to_with_progress(..., sink, callback)` preserves that contract and adds a synchronous `(written_bytes, fresh_total: Option<usize>)` callback before the first chunk and after each successful sink write. An absent total is unknown, not zero. The compatibility `media_to` uses a no-op callback. Relic coalesces updates into one bounded slot and cancels the owning HTTP response when its media reader closes; no timer or source schedule retries a cancelled attachment.

Review update (22 September 2026): unchanged peer cache updates avoid persistence;
changed updates serialize disk writes separately from the short in-memory reader
lock and publish memory only after durable success. Existing parent permissions
are not changed; newly created session directories use 0700 and files use 0600.
Shared-contact message phone numbers remain evidence; authentication/account
phone fields outside Contact media are still removed.
