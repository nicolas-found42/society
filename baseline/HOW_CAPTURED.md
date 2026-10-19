# Baseline corpus (ticket #7)

Captured from a tree at commit `c4467d5` — the freeze commit, before slice 1 —
by `crates/server/src/bin/capture.rs` (a temporary bin, since deleted with the
cutover), running the old engine live: the OpenRouter judgment layer chose each
action and a local `qwen3:1.7b` via Ollama wrote each line.

## Shape

- `corpus.jsonl` — seed 20261019, 30 turns, 278 villager records. One JSON
  object per record: `turn`, `seed`, `world` (the full serialised turn-start
  world), and `agents` (that turn's decisions and lines). Every villager in a
  turn decided and spoke against the same turn-start state, so the world is
  stored once per turn; the record set is what the parity harness replays.
- `metrics.json` — outcome metrics from a side-by-side run at seed 7, 40
  turns: births, deaths, discredited rate, trust mean and spread. Context for
  the outcome report; the acceptance bar is the paired judgment, not these
  numbers.

## Do not recapture

Every later slice changes the legacy engine's behaviour, because it shares the
simulation crate. This is the only capture. If the harness needs more records,
extend the new engine's own soak, never this corpus.
