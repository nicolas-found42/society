//! The speaker (spec #5, tickets #12 and #14): the rule that turns a decided
//! intent into one spoken line.
//!
//! One line per speaking villager, rendered from a rule grammar over typed
//! slots filled with what the villager can observe or remember. The rule key
//! is the action, not the act, except for `speak`, whose act is the policy's
//! choice. A per-villager ledger forbids an exact repeat inside its window.
//! A villager with nobody to address speaks no line; silence is recorded as
//! silence.
//!
//! Lies are designed: under the lie table's conditions the speaker enters lie
//! mode at a documented rate and appends a clause carrying a false claim.
//! The check decides whether that claim is settleable; an unverifiable lie
//! lands.

use crate::action::SocialAct;
use crate::check::Claim;
use crate::grammar::{
    act_key, lie_claim, place_word, register_for_mood, second_for, LieKind, FIRST, LIE_CLAUSES,
};

use crate::policy::{agent_rng, Decision, PROMISE_OVERDUE_TURNS};
use crate::profession;
use crate::world::{Agent, Place, World};

/// A line the ledger forbids repeating, measured in turns (spec constants).
pub const LEDGER_WINDOW: u64 = 6;
/// The word budget: one sentence, twenty words (spec constants).
pub const WORD_BUDGET: usize = 20;
/// The chance a rendered line carries a second clause.
pub(crate) const SECOND_CLAUSE_RATE: f32 = 0.35;
/// The lie base rate: 6% of spoken lines (spec constants).
pub const LIE_BASE_RATE: f32 = 0.06;
/// No state turns a villager into a compulsive liar.
pub const LIE_RATE_CAP: f32 = 0.20;

// The lie table multipliers (spec: "The lie table").
pub const LIE_HUNGER_MULT: f32 = 2.0;
pub const LIE_GRUDGE_MULT: f32 = 1.8;
pub const LIE_THREAT_MULT: f32 = 1.5;
pub const LIE_PROMISE_MULT: f32 = 1.6;
pub const LIE_HOPE_MULT: f32 = 1.3;
/// The grudge at or above which the addressee earns the lie multiplier.
pub(crate) const LIE_GRUDGE: f32 = 1.5;
/// Hope below this multiplies the lie rate.
pub const LIE_HOPE: f32 = 1.0;

/// One rendered line: its text, the claims it makes, the act it performed,
/// whom it addressed, and whether it was produced in lie mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub claims: Vec<Claim>,
    pub act: SocialAct,
    pub addressee: Option<String>,
    pub lie: bool,
}

/// A villager with nobody to address speaks no line (`None`); silence is the
/// recorded absence.
#[must_use]
pub fn speak(w: &World, name: &str, d: &Decision) -> Option<Line> {
    let a = w.agent(name)?;
    if !a.alive {
        return None;
    }
    let others = w.present(a.place, name);
    if others.is_empty() {
        return None;
    }
    let mut rng = agent_rng(w, name, "line");
    let register = register_for_mood(a.mood);
    let key = if d.action == "speak" {
        act_key(d.act)
    } else if d.action.starts_with("go:") {
        "go".to_owned()
    } else {
        d.action.clone()
    };

    // Exhaustion: every rendering is spent, so the villager says the plain
    // fallback outside the grammar rather than repeat - unless the fallback
    // itself is spent, in which case silence (ticket #21: the fallback is
    // one shared line, so two exhausted turns running would repeat it
    // inside the window).
    let (text, mut claims) = if let Some(rendered) = render(w, a, d, &key, register, &mut rng) {
        rendered
    } else {
        let fallback = sentence("I will see to it");
        if recent_lines(w, a).contains(&fallback) {
            return None;
        }
        (fallback, Vec::new())
    };
    // Lie mode (ticket #14): the rate comes from the lie table, the clause
    // carries a false claim, and the check does the rest.
    let rate = lie_rate(w, a, d.addressee.as_deref());
    let lie = rng.chance(rate);
    let mut lie = lie;
    if lie {
        if let Some((clause, claim)) = render_lie(w, a, d, &mut rng) {
            let with_clause = if clause.starts_with("I ") {
                format!("{text}, and {clause}")
            } else {
                format!("{text}. {clause}")
            };
            let with_clause_words = with_clause.split_whitespace().count();
            // The lie clause takes the second clause's budget: a lie that does
            // not fit would not be a designed lie, so it replaces decoration.
            if with_clause_words <= WORD_BUDGET + 8 {
                claims.push(claim);
                return Some(finish(&with_clause, claims, d, true));
            }
            // The clause alone is the line.
            claims.clear();
            claims.push(claim);
            return Some(finish(&clause, claims, d, true));
        }
        lie = false;
    }
    Some(finish(&text, claims, d, lie))
}

