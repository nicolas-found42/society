# Replace the judgment layer and the voice model with code

Status: accepted (2026-10-19)

The society will run with no language model in the loop at all. The two cloud
backends the runtime depended on - a judgment layer that chose each villager's
action and a local 350M model that wrote each villager's line - are replaced by
rules in the pure simulation crate: a policy that scores the shortlist, a
speaker that writes a line from a grammar, and a check that decides whether the
line's claims hold. Jev is kept, but only as the oracle that measures whether
the new engine is as sensible as the old one.

## Why

Every question the judgment layer was asked had been measured on this machine
before it was written down, and the measurements were not flattering. The tone
question came back "plain and neutral" for 38 of 45 lines while still moving
trust; the pressure ordinal never passed 2.21, so the world's rumor and
threat-morale branches could not fire; the addressee was asked before the line
existed, so it guessed. Those three were moved into code in the previous
commits, and the register, the intensity and the addressee are now derived.

What remains is the action choice, the motive label and the gate. The motive is
the same reading as the action, named. The gate can only find a line's false
claims if a model can write them: a rule-based speaker cannot invent a number,
so the gate's job shrinks to checking a deliberately made lie. The action
choice is the one real judgement left, and it is a choice among two to five
options the code already computed.

Against that, the cost: two cloud calls and up to eighteen local calls per
turn, about $0.001 and 20,759 input tokens per turn, an API key the app refuses
to start without, and a world that halts when a network call fails.

## Considered options and why they lost

- **Keep the judgment layer for the action choice.** The README measured 3.88/4
  for the judge against 3.12/4 for a deterministic policy. That gap is a
  measurement of one naive policy, not a verdict on code, but it is the only
  evidence that exists and it points the other way. Rejected: the policy is
  built to be measured against the same protocol, and the measurement, not the
  assumption, decides.
- **Keep the voice model.** It is the weakest link measured: 38% of
  LFM2.5-350M lines were discredited, 21% for the better 0.6B model, and the
  copy loop it caused needed its own filter, its own retry and its own
  infrastructure. Rejected: a rule-based speaker cannot copy a card or invent a
  fact, so the copy filter, the retry and the concurrency machinery all become
  unnecessary.
- **Keep both backends pluggable behind a trait.** Rejected: roughly 1,500
  lines of dead-by-default code and two paths to keep honest, for a comparison
  that the measurement harness performs offline instead.
- **Fit the policy's weights to the judge's answers.** Rejected: that builds a
  clone of the judge, which drifts the moment the state changes and is the
  opposite of the point. The policy is designed for the state the village will
  have, and the judge measures it.
- **A phrase bank instead of a grammar.** Rejected: the village's speech would
  be a small number of recognisable shapes, and the depth the world is heading
  toward needs a grammar whose rules are added to, not replaced.

## Consequences

- The doctrine that judgment is not optional inverts. The society can no longer
  fail to think, so the app needs no API key, no Ollama daemon and no network,
  and a turn costs a fraction of a millisecond instead of about 700 ms.
- The discredited mechanic - a line that contradicts the world is spoken but
  moves no trust - survives, but its source changes. Lying becomes designed:
  a villager under hunger, grudge, fear or a promise about to be missed makes a
  claim the world does not support, and the check catches it when the claim can
  be settled at all. A lie about the past, about intent or about a quantity
  nobody present can see is unverifiable, so it stays credible and can move
  trust on false information. That is deliberate: without it every lie would be
  caught by construction and lying would never work, which is not what the old
  engine did.
- The speaker's rules are keyed on the action, not the social act. Thirteen of
  the twenty actions perform no social act, so keying on the act would put seven
  lines in ten into one shared bank. The act key is used only for `speak`, where
  the policy chooses the act and the line has to perform it.
- The published judgment-state document and the prompt card are dropped. The
  policy reads the world directly, so a rendered JSON brief for a model that no
  longer exists has no reader. The inspector shows the policy's decision
  factors instead.
- The save format is free to change, and the existing village is discarded
  rather than migrated.
- The 3.88 against 3.12 gap becomes the acceptance bar rather than a footnote.
  The bar is parity, measured by the same judge that produced the original
  number, and a failure to reach it stops the work rather than lowering it.
