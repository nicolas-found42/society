//! The policy (spec #5, ticket #9): the rule that chooses one intent per
//! villager per turn from the shortlist.
//!
//! Tier 0 (survival) and tier 1 (obligation) are hard precedence; tier 2
//! scores weighted feature terms over the shortlist, and the scores become a
//! distribution so confidence and the dilemma survive as real numbers. The
//! policy reads the world directly and is a pure function of (world, name):
//! the same world always yields the same decision.

use serde::{Deserialize, Serialize};

use crate::action::{shortlist, SocialAct, Verb, NOVELTY_WINDOW};
use crate::catalog::{self, Category, Need, SPEECH_MENU};
use crate::profession;
use crate::rng::Rng;
use crate::world::{Agent, Kinship, Mood, Place, Season, World};

/// The dilemma margin, kept from the measured spec constants.
pub const DILEMMA_MARGIN: f32 = 0.12;
/// The runner-up must hold at least a fifth of the mass for a dilemma.
pub const DILEMMA_RUNNERUP_MASS: f32 = 0.20;
/// Softmax temperature (spec constants).
pub const POLICY_TEMPERATURE: f32 = 0.75;
/// Survival thresholds, the goal ladder's own (spec: "Survival and obligation
/// thresholds").
pub const SURVIVAL_HUNGER: u8 = 2;
pub const SURVIVAL_THIRST: u8 = 2;
pub const SURVIVAL_WARMTH: u8 = 1;
/// A grudge at or above this against someone present is obligation (tier 1).
pub const OBLIGATION_GRUDGE: f32 = 1.5;
/// An unkept promise older than this many turns multiplies the lie rate.
pub const PROMISE_OVERDUE_TURNS: u64 = 7;

/// Why a villager acts this turn, named as one word (glossary: motive). The
/// motive is derived from the action and the world, never asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Motive {
    Hunger,
    Thirst,
    Kinship,
    Promise,
    Fear,
    Duty,
}

impl Motive {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Hunger => "hunger",
            Self::Thirst => "thirst",
            Self::Kinship => "kinship",
            Self::Promise => "promise",
            Self::Fear => "fear",
            Self::Duty => "duty",
        }
    }
}

/// The hesitation a villager shows when they were nearly pulled two ways.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dilemma {
    pub chosen: String,
    pub alternative: String,
    pub chosen_prob: f32,
    pub alternative_prob: f32,
}

