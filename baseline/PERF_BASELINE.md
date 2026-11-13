# Performance baseline (turn, save, load, observer)

The claim "a turn costs a fraction of a millisecond" was in the README before
anything measured it on this machine. This is the measurement, and the
threshold every later pull request is judged against.

Updated 2026-11-13 on this machine with the release profile
(`opt-level = "s"`, LTO on). The raw numbers live in
`baseline/perf_baseline.json`; this file says what they mean.

## The paths

The village's paths were picked by reading the call sites, not by guessing:

| path | code | how often it runs |
|---|---|---|
| `engine_turn` | `society_sim::turn::step` | once per committed turn |
| `durable_save` | `society_server::persistence::save` | once per committed turn, right after it |
| `observer_snapshot` | `society_server::observer::encode` — the village as the bytes on the wire | per observer request (`/state`, `/social`) |
| `durable_load` | `society_server::persistence::load` | once per start (and after `--reset`) |
| `observer_health` | `society_server::diagnostics::journal_growth` | per `/health` request |
| `turn_cycle` | the engine step plus every sink `commit_turn` writes | once per committed turn |

`turn_cycle` exists because the first three paths are stages, and the user pays
for their sum: the engine step, the `turn_started` audit, the save, the journal
row and the `turn_completed` audit. A regression in the *number* or *order* of
those sinks is invisible in the stage numbers and obvious here.

## The numbers

1,000 measured iterations per path, after 20 warmup iterations, on the
deterministic founding fixture (`--seed 7 --turns 233`, turn 233, 12 living
villagers / 15 total agents, 2,681 events, a 1.13 MB save):

| path | p50 (ms) | p95 (ms) | p99 (ms) | alloc B/iter | payload B |
|---|---|---|---|---|---|
| `engine_turn` | 0.206 | 0.329 | 0.395 | 227,467 | 9,691 |
| `durable_save` | 8.814 | 10.129 | 11.610 | 2,496,881 | 1,133,854 |
| `observer_snapshot` | 0.087 | 0.132 | 0.155 | 80,186 | 67,728 |
| `durable_load` | 3.291 | 4.357 | 4.775 | 2,494,413 | 1,133,854 |
| `observer_health` | 10.384 | 11.391 | 12.037 | 50,221 | 10,082,700 |
| `turn_cycle` | 20.137 | 23.159 | 25.157 | 2,768,622 | 1,141,328 |

Resident memory is flat across every path: no path retains what it allocates
(live heap growth is 0 bytes at every path boundary). `prepare()`
pre-reserves steady-state headroom on cloned `Vec` buffers (`World.events`,
`Agent.facts`, etc.) outside the timed and counted `body()` so `Clone`
capacity shrinkage does not charge a full-history `Vec` reallocation on
every iteration.

`observer_health` is measured against the journal the `turn_cycle` iterations
before it produced (1,000 rows, 10.1 MB), so the number is a health poll of a
1,000-turn village. It therefore scales with `--iterations`: at `--iterations
100` the same path measures a 1.2 MB journal and takes ~1.2 ms. `--check`
uses the default, which is the number in the table.

**Read the tail with the disk in mind.** Across captures on this machine the
medians barely move for the CPU paths, while the two fsync paths swing with
whatever the disk is doing: `durable_save` measured 7.0–8.8 ms at p50 and 9.0–
17.4 ms at max across the captures of this session. The allocations, by
contrast, are exact — the same bytes to the byte every time, because the work
does not change, only the disk does.

## What the numbers say

**A turn is 20 ms, and half of it is durability.** `turn_cycle` is 20.1 ms
while the engine step inside it is 0.21 ms: the turn is 1% thinking and 99%
making the thinking survive. The save is 8.8 ms of that, the journal row 3.8 ms
and the two audit rows 6.0 ms. Each of the three files is fsynced before the
turn is reported committed, and on this machine `sync_all` is `F_FULLFSYNC`:
measured 3.7–3.8 ms per call for a 300-byte file, against 0.03 ms for a bare
`fsync`. The four flushes per turn are therefore ~11–15 ms of latency the code
does not own, and the remaining ~5–8 ms is serialisation.

**The save is the CPU.** Serialising the village costs 1.2 ms, parsing it back
to prove it would load costs 1.8 ms, and writing plus fsyncing 1.13 MB costs
4.0 ms. All three grow with the village: at turn 1200 the same stages are
7.8 ms, 9.8 ms and 5.4 ms for a 6.19 MB save. This is why fast-forward is
bounded by disk throughput and save size rather than by the engine — the
engine stays at 0.2 ms because it no longer walks the whole history (see the
turn-1000 section below).

