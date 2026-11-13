# Retained evidence and investigation

Audit date: 2026-11-04. This covers all three workspace crates and repository
execution tools, including the pre-existing uncommitted working tree. It is a
boundary and workflow audit, not a line-by-line proof of every simulation rule.
No production credentials, collector, hosted dashboard, or deployment was changed.

## Previously visible

The starting tree already implemented atomic validated saves, turn records,
seed/turn/name decision IDs, policy factors and rejected candidates, event index
ranges, eviction counters, a 240-record memory tail, JSONL journal/audit writers,
and a founding-seed replay endpoint. See [turn.rs](../crates/sim/src/turn.rs),
[world.rs](../crates/sim/src/world.rs), [policy.rs](../crates/sim/src/policy.rs),
[persistence.rs](../crates/server/src/persistence.rs), and
[diagnostics.rs](../crates/server/src/diagnostics.rs).

**Code instrumentation was not proof of collected history.** At audit start:

```text
curl --max-time 2 http://127.0.0.1:8100/health
# no listener (curl exit 7)
local data/society.json: version=2 seed=7 turn=233 events=2837
local data/*.jsonl: []
python3 tools/evidence.py --limit 1
# entries=[], files=[], head_missing=true
```

The existing save was only read, never advanced or replaced. It retains current
world state and domain events, but no wall-clock request sequence or prior save
outcomes. Old model research and parity files under `models/` and `baseline/`
are frozen measurement artifacts, not evidence about yesterday's running app.
GitHub incidents [#3](https://github.com/nicolas-found42/society/issues/3) (morale
lost during settlement) and [#21](https://github.com/nicolas-found42/society/issues/21)
(repeated speech hidden by a vacuous soak assertion) were retrieved with `gh issue view 3 --json number,title,body` and
`gh issue view 21 --json number,title,body`. They motivate state/decision history; this
pass does not claim to independently reproduce or fix those domain bugs.

## Coverage inventory

“Inspected” means source boundaries, evidence paths, failure branches and relevant
tests were inspected. “Exercised” below identifies actual runtime verification.
No repository component group remains uninspected; inaccessible runtime systems
and unavailable historical evidence remain explicitly separate.

| Area and source | Status | Visibility and limitation |
|---|---|---|
| Pure engine orchestration: `sim/turn.rs`, `sim/lib.rs` | Inspected, exercised via API/tests | Decisions, accepted/rejected actions, summary, births, lines, decision IDs and event ranges in journal. No IO/clock/network inside engine. |
| Policy, affordances, effects, professions: `sim/{policy,action,catalog,profession}.rs` | Inspected, regression tested | Factors and runners-up explain choices; action/event evidence explains effects. Not every candidate/state variable has a historical snapshot. |
| Speaker, grammar tables, typed checks: `sim/{speaker,grammar,grammar_first,grammar_second,check}.rs` | Inspected, regression tested | Spoken lines, silence notes, lie/verdict, factors available; full claims and all random draws are not retained. Exact grammar wording assessed through existing tests, not newly instrumented per rule. |
| World, settlement, memory, RNG: `sim/{world,memory,rng}.rs`, `sim/tests.rs` | Inspected, regression tested | Current full save, append-only domain events, RNG in save, capped ordinary memories and persistent scars; aggregate vitals before/after turns added. Not all previous world snapshots recoverable. |
| Control API, request validation and world mutex: `server/devserver.rs` | Inspected, exercised over real HTTP | Mutations/errors, status, duration, operation ID; turn lock wait, pre/post vitals, persistence outcomes. Successful GETs omitted to constrain polling volume. |
| Save/load/reset and validation: `server/persistence.rs`, `server/lib.rs` | Inspected, save/reload and failure exercised | Validated atomic replacement; save stage and recovery retained. Save, identity and evidence are separate files, not a transaction. |
| Journal/audit/rotation/retrieval/replay: `server/{diagnostics,telemetry}.rs` | Inspected, exercised | Bounded private files, corruption/read/missing/truncation indicators, independent sinks, identities and time/operation queries. Replay is a current pure-engine baseline comparison only. |
| Desktop launch, headless, warmup, server task: `desktop/main.rs` | Inspected; headless exercised | Boot (actual vs requested seed), OS/arch, server/window errors, warmup source, shutdown, latest panic location. No SIGKILL/OOM/power-loss cause capture. Warmup uses desktop civic effects. |
| UI worker, interventions, chronicle, civic systems: `desktop/window.rs` | Inspected; shared persistence and headless UI tests exercised | Shared per-turn commit; worker disconnect; skipped busy interventions; immediate native observer saves and outcomes. Interactive window, GPU failures and real UI auto-tick lifecycle not manually exercised. |
| Network parity harness: `server/bin/parity.rs` | Inspected, local mock exercised | Separate `data/parity.diagnostics.jsonl`, run/chunk/progress results, outcome classes and duration. No retries; 180s timeout. Live provider, real timeout expiry, TLS and billing inaccessible/not exercised. |
| Screenshot tooling: `tools/shot.{sh,swift}` | Inspected; live capture inaccessible in this pass | Explicit screenshot artifact and stderr failure only. Screen Recording permission and visible target required. Screenshot timestamp/build provenance is caller-owned. No automatic capture introduced. |
| Baselines, research, ADR/glossary, agent docs, Cargo configuration | Inspected | Frozen artifact provenance and standards; no collector or CI/deployment config present in this tree. |
| Hosted metrics/tracing/error reporting, external logs/backups | Inaccessible/unknown | No configured collector/account or retained external dataset supplied. No claims about external retention/access. |
| Distributed queues, DB, service retries, remote runtime inference | Not applicable | Product runtime is in-process and offline; parity is a separate developer-only HTTP client. No queue or database exists here. |

Jevgrep discovery ended with `resource_limit` and output truncation; direct
source reads and focused searches supplied the remaining boundary inventory.

## Blind spots identified and acceptance checks

All scenarios below are hypothetical except the linked historical domain issues.
Impact/likelihood/diagnostic value are qualitative, not measured probabilities.

| Priority (impact / likelihood / value) | Gap and question | Acceptance and result |
|---|---|---|
| P1 high / medium / high | Batch saved before journaling; process death loses preceding evidence. Did a step reach save, journal, or only memory? | All API, UI-worker and warmup steps use `start_step` + `commit_turn` under world lock. Fault exercises retrieve independent `saved`/`journaled` outcomes. Passed. A kill between writes is still ambiguous, explicitly not transactional. |
| P1 high / medium / high | Reset reused seed/turn IDs; journal dedup hid history. Which village and operation? | Persistent world ID survives resume, changes on reset; session changes on restart; request operation propagated to journal/audit and response header. Both same-seed turn-zero rows survive. Passed. |
| P1 high / medium / high | Rotation filenames unreadable; segment count effectively unbounded. Can old evidence be found, with bounded storage? | Read head + numeric rotations, retain four rotations, chronological append order across turn resets. Tiny-threshold rotation test retrieves last five records. Passed. |
| P1 high / medium / high | Read/write errors looked like empty healthy evidence. Is the absence supported? | Explicit `head_missing`, `read_errors`, `corrupt_lines`, `truncated`, sink failure count/last category in health. Audit and journal failures do not fail successful gameplay saves. Passed. |
| P1 high / medium / high | Native interventions not saved until next turn. Did it survive close/reopen? | Shared native observer save immediately round-trips changed inventory without a later turn; failed save exposed. Passed at persistence boundary. Interactive buttons not exercised. |
| P2 medium / high / high | No retained HTTP rejection or worker timing. Was a retry a new operation, and where did it wait? | Real malformed request retains 400, concurrent submissions have distinct IDs and ordered world commits; API/UI turn lock wait plus total duration. Passed for API. No idempotency introduced. |
| P2 medium / medium / high | No useful persistent panic location or detached server failure. Where did worker/process fail? | Separate latest-panic file omits message and records source line/build/session; child-process panic test passes. Server bind/window failures audited. Signal/OOM/native crash diagnosis remains unknown. |
| P2 medium / medium / high | Parity abort lost chunk evidence; provider error could leak raw response/URL. | First mocked chunk progress survives second-chunk 503; audit/stderr omit sentinel key, URL token and body. Passed. Real timeout expiry not exercised. |
| P2 high / medium / high | Replay success implied more than retained evidence supports. Did the actual historical world replay? | Empty/incomplete evidence explicit; mixed village and >10000-turn replays rejected. Endpoint describes current founding-engine comparison, excluding interventions/civic systems. Does not prove past state equivalence. Passed for pure baseline and missing-turn divergence. |
| Remaining P1 high / medium / high | Full disk failure can remove audit, journal and crash evidence together. | Local failure counters prove degradation while process lives; external archive/independent sink needed for durable diagnosis after total store loss. Deployment requirement, not silently provisioned. |
| Remaining P2 medium / medium / high | Historical exact state, arbitrary observer payloads and desktop civic transitions cannot be replayed from turn records alone. | To close: design consent-aware input journal + periodic state checkpoints with total byte budget and replay all effects against historical source build. Requires domain/privacy design; not claimed complete here. |
| Remaining P2 medium / medium / medium | Multiple processes can share a save path; World.events/scars/save size grow. | Long-running turn-1077 latency is now measured (`baseline/PERF_BASELINE.md`): `engine_turn` stays 0.138–0.215 ms p50 and `observer_snapshot` 0.076–0.092 ms p50 after tail-bounded event scans and live-fact caching, while `durable_save` grows linearly with save bytes (19.8–22.3 ms p50 / ~15.2–15.7 MB allocs at ~6 MB). To close: enforce single writer and evaluate bounded event migration (event ranges and simulation history depend on indexes); no migration in this pass. |

## What changed and how to retrieve it

All runtime evidence stays beside the configured save (`SOCIETY_SAVE_PATH`,
otherwise `data/society.json`). Files newly created by evidence writers have mode
0600 on Unix. Existing files' modes are not changed. The loopback API retains its
existing access model (local machine users), without exposing new network listeners.

| Source | Records / retention / sampling | Retrieval |
|---|---|---|
| `<save>` | Current complete world including events, memories and observer prose. Overwritten every save; no previous snapshot guarantee. Events/scars remain unbounded as before. | `/state`, `/events?since=N&limit=M`, `/agent/Ada`; private local save via `persistence::inspect/load`. Event offsets refer to that world only. |
| `<save>.journal.jsonl` + `.journal.1.jsonl` … `.4.jsonl` | Full turn record with build/schema/seed/time/context. 32 MiB per segment; head plus 4 rotations; each may exceed threshold by one ≤1 MiB record. No turn sampling. | `tools/evidence.py --journal`; `/log` exposes durable records, one envelope per row, quality, and (unfiltered) any committed turn with no durable row — a trailing turn or an interior gap a failed append left. The UI still uses its 240-turn memory tail. |
| `<save>.diagnostics.jsonl` + `.diagnostics.1.jsonl` … `.4.jsonl` | Boot, mutation/error request boundaries, save stage/recovery, step pre/post digest, independent persistence outcomes, native observer acts, lifecycle errors. 8 MiB segment, same 5-segment retention. Successful GETs omitted. | `tools/evidence.py`; `/diagnostics`. |
| `<save>.identity` | Current village incarnation identifier, reused on resume. New on reset/fresh founding; no historical payload. | `/health` → `.telemetry.context.world_id`; `identity_loaded_at_boot` says whether a prior identity was reused. Missing/invalid identity breaks continuity; never infer an old world ID. |
| `<save>.crash.json` | Latest Rust panic location, process/source/operation and the last published `world_id`; the id is blank when the process panicked before loading an identity or while the publication slot was held, because the hook takes no telemetry lock. Overwritten on each panic, no message/stack. Direct separate writer avoids journal-lock deadlock. | `tools/evidence.py --crash`; no panic file is evidence only of no retained panic file. |
| `data/parity.diagnostics.jsonl` and `data/parity.crash.json` | Separate harness run/chunk/progress results; bounded audit rotation as above; no raw provider payloads/keys/URLs in added telemetry. | `tools/evidence.py --save data/parity`; add `--crash` for latest panic. Existing final `baseline/round*_*.json` remains the outcome artifact. |
| stderr, screenshots, tests, build output | Console output is not automatically persisted; screenshot only when requested. Test logs captured during this audit under `/tmp/society-observability-*.log`. | Explicit shell redirect; `tools/shot.sh OUTPUT.png`; test/run commands below. |

For journal and audit: no age guarantee; volume, disk availability and deletion
control coverage. Total normal evidence allocation is approximately 210 MiB per
save (160 MiB journal + 40 MiB audit + at most one MiB overshoot per segment across ten segments),
excluding the existing world save. The first rotation also prunes numeric segments
above the new count left by older installations; archive those first if needed.
No automatic external upload or backup is configured.

Queries retain the latest matching tail in append order, at most 1000 entries and
8 MiB of encoded records; a line is capped at 1 MiB. Filtering happens before
result limiting. `truncated` reports query clipping, **not** all historic expiry.
The offline reader accepts ISO-8601 times where the HTTP endpoints take epoch
seconds and rejects a negative `--limit` like the server's HTTP 400, so a
command validated against one surface is not automatically valid on the other.
`--since-turn` selects journal rows only; the audit file has no turn window.
The `/health` journal fields are capped the same way: when
`journal_depth_limited` is true, `journal_window_oldest_turn` is the oldest turn
in the read window, not the oldest retained turn. `/log` reports `rows`,
`envelopes` (one per row, `null` for a turn held only in memory), `from_memory`,
`from_journal`, `memory_cap` for the in-memory tail, and `order` for the tail
convention. A turn with no durable row is merged only when no time or identity
filter is set: `since_turn` and the retained tail bound the window it may fill,
so a clipped scan never resurrects an older in-memory turn.
Rotation itself leaves no tombstone, and a foreign directory left at a slot path
is removed before the chain shifts so it cannot be renamed into the retained
slots. `files`/`files_read` identify what was read;
missing/unreadable/corrupt evidence is not a clean bill of health. Rust readers
validate typed records; offline Python validates envelopes but is not a complete
schema validator. Offline reads while rotation occurs can report a disappeared
file: copy the evidence or repeat after stopping the writer for a stable view.

`context` joins world, process session, operation and source identity. Reset requests span old/new world IDs under the same operation. Operation
IDs are generated locally (never trust a supplied header). No idempotency or retry
policy was added: two submissions are two operations. Request boundary `turn=0`
means no world lock/state was read, **not** proof the request affected turn zero;
join the same operation's lock/step/save events. `TurnRecord.turn` is the pre-step
turn, while completed save/digest `turn` is post-step. Ordering across files uses
operation/turn/stage, not seconds alone. Wall timestamps are epoch seconds and can
jump with the clock; elapsed microseconds use a monotonic clock.

`source_id` contains git HEAD at build time plus a non-cryptographic fingerprint
of workspace sources/manifests/lockfile, Cargo config and compiler version. It
distinguishes dirty source trees whose package version remains 0.1.0; it is not an
artifact signature or complete deployment manifest. Boot includes actual/requested
seed, PID, OS and arch. Save/journal schemas accompany records. No environment
dump or CLI argument dump is written.

Audit detail intentionally excludes observer prose, halt reasons, body/query
strings, unknown route names, keys, URLs and raw provider errors. It records bounded
route/stage/category strings and numeric state. **Existing saves and full turn
journals are sensitive**: grammar can incorporate an observer's promise wording.
They have not been converted to redacted archives. Share audit summaries first;
review saved/journal/screenshot content before publishing it. The runbook does not
send files to third parties.

Filesystem writes remain synchronous and fsynced, as in the existing persistence
model. A successful write is not proof of survival of power loss: directory
entries are not fsynced and save/journal/audit/identity are separate files. A stalled
filesystem can delay the process; errors are isolated, hangs are not timeout fenced.
Readers stream all retained segments while bounding returned data; broad queries
and `/health` scans cost IO/CPU. Do not poll full history at frame rate. No sustained
production-load or filesystem-fault performance guarantee was established.

## Short investigation runbook

1. Capture the time window with a timezone, save path, response operation ID if
   available, and the affected villager/turn. Preserve the save and its sibling
   evidence before resetting, rotating, restoring or reproducing. Test commands
   below use isolated stores; do not run `--reset` against the observed village.
2. Check `/health` for the actual world/session/source/schema and latched sink
   failures. A restarted process resets the in-memory failure counter; use old
   audit outcomes too. If the server is unavailable, use the offline reader:

   ```bash
   curl -sS http://127.0.0.1:8100/health | jq '{turn,telemetry,journal_quality,retention,last_save_error}'
   python3 tools/evidence.py --save data/society.json \
     --since 2026-11-03T00:00:00Z --until 2026-11-03T23:59:59Z --limit 1000
   python3 tools/evidence.py --save data/society.json --crash
   ```

3. Narrow by operation (or by world/session and time), keeping quality fields in
   the output. The offline reader exits 2 for corrupt/unreadable evidence while
   still returning valid rows; missing heads and clipped results are JSON flags.

   ```bash
   python3 tools/evidence.py --save data/society.json --operation OPERATION_ID
   python3 tools/evidence.py --save data/society.json --operation OPERATION_ID --journal
   curl -sS 'http://127.0.0.1:8100/diagnostics?world_id=WORLD_ID&since_time=1793664000&until_time=1793750399&limit=1000'
   curl -sS 'http://127.0.0.1:8100/log?world_id=WORLD_ID&since_turn=20&limit=100'
   ```

4. Reconstruct `request_started → world_lock_acquired → turn_started →
   save_error/state_saved → turn_completed → request_finished`. Match journal
   `context.operation_id` and `record.turn` to audit `record_turn`; join individual
   decisions with **world ID plus decision ID**. Compare before/after digest,
   event counts, summary and persistence flags. A long lock wait differs from a
   long commit; commit time includes save/journal IO. Missing finish is an
   interruption/evidence-loss hypothesis, not proof of a crash. For the native
   worker, inspect `worker_started`, `observer_controls`, `launch` and
   `worker_disconnected` to distinguish pause/configuration from a failed task.
5. Inspect the saved world/events to test the state hypothesis. A journal records
   what ran in memory even if saving failed; it is not a write-ahead recovery log.
   Legacy contextless records cannot be safely assigned to a world incarnation.
   External save restores or a missing identity sidecar can also break continuity.
   Old installations' segment numbering may differ; do not infer cross-file
   chronology of legacy rows solely from the numeric suffix.
6. Reproduce the suspected bug safely against the matching source/configuration,
   apply the fix and run its behavioral acceptance test. Optionally compare a
   **pure** sequence with `/verify?world_id=WORLD_ID&since_turn=0&limit=200`.
   `baseline_matches` means selected record fields match the current pure engine,
   not that every historical state field matched. Births are compared as part of
   the record; narrative and decision-side fields are not. A matching prefix can
   still stop short of the live world or skip a turn: compare `journal_current`
   (and `world_turn`) — `false` means the window has a gap or does not reach the
   live turn, which a prefix match cannot show. `journal_current` is `null` for a
   read that starts after turn 0 or clips its window: it reports only on holes it
   could have seen. Interventions, civic systems,
   missing turns and source changes can explain divergence without an engine bug.
   Write down supported conclusions, alternative causes, missing evidence and the
   check that would distinguish them. Instrumentation tests alone do not verify a
   domain bug fix.

## Actual verification and reconstruction

Re-run the retained-evidence exercise (debug binary, no remote calls or display):

```bash
cargo build -p society --bin society
python3 tools/verify-observability.py
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --document-private-items
```

The exercise chooses a free loopback port, creates an isolated `.scratch/` store,
and stops only its own child processes. It intentionally retains files and a
`result.json` manifest for future inspection, including one deliberately corrupt
line. It never opens or resets `data/society.json`.

One actual successful run is retained at
`.scratch/observability-n6yu6j6n/result.json`. The audit can be retrieved with:

```bash
python3 tools/evidence.py --save .scratch/observability-n6yu6j6n/v.json \
  --operation 74859-18e50c9107b2df48-13
python3 tools/evidence.py --save .scratch/observability-n6yu6j6n/v.json \
  --operation 74859-18e50c9107b2df48-13 --journal
```

The retrieved audit establishes this sequence (not a speculative diagnosis):

```text
world_lock_acquired  turn=6
turn_started        state.turn=6
save_error          turn=7 stage=replace
turn_completed      record_turn=6 state.turn=7 saved=false journaled=true
request_finished    status=500
later state_saved   recovered=true
restart boot        same world_id, different session_id, loaded turn=8
```

Supported diagnosis: the engine completed turn 6 in memory, atomic save replacement
failed, its turn record still reached a separate journal, and a later save
recovered. The controlled reproducer made the save destination a directory. The
**audit alone** narrows the failure to replacement; it does not distinguish that
cause from permission, mount, filesystem or race failures. The existing HTTP error
body/`last_save_error` can give a live OS error but is not the durable audit payload.
Exact historical inventory and the original OS error cannot be reconstructed from
this digest alone. SIGKILL survival proves completed fsynced evidence survived that
process death, not that it will survive machine power loss.

A second controlled failure made only the journal path a directory. Its operation
`74859-18e50c9107b2df48-10` returned HTTP 200, with `saved=true, journaled=false`
and health reporting a write failure. No row for that turn was invented later.
The pure baseline subsequently reported divergence at the missing turn; this is
an evidence gap, not evidence that the simulation algorithm was wrong.

| Verification | Observed result |
|---|---|
| Full workspace tests | `cargo test --workspace`: desktop 19 passed + 1 ignored release probe, server 62, sim 128. Includes the existing determinism/soak tests. |
| Real HTTP/headless smoke | Passed malformed JSON rejection, request/turn correlation, baseline comparison, concurrent separate operations, independent journal/save failures, save recovery, SIGKILL/restart, torn-tail separation, missing-turn divergence, same-seed reset isolation, and sentinel redaction (including ignored/unknown inject fields). |
| Native observer persistence | Direct regression changes stores, saves without advancing, reloads same value; world ID resumes and resets correctly. No manual UI/GPU interaction claimed. |
| Rotation and retrieval | Both tiny-threshold eviction/order and actual 8 MiB production audit-writer rollover pass; sparse padding is reported corrupt, valid rows remain retrievable. |
| Panic handling | Child test process panics; latest record contains file/line/source/session and omits sentinel message. |
| Sink failure isolation | Audit failure still permits successful saved/journaled operation; journal failure still permits successful save; save failure independently retains journal and reports recovery. |
| Parity dependency mock | Actual `parity decisions 91` against local HTTP server: first 24-question chunk succeeds, second returns 503; completed picks retained, run fails, provider body/key/URL sentinels absent from audit and stderr. Retained at `.scratch/parity-observability-mvxi_7gy/result.json`; query with `--save .scratch/parity-observability-mvxi_7gy/data/parity`. No real provider accessed. |
| Compiler/lint/format/docs | Workspace build, Clippy, formatting, and rustdoc checked. Stale simulation documentation links and formatting were repaired without changing simulation behavior. |
| Offline script | Python compilation and real smoke queries pass; initial missing-history query reports explicit `head_missing`. |

## Next iteration targets

The inventory is complete for repository component boundaries, while runtime
coverage remains limited as marked above. No claim of universally diagnosable
failures or complete historical replay is made. Prioritize an independent archive
with a documented retention/access policy if yesterday's evidence must survive
loss of the local disk; measure history query costs alongside the recorded
turn-1077 `durable_save` / `engine_turn` / `observer_snapshot` baselines;
then design privacy-aware checkpoints and replayable observer/civic inputs.
Live provider timeout/TLS behavior, interactive UI/graphics failures, multi-writer
races, filesystem hangs and machine power-loss durability remain untested. No
credentials, external infrastructure, deployment or consent for archiving private
observer inputs was requested or provisioned.
