# society

A living settlement of small local models. Nine villagers with memory, hunger,
grudges and promises run on `qwen3:0.6b`; a typed judgment layer decides what
they do; you watch. It is a native desktop app with a control API so it can be
driven and inspected from a terminal.

```bash
git clone https://github.com/nicolas-found42/society.git
cd society
export OPENROUTER_API_KEY=...     # required, see "Judgment is not optional"
cargo run --release                # window + dev server on :8100
cargo run --release -- --headless  # no window, same engine

# open straight to a panel, advance turns before showing, run a second instance
cargo run --release -- --view feed --warmup 10
cargo run --release -- --view inspector --port 8101
cargo run --release -- --focus --paused # open in front for inspection

# several villages side by side; --reset discards one explicitly
SOCIETY_SAVE_PATH=data/second.json cargo run --release -- --port 8101
cargo run --release -- --reset      # start the observed village over
```

The village is written to disk after every committed turn and read back at
startup. Close the window, come back later, and it is where you left it: same
turn, same stores, same grudges, same dead. `--seed` only founds a world when
there is no save; a corrupt or wrong-version save is a startup failure naming the
file, never a silent reset.

| Variable | Default | Meaning |
|---|---|---|
| `SOCIETY_SAVE_PATH` | `data/society.json` | village file; one file per village |

The window never takes focus and never raises itself, so it can sit behind what
you are working on. Screenshot it at any time, from any app, with:

```bash
tools/shot.sh                     # -> /tmp/society.png
tools/shot.sh /tmp/turn40.png 5   # wait 5s, then capture
```

`tools/shot.sh` builds `tools/shot.swift` on first use and captures the window's
own surface through ScreenCaptureKit. Because it reads the window rather than the
screen, the result is the same whether the window is frontmost or completely
covered, and nothing is clicked, focused, or moved. It needs Screen Recording
permission for the terminal that runs it.

## Why the architecture looks like this

Every structural decision below was measured on this machine before it was
written down. The numbers are in `models/research-*.md` and in the notes at the
end of this file.

**Code owns legality and arithmetic. The judgment layer owns meaning. The voice
model owns wording.** Splitting the three is not stylistic: a 0.6B model asked to
decide *and* phrase does both badly, and a judgment model asked to check
deterministic rules is worse than a `match`.

| Task | Owner | Evidence |
|---|---|---|
| Which moves are legal, what they cost, who owns what | code | Jev "is this legal" scored 0.56–0.59; code is 100% by construction |
| Among 2–5 legal actions, which one now | Jev | 3.88/4 sensible vs 3.12/4 for a deterministic policy, blind-judged by the 12B |
| One spoken line | qwen3:0.6b | 0.2–0.5 s solo, 1–3 s at 16-way concurrency |
| "Does this line fit the world" | Jev | grounding 10/10, true 0.72 vs false 0.05 |
| "What kind of social act is that" | Jev | 17/18 labelled set |
| Memory salience | Jev | absolute ranking is unreliable; *conditional* selection is strong |

Two failure modes are designed around explicitly:

1. **Field names are semantics.** `{"here": 3}` makes the judge say *speak*;
   `{"food_here": 3}` makes it say *take*. Identical values. `judgment_state`
   in `crates/sim/src/card.rs` is therefore a published schema, not an
   implementation detail.
2. **A judgment model has priors.** With no explicit affordance state it picks
   `speak` 5/5 regardless of hunger. Every question hands it the code-computed
   legal set instead of asking it to imagine one.

## The turn

```
code: enumerate legal actions per living villager
  └─ Jev: one batched request → intent + register + addressee for everyone
      └─ qwen × 9: one line each, concurrent, AIMD-capped
          └─ Jev: one batched request → fidelity, grounding, social act, intensity
              └─ code: validate, apply, settle, log
```

One turn for nine villagers costs about **1.8–2.5 s** and **$0.0004**. Nothing
is committed until every judgment has landed, so a mid-turn failure leaves the
world untouched.

A line that contradicts its own decision or the world is still spoken, but is
marked `discredited` and cannot move trust. That is what makes lying *cost*
something: the listener's memory records the claim, and the gate records that it
did not hold.

## Judgment is not optional

If Jev is unreachable the society halts and says so. There is no degraded mode.
A local model that keeps muttering without a judge is not a society, it is a
noise generator; measured, its output drifts to restatement within a few turns.
`/health` reports this before you start:

```json
{"judgment": {"reachable": true, "required": true},
 "voice": {"reachable": true, "models": ["gemma4:12b", "qwen3:0.6b"]}}
```

## World

Seven places in a ring (`granary → market → well → hut → shrine → woods → field`),
six resources (food, water, wood, herbs, tools, and coin), village standing,
and trust as an 0..4 number per ordered pair. Seasons, time of day, weather,
and temperature affect village life. Villagers have roles, skills, health,
thirst, energy, warmth, moods, goals, and detailed social ties.