/// One candidate's score and the term contributions behind it.
type Scored = (String, f32, Vec<(&'static str, f32)>);

/// One candidate the policy scores: an (action, act) pair. A dedicated action
/// fixes its act; a `speak` action carries one candidate per speech-menu act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub action: String,
    pub verb: Verb,
    pub place: Option<Place>,
    pub act: SocialAct,
}

impl Candidate {
    fn key(&self) -> String {
        if self.act == SocialAct::None {
            self.action.clone()
        } else {
            format!("{}:{}", self.action, self.act.id())
        }
    }
}

/// Why a villager is doing what they are doing, in the policy's words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub action: String,
    pub verb: Verb,
    pub place: Option<Place>,
    pub act: SocialAct,
    /// Whom the act addresses, bound by the act (ticket #10).
    pub addressee: Option<String>,
    pub motive: Motive,
    /// The work order this decision advances, when the action is work.
    pub job: Option<&'static str>,
    pub confidence: f32,
    pub dilemma: Option<Dilemma>,
    /// The chosen candidate's term contributions, six to twelve entries.
    pub factors: Vec<(&'static str, f32)>,
}

// ---- The feature-term registry (ticket #8) ------------------------------
//
// Each term is a pure function of (world, villager, candidate) with a
// documented weight. A new consideration is a new term, not a new branch.

pub(crate) struct Term {
    pub name: &'static str,
    pub weight: f32,
    /// `true` when a higher raw score must *raise* the weighted score.
    pub f: fn(&World, &Agent, &Candidate) -> f32,
}

/// The twelve pinned terms (spec: "Feature-term weights"). The weights are the
/// first cut; a revision is a design change reported with its A/B result.
pub(crate) const TERMS: [Term; 12] = [
    Term {
        name: "profession_ready",
        weight: 3.0,
        f: term_profession_ready,
    },
    Term {
        name: "promise_pull",
        weight: 2.5,
        f: term_promise_pull,
    },
    Term {
        name: "social_need",
        weight: 2.2,
        f: term_social_need,
    },
    Term {
        name: "grudge_pull",
        weight: 2.0,
        f: term_grudge_pull,
    },
    Term {
        name: "kinship_pull",
        weight: 1.8,
        f: term_kinship_pull,
    },
    Term {
        name: "role_duty",
        weight: 1.6,
        f: term_role_duty,
    },
    Term {
        name: "place_need",
        weight: 1.4,
        f: term_place_need,
    },
    Term {
        name: "trust_pull",
        weight: 1.2,
        f: term_trust_pull,
    },
    Term {
        name: "mood_pull",
        weight: 1.0,
        f: term_mood_pull,
    },
    Term {
        name: "novelty",
        weight: 0.8,
        f: term_novelty,
    },
    Term {
        name: "energy_cost",
        weight: 0.5,
        f: term_energy_cost,
    },
    Term {
        name: "ownership",
        weight: 0.4,
        f: term_ownership,
    },
];

const fn is_workish(c: &Candidate) -> bool {
    matches!(
        c.verb,
        Verb::Tend
            | Verb::Chop
            | Verb::GatherHerbs
            | Verb::Craft
            | Verb::Forage
            | Verb::Repair
            | Verb::Take
            | Verb::DrawWater
            | Verb::Trade
    )
}

/// 1.0 when a work order of the villager's profession is waiting here and the
/// candidate advances it.
fn term_profession_ready(w: &World, a: &Agent, c: &Candidate) -> f32 {
    if profession::order_for(w, a, c.verb).is_some() {
        1.0
    } else if is_workish(c) {
        0.2
    } else {
        0.0
    }
}

/// An unkept promise to someone present pulls toward the promise, and toward
/// keeping it by hand (giving them food) when words are not the point.
fn term_promise_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let Some(pr) = &a.promise else { return 0.0 };
    if pr.kept.is_some() {
        return 0.0;
    }
    let owed_present = w.present(a.place, &a.name).contains(&pr.to);
    match (c.action.as_str(), owed_present) {
        ("promise", true) => 1.0,
        ("promise", false) => 0.3,
        ("give" | "help", true) if pr.to != a.name => 0.8,
        _ if owed_present && pr.to == addressee_of(w, a, c).as_deref().unwrap_or("") => 0.6,
        _ => 0.0,
    }
}

/// Someone present who is starving, ill or thirsty needs the acts that
/// answer the need: food acts for hunger and thirst, heal and console for
/// illness and grief. Speaking near need is worth something; consoling
/// beside an empty stomach is not.
fn term_social_need(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let others: Vec<String> = w.present(a.place, &a.name);
    if others.is_empty() {
        return 0.0;
    }
    let needy = |o: &String| w.agent(o).is_some_and(|x| x.hunger >= 2 || x.thirst >= 2);
    let ill = |o: &String| {
        w.agent(o)
            .is_some_and(|x| x.ailment.is_some() || x.health < 6)
    };
    let grieving = |o: &String| w.agent(o).is_some_and(|x| x.stress >= 2.8 || x.hope < 0.8);
    match c.action.as_str() {
        // Feeding answers hunger; healing answers illness; consoling answers
        // grief. A console beside mere hunger answers nothing, and a heal
        // beside a busy day is not medicine.
        "give" | "help" if others.iter().any(needy) => 1.0,
        "heal" if others.iter().any(ill) => 1.0,
        "heal" if others.iter().any(grieving) => 0.3,
        "console" if others.iter().any(ill) || others.iter().any(grieving) => 1.0,
        "speak" if others.iter().any(needy) || others.iter().any(ill) => 0.4,
        _ => 0.0,
    }
}