fn finish(text: &str, claims: Vec<Claim>, d: &Decision, lie: bool) -> Line {
    Line {
        text: sentence(text),
        claims,
        act: d.act,
        addressee: d.addressee.clone(),
        lie,
    }
}

/// Capitalise and period: one sentence out of the clause pair. Also fixes
/// the one agreement a slot can break: "one measures".
fn sentence(s: &str) -> String {
    let s = s.trim();
    let s = s.replace("one measures", "one measure");
    let mut out = String::with_capacity(s.len() + 1);
    let mut chars = s.chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
    while out.ends_with(['.', ',', ';', ' ']) {
        out.pop();
    }
    out.push('.');
    out
}

/// Render the first clause from the cell's rules and fill the second when the
/// budget holds. Returns `None` when every rendering is spent: the caller
/// falls back to a line outside the grammar rather than repeat.
fn render(
    w: &World,
    a: &Agent,
    d: &Decision,
    key: &str,
    register: &str,
    rng: &mut crate::rng::Rng,
) -> Option<(String, Vec<Claim>)> {
    // Ledger: exact repeats forbidden inside the window. The ledger holds
    // finished lines (ticket #21: raw templates never equal ledger entries,
    // so comparing unfinished text could never fire), so candidates are
    // finished first, then compared, in rng order.
    let spent: Vec<String> = recent_lines(w, a);
    let cell: Vec<&&'static str> = FIRST
        .iter()
        .find(|((k, r), _)| *k == key && *r == register)
        .map(|(_, rules)| rules.iter().collect())
        .unwrap_or_default();
    // A cell always has rules (asserted by the registry test); the plain
    // register is the fallback if a key slipped the table.
    let cell: Vec<&&'static str> = if cell.is_empty() {
        FIRST
            .iter()
            .find(|((k, r), _)| *k == key && *r == "plain")
            .map(|(_, rules)| rules.iter().collect())
            .unwrap_or_default()
    } else {
        cell
    };
    if cell.is_empty() {
        return None;
    }

    let second = second_for(key);
    let mut order: Vec<usize> = (0..cell.len()).collect();
    rng.shuffle(&mut order);
    for idx in &order {
        let template = cell[*idx];
        // The second clause is decided per candidate: trying the bare clause
        // only would report exhaustion when a decorated rendering is still
        // free, and trying only the decorated one would miss the bare form.
        for decorated in [true, false] {
            let branch = rng.next_u64();
            let seeded = crate::rng::Rng::new(branch);
            if let Some((text, claims)) =
                finish_candidate(w, a, d, template, second, decorated, seeded)
            {
                if !spent.contains(&text) {
                    *rng = crate::rng::Rng::new(branch.wrapping_add(1));
                    return Some((text, claims));
                }
            }
        }
    }
    None
}

