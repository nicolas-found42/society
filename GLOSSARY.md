# society

A living settlement of nine villagers in a ring of seven places: they hunger,
grudge, promise, lie, remember, and are remembered. You watch.

## The settlement

**Villager**:
A member of the settlement, with their own hunger, memory, trust and kin.
_Avoid_: agent, character, NPC

**Place**:
One of the seven fixed stations in the ring: granary, market, well, hut, shrine,
woods, field.
_Avoid_: location, node, tile

**Ring**:
The closed path through the seven places. A villager can only walk to a place
that touches theirs.
_Avoid_: map, cycle, board

**Measure**:
The whole unit food is counted in.
_Avoid_: unit, portion, ration

**Granary**:
The settlement's store of food. The field and the woods yield into it, because a
field that keeps its grain is a field nobody visits.
_Avoid_: storehouse, silo, larder

**Standing**:
A villager's regard in the settlement as a whole, as distinct from the regard
one villager holds for another.
_Avoid_: reputation, fame, prestige

**Morale**:
The settlement's shared confidence, which rises and falls with what happens
inside it.
_Avoid_: mood, spirits

**Season**:
One of the four divisions of the year. Winter closes the work the other seasons
allow.
_Avoid_: weather

**Divine**:
An intervention from outside the settlement. Every intervention is logged, and
the villagers remember it only if they were there.
_Avoid_: intervention, command, cheat

**Observer**:
Whoever watches: the person at the window, the reader of the control API.
_Avoid_: user, player, admin, god

## A villager

**Trust**:
The regard one villager holds for another, from 0 to 4, and never the same in
both directions.
_Avoid_: affinity, relation score, regard

**Kinship**:
The family tie between two villagers, separate from what either of them thinks
of the other.
_Avoid_: relation, family, tie

**Grudge**:
The stored resentment a betrayal or a broken promise leaves behind.
_Avoid_: anger, feud, resentment

**Promise**:
A commitment one villager makes to another to do something later. An unkept
promise becomes a scar.
_Avoid_: oath, debt, contract, bargain

**Scar**:
A permanent memory of a wrong. Scars never decay and are never evicted.
_Avoid_: trauma, permanent memory, grudge

**Memory**:
One thing a villager remembers, carrying a salience and a kind. A villager holds
a small ring of them.
_Avoid_: log, history, record

**Mood**:
The villager's present state, which decides what tone they speak in.
_Avoid_: emotion, temper, register

**Register**:
The tone a villager speaks in, derived from their mood.
_Avoid_: tone, voice, mood

**Profession**:
A trade's queue of work orders: what a villager of that trade is expected to
make or do.
_Avoid_: job, task, order, craft, career

**Work order**:
One item of a profession's queue. What it yields, and how well, depends on the
worker's skill and tools.
_Avoid_: task, job, recipe, order

**Quality**:
How well a made thing was made, from the worker's skill and tools.
_Avoid_: grade, tier, condition

**Goal**:
What a villager is currently trying to achieve, in their own words.
_Avoid_: intent, motive, plan

## A turn

**Turn**:
One round in which every living villager acts once.
_Avoid_: step, tick, round, cycle

**Affordance**:
An action a villager is allowed to take right now.
_Avoid_: option, move, legal action

**Shortlist**:
The few affordances a villager is offered each turn: one per kind of act, minus
whatever they have just done.
_Avoid_: menu, options, candidates

**Intent**:
The one thing a villager decides to do this turn.
_Avoid_: choice, decision, plan, action

**Motive**:
Why a villager acts this turn, named as one word: hunger, thirst, kinship,
promise, fear or duty.
_Avoid_: reason, rationale, justification

**Social act**:
What a line does toward someone present: a promise, a gift, an accusation, a
threat, gossip, a plea, gratitude, counsel, a trade offer, a blessing,
forgiveness, or nothing.
_Avoid_: speech act, dialogue type, mood, tone

**Addressee**:
The villager a line was spoken to.
_Avoid_: listener, target, recipient

**Dilemma**:
The hesitation a villager shows when they were nearly pulled two ways.
_Avoid_: conflict, tie, indecision

**Credible**:
A line whose claims are each true or unverifiable, and which may therefore move
trust.
_Avoid_: trusted, valid, true

**Discredited**:
A line that states something the world contradicts. It is still spoken, but it
moves no trust.
_Avoid_: rejected, false, failed, ignored

**Unverifiable**:
A claim no one in the village can settle - about the past, about intent, or
about a quantity nobody present can see. It is not false, so the line stays
credible, which is why a lie can land.
_Avoid_: unknowable, unknown, doubtful

**Lie**:
A claim a villager deliberately makes that the world does not support.
_Avoid_: falsehood, error, mistake, discredited

**Silence**:
A turn in which a villager has nobody to address, and so speaks no line.
_Avoid_: dropped line, empty line, mute

## The engine

**The policy**:
The rule that chooses one intent per villager per turn from the shortlist.
_Avoid_: judgment layer, judge, chooser, planner, brain

**The speaker**:
The rule that turns a decided intent into one spoken line.
_Avoid_: voice, voice model, narrator, mouth

**The check**:
The rule that compares a line's claims against the world and marks the line
discredited when they do not hold.
_Avoid_: gate, verifier, validator, judge

**Claim**:
One assertion a line makes about the world: a count, a person, a possession, a
place, an obligation.
_Avoid_: fact, statement, detail