/// A grudge of 1.5 or more against someone present pulls toward the
/// accusation, and colours the other acts aimed at them.
fn term_grudge_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let grudged = w
        .present(a.place, &a.name)
        .into_iter()
        .filter(|n| {
            a.relation_with(n)
                .is_some_and(|r| r.grudge >= OBLIGATION_GRUDGE)
        })
        .collect::<Vec<_>>();
    if grudged.is_empty() {
        return 0.0;
    }
    let aimed = addressee_of(w, a, c).is_some_and(|t| grudged.contains(&t));
    match c.action.as_str() {
        "accuse" => 1.0,
        "speak" if c.act == SocialAct::Threat && aimed => 0.9,
        _ if aimed => 0.3,
        _ => 0.0,
    }
}

/// Kin present pull the caring acts toward them.
fn term_kinship_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let kin = w
        .present(a.place, &a.name)
        .into_iter()
        .filter(|n| {
            a.relation_with(n).is_some_and(|r| {
                matches!(
                    r.kinship,
                    Kinship::Partner | Kinship::Parent | Kinship::Child | Kinship::Sibling
                )
            })
        })
        .collect::<Vec<_>>();
    if kin.is_empty() {
        return 0.0;
    }
    let aimed = addressee_of(w, a, c).is_some_and(|t| kin.contains(&t));
    match c.action.as_str() {
        "give" | "help" | "heal" | "console" if aimed => 1.0,
        "speak" if aimed => 0.5,
        _ => 0.0,
    }
}

/// The role's own action.
fn term_role_duty(_w: &World, a: &Agent, c: &Candidate) -> f32 {
    let meta = catalog::meta_for(&c.action);
    match meta.role_affinity {
        Some(role) if role == a.role => 1.0,
        _ => 0.0,
    }
}

/// The place is broken, empty or unowned, and the candidate works on it.
fn term_place_need(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let st = w.place_state(a.place);
    let broken = st.condition < 5 || (a.place == Place::Hut && !st.hearth_lit);
    let granary_low = w.place_state(Place::Granary).food < 6;
    let empty = st.food == 0 || (a.place.can_draw_water() && st.water == 0);
    let unowned = st.owner.is_none();
    let work_here = is_workish(c) && c.place.is_none_or(|p| p == a.place);
    let mut v = 0.0;
    if broken {
        v += f32::from(matches!(c.verb, Verb::Repair));
    }
    if granary_low {
        v += f32::from(matches!(c.verb, Verb::Tend | Verb::Forage));
    }
    if empty {
        v += f32::from(matches!(
            c.verb,
            Verb::DrawWater | Verb::Take | Verb::Forage | Verb::GatherHerbs | Verb::Chop
        ));
    }
    if unowned && work_here {
        v += 0.3;
    }
    v.min(1.0)
}

/// The most-trusted villager present pulls the addressed acts.
fn term_trust_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let Some(most) = most_trusted_present(w, a) else {
        return 0.0;
    };
    let addressed = addressee_of(w, a, c).is_some_and(|t| t == most);
    let social = catalog::meta_for(&c.action).category == Category::Social || c.verb == Verb::Speak;
    if addressed && social {
        a.trust_of(&most) / 4.0
    } else {
        0.0
    }
}