/// One candidate rendering of a template: filled, finished, and decorated
/// with the matched second clause when asked and inside the word budget.
fn finish_candidate(
    w: &World,
    a: &Agent,
    d: &Decision,
    template: &str,
    second: Option<(&'static str, [&'static str; 3])>,
    decorated: bool,
    mut rng: crate::rng::Rng,
) -> Option<(String, Vec<Claim>)> {
    let (text, claims) = fill(w, a, d, template, &mut rng);
    if !decorated {
        return Some((sentence(&text), claims));
    }
    if !rng.chance(SECOND_CLAUSE_RATE) {
        return None;
    }
    let (connective, forms) = second?;
    let form = forms[rng.below(forms.len())];
    let (clause, mut more) = fill(w, a, d, form, &mut rng);
    let words = text.split_whitespace().count() + clause.split_whitespace().count() + 1;
    if words > WORD_BUDGET {
        return None;
    }
    let mut claims = claims;
    claims.append(&mut more);
    Some((sentence(&format!("{text}, {connective} {clause}")), claims))
}
/// Every filled count slot contributes its claim; a person contributes their
/// presence.
#[allow(
    clippy::too_many_lines,
    reason = "one sequential slot table; splitting hides the scope rule"
)]
fn fill(
    w: &World,
    a: &Agent,
    d: &Decision,
    template: &str,
    rng: &mut crate::rng::Rng,
) -> (String, Vec<Claim>) {
    let others = w.present(a.place, &a.name);
    let mut claims: Vec<Claim> = Vec::new();
    let here = w.place_state(a.place);
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        let slot = &rest[start + 1..start + end];
        match slot {
            "count" => {
                out.push_str(&resource_phrase("food", a.food));
                claims.push(Claim::Count {
                    resource: "food",
                    value: a.food,
                });
            },
            "store" => {
                out.push_str("the store");
                claims.push(Claim::Store {
                    place: a.place,
                    resource: "food",
                    value: here.food,
                });
            },
            "granary" => {
                let g = w.place_state(Place::Granary).food;
                out.push_str("the granary");
                let _ = g;
                claims.push(Claim::Store {
                    place: Place::Granary,
                    resource: "food",
                    value: g,
                });
            },
            "water" => {
                out.push_str(&resource_phrase("water", a.water));
                claims.push(Claim::Count {
                    resource: "water",
                    value: a.water,
                });
            },
            "wood" => {
                out.push_str(&resource_phrase("wood", a.wood));
                claims.push(Claim::Count {
                    resource: "wood",
                    value: a.wood,
                });
            },
            "herbs" => {
                out.push_str(&resource_phrase("herbs", a.herbs));
                claims.push(Claim::Count {
                    resource: "herbs",
                    value: a.herbs,
                });
            },
            "tools" => {
                out.push_str(&resource_phrase("tools", a.tools));
                claims.push(Claim::Count {
                    resource: "tools",
                    value: a.tools,
                });
            },
            "coin" => {
                out.push_str(&resource_phrase("coin", a.coin));
                claims.push(Claim::Count {
                    resource: "coin",
                    value: a.coin,
                });
            },
            "person" => {
                let person = d.addressee.clone().unwrap_or_else(|| pick(&others, rng, a));
                out.push_str(&person);
                claims.push(Claim::PersonPresent { name: person });
            },
            "other" => {
                let person = pick(&others, rng, a);
                out.push_str(&person);
                claims.push(Claim::PersonPresent { name: person });
            },
            "place" => {
                let place = d
                    .place
                    .unwrap_or_else(|| Place::ALL[rng.below(Place::ALL.len())]);
                out.push_str(place_word(place));
            },
            "obligation" => {
                out.push_str(&obligation_phrase(a));
            },
            "quality" => {
                let q = profession::maker_quality(a);
                out.push_str(quality_word(q));
                claims.push(Claim::Quality {
                    item: "tool",
                    value: q,
                });
            },
            other => {
                // An unknown slot is a template bug; render it inert rather
                // than leak braces into the chronicle.
                out.push_str("something");
                let _ = other;
            },
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    (out, claims)
}

/// A count reads as a villager speaks it: "no", "one", "two"... digits past
/// twelve. The typed claim keeps the number; only the surface changes.
fn count_word(n: u32) -> String {
    match n {
        0 => "no".into(),
        1 => "one".into(),
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        6 => "six".into(),
        7 => "seven".into(),
        8 => "eight".into(),
        9 => "nine".into(),
        10 => "ten".into(),
        11 => "eleven".into(),
        12 => "twelve".into(),
        other => other.to_string(),
    }
}

/// A resource slot renders as the noun phrase a villager would say, count
/// included. The typed claim stays numeric.
fn resource_phrase(resource: &str, n: u32) -> String {
    let word = count_word(n);
    match resource {
        "food" => match n {
            0 => "no grain".into(),
            1 => "a measure".into(),
            _ => format!("{word} measures"),
        },
        "water" => match n {
            0 => "no water".into(),
            1 => "a pail of water".into(),
            _ => format!("{word} pails of water"),
        },
        "wood" => match n {
            0 => "no wood".into(),
            1 => "a load of wood".into(),
            _ => format!("{word} loads of wood"),
        },
        "herbs" => match n {
            0 => "no herbs".into(),
            1 => "a handful of herbs".into(),
            _ => format!("{word} handfuls of herbs"),
        },
        "tools" => match n {
            0 => "no tools".into(),
            1 => "a tool".into(),
            _ => format!("{word} tools"),
        },
        "coin" => match n {
            0 => "no coin".into(),
            1 => "a coin".into(),
            _ => format!("{word} coins"),
        },
        _ => word,
    }
}

fn pick(others: &[String], rng: &mut crate::rng::Rng, a: &Agent) -> String {
    let candidates: Vec<&String> = others.iter().filter(|n| ***n != *a.name).collect();
    let fallback = others
        .first()
        .cloned()
        .unwrap_or_else(|| "friend".to_owned());
    candidates
        .get(rng.below(candidates.len()))
        .map_or(fallback, |s| (*s).clone())
}

/// The duty phrase a `{obligation}` slot renders when no promise is held.
fn obligation_phrase(a: &Agent) -> String {
    if let Some(pr) = &a.promise {
        if pr.kept.is_none() {
            return pr.text.clone();
        }
    }
    match a.role {
        crate::world::Role::Steward => "keep the stores true".to_owned(),
        crate::world::Role::Farmer => "bring the harvest in".to_owned(),
        crate::world::Role::Woodcutter => "keep the woodpile high".to_owned(),
        crate::world::Role::Herbalist => "heal whoever is feverish".to_owned(),
        crate::world::Role::Artisan => "keep the tools sound".to_owned(),
        crate::world::Role::Watchman => "walk the ring tonight".to_owned(),
        crate::world::Role::Elder => "keep the village's memory".to_owned(),
        crate::world::Role::Trader => "fetch a fair price".to_owned(),
        crate::world::Role::Forager => "find what the woods hide".to_owned(),
    }
}

const fn quality_word(q: u8) -> &'static str {
    match q {
        1 => "crude",
        2 => "plain",
        3 => "sound",
        4 => "fine",
        _ => "masterwork",
    }
}