**The observer answers are cheap.** `/state` is 0.087 ms, `/social` and
`/events` are under a millisecond end to end over HTTP, and `/health` now
answers a 1,000-turn village in 10.4 ms where it used to parse every retained
turn record.

## What the load cost, and what it costs now

A restart reads the save back into a `World`. The old implementation built a
`serde_json::Value` of the whole document and then deserialised the world out
of that tree, so every string and every map was allocated twice: 11,682,097 B
per load, 7.0 ms at turn 233.

`persistence::load` now reads the envelope in two passes over the same bytes: a
header pass that stops at the version (skipping the world's keys rather than
building them), and a typed pass that builds the world once. The error
behaviour is unchanged — a missing version, a foreign version, a missing world
and unreadable JSON still fail as `Corrupt`/`Version` naming the path — and the
version is still settled before the world is parsed, so a file from another
build fails on its version and not on whichever field that build renamed.

| | before | after |
|---|---|---|
| `load` p50 at turn 233 | 7.025 ms | **2.892 ms** |
| allocation per load | 11,682,097 B | **2,494,413 B** |

The same change covers a 6.19 MB turn-1200 save at 15.4 ms.

## What `/health` cost, and what it costs now

`/health` reports the retained journal window: how many rows it holds, the
oldest turn in it, whether it was clipped, and how much of it is damaged. The
old reader answered that by deserialising every retained `JournalEntry` — the
full `TurnRecord` of every turn, ~9.4 KB each — into memory and then reading
two numbers off it. A 600-turn village made one health poll cost 5.65 MB of
typed parsing, and the number grew with every turn the store retained.

`diagnostics::journal_growth` reads the same window with the same
oldest-out retention and the same clipping rule, but parses only the row's
`turn`: the records themselves stay in the file. In-process, over the same
662 KB journal, the typed reader costs 1.564 ms and the growth reader 0.695 ms,
with 50× fewer allocated bytes (1,421,745 → 28,395).

End to end, over HTTP, on a 600-turn village (`/turn` to 600, then 25 polls):

| | before | after |
|---|---|---|
| `/health` p50 | 15.742 ms | **6.566 – 7.095 ms** (two runs) |
| `/health` max | 18.931 ms | **7.153 – 7.878 ms** |
| response bytes | 816 | 816 |