/// The action fits the villager's state: their mood, and their appetite
/// below the survival threshold - a villager who is hungry and empty-handed
/// is already leaning toward food, even before the tier takes over.
fn term_mood_pull(_w: &World, a: &Agent, c: &Candidate) -> f32 {
    let hungry = a.hunger >= 1 && a.food == 0;
    let peckish = a.hunger >= 1 && here_has_food_hint(c);
    let fits = if hungry || peckish {
        matches!(
            c.verb,
            Verb::Take | Verb::Forage | Verb::Steal | Verb::Move | Verb::DrawWater
        )
    } else {
        match a.mood {
            Mood::Desperate => {
                matches!(c.verb, Verb::Take | Verb::Forage | Verb::Steal | Verb::Move)
            },
            Mood::Weary => c.verb == Verb::Rest,
            Mood::Bitter => {
                matches!(c.verb, Verb::Accuse)
                    || (c.verb == Verb::Speak && c.act == SocialAct::Threat)
            },
            Mood::Anxious => matches!(c.verb, Verb::Console | Verb::Pray),
            Mood::Grieving => matches!(c.verb, Verb::Pray | Verb::Console),
            Mood::Feverish => matches!(c.verb, Verb::Heal | Verb::Rest),
            Mood::Hopeful => matches!(c.verb, Verb::Give | Verb::Help) || is_workish(c),
            Mood::Determined => is_workish(c),
            Mood::Content => is_workish(c) || c.verb == Verb::Speak,
        }
    };
    f32::from(fits)
}

/// Does the candidate act where food is? The appetite fit is about the
/// candidate's own reach, not a world scan: taking, foraging and moves read
/// as food-seeking; the place itself decides the rest.
const fn here_has_food_hint(c: &Candidate) -> bool {
    matches!(c.verb, Verb::Take | Verb::Forage | Verb::Move)
}

/// Not taken in the last three turns (the novelty window).
fn term_novelty(w: &World, a: &Agent, c: &Candidate) -> f32 {
    #[allow(
        clippy::suspicious_operation_groupings,
        reason = "an event's actor against the villager's name is the intended join"
    )]
    let recent = w.events.iter().any(|e| {
        e.action == c.action && e.actor == a.name && w.turn.saturating_sub(e.turn) <= NOVELTY_WINDOW
    });
    f32::from(!recent)
}

/// Cheaper actions when tired; the value is the energy kept.
fn term_energy_cost(_w: &World, a: &Agent, c: &Candidate) -> f32 {
    let cost = f32::from(catalog::meta_for(&c.action).energy_cost);
    let keep = 1.0 - cost / 10.0;
    if a.energy <= 4 {
        keep
    } else {
        keep.mul_add(0.5, 0.5)
    }
}

/// Acting on ground the villager holds is steadier than ground another holds.
fn term_ownership(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let st = w.place_state(a.place);
    match st.owner.as_deref() {
        Some(o) if o == a.name => 1.0,
        Some(_) => f32::from(c.verb == Verb::Steal) * 0.2,
        None => 0.5,
    }
}

// ---- Addressee binding (ticket #10) --------------------------------------

/// The villager a candidate's act addresses, bound by the act: the accused
/// for an accusation, the beneficiary for a promise, otherwise the
/// most-trusted villager present, ties broken by name.
///
/// `None` when nobody is present or the act addresses nobody.
#[must_use]
pub fn addressee_of(w: &World, a: &Agent, c: &Candidate) -> Option<String> {
    let others = w.present(a.place, &a.name);
    if others.is_empty() {
        return None;
    }
    match c.act {
        SocialAct::Accusation => {
            // The accused: the present villager with the largest grudge, ties
            // broken by name.
            others
                .into_iter()
                .min_by(|x, y| {
                    let gx = a.relation_with(x).map_or(0.0, |r| r.grudge);
                    let gy = a.relation_with(y).map_or(0.0, |r| r.grudge);
                    gy.total_cmp(&gx).then_with(|| x.as_str().cmp(y.as_str()))
                })
                .or_else(|| most_trusted_present(w, a))
        },
        SocialAct::Promise => a
            .promise
            .as_ref()
            .filter(|pr| pr.kept.is_none() && others.contains(&pr.to))
            .map(|pr| pr.to.clone())
            .or_else(|| most_trusted_present(w, a)),
        _ => most_trusted_present(w, a),
    }
}