The desktop World view shows every stockpile, building condition, hearth,
fertility, and ownership. Click a villager on the map or in the roster to open
the inspector: vitals, inventory, skills, available actions, promises, kinship,
trust, memories, beliefs, and facts. The Feed includes saved history and the
new crafting, healing, trading, prayer, repair, and consolation events.

- Eating costs one measure per villager per turn; hunger climbs to 3 and death
  follows after three turns at 3.
- The field and woods yield into the **granary**, because a field that keeps its
  grain is a field nobody visits.
- A starving villager with nothing to hand is not offered `speak`, `rest` or
  `give`. Without that filter the settlement reliably starved with a full store.
- Giving your last measure while starving is rejected in code.
- A promise unkept for ten turns becomes a permanent memory ("scar") and lowers
  trust. Deaths and betrayals also leave scars; scars never decay and are never
  evicted.
- Memory is a ring of eight, weighted by salience written once at formation and
  decayed in code. Reflections fire on an accumulated-importance budget, not a
  clock.
- Villagers grow. At the end of a turn, two villagers who are both fed, in the
  same place, at least 24 turns old, and who trust each other at 3.5 of 4 may
  have a child — at most one per turn, and the granary pays 6 measures.
  Population is capped at 12. A child inherits a mixed persona, both parents'
  names, and generation one above the elder parent; it starts with one measure
  of food, neutral trust toward everyone, and no memories. Births appear in the
  event log and the feed like any other event.

## Observer, not god

You can whisper a thought, leave food, or turn the weather. Every intervention is
logged as a `divine` event, and agents only remember it if they were there. You
cannot edit minds or kill directly.

## Control API (127.0.0.1 only)

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | both backends, permits, models, population cap, save path |
| GET | `/state` | world snapshot: agents, places, food, trust, memories |
| GET | `/agent/{name}` | one villager, including the exact JSON Jev sees |
| GET | `/events?since=N&limit=M` | append-only event log |
| GET | `/log?since_turn=T` | per-turn records: actions, lines, gate scores, cost |
| POST | `/turn` | `{"turns":N,"timeout_ms":M}` |
| POST | `/reset` | `{"seed":N}` — clears world and save |
| POST | `/inject` | `{"kind":"whisper\|gift\|weather",...}` |
| POST | `/halt` | `{"reason":"..."}` |

```bash
curl -s -X POST localhost:8100/turn -d '{"turns":3}' -H 'content-type: application/json'
curl -s localhost:8100/log | jq '.rows[-1].agents[] | {name,action,say,intensity}'
curl -s -X POST localhost:8100/inject -H 'content-type: application/json' \
  -d '{"kind":"whisper","target":"Ada","text":"the granary door was open last night"}'
```

## Crates

| Crate | Owns | Depends on |
|---|---|---|
| `society-sim` | world state, legality, effects, memory, RNG, card formatting | nothing but serde |
| `society-server` | Jev client, Ollama client, turn engine, control API | sim, reqwest, axum, tokio |
| `society` (bin) | native window, CLI, wiring | server, eframe |

`society-sim` is pure: same seed, same founding; every rule is unit-tested
without a network call.

## Tests

```bash
cargo test --workspace    # no network, no models
```

They cover the rules that are easy to get wrong and expensive to get wrong:
affordances never offer an illegal move, taking never drives a store negative,
a starving villager cannot give away their last meal, memory caps never evict a
scar, starvation kills on the third turn and leaves scars in the survivors, a
broken promise becomes permanent, and the judgment-state field names that
measurement showed to be load-bearing are still present.

Growth and persistence are tested at the two seams that matter. Births are
driven through the pure simulation: a fed, trusting, co-located pair has a child
with the right generation and both parents; hunger, low trust, separation, the
population cap, a short granary, or too-new parents each block one; at most one
birth lands per turn; and a birth in the same turn as a death leaves the scar.
Persistence is driven through save/load against a temporary directory: a lived-in
world round-trips field for field and continues identically after the reload, an
absent file founds fresh, a corrupt or wrong-version file names its path and
fails, and a save that would not load back never replaces a good one.

## Notes and sources

- `models/research-simulation-code.md` — Generative Agents, Concordia, OASIS,
  AgentSociety, AI Town: what to port and what to skip.
- `models/research-memory-code.md` — Letta, MemoryOS, A-MEM, mem0, Graphiti,
  Reflexion, ToMi: tiers, write policy, contradiction handling, belief state.
- `models/research-small-models.md` — constrained decoding, small-model agent
  capability, Rust crates, and why the planner/caller split is forced.

Adopted from that reading: reasoning field before the answer in every schema
(field order is what structured-output studies actually fix), 2–5 candidates for
the chooser, bi-temporal facts with code-owned invalidation, beliefs with
`as_of_tick` so divergence and staleness are computed rather than stored, an
importance budget instead of a reflection schedule, and AIMD concurrency.