`journal_growth` counts a line as corrupt when it is not a journal row (the
line's JSON and its `turn`). The typed validation of the record inside the row
belongs to `/log`, `/diagnostics`, `/verify` and the offline reader, which still
deserialise `JournalEntry`; `/health` reports growth, and the row's presence is
what growth means. `tools/verify-observability.py` exercises the typed readers.

## Two smaller costs removed from every turn

The save buffer is now sized from the save it replaces instead of growing from
128 bytes: a village grows by a few hundred bytes a turn, so the previous file's
length is within a doubling step of the next one. At turn 1200 the 6.19 MB of
bytes are written with 26% fewer allocated bytes and ~5% less time
(7.752 → 7.378 ms); at turn 233 the save's per-iteration allocation falls from
3,456,085 B to 2,496,881 B.

An evidence append used to open the file twice: once to append, once to read the
last byte and confirm the previous row ended in a newline. It now opens one
read-append handle and reads that byte from the descriptor it is about to write
to. `O_APPEND` still places the write at the end whatever the read cursor is, so
the torn-tail protection is unchanged; the per-append cost is one open and one
read fewer, four times a turn.

## Long-running turn-1000+ scaling

Running `tools/bench.sh --turns 1077 --check` and
`tools/bench.sh --source data/society.json --check` exercises the engine after
more than a thousand turns (12,809–13,641 append-only events, 21,198 facts):

| fixture | `engine_turn` p50 / p99 | `engine_turn` alloc | `durable_save` p50 / p99 | `durable_save` alloc | `observer_snapshot` p50 |
|---|---|---|---|---|---|
| `seed=7, turn=233` (1.13 MB) | 0.206 / 0.395 ms | 227,467 B | 8.814 / 11.610 ms | 2,496,881 B | 0.087 ms |
| `seed=7, turn=1077` (12,809 events) | 0.215 / 0.369 ms | 206,659 B | 19.778 / 25.009 ms | 15,171,927 B | 0.092 ms |
| `data/society.json` turn 1077 (13,641 events, 6.37 MB) | 0.138 / 0.191 ms | 163,696 B | 22.296 / 28.874 ms | 15,741,078 B | 0.076 ms |

Before tail-bounded event scans (`action::recent_actions`,
`policy::term_novelty`, `policy::threatened_this_turn`, `turn::step` births,
`world::settle` overdue promises), live-fact index caching in
`Agent::record_fact`, and fixing `prepare()` `Vec` capacity shrinkage,
`data/society.json` at turn 1077 degraded to `2.90–3.00 ms` p50
(`3.50 ms` p99, `3,540,160 B/iter`). With those fixes, `engine_turn` and
`observer_snapshot` stay constant-time at turn 1077 while `World.events` stays
append-only (`AgentTurn.event_range` preserved). `durable_save` grows linearly
with total save size (`1.13 MB` -> `6.37 MB`), so `--source data/society.json`
exceeds the turn-233 `durable_save` budget while passing `engine_turn` and
`observer_snapshot`.

## What was not changed, and why

- **The verify parse in `save`.** Serialising and parsing back is ~40% of the
  save's CPU, and it is the price of "a save you cannot load is never written".
  A cheaper validation would have to be a weaker one: `serde_json` writes a
  non-finite float as `null`, which only a typed parse of the bytes can catch
  in every field.
- **The number of fsyncs per turn.** The save, the journal row and both audit
  rows are each made durable before the turn is reported. Merging them or
  dropping one would trade the evidence contract for latency.
- **`/log` still reads the retained journal.** Its window is the newest 1,000
  matching rows or 8 MiB, but the match is a filter over append order, which a
  reset makes non-monotonic, so the reader cannot stop at the window without
  changing which rows it can see. On a 600-turn village `/log` takes ~16 ms; it
  is a history query, not a frame-rate path.
- **`/health` still reads up to its 8 MiB window.** The window is 9.4 KB per
  turn, so a health poll of a 1,000-turn village reads ~8–10 MB, and the
  retention cap (160 MiB of journal) bounds the worst case. Shrinking the
  window would change the reported `journal_turns`.

## Running it

```bash
tools/bench.sh            # measure and rewrite baseline/perf_baseline.json
tools/bench.sh --check    # measure and compare against the recorded budgets
tools/bench.sh --breakdown   # per-stage cost inside save, append, load and the reads
```

`--check` exits non-zero when a path is outside its budget. The budgets live
in the `budget` block of each path in the fixture, so the threshold a pull
request is judged against is a file, not a sentence. The latency budget is
the loosest of three rules — 25% over the recorded p99, double the recorded
p95, and the recorded p99 plus a 0.50 ms jitter floor — because the p99 of a
sub-millisecond path is scheduler noise and the p99 of an fsync is disk noise.
The allocation budget is a flat 25%, because allocation counts do not move:
every capture measured the same bytes to the byte.

The harness is a workspace crate, `tools/bench`, written on `std` alone: the
counting global allocator, the percentile math, the RSS sampling and the JSON
fixture are all in it. It adds no dependency to the tree — it borrows
`society-sim`, `society-server`, `serde` and `serde_json`, all of which were
already here. It measures the production functions themselves, not copies of
them: the observer path is measured through `observer::encode`, the health path
through `diagnostics::journal_growth`, the same calls the handlers make.

Useful flags: `--iterations N`, `--warmup N`, `--seed N` (default `7`),
`--turns N` (default `233`), `--out PATH`, `--source SAVE` (load an explicit
save instead of founding and stepping the deterministic seed-7 fixture),
`--breakdown` (print the stages and exit without writing the fixture).

## Do not recapture casually

The committed baseline in `baseline/perf_baseline.json` is tied to the
deterministic `seed=7, turns=233` fixture so `tools/bench.sh --check` runs
reproducibly on any checkout without depending on local `data/society.json`.
Re-run `tools/bench.sh` when a change is expected to move the numbers, and
update `baseline/perf_baseline.json` and this file in the same commit.

The observer numbers were last moved on 2026-10-21, twice on the same day.
First the snapshot stopped being built as a `serde_json::Value` tree and
started being written straight from the world: 14.115 ms at p50 and
41,623,858 B of churn per request became 2.856 ms and 3,158,394 B. Then
`/state` was cut over to the living village, and the claims moved to
`/agent/{name}`: 0.073 ms and 75,360 B.

The load and health numbers moved on 2026-11-13: `load` from 7.025 ms to
2.892 ms (11,682,097 B to 2,494,413 B per load), `/health` from 15.742 ms to
6.566 ms at p50 on a 600-turn village, and the save's buffer from growth to a
sized allocation. `durable_load`, `observer_health` and `turn_cycle` became
gated paths in the same commit.