/// The present villager `a` trusts most, ties broken by the smaller name.
#[must_use]
pub(crate) fn most_trusted_present(w: &World, a: &Agent) -> Option<String> {
    w.present(a.place, &a.name).into_iter().min_by(|x, y| {
        a.trust_of(y)
            .total_cmp(&a.trust_of(x))
            .then_with(|| x.as_str().cmp(y.as_str()))
    })
}

// ---- The candidate set ----------------------------------------------------

/// `go:<place>` and plain ids resolve to their verb; `speak` resolves
/// directly.
fn verb_of(action: &str, from: Place) -> Verb {
    crate::action::Intent::parse(action, from).map_or(Verb::Move, |i| i.verb)
}

// ---- The tiers -------------------------------------------------------------

/// Survival: hunger with no carried food, thirst with no carried water, any
/// ailment, or winter cold.
///
/// These reuse the goal ladder's thresholds so the policy and the villager's
/// stated goal cannot disagree about what urgent means.
#[must_use]
pub fn is_survival(w: &World, a: &Agent) -> bool {
    (a.hunger >= SURVIVAL_HUNGER && a.food == 0)
        || (a.thirst >= SURVIVAL_THIRST && a.water == 0)
        || a.ailment.is_some()
        || (wintery(w) && a.warmth <= SURVIVAL_WARMTH)
}

fn wintery(w: &World) -> bool {
    w.season() == Season::Winter
}

/// Obligation: an unkept promise or a grudge of 1.5 or more against someone
/// present.
#[must_use]
pub fn obligation_against(w: &World, a: &Agent) -> bool {
    let owed_present = a
        .promise
        .as_ref()
        .is_some_and(|pr| pr.kept.is_none() && w.present(a.place, &a.name).contains(&pr.to));
    let grudge_present = w.present(a.place, &a.name).into_iter().any(|n| {
        a.relation_with(&n)
            .is_some_and(|r| r.grudge >= OBLIGATION_GRUDGE)
    });
    owed_present || grudge_present
}

/// Does the candidate serve the villager's active survival pressure?
fn serves_survival(w: &World, a: &Agent, c: &Candidate) -> bool {
    let meta = catalog::meta_for(&c.action);
    match meta.need {
        Need::Food => a.hunger >= SURVIVAL_HUNGER && a.food == 0,
        Need::Water => a.thirst >= SURVIVAL_THIRST && a.water == 0,
        Need::Health => a.ailment.is_some() || a.health < 10,
        Need::Warmth => wintery(w) && a.warmth <= SURVIVAL_WARMTH,
        Need::Rest | Need::Faith | Need::None => false,
    }
}

/// Does the candidate answer the active obligation?
fn serves_obligation(w: &World, a: &Agent, c: &Candidate) -> bool {
    let owed = a.promise.as_ref().is_some_and(|pr| {
        pr.kept.is_none()
            && w.present(a.place, &a.name).contains(&pr.to)
            && (c.action == "promise"
                || ((c.action == "give" || c.action == "help")
                    && addressee_of(w, a, c).as_deref() == Some(pr.to.as_str())))
    });
    let grudge = w.present(a.place, &a.name).iter().any(|n| {
        a.relation_with(n)
            .is_some_and(|r| r.grudge >= OBLIGATION_GRUDGE)
    }) && (c.action == "accuse"
        || (c.action == "speak" && c.act == SocialAct::Threat));
    owed || grudge
}

// ---- Motive derivation ----------------------------------------------------