// ---- The ledger -------------------------------------------------------------

/// The villager's still-live lines: everything spoken within the window.
#[must_use]
pub(crate) fn recent_lines(w: &World, a: &Agent) -> Vec<String> {
    a.line_ledger
        .iter()
        .filter(|(t, _)| w.turn.saturating_sub(*t) < LEDGER_WINDOW)
        .map(|(_, s)| s.clone())
        .collect()
}

/// Record a spoken line in the villager's ledger and trim past the window.
/// The turn seam calls this after the line is committed.
pub(crate) fn ledger_push(w: &mut World, name: &str, line: &str) {
    if let Some(a) = w.agent(name) {
        let turn = w.turn;
        let mut ledger: Vec<(u64, String)> = a
            .line_ledger
            .iter()
            .filter(|(t, _)| turn.saturating_sub(*t) < LEDGER_WINDOW)
            .cloned()
            .collect();
        ledger.push((turn, line.to_owned()));
        if let Some(a) = w.agent_mut(name) {
            a.line_ledger = ledger;
        }
    }
}

// ---- The lie table -----------------------------------------------------------

/// The lie rate for this line: the base rate times each satisfied condition's
/// multiplier, capped (spec: "The lie table"). Each constant is named in the
/// simulation crate with its table row.
#[must_use]
pub(crate) fn lie_rate(w: &World, a: &Agent, addressee: Option<&str>) -> f32 {
    let mut rate = LIE_BASE_RATE;
    // hunger is 3 with no carried food: x2.0
    if a.hunger == 3 && a.food == 0 {
        rate *= LIE_HUNGER_MULT;
    }
    // a grudge of 1.5 or more against the addressee: x1.8
    if let Some(to) = addressee {
        if a.relation_with(to).is_some_and(|r| r.grudge >= LIE_GRUDGE) {
            rate *= LIE_GRUDGE_MULT;
        }
    }
    // a threat was overheard this turn (fear): x1.5
    #[allow(
        clippy::suspicious_operation_groupings,
        reason = "an event's actor against the villager's name is the intended join"
    )]
    let overheard = w.events.iter().any(|e| {
        e.turn == w.turn
            && e.place == a.place
            && e.actor != a.name
            && e.social.as_deref() == Some("threat")
    });
    if overheard {
        rate *= LIE_THREAT_MULT;
    }
    // an unkept promise older than the overdue threshold: x1.6
    if let Some(pr) = &a.promise {
        if pr.kept.is_none() && w.turn.saturating_sub(pr.made_turn) > PROMISE_OVERDUE_TURNS {
            rate *= LIE_PROMISE_MULT;
        }
    }
    // the villager's hope is below 1.0: x1.3
    if a.hope < LIE_HOPE {
        rate *= LIE_HOPE_MULT;
    }
    rate.min(LIE_RATE_CAP)
}

