# Results

Three kinds of numbers, kept apart on purpose:

- **Live workspace:** what Relic did with real sources. Counts only, from a
  read-only snapshot. No message content is published here.
- **Evaluation gates:** authored, fictional test sets that a model profile must
  pass before it is switched on. They are development checks, not estimates of
  accuracy on unseen real-world messages.
- **Performance:** fictional, reproducible workloads on one desktop machine.

## Live workspace

Collected from 27 September to 3 November 2026 from 9 subscriptions (7 Telegram
channels, 2 X profiles). Snapshot taken 24 November 2026.

| | |
| --- | --- |
| Messages in the current publication | **6,306** |
| Read by the model (translation, events, claims, places) | **5,146** (82%) |
| Messages that report at least one event | 3,707 |
| ...of which placed on the map | **1,845** (50%): 1,037 at a point, the rest as an area |
| Languages read | English, Arabic, Hebrew, Persian and mixed-language posts |
| Candidate report pairs compared for "same event" | 11,596 |
| Relationships published | **1,601**: 1,058 same event, 524 updates, 16 denials, 3 corrections |

Location coverage over time, for messages that report an event:

| Measured | Located |
| --- | --- |
| before location context v12 (Sept 2026) | 27% |
| v12 (26 Sept 2026) | 43.7% |
| current workspace (3 Nov 2026) | **49.8%** |

The other half are not guessed. A message whose place is missing, ambiguous
(for example a town name that exists in several countries) or outside the
imported reference data stays **unlocated** and is still listed in the feed.
Relic never invents coordinates to fill the map.

## Evaluation gates

Each model profile (provider, prompt, schema, reasoning setting and decoder)
is pinned. Answers are validated in Rust: quotes must appear in the source,
negations must survive, and places must resolve in the reference data. A
failing profile is not activated.

| Task | Test set | Result |
| --- | --- | --- |
| Reading a message: translation, event extraction, claim status, negation, places | 67 authored cases in English, Hebrew, Persian and Arabic, including bilingual, long, correction and prompt-injection cases | **67/67**; the whole set costs $0.017 to run |
| Are two reports about the same event? | 40 authored pairs (20 development, 20 challenge) | **39/40** exact; **0 false links**. The one miss answered "uncertain" where "distinct" was expected, so it also did not link. |
| Candidate retrieval for event grouping | 62 authored cases, including reports with no resolved place | All positives exact, **0 unsafe links** |
| Finding ships named in a message | 40 cases, 12 of them negatives | Precision **1.0**, recall **1.0**, no ships on the negatives, identifiers exact |

Every evaluation request is admitted durably before it is sent, and its raw
response is kept. A rerun replays the stored answers with **0 model calls**,
so the gates can be checked again without paying for them.

## Performance

Fictional workload of 50,000 report revisions, measured with a
10,000-report scope. Machine: AMD Ryzen 7 7700, 32 GB RAM. Source:
[`validation/sustained-http-20260923.json`](../validation/sustained-http-20260923.json).

| Measurement | p95 |
| --- | --- |
| Open one retained message (`/evidence`) | **177 ms** |
| Scope query over 10,000 matching reports (`/query`) | 847 ms |
| Import batch, committed and published (`/imports`) | 2.3 s |

The two-minute soak had zero failures. The server was then killed with
`SIGKILL` and restarted on the same workspace: all 4,900 committed imports were
visible, SQLite `quick_check` passed, and the exact original message reopened
in the browser.

## Tests

| Suite | Count |
| --- | --- |
| Core (`app/`): unit and integration tests | **364** |
| Telegram adapter, offline (no test contacts Telegram) | 14 |
| Frontend | `tsc` type check plus six Node contract checks in `validation/` |
| Private adapters (not in this repository) | 75 connector tests plus 22 adapter and integration tests |

`make test` runs everything in this repository.