/// The motive derived from the action and the world (never asked). The
/// precedence follows the goal ladder: hunger, thirst, promise, fear,
/// kinship, duty.
#[must_use]
pub fn derive_motive(w: &World, a: &Agent, d: &Decision) -> Motive {
    let meta = catalog::meta_for(&d.action);
    if matches!(meta.need, Need::Food) && (a.hunger >= 1 || a.food == 0) {
        return Motive::Hunger;
    }
    if matches!(meta.need, Need::Water) && (a.thirst >= 1 || a.water == 0) {
        return Motive::Thirst;
    }
    if d.action == "promise"
        || a.promise
            .as_ref()
            .is_some_and(|pr| d.addressee.as_deref() == Some(pr.to.as_str()))
    {
        return Motive::Promise;
    }
    if a.ailment.is_some()
        || (w.season() == Season::Winter && a.warmth <= SURVIVAL_WARMTH)
        || threatened_this_turn(w, a)
    {
        return Motive::Fear;
    }
    if let Some(target) = d.addressee.as_deref() {
        if a.relation_with(target).is_some_and(|r| {
            matches!(
                r.kinship,
                Kinship::Partner | Kinship::Parent | Kinship::Child | Kinship::Sibling
            )
        }) {
            return Motive::Kinship;
        }
    }
    Motive::Duty
}

/// A threat was overheard this turn: anyone at the speaker's place spoke one.
fn threatened_this_turn(w: &World, a: &Agent) -> bool {
    #[allow(
        clippy::suspicious_operation_groupings,
        reason = "an event's actor against the villager's name is the intended join"
    )]
    w.events.iter().any(|e| {
        e.turn == w.turn
            && e.place == a.place
            && e.actor != a.name
            && e.social.as_deref() == Some("threat")
    })
}

// ---- The distribution ------------------------------------------------------

/// Softmax over the scores at the pinned temperature, in candidate order.
fn distribution(scores: &[(String, f32)]) -> Vec<(String, f32)> {
    let t = POLICY_TEMPERATURE.max(1e-6);
    let max = scores.iter().map(|(_, s)| *s).fold(f32::MIN, f32::max);
    let exps: Vec<(String, f32)> = scores
        .iter()
        .map(|(k, s)| (k.clone(), ((s - max) / t).exp()))
        .collect();
    let sum: f32 = exps.iter().map(|(_, e)| *e).sum();
    exps.into_iter().map(|(k, e)| (k, e / sum)).collect()
}