/// Render one lie clause with a false claim (ticket #14: a line inside lie
/// mode makes at least one false claim).
pub(crate) fn render_lie(
    w: &World,
    a: &Agent,
    d: &Decision,
    rng: &mut crate::rng::Rng,
) -> Option<(String, Claim)> {
    let kinds = [
        LieKind::InflatedCount,
        LieKind::PastGift,
        LieKind::HiddenCache,
        LieKind::FakePromise,
        LieKind::ClaimedVisit,
        LieKind::OthersIntent,
    ];
    let kind = kinds[rng.below(kinds.len())];
    render_lie_kind(w, a, d, kind, rng)
}

/// One lie kind's clause, filled from the villager's own knowledge. `None`
/// when the clause would happen to be true: no lie, no clause.
pub(crate) fn render_lie_kind(
    w: &World,
    a: &Agent,
    d: &Decision,
    kind: LieKind,
    rng: &mut crate::rng::Rng,
) -> Option<(String, Claim)> {
    let (_, forms) = LIE_CLAUSES.iter().find(|(k, _)| *k == kind)?;
    let template = forms[rng.below(forms.len())];
    let others = w.present(a.place, &a.name);
    let person = d.addressee.clone().unwrap_or_else(|| pick(&others, rng, a));
    // The inflated count is the lie: one to four more than carried. It is
    // substituted before the filler runs, so the filler never writes the true
    // count into a lie clause.
    let count = a.food + 1 + rng.below(4) as u32;
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "the braces are the grammar's slot syntax, not a format string"
    )]
    let template = template.replace("{count}", &count.to_string());
    let (text, _true_claims) = fill(w, a, d, template.as_str(), rng);
    let claim = lie_claim(kind, &person, count);
    // A lie clause is only a lie when its claim actually settles false or
    // rests on a falsehood: the inflated count is always false, the fake
    // promise false when no promise exists, the rest unverifiable by kind.
    let settles_false = matches!(
        crate::check::settle(w, &a.name, &claim),
        crate::check::Verdict::False
    );
    let unverifiable =
        crate::check::settle(w, &a.name, &claim) == crate::check::Verdict::Unverifiable;
    (settles_false || unverifiable).then_some((text, claim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check;
    use crate::policy;

    #[test]
    fn an_exhausted_cell_returns_no_spent_line() {
        // Ticket #21: the (console, plain) cell holds two templates. With
        // both finished lines spent, render must report exhaustion rather
        // than repeat one of them; the ledger check runs on finished lines,
        // because raw templates never equal ledger entries.
        let mut w = World::founding(77);
        w.agents.clear();
        let ada = Agent::new("Ada", "plain", Place::Hut, 2, 0, 0, 0, vec![], &[]);
        let bo = Agent::new("Bo", "plain", Place::Hut, 2, 0, 0, 0, vec![], &[]);
        w.agents.push(ada);
        w.agents.push(bo);
        // Spend every finished rendering of the two-templated (console,
        // plain) cell: both first clauses, each with and without its matched
        // second clause, as committed through the real seam.
        for seed in 0..200_u64 {
            let mut w2 = World::founding(seed);
            w2.agents.clear();
            w2.agents.push(Agent::new("Ada", "plain", Place::Hut, 2, 0, 0, 0, vec![], &[]));
            w2.agents.push(Agent::new("Bo", "plain", Place::Hut, 2, 0, 0, 0, vec![], &[]));
            let d2 = Decision {
                action: "console".to_owned(),
                verb: crate::action::Verb::Console,
                place: None,
                act: SocialAct::Counsel,
                addressee: Some("Bo".to_owned()),
                motive: policy::Motive::Duty,
                job: None,
                confidence: 1.0,
                dilemma: None,
                factors: vec![],
                rejected: vec![],
            };
            if let Some(line) = speak(&w2, "Ada", &d2) {
                ledger_push(&mut w, "Ada", &line.text);
            }
        }
        let a = w.agent("Ada").unwrap().clone();
        let d = Decision {
            action: "console".to_owned(),
            verb: crate::action::Verb::Console,
            place: None,
            act: SocialAct::Counsel,
            addressee: Some("Bo".to_owned()),
            motive: policy::Motive::Duty,
            job: None,
            confidence: 1.0,
            dilemma: None,
            factors: vec![],
            rejected: vec![],
        };
        let mut rng = crate::rng::Rng::new(1);
        let out = render(&w, &a, &d, "console", "plain", &mut rng);
        assert!(out.is_none(), "exhausted cell returned spent {out:?}");
    }

    fn world_alone() -> World {
        let mut w = World::founding(11);
        w.agents.retain(|a| a.name == "Ada");
        w
    }

    #[test]
    fn a_villager_with_nobody_present_is_silent() {
        let w = world_alone();
        let d = policy::decide(&w, "Ada").unwrap();
        assert_eq!(speak(&w, "Ada", &d), None, "nobody to address");
    }

    #[test]
    fn a_line_never_leaks_a_brace() {
        let mut w = World::founding(12);
        w.agents.retain(|a| a.name == "Ada" || a.name == "Bo");
        let d = policy::decide(&w, "Ada").unwrap();
        if let Some(line) = speak(&w, "Ada", &d) {
            assert!(!line.text.contains('{'), "{}", line.text);
        }
    }

    #[test]
    fn outside_lie_mode_every_claim_is_true_or_unverifiable() {
        for seed in [1_u64, 2, 3, 4, 5, 6, 7, 8] {
            let mut w = World::founding(seed);
            for _ in 0..6 {
                for name in w.living().map(|a| a.name.clone()) {
                    let Some(d) = policy::decide(&w, &name) else {
                        continue;
                    };
                    if let Some(line) = speak(&w, &name, &d) {
                        if !line.lie {
                            for c in &line.claims {
                                assert_ne!(
                                    check::settle(&w, &name, c),
                                    check::Verdict::False,
                                    "seed {seed}: accidental false claim in {:?}",
                                    line.text
                                );
                            }
                        }
                    }
                }
                w.settle();
            }
        }
    }

    #[test]
    fn the_lie_rate_is_the_table_applied_and_capped() {
        let mut w = World::founding(13);
        w.agents.retain(|a| a.name == "Ada");
        w.agent_mut("Ada").unwrap().hope = 2.5;
        let base = lie_rate(&w, w.agent("Ada").unwrap(), None);
        assert!((base - LIE_BASE_RATE).abs() < 1e-6);
        {
            let ada = w.agent_mut("Ada").unwrap();
            ada.hunger = 3;
            ada.food = 0;
            ada.hope = 0.5;
        }
        let stacked = lie_rate(&w, w.agent("Ada").unwrap(), None);
        assert!(
            (LIE_BASE_RATE * LIE_HUNGER_MULT)
                .mul_add(-LIE_HOPE_MULT, stacked)
                .abs()
                < 1e-6
        );
        assert!(stacked <= LIE_RATE_CAP);
    }
}
