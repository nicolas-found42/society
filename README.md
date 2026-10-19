# society

A living settlement. Nine villagers with memory, hunger, grudges and promises
live in a ring of seven places, choose what to do and what to say for
themselves, and are watched doing it. There is no language model in the loop:
every decision, every spoken line and every check is a rule in the simulation,
and the whole village replays identically from a seed.

> **Status.** This README describes the code-only engine decided in
> [ADR 0001](docs/adr/0001-replace-the-judgment-layer-and-the-voice-model-with-code.md)
> and specified in [issue #5](https://github.com/nicolas-found42/society/issues/5).
> That rewrite is in progress: **the tree on `main` still runs the judgment layer
> and the local voice model**, so the install lines above and the "Then" column
> below describe what works today, and the "Now" column describes the target.

```bash
git clone https://github.com/nicolas-found42/society.git
cd society
cargo run --release                # window + control API on :8100
cargo run --release -- --headless  # no window, same engine

# open straight to a panel, advance turns before showing, run a second village
cargo run --release -- --view feed --warmup 10
cargo run --release -- --view inspector --port 8101
cargo run --release -- --focus --paused # open in front for inspection

# several villages side by side; --reset discards one explicitly
SOCIETY_SAVE_PATH=data/second.json cargo run --release -- --port 8101
cargo run --release -- --reset      # start the observed village over
```

No API key, no model to pull, no daemon to keep alive - **once the rewrite
lands.** Until then, `main` still needs the old backends, and these two lines
are the whole setup:

```bash
export OPENROUTER_API_KEY=...                                     # today only
ollama pull hf.co/LiquidAI/LFM2.5-350M-GGUF:Q4_K_M                # today only
```

The village is written to disk after every committed turn and read back at
startup. Close the window, come back later, and it is where you left it: same
turn, same stores, same grudges, same dead. `--seed` only founds a world when
there is no save; a corrupt or wrong-version save is a startup failure naming
the file, never a silent reset.

| Variable | Default | Meaning |
|---|---|---|
| `SOCIETY_SAVE_PATH` | `data/society.json` | village file; one file per village |

The old engine additionally reads `OPENROUTER_API_KEY`, `SOCIETY_JEV_ENDPOINT`,
`SOCIETY_JEV_MODEL`, `OLLAMA_HOST`, `SOCIETY_ACTOR_MODEL`,
`SOCIETY_ELDER_MODEL`, `SOCIETY_ACTOR_THINK` and `SOCIETY_ELDER_THINK`; all
eight go at the cutover, with the `--permits` flag.

A turn costs a fraction of a millisecond once the rewrite lands: there is then no
call to rate-limit, retry or queue, and no permission window to size. The same
seed and the same save produce the same turns, the same lines and the same
village, every time. Today a turn is a network round trip, and two runs of one
seed differ.

The window never takes focus and never raises itself, so it can sit behind what
you are working on. Screenshot it at any time, from any app, with:

```bash
tools/shot.sh                     # -> /tmp/society.png
tools/shot.sh /tmp/turn40.png 5   # wait 5s, then capture
```

`tools/shot.sh` builds `tools/shot.swift` on first use and captures the window's
own surface through ScreenCaptureKit. Because it reads the window rather than
the screen, the result is the same whether the window is frontmost or completely
covered, and nothing is clicked, focused, or moved. It needs Screen Recording
permission for the terminal that runs it.

## Why the architecture looks like this

The first version of this village ran on two backends: a cloud judgment layer
that chose what each villager did, and a local 350M model that wrote what they
said. Every structural decision was measured on this machine before it was
written down, and the measurements are in `models/research-*.md` and in the
notes at the end of this file.

| Task | Then | Now |
|---|---|---|
| Which moves are legal, what they cost, who owns what | code | code |
| Among 2-5 legal actions, which one now | the judge, 3.88/4 blind-judged | the policy, in code |
| Why a villager acts | the judge, one word | derived from the action and the state |
| What kind of social act a line performs | the judge, after the line | derived from the action |
| Whether a line holds against the world | the judge, 0.0-1.0 | the check, on the line's own claims |
| One spoken line | a local 350M model | the speaker, from a grammar |
| Memory salience | code | code |

**Code owns legality, arithmetic, meaning and wording.** The village used to
be a prompt with a state machine behind it. Now the state machine is the whole
thing, and the prompt is gone.

Four things ended the judgment layer, each of them measured:

1. **Most of what it was asked had no meaning in it.** The tone question came
   back "plain and neutral" for 38 of 45 lines while still moving trust; the
   pressure ordinal never passed 2.21, so the world's rumor (3.2) and
   threat-morale (3.0) branches could not fire; the addressee was asked before
   the line existed, so it guessed. All three are derived in code now.
2. **The motive is the action's reading, named.** Asking for it separately
   bought a second opinion about a decision the same call had already made.
3. **A gate can only find a false claim if something can write one.** A
   350M model invents numbers - 38% of its lines were discredited, 21% for the
   better 0.6B model - so it needed a gate. A speaker that writes from a
   grammar cannot invent a number, so what the gate has left to do is check the
   claims the line actually makes.
4. **The action choice is a choice among two to five options the code already
   computed.** The judge read a richer set of contextual features than a naive
   policy, and it scored 3.88/4 against that policy's 3.12/4, blind-judged by
   the 12B. That is a real gap and it is the bar this rewrite is held to: the
   policy is measured against the same judge, on the same recorded states, and
   parity - not a claim - is what closes it.

What it cost: two cloud calls and up to eighteen local calls per turn, about
$0.001 and 20,759 input tokens per turn, an API key the app refused to start
without, and a village that halted when a network call failed.

The judge itself is not thrown away. It is now the **oracle**: the same model
that produced the 3.88/4 decisions blind-judges the new engine's decisions
against the old engine's, on the same recorded states, and decides whether the
rewrite met the bar.

Two rules are designed around explicitly:

1. **The action id is the meaning.** `give` performs a gift, `accuse` performs
   an accusation, `console` performs counsel. The line is written for the act
   the action implies, and nothing downstream re-derives it.
2. **A rule has somewhere to be wrong.** Every claim a line makes is recorded
   when the line is written, so the check compares a claim against the world
   instead of re-reading prose. A generator bug cannot slip past the check
   unnoticed, because a property test holds every line outside lie mode to
   making only true claims.

## The turn

```
code: enumerate the legal actions per living villager, shortlist them, score every candidate
  └─ policy: intent + motive + social act for everyone, in code
      └─ speaker: one line per villager, from a grammar
          └─ check: the line's claims against the world, in code
              └─ code: validate, apply, settle, log
```

### The policy

Three layers, in order:

1. **Hard tiers.** Survival first - hunger with no food, thirst with no
   water, an ailment, winter cold - and then obligation, an unkept promise or
   a grudge against someone present. Nothing below may talk a villager out of
   finding food.
2. **Registered feature terms.** Every remaining candidate is scored by named
   terms - the work waiting in the villager's profession, their role's duty,
   what the place needs, someone present who is starving, kinship, trust, a
   grudge, their mood, an unkept promise, novelty, what the action costs in
   energy, whose ground they stand on. Each term is a pure function with a
   documented weight, and a more complicated village later is more terms, not a
   different policy.
3. **A distribution.** The scores become a distribution over the candidates.
   That is where confidence comes from, and where the **dilemma** comes from -
   the same margin the judge used to report, computed in code.

The judge is offered a shortlist of the legal actions, one per kind of act
(need, social, work, rest, move) minus whatever the villager has just done for
two turns running, instead of all of them: a menu of eighteen is not a choice,
and an unchanging menu invites the same answer twice. A legal action outside
the shortlist stays legal and is still accepted - the menu is a prompt, not the
law.

### The speaker

A rule-based grammar over typed slots - a count, a person, a place, a resource,
an obligation, a quality. **The rules are keyed on the action, not the social
act**: thirteen of the twenty actions perform no social act at all, so keying on
the act would put seven lines in ten into one shared bank and stop the speaker
saying what the villager actually did. A `speak` action is the exception - its
act is chosen by the policy, so its rules are keyed on that act. A rule may
carry a second clause, a consequence or a request, joined by a connective
matched to the action family. Every line is grammatical by construction, because
the rules are the grammar, and a long first clause only pairs with a short
second, so the twenty-word budget holds without a check.

A villager does not repeat themselves: a ledger of recently used lines makes an
exact repeat impossible inside six turns. Variety comes from the rule set, the
slot fillers and a per-villager random stream derived from the seed, the turn and
the name, so two villagers acting at once cannot draw the same line. Each
`(action, register)` cell holds at least two first-clause rules, and each action
family at least three second-clause forms.

**A villager knows what is observable and what they remember.** Who is here,
the weather, the place's stores, their own carry - and their memories, their
promises, what they know about other people. They do not know another
villager's inventory unless they remember it. A villager with nobody to
address speaks no line at all: silence is a signal, and the turn summary counts
it.

### The check and the lies

The speaker returns the line together with the claims it made. The check
compares each claim with the world. True claims pass; a false claim marks the
line **discredited** - still spoken, but it moves no trust, and the listener's
memory records the claim and the failure of it.

Lying is designed rather than accidental. A villager lies when the world pushes
them to: hunger with nothing in hand, a grudge against the person they are
addressing, a promise about to be missed, fear. The claim is a plausible
overstatement - their count, a debt, a gift they did not make - the same lie
the world would punish. The rate is a documented constant, and the soak asserts
it stays inside a stated band.

### Professions

Each role owns a profession, and each profession owns a queue of work orders:
what a villager of that trade is expected to make or do. A work order yields
something, and the quality of it comes from the worker's skill and their tools.
The job is a separate field on the decision, the way the motive is, so the
action vocabulary stays atomic.

## The world

Seven places in a ring (`granary → market → well → hut → shrine → woods → field`),
six resources (food, water, wood, herbs, tools, and coin), village standing,
and trust as an 0..4 number per ordered pair. Seasons, time of day, weather,
and temperature affect village life. Villagers have roles, skills, health,
thirst, energy, warmth, moods, goals, and detailed social ties.

The desktop World view shows every stockpile, building condition, hearth,
fertility, and ownership. Click a villager on the map or in the roster to open
the inspector: vitals, inventory, skills, available actions, promises, kinship,
trust, memories, beliefs, facts, and the factors behind their last decision.
The Feed includes saved history and the new crafting, healing, trading,
prayer, repair, and consolation events, with a per-turn digest of who spoke,
who was silent, who was discredited, and which acts and motives the turn
produced.

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
  of food, neutral trust toward everyone, and no memories. Births appear in
  the event log and the feed like any other event.

## Observer, not god

You can whisper a thought, leave food, or turn the weather. Every intervention
is logged as a `divine` event, and agents only remember it if they were there.
You cannot edit minds or kill directly.

## Control API (127.0.0.1 only)

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | the engine's own status: population cap, birth cost, save path |
| GET | `/state` | world snapshot: agents, places, food, trust, memories |
| GET | `/social` | the social and kinship matrix |
| GET | `/agent/{name}` | one villager, with the factors behind their last decision |
| GET | `/places/{id}` | one place: stores, owner, condition, hearth, fertility |
| GET | `/events?since=N&limit=M` | append-only event log |
| GET | `/log?since_turn=T` | per-turn records: actions, lines, acts, the turn digest |
| POST | `/turn` | `{"turns":N}` |
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
| `society-sim` | world state, legality, effects, memory, RNG, the policy, the speaker, the check, professions | nothing but serde |
| `society-server` | the turn loop, persistence, control API | sim, axum, tokio |
| `society` (bin) | native window, CLI, wiring | server, eframe |

`society-sim` is pure: same seed, same founding, same turns; every rule is
unit-tested without a network call. The decisions live here too, because a
decision that cannot be tested without a process boundary is a decision nobody
will test.

## Tests and the measurements

```bash
cargo test --workspace    # no network, no models
```

They cover the rules that are easy to get wrong and expensive to get wrong:
affordances never offer an illegal move, taking never drives a store negative,
a starving villager cannot give away their last meal, memory caps never evict a
scar, starvation kills on the third turn and leaves scars in the survivors, a
broken promise becomes permanent, and the policy's survival tier still wins
over a villager's motive.

The turn's own rules are covered too: the policy picks a legal action, a line
that is not a repeat of a recent one, the register follows the mood, the
addressee is the act's target, the intensity comes from the act and clears the
rumor threshold, the motive survives the turn while the survival ladder still
wins over it, the shortlist is a subset of the legal actions and breaks a
groove, and no line outside lie mode makes a claim the world does not support.

Five measurements are held over the rewrite, and they are the acceptance bar:

1. **A live multi-turn run** on the real stack: the village lives, grows and
   does not stall.
2. **A metrics diff** against the old engine on the same seed: trust
   distribution, scar count, births and deaths, event-kind mix, distinct-line
   count, discredited rate, turn latency, cost.
3. **The judge A/B.** The old engine records the state, the decision and the
   line for every villager and turn; the new policy is fed those same recorded
   states and the judge scores old choice against new choice, blind. The state
   is identical on both sides, so the comparison isolates the decision.
4. **A soak**: two hundred turns, with the invariants checked - no starvation
   spiral, births happen, trust stays bounded, memory caps hold, no line
   repeats inside its window, the discredited rate inside its band.
5. **Determinism and cost.** The same seed and the same save produce identical
   turns across seeds, a save and reload continues identically, the turn costs
   nothing, and the process makes no network call at all.

Growth and persistence are tested at the two seams that matter. Births are
driven through the pure simulation: a fed, trusting, co-located pair has a child
with the right generation and both parents; hunger, low trust, separation, the
population cap, a short granary, or too-new parents each block one; at most one
birth lands per turn; and a birth in the same turn as a death leaves the scar.
Persistence is driven through save/load against a temporary directory: a
lived-in world round-trips field for field and continues identically after the
reload, an absent file founds fresh, a corrupt or wrong-version file names its
path and fails, and a save that would not load back never replaces a good one.

## Notes and sources

The research notes below are local working files in `models/`; they are not
tracked in this repository, so a clone will not contain them.

- `docs/adr/0001-replace-the-judgment-layer-and-the-voice-model-with-code.md` —
  why the judgment layer and the voice model went, and what was rejected.
- `GLOSSARY.md` — the vocabulary of the village, and the words that were
  retired with the old engine.
- `models/research-simulation-code.md` — Generative Agents, Concordia, OASIS,
  AgentSociety, AI Town: what to port and what to skip.
- `models/research-memory-code.md` — Letta, MemoryOS, A-MEM, mem0, Graphiti,
  Reflexion, ToMi: tiers, write policy, contradiction handling, belief state.
- `models/research-small-models.md` — constrained decoding, small-model agent
  capability, Rust crates, and why the planner/caller split was forced.
- `models/research-small-voice-models.md` — the model comparison behind the
  38% discredited figure.
- `models/research-turn-latency.md` — where the turn time went, and what the
  local model was and was not buying.

Adopted from that reading: reasoning field before the answer in every schema
(field order is what structured-output studies actually fix), 2–5 candidates for
the chooser, bi-temporal facts with code-owned invalidation, beliefs with
`as_of_tick` so divergence and staleness are computed rather than stored, an
importance budget instead of a reflection schedule, and AIMD concurrency - the
last one now gone with the concurrency it was there to bound.