/// Decide without applying or settling. This is the harness's entry point and
/// the seam the turn function uses. `None` when there is no such living
/// villager or the world has halted.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one sequential tier table; splitting hides the precedence"
)]
pub fn decide(w: &World, name: &str) -> Option<Decision> {
    if w.halted.is_some() {
        return None;
    }
    let a = w.agent(name)?;
    if !a.alive {
        return None;
    }

    // The candidate set. An empty shortlist falls back to the full legal set;
    // the result is always legal.
    // The candidate set: the shortlist expanded into (action, act) pairs. An
    // empty shortlist falls back to the full legal set.
    let menu: Vec<String> = {
        let short = shortlist(w, a);
        if short.is_empty() {
            crate::action::affordances(w, a)
        } else {
            short
        }
    };
    let pool: Vec<Candidate> = menu
        .iter()
        .flat_map(|action| {
            let verb = verb_of(action, a.place);
            if action == "speak" {
                SPEECH_MENU
                    .iter()
                    .map(|act| Candidate {
                        action: action.clone(),
                        verb,
                        place: None,
                        act: *act,
                    })
                    .collect::<Vec<_>>()
            } else {
                let act = catalog::act_for(action, SocialAct::None);
                let place = crate::action::Intent::parse(action, a.place).and_then(|i| i.place);
                vec![Candidate {
                    action: action.clone(),
                    verb,
                    place,
                    act,
                }]
            }
        })
        .collect();
    if pool.is_empty() {
        return None;
    }

    // Tiered precedence. Tier 0 beats tier 1 beats tier 2.
    let survival = is_survival(w, a);
    let obligation = obligation_against(w, a);
    let tiered: Option<Vec<Candidate>> = if survival {
        Some(
            pool.iter()
                .filter(|c| serves_survival(w, a, c))
                .cloned()
                .collect(),
        )
    } else if obligation {
        Some(
            pool.iter()
                .filter(|c| serves_obligation(w, a, c))
                .cloned()
                .collect(),
        )
    } else {
        None
    };
    let hard_tier = tiered.as_ref().filter(|t| !t.is_empty());
    let scope: Vec<Candidate> = hard_tier.cloned().unwrap_or_else(|| pool.clone());
    let hard = hard_tier.is_some();

    // Score every term for every candidate in scope. Contributions are
    // weight x raw, and the factor list for the chosen candidate keeps the
    // spec's six-to-twelve shape.
    let mut scored: Vec<Scored> = Vec::with_capacity(scope.len());
    for c in &scope {
        let mut total = 0.0;
        let mut factors = Vec::with_capacity(TERMS.len());
        for term in &TERMS {
            let contribution = term.weight * (term.f)(w, a, c);
            total += contribution;
            factors.push((term.name, (contribution * 100.0).round() / 100.0));
        }
        scored.push((c.key(), total, factors));
    }

    // Pick. Hard tiers take the top score outright; tier 2 goes through the
    // distribution so confidence and the dilemma are real numbers.
    let (chosen_key, confidence, dilemma) = if hard {
        let mut best = 0_usize;
        for i in 1..scored.len() {
            if scored[i].1 > scored[best].1 {
                best = i;
            }
        }
        (scored[best].0.clone(), 1.0, None)
    } else {
        let dist = distribution(
            &scored
                .iter()
                .map(|(k, s, _)| (k.clone(), *s))
                .collect::<Vec<_>>(),
        );
        let mut best = 0_usize;
        for i in 1..dist.len() {
            // Strictly greater: ties keep the earlier (shortlist-order) pair.
            if dist[i].1 > dist[best].1 {
                best = i;
            }
        }
        let (win_key, win_p) = dist[best].clone();
        let runner = dist
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != best)
            .max_by(|(_, a), (_, b)| a.1.total_cmp(&b.1))
            .map(|(_, (k, p))| (k.clone(), *p));
        let dilemma = runner.and_then(|(alt_key, alt_p)| {
            (win_p - alt_p <= DILEMMA_MARGIN && alt_p >= DILEMMA_RUNNERUP_MASS).then(|| Dilemma {
                chosen: win_key.clone(),
                alternative: alt_key,
                chosen_prob: (win_p * 100.0).round() / 100.0,
                alternative_prob: (alt_p * 100.0).round() / 100.0,
            })
        });
        (win_key, (win_p * 100.0).round() / 100.0, dilemma)
    };

    let mut chosen = scope
        .iter()
        .find(|c| c.key() == chosen_key)
        .cloned()
        .unwrap_or_else(|| scope[0].clone());
    let factors = scored
        .iter()
        .find(|(k, _, _)| *k == chosen_key)
        .map(|(_, _, f)| f.clone())
        .unwrap_or_default();
    chosen.place = crate::action::Intent::parse(&chosen.action, a.place).and_then(|i| i.place);

    // The addressee is bound by the act: give, help, heal and console work on
    // a person; the other acts address one when their act is social.
    let personward = matches!(
        chosen.verb,
        Verb::Give | Verb::Help | Verb::Console | Verb::Heal
    ) || catalog::act_for(&chosen.action, chosen.act) != SocialAct::None;
    let addressee = if personward {
        addressee_of(w, a, &chosen)
    } else {
        None
    };
    let job = profession::order_for(w, a, chosen.verb).map(|o| o.id);

    let mut decision = Decision {
        action: chosen.action,
        verb: chosen.verb,
        place: chosen.place,
        act: chosen.act,
        addressee,
        motive: Motive::Duty,
        job,
        confidence,
        dilemma,
        factors,
    };
    decision.motive = derive_motive(w, a, &decision);
    Some(decision)
}

// ---- The per-villager RNG streams -----------------------------------------

