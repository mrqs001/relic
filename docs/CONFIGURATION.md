# Configuration

Relic reads one dotenv file, given explicitly: `./relic --env .env`. Copy
[`.env.example`](../.env.example) to start. Without `--env`, Relic reads no
credentials and makes no source or model requests, which is how the demo and
the test suites run.

Values are data, never shell code. Relative paths resolve from the file's own
directory. Credentials stay on the server: the browser only ever talks to
Relic's own API, and Relic never sends settings to the browser, the model or
its logs.

## Server and storage

| Setting | Meaning |
| --- | --- |
| `RELIC_HOST`, `RELIC_PORT` | Listen address, `127.0.0.1:8790` by default. Relic has no login; keep it on localhost or put it behind your own access control. |
| `RELIC_DATA_DIR` | The workspace: `relic.sqlite3` (authoritative state), `objects/sha256/` (immutable originals and media) and `staging/`. One process holds a lock on it at a time. |
| `RELIC_STATE_DIR` | Disposable caches. Safe to delete. |
| `RELIC_SESSION_DIR` | Account sessions (Telegram). Private. |
| `RELIC_AUTO_PRUNE` | `false`: saved evidence is never discarded silently. |

`--data-dir` and `--port` on the command line override the file.

## Sources

| Setting | Meaning |
| --- | --- |
| `TELEGRAM_API_ID`, `TELEGRAM_API_HASH` | Your Telegram application credentials. You log in once from the Sources view; the session is stored under `RELIC_SESSION_DIR`. |
| `KEELWATCH_URL`, `KEELWATCH_TOKEN` | A KeelWatch AIS service for the ship layer, ship tracks and the "ships near this message" finder. |
| `KEELWATCH_FRESHNESS` | How recent a ship position must be to show: `30m`, `2h`, `24h`, `7d` or `all`. |

Other providers (X, aircraft, a second ship feed) plug in as adapters; see
[SOURCES.md](SOURCES.md). Their settings belong to the adapter, not to the core.

## AI

| Setting | Meaning |
| --- | --- |
| `OPENROUTER_API_KEY`, `OPENROUTER_BASE_URL` | Model access. Without a key, messages are still collected, stored, searchable and shown in their original language. |
| `AI_DEFAULT_MODEL` | The reading model. Requests are pinned to the evaluated provider with no fallback, strict JSON output and a validated schema. |
| `AI_EMBEDDING_MODEL` | Embeddings for finding reports of the same event. |

Having a key does not start any work. Reading, event grouping and the
supplementary hints each have their own switch in the Processing view, with
Pause and Stop, per-request cost receipts and unknown-cost accounting. A
model's confidence never changes saved evidence: every structured answer is
validated in Rust against the source text before it is published.
