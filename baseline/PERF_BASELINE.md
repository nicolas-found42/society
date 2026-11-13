# Performance baseline (turn, save, observer)

The claim "a turn costs a fraction of a millisecond" was in the README before
anything measured it on this machine. This is the measurement, and the
threshold every later pull request is judged against.

Updated 2026-11-13 on this machine with the release profile
(`opt-level = "s"`, LTO on). The raw numbers live in
`baseline/perf_baseline.json`; this file says what they mean.

## The three paths

The village has three hot paths, and they were picked by reading the call
sites, not by guessing:

| path | code | how often it runs |
|---|---|---|
| `engine_turn` | `society_sim::turn::step` | once per committed turn |
| `durable_save` | `society_server::persistence::save` | once per committed turn, right after it |
| `observer_snapshot` | `society_server::observer::encode` — the village as the bytes on the wire | per observer request (`/state`, `/social`) |

The first two are the turn cycle: `run_turns` commits a turn and then saves it
before the next one starts (`crates/server/src/devserver.rs`). The third is
what every observer of the control API reads; the desktop window polls the
world four times a second, and an external monitor polls it as often as it
likes.

## The numbers

1,000 measured iterations per path, after 20 warmup iterations, on the
deterministic founding fixture (`--seed 7 --turns 233`, turn 233, 12 living
villagers / 15 total agents, 2,681 events, a 1.13 MB save):

| path | p50 (ms) | p95 (ms) | p99 (ms) | max (ms) | alloc B/iter | payload B |
|---|---|---|---|---|---|---|
| `engine_turn` | 0.184 | 0.209 | 0.222 | 0.238 | 227,467 | 9,691 |
| `durable_save` | 8.004 | 9.066 | 9.994 | 16.810 | 3,456,085 | 1,133,854 |
| `observer_snapshot` | 0.085 | 0.101 | 0.107 | 0.117 | 80,186 | 67,728 |

Resident memory is flat across all three paths: no path retains what it
allocates (live heap growth is 0 bytes at every path boundary). `prepare()`
pre-reserves steady-state headroom on cloned `Vec` buffers (`World.events`,
`Agent.facts`, etc.) outside the timed and counted `body()` so `Clone`
capacity shrinkage does not charge a full-history `Vec` reallocation on
every iteration.

**Read the tail with the disk in mind.** Across captures on this machine the
medians barely move (`durable_save` p50 13.0–14.3 ms, `engine_turn` p50
0.20–0.22 ms) while the save path's p99 swings between 14.9 ms and 27.9 ms:
the fsync waits for whatever the disk is doing, and a capture taken while
APFS was still flushing a build is the slow end. The allocations, by
contrast, are exact — the same bytes to the byte every time, because the work
does not change, only the disk does.

## What the snapshot cost, and what it costs now

The observer snapshot used to be the worst path in the village by an order of
magnitude: 41.6 MB of allocation churn and 14.1 ms to answer one `/state`
request, against a 3.4 MB response. Two things were wrong with it, and both
were measured before either was changed.

**First: the answer was built twice.** The payload was built as a
`serde_json::Value` tree — every one of the village's 18,279 claims became a
map with owned keys — and then the tree was walked again to write the bytes.
A tree of that payload costs about twelve times the payload in allocations,
and 412,683 of them.

`crates/server/src/observer.rs` builds the payload as `Serialize` types
instead, so the bytes are written once, straight from the world. A villager's
record is the villager's own serialization with the observer's two rounded
fields folded in, so a field added to the world is still carried without
anyone remembering to carry it. The answer is 8.1% smaller because a `f32` is
no longer widened to `f64` on its way through a tree: a trust of 3.2 used to
be published as `3.200000047683716`, and is now published as `3.2`, which is
what the rounding was for in the first place.

**Second: the answer carried the whole memory of every villager.** The
claims a villager holds are 98.5% of the answer (3,352,136 B of
3,402,872 B), and they grow with every day the village lives. The living
village does not need them; one villager's claims are served by
`/agent/{name}`, which now carries the whole memory. So `/state` answers
with what the settlement is now, and `/places/{id}` answers with the living
view of the villagers there.

| | before | after the tree | after the cutover |
|---|---|---|---|
| p50 latency | 14.115 ms | 2.856 ms | **0.073 ms** (193× faster) |
| p99 latency | 15.753 ms | 2.950 ms | **0.088 ms** (179× faster) |
| allocation churn | 41,623,858 B | 3,158,394 B | **75,360 B** (553× less) |
| response bytes | 3,425,479 B | 3,148,762 B | **52,412 B** (65× smaller) |
| process RSS moved | +29,008 KB | +48 KB | **+16 KB** |

Both changes were checked against the code they replaced: the payload after
the first change was the old payload, field for field, and the payload after
the second change is that same payload with the claims taken out. Nothing
else moved.

## What the numbers say

**The save is the turn.** The engine turn is 0.184 ms; the save that follows it
is 8.0 ms on the turn-233 fixture (and 22.3 ms on a 6.37 MB turn-1077 save). A
committed turn spends ~98% of its time in persistence: serialise the world,
parse it back to prove it would load, write it to a temporary file, fsync it,
rename it over the target.

**The observer answer is now cheap enough to ignore.** `/state` answers in
0.085 ms with 80 KB of allocation, for a 68 KB answer: the living village
without the claims each villager holds. A monitor polling once a second
spends ~0.01% of a core. The claims themselves grow with every day the village
lives and remain on `/agent/{name}`, where one observer asks for one villager
instead of asking for the whole settlement every time.

**The engine stays flat as history grows.** On turn 233, nine to twelve living
villagers decide, speak, be checked, act and settle in 0.19 ms, with three
orders of magnitude of headroom inside the 50 ms soak bound
(`crates/sim/src/tests.rs`).

## Long-running turn-1000+ scaling

Running `tools/bench.sh --turns 1077 --check` and
`tools/bench.sh --source data/society.json --check` exercises the engine after
more than a thousand turns (12,809–13,641 append-only events, 21,198 facts):

| fixture | `engine_turn` p50 / p99 | `engine_turn` alloc | `durable_save` p50 / p99 | `durable_save` alloc | `observer_snapshot` p50 |
|---|---|---|---|---|---|
| `seed=7, turn=233` (1.13 MB) | 0.184 / 0.222 ms | 227,467 B | 8.004 / 9.994 ms | 3,456,085 B | 0.085 ms |
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

## Running it

```bash
tools/bench.sh            # measure and rewrite baseline/perf_baseline.json
tools/bench.sh --check    # measure and compare against the recorded budgets
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
`society-sim`, `society-server` and `serde_json`, all of which were already
here. It measures the production functions themselves, not copies of them:
the observer path is measured through `observer::encode`, the same call the
`/state` handler makes.

Useful flags: `--iterations N`, `--warmup N`, `--seed N` (default `7`),
`--turns N` (default `233`), `--out PATH`, `--source SAVE` (load an explicit
save instead of founding and stepping the deterministic seed-7 fixture).

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