/// FNV-1a, so a name becomes a stream offset without a dependency.
#[must_use]
pub(crate) fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The per-villager stream (spec: "The RNG is split"): derived as
/// `mix(seed, turn, name, purpose)`, so the same seed and save replay
/// identically and the world's own stream never moves.
#[must_use]
pub fn agent_rng(w: &World, name: &str, purpose: &str) -> Rng {
    let mix = w.seed
        ^ w.turn.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ fnv1a(name)
        ^ fnv1a(purpose).rotate_left(17);
    Rng::new(mix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Agent;

    fn villager(name: &str, place: Place) -> Agent {
        Agent::new(name, "plain", place, 2, 0, 0, 0, vec![], &[])
    }

    #[test]
    fn the_motive_matches_the_action_on_a_constructed_world() {
        let mut w = World::founding(3);
        w.agents.clear();
        let mut hungry = villager("Bo", Place::Woods);
        hungry.hunger = 3;
        hungry.food = 0;
        w.agents.push(hungry);
        let d = decide(&w, "Bo").expect("decision");
        assert_eq!(d.motive, Motive::Hunger);
        assert!(matches!(
            d.verb,
            Verb::Take | Verb::Forage | Verb::Move | Verb::Steal | Verb::Speak
        ));
    }

    #[test]
    fn survival_always_offers_food_when_one_is_legal() {
        let mut w = World::founding(4);
        w.agents.clear();
        let mut b = villager("Bo", Place::Market);
        b.hunger = 2;
        b.food = 0;
        w.agents.push(b);
        let d = decide(&w, "Bo").unwrap();
        assert_eq!(d.motive, Motive::Hunger);
        assert_eq!(d.confidence, 1.0, "tier 0 is hard precedence");
        assert!(matches!(
            d.verb,
            Verb::Take | Verb::Forage | Verb::Move | Verb::Speak
        ));
    }

    #[test]
    fn the_policy_always_returns_a_legal_candidate_even_with_an_empty_shortlist() {
        let mut w = World::founding(5);
        w.agents.clear();
        // A villager alone in the woods with food in hand and nothing broken:
        // the shortlist can be near-empty, the legal set never is.
        let mut n = villager("Nell", Place::Woods);
        n.food = 2;
        n.energy = 10;
        w.agents.push(n);
        let d = decide(&w, "Nell").unwrap();
        let legal = crate::action::affordances(&w, w.agent("Nell").unwrap());
        assert!(legal.contains(&d.action), "{} not legal", d.action);
    }

    #[test]
    fn confidence_and_dilemma_agree_with_the_distribution() {
        let mut w = World::founding(6);
        w.agents.clear();
        let mut b = villager("Bo", Place::Market);
        b.food = 2;
        w.agents.push(b);
        if let Some(d) = decide(&w, "Bo") {
            assert!((0.0..=1.0).contains(&d.confidence));
            if let Some(dl) = &d.dilemma {
                assert!(dl.chosen_prob - dl.alternative_prob <= DILEMMA_MARGIN + 1e-6);
                assert!(dl.alternative_prob >= DILEMMA_RUNNERUP_MASS);
            }
        }
    }

    #[test]
    fn the_addressee_falls_back_to_the_most_trusted_present() {
        let mut w = World::founding(7);
        w.agents.clear();
        let mut ada = villager("Ada", Place::Granary);
        ada.set_trust("Cor", 3.9);
        ada.set_trust("Bo", 3.9);
        w.agents.push(ada);
        w.agents.push(villager("Cor", Place::Granary));
        w.agents.push(villager("Bo", Place::Granary));
        let a = w.agent("Ada").unwrap();
        let c = Candidate {
            action: "speak".into(),
            verb: Verb::Speak,
            place: None,
            act: SocialAct::Gossip,
        };
        // 3.9 ties break by the smaller name: Bo.
        assert_eq!(addressee_of(&w, a, &c).as_deref(), Some("Bo"));
    }
}
