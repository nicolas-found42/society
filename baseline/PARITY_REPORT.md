# Parity report (tickets #16 and #17)

Measured 2026-10-19 by `crates/server/src/bin/parity` against the frozen
baseline corpus (`baseline/corpus.jsonl`, 30 turns, 278 villager records
captured from `c4467d5`). The judge (the same TypeSafe Jev endpoint the old
judgment layer used) scored the old and new choices blind and paired: both
options in one question, A/B assignment randomized per pair, the mapping
recorded but never shown. Parity is relative: the new engine must not lose by
more than the 95% noise band, +/-5.9 percentage points at this pair count.

## Decisions (ticket #16) - FAILED, stopped after the revision budget

| Round | Revision measured | Pairs | Ties | New wins | Old wins | New win rate | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | the first cut (weights per the spec table) | 184 | 94 | 46 | 138 | 25.0% | loses |
| 2 | `social_need` split: console answers grief only, heal answers illness only, feeders feed | 148 | 130 | 52 | 96 | 35.1% | loses |
| 3 | `profession_ready` 3.0 -> 2.2, `role_duty` 1.6 -> 1.2, `mood_pull` 1.0 -> 1.6 with an appetite/state fit | 143 | 135 | 47 | 96 | 32.9% | loses |

Every revision was justified by the previous round's disagreement matrix
(`baseline/roundN_decisions_pairs.json`): round 2 collapsed the console
over-pick (74 of 184 disagreements became ~8) and round 3 collapsed the heal
over-pick. Agreement with the old choices rose 94 -> 130 -> 135 of 247. The
judged win rate on the remaining decided pairs did not.

**The stop condition is met.** After the third measured revision the new
policy still loses beyond noise. Per the spec the work stops here and reports
rather than lowering the bar; the old engine is NOT deleted.

**Reading, stated plainly.** The old choices were produced by this same judge
family reading a rich judgment-state document; the pair question shows a
thinner digest of the same recorded world. A same-family self-consistency
effect inflates the old side's win rate by an amount this protocol cannot
separate. That caveat is why the bar was made relative and paired - but the
protocol is what was agreed, the number is what it is, and the observation
stands: on the agreed measurement, the old engine wins the decision
comparison. Chasing the remaining gap would mean fitting a clone of the
judge, which the ADR rejects on principle.

## Lines (ticket #17) - within noise of parity on the first measurement

| Round | Revision measured | Pairs | Ties | New wins | Old wins | New win rate | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | the shipped grammar (270 first-clause rules, 81 second-clause forms) | 236 | 42 | 105 | 131 | 44.5% | within noise |

The rule grammar reached the bar with no revision spent. The old voice model
needed a copy filter, a retry path and an AIMD window to reach 62% credible
lines; the grammar's lines are credible by construction and the check catches
the lies.

## Side-by-side outcome run (ticket #17)

Seed 7, 40 turns, recorded in `baseline/side_by_side.json` beside the old
run's `baseline/metrics.json`. Context only; not used to judge the decision.

| Metric | Old engine | New engine |
|---|---|---|
| turns | 40 | 40 |
| population end | 12 | 12 |
| births / deaths | 3 / 0 | 3 / 0 |
| lines | 391 | 344 |
| discredited rate | 0.0% | 7.6% (26; all 26 lies caught) |
| trust mean / sd | 3.42 / 0.75 | 0.94 / 0.94 |

The new world is a harder place: trust runs lower and wider because
discredited lines and accusations actually bite, which the old engine's
near-zero discredited rate never exercised. Whether that is a better village
is the observer's call, not the measurement's.

## Cost

The parity runs cost about $0.05 of judged questions. A new-engine turn costs
$0.00 and performs no network call: the soak test (`the_soak_holds_the_invariants`)
runs 200 turns asserting the invariants offline, at under 50 ms per
nine-villager turn. The old engine's turn remains ~$0.001 and 20,759 input
tokens.

## The decision this report asks of the observer

1. **Accept the lower bar.** Keep both engines as they stand: the legacy
   runtime with the new engine measured beside it, or run the new engine as
   the runtime knowing the decision gap. (`#18`, `#19`, `#20` unblock.)
2. **Keep the old engine.** The new engine stays in the tree as the measured
   alternative; the old engine stays the runtime; the parity tickets close as
   reported here.
3. **Re-admit a small model** for the action choice only, keeping the
   grammar, the check and the professions, which passed their bars.
