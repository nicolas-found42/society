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

/// One candidate the policy considered and did not take. `reason` is
/// `"outscored"` (scored below the winner) or `"tier"` (a hard survival or
/// obligation tier filtered it before scoring, so it carries no score).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rejected {
    pub candidate: String,
    pub score: Option<f32>,
    pub reason: String,
}

/// One candidate's score and the term contributions behind it.
type Scored = (String, f32, Vec<(&'static str, f32)>);

/// One scored candidate by reference, for runner-up selection.
type ScoredRef<'a> = &'a Scored;

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

/// The decision as the need-seeking helpers read it: action, verb, place.
fn candidate_of(d: &Decision) -> Candidate {
    Candidate {
        action: d.action.clone(),
        verb: d.verb,
        place: d.place,
        act: d.act,
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
    /// The three strongest candidates the policy considered and did not
    /// take, in consideration order: hard-tier rejects first, then the
    /// nearest outscored runners-up. Bounded at three so the turn record
    /// stays small; scoring stays total for determinism.
    #[serde(default)]
    pub rejected: Vec<Rejected>,
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

/// The thirteen pinned terms (spec: "Feature-term weights"). The weights are
/// the first cut; a revision is a design change reported with its A/B result.
/// `shelter_pull` (ticket #24) is the home-fire nudge: at the need level
/// beside `social_need`, below trade; the feet score below need so they
/// steer only when nothing presses.
pub(crate) const TERMS: [Term; 13] = [
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
        name: "shelter_pull",
        weight: 2.2,
        f: term_shelter_pull,
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
/// candidate advances it. A move that shortens the ring distance to a stocked
/// work-order place scores 0.4: the trade pulls the feet, not just the
/// hands, so the farmer walks to the field instead of taking in the
/// granary. The pull stays below eating here (0.2 trade + appetite terms),
/// so a starving villager takes before they walk. Other work of the
/// villager's own trade scores 0.2; another trade's work scores nothing, so
/// the woods do not become the granary with trees.
fn term_profession_ready(w: &World, a: &Agent, c: &Candidate) -> f32 {
    if profession::order_for(w, a, c.verb).is_some() {
        1.0
    } else if c.verb == Verb::Move && approaches_stocked_order(w, a, c) {
        0.4
    } else if is_workish(c) && trade_matches(a, c) {
        0.2
    } else {
        0.0
    }
}

/// The candidate's verb is work the villager's own profession names anywhere
/// in its queue. A verb no order of the trade carries is another trade's
/// work, however workish it reads.
fn trade_matches(a: &Agent, c: &Candidate) -> bool {
    profession::profession_for(a.role).is_some_and(|p| p.orders.iter().any(|o| o.verb == c.verb))
}

/// The move's destination is nearer than here to some stocked work-order
/// place of the villager's profession. Standing already at a stocked place
/// is arrival, not approach: only a strictly shorter distance counts.
fn approaches_stocked_order(w: &World, a: &Agent, c: &Candidate) -> bool {
    let Some(dest) = c.place else {
        return false;
    };
    profession::stocked_order_places(w, a)
        .iter()
        .any(|p| dest.ring_distance(*p) < a.place.ring_distance(*p))
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

/// Someone present who is hungry, ill or thirsty needs the acts that answer
/// the need: food acts for hunger and thirst, heal and console for illness
/// and grief. The gift is aimed: it scores only when the addressee themself
/// is hungry, is kin, or is owed the villager's unkept promise - feeding a
/// fed stranger beside a hungry one is not generosity, and a camp that feeds
/// itself has no reason to ever leave.
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
        // beside a busy day is not medicine. The gift fires on presence -
        // someone here is hungry, is kin, or is owed the villager's word -
        // and the addressee aims it at the neediest below satiety (ticket
        // #23). Frequency is presence-gated; targeting is need-first.
        "give" | "help" if gift_lands(w, a, c, &others) => 1.0,
        "heal" if others.iter().any(ill) => 1.0,
        "heal" if others.iter().any(grieving) => 0.3,
        "console" if others.iter().any(ill) || others.iter().any(grieving) => 1.0,
        "speak" if others.iter().any(needy) || others.iter().any(ill) => 0.4,
        _ => 0.0,
    }
}

/// Someone present is hungry or thirsty, is kin, or is owed the villager's
/// unkept promise. The addressee aims the gift (see [`addressee_of`]); the
/// term fires on presence so need-directed aiming does not starve the
/// frequency that carries trust and births.
fn gift_lands(w: &World, a: &Agent, c: &Candidate, others: &[String]) -> bool {
    let Some(to) = addressee_of(w, a, c) else {
        return false;
    };
    // The addressee themself must be needy, kin, or owed (ticket #23: the
    // most-trusted fallback may name a sated stranger when the room is all
    // fed - the veto follows the gift, not just the shortlist, or all-fed
    // rooms re-concentrate). The owed promisee is exempt: keeping a word
    // is not feeding the fed.
    let to_owed = a.promise.as_ref().is_some_and(|pr| pr.kept.is_none() && pr.to == to);
    let to_needy = w.agent(&to).is_some_and(|x| x.food < SATIETY_MEASURES)
        || a.relation_with(&to).is_some_and(|r| {
            matches!(
                r.kinship,
                Kinship::Partner | Kinship::Parent | Kinship::Child | Kinship::Sibling
            )
        })
        || to_owed;
    others.iter().any(|o| {
        a.promise.as_ref().is_some_and(|pr| pr.kept.is_none() && pr.to == *o)
            || a.relation_with(o).is_some_and(|r| {
                matches!(
                    r.kinship,
                    Kinship::Partner | Kinship::Parent | Kinship::Child | Kinship::Sibling
                )
            })
            || w.agent(o).is_some_and(|x| x.hunger >= 1 || x.thirst >= 1)
    }) && to_needy
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

/// The place is broken, empty or unowned, and the candidate works on it. The
/// empty-ground pull is appetite's business: a hungry villager strips the
/// patch, a fed one walks to their trade instead - or every foraging spot
/// becomes a camp.
fn term_place_need(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let st = w.place_state(a.place);
    let broken = st.condition < 5 || (a.place == Place::Hut && !st.hearth_lit);
    let granary_low = w.place_state(Place::Granary).food < 6;
    let empty = st.food == 0 || (a.place.can_draw_water() && st.water == 0);
    let appetite = (a.hunger >= 1 && a.food == 0) || (a.thirst >= 1 && a.water == 0);
    let unowned = st.owner.is_none();
    let work_here = is_workish(c) && c.place.is_none_or(|p| p == a.place);
    let mut v = 0.0;
    if broken {
        v += f32::from(matches!(c.verb, Verb::Repair));
    }
    if granary_low {
        v += f32::from(matches!(c.verb, Verb::Tend | Verb::Forage));
    }
    if empty && appetite {
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
/// is already leaning toward food, even before the tier takes over. A move
/// only fits an appetite when its destination can answer it: a hungry
/// villager walks toward grain, a thirsty one toward the well.
fn term_mood_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    let hungry = a.hunger >= 1 && a.food == 0;
    let thirsty = a.thirst >= 1 && a.water == 0;
    let peckish =
        a.hunger >= 1 && matches!(c.verb, Verb::Take | Verb::Forage | Verb::Steal | Verb::Move);
    let fits = if hungry || peckish {
        matches!(
            c.verb,
            Verb::Take | Verb::Forage | Verb::Steal | Verb::DrawWater
        ) || (c.verb == Verb::Move && move_seeks_food(w, a, c))
    } else if thirsty {
        c.verb == Verb::DrawWater || (c.verb == Verb::Move && move_seeks_water(w, a, c))
    } else {
        match a.mood {
            Mood::Desperate => {
                matches!(c.verb, Verb::Take | Verb::Forage | Verb::Steal)
                    || (c.verb == Verb::Move && move_seeks_food(w, a, c))
            },
            Mood::Weary => {
                c.verb == Verb::Rest || (c.verb == Verb::Move && move_seeks_hut(a, c))
            },
            Mood::Bitter => {
                matches!(c.verb, Verb::Accuse)
                    || (c.verb == Verb::Speak && c.act == SocialAct::Threat)
            },
            Mood::Anxious | Mood::Grieving => {
                matches!(c.verb, Verb::Console | Verb::Pray)
                    || (c.verb == Verb::Move && move_seeks_shrine(a, c))
            },
            Mood::Feverish => matches!(c.verb, Verb::Heal | Verb::Rest),
            Mood::Hopeful => matches!(c.verb, Verb::Give | Verb::Help) || is_workish(c),
            Mood::Determined => is_workish(c),
            Mood::Content => is_workish(c) || c.verb == Verb::Speak,
        }
    };
    f32::from(fits)
}

/// The home-fire nudge (ticket #24): a deeply lapsed prayer scores flat at
/// the need level, rest at the hut scores with tiredness, and approach
/// moves toward either score a fixed step when the matching need is live.
/// Devotion fades 0.05 a turn and only prayer restores it, so the shrine
/// calls in ordinary states - not just grief. The weight (2.2, beside
/// `social_need`) makes prayer competitive; the walk scores below need,
/// so the feet steer without hijacking the day.
fn term_shelter_pull(w: &World, a: &Agent, c: &Candidate) -> f32 {
    // The feet only move the sated and stocked (ticket #24's seed-42
    // lessons): Ada deferred drawing water for a shrine-approach, and Bo
    // left the hut for the shrine on empty pockets - each seeded a woods
    // social cascade. A villager with any hunger or thirst, or empty
    // pockets, keeps their feet on needs. The acts carry no appetite gate:
    // real urgency is already handled below this term - the desperate get
    // a restricted menu without prayer, and the survival tier excludes
    // faith.
    if c.verb == Verb::Move && (a.hunger >= 1 || a.thirst >= 1 || a.food == 0 || a.water == 0) {
        return 0.0;
    }
    if c.verb == Verb::Pray {
        // Below 0.5 the villager owes the shrine a visit: the flat 2.0 at
        // weight 2.2 (4.4 points) beats company and, on raw points, a
        // stocked order too - which is why orders take menu priority above:
        // prayer fills the Work slot only when no order waits, so the two
        // never compete on the menu. Prayer restores 0.3 a visit, so the
        // pull spends itself and the loop-breaker keeps the shrine social.
        // Above 0.5 no pull: the threshold keeps early-game feet on needs
        // (seed 99 tips when a turn-21 prayer delays a woods round; at 0.05
        // decay nobody lapses that early).
        if a.devotion < 0.5 {
            return 2.0;
        }
        return 0.0;
    }
    if c.verb == Verb::Rest && a.place == Place::Hut {
        return f32::from(a.energy <= 5);
    }
    if c.verb == Verb::Move {
        // Home territory only (ticket #24's seed-42 lesson): approach moves
        // through the woods harvest corridor trap shrine-seekers - easy to
        // enter, hard to leave when foraging outscores the walk. Seeking
        // flows within market/well/hut/shrine; granary/field/woods are work
        // territory, and the granary gate holds the feet when stores run low.
        let home = matches!(
            a.place,
            Place::Market | Place::Well | Place::Hut | Place::Shrine
        );
        let stores = w.place_state(Place::Granary).food >= 6;
        if a.devotion < 1.5 && home && stores && move_seeks_shrine(a, c) {
            return 0.45;
        }
        if a.energy <= 5 && home && stores && move_seeks_hut(a, c) {
            return 0.45;
        }
    }
    0.0
}
/// The move's destination holds food, can be foraged, or is nearer than
/// here to the granary while the granary holds food: hunger walks downhill
/// toward grain even when the next step itself is bare.
fn move_seeks_food(w: &World, a: &Agent, c: &Candidate) -> bool {
    let Some(dest) = c.place else {
        return false;
    };
    if w.place_state(dest).food > 0 || dest.can_harvest() {
        return true;
    }
    w.place_state(Place::Granary).food > 0
        && dest.ring_distance(Place::Granary) < a.place.ring_distance(Place::Granary)
}

/// The move's destination is the well with water to draw, or nearer than
/// here to it: thirst walks downhill toward water.
fn move_seeks_water(w: &World, a: &Agent, c: &Candidate) -> bool {
    let Some(dest) = c.place else {
        return false;
    };
    dest == Place::Well && w.place_state(Place::Well).water > 0
        || dest.ring_distance(Place::Well) < a.place.ring_distance(Place::Well)
}

/// The move's destination is nearer than here to the shrine: grief walks
/// downhill toward the moss (ticket #24: nothing pulled villagers to the
/// shrine, so pray went unexercised for 300 turns).
const fn move_seeks_shrine(a: &Agent, c: &Candidate) -> bool {
    let Some(dest) = c.place else {
        return false;
    };
    dest.ring_distance(Place::Shrine) < a.place.ring_distance(Place::Shrine)
}

/// The move's destination is nearer than here to the hut: weariness walks
/// downhill toward rest (ticket #24: rest is hut-optimal but nobody walked
/// there for it).
const fn move_seeks_hut(a: &Agent, c: &Candidate) -> bool {
    let Some(dest) = c.place else {
        return false;
    };
    dest.ring_distance(Place::Hut) < a.place.ring_distance(Place::Hut)
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

/// The villager a candidate's act addresses, bound by the act.
///
/// The accused for an accusation, the beneficiary for a promise, otherwise
/// the most-trusted present villager below satiety, need breaking trust
/// ties before the name does (ticket #23: pure trust routing concentrates
/// food in one villager; need-first routing instead starves the gift
/// frequency the trust terms score, and seed 2026 collapses). A villager
/// holding [`SATIETY_MEASURES`] or more measures is never a give/help
/// addressee, however trusted - the veto is what de-concentrates.
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
        SocialAct::Gift if matches!(c.verb, Verb::Give | Verb::Help) => {
            fed_among_trusted(w, a, &others).or_else(|| most_trusted_present(w, a))
        },
        _ => most_trusted_present(w, a),
    }
}

/// The measures of carried food at or above which a villager is never a
/// give/help addressee.
///
/// Feeding the fed concentrates the stores (ticket #23: seed 7 measured one
/// villager at 89 measures against a 1-measure median).
pub const SATIETY_MEASURES: u32 = 4;

/// The most-trusted present villager below satiety, need breaking trust
/// ties before the name does - except the owed promisee, who jumps the
/// queue (ticket #22: a keeping gift must be able to reach its promisee, or
/// `serves_obligation` never admits it and no promise is ever kept). Trust
/// orders so the gift keeps carrying the terms that score it; need outranks
/// the name so the hungry eat before the merely early-alphabetical. The owed
/// promisee is exempt from the satiety filter: a gift that keeps a promise
/// is keeping a word, not feeding the fed.
fn fed_among_trusted(w: &World, a: &Agent, others: &[String]) -> Option<String> {
    let owed = a.promise.as_ref().filter(|pr| pr.kept.is_none()).map(|pr| pr.to.clone());
    others
        .iter()
        .filter(|n| {
            w.agent(n).is_some_and(|x| x.food < SATIETY_MEASURES) || owed.as_ref() == Some(*n)
        })
        .min_by(|x, y| {
            let ox = owed.as_ref() == Some(*x);
            let oy = owed.as_ref() == Some(*y);
            // The owed promisee first; then trust, need, name.
            oy.cmp(&ox)
                .then_with(|| a.trust_of(y).total_cmp(&a.trust_of(x)))
                .then_with(|| need_score(w, a, x).total_cmp(&need_score(w, a, y)))
                .then_with(|| x.as_str().cmp(y.as_str()))
        })
        .cloned()
}

/// Lower is needier: hunger outranks thirst, thirst outranks an owed promise,
/// an owed promise outranks kinship, kinship outranks the fed.
fn need_score(w: &World, a: &Agent, name: &str) -> f32 {
    let Some(x) = w.agent(name) else {
        return f32::MAX;
    };
    if x.hunger >= 2 {
        return 0.0 - f32::from(x.hunger);
    }
    if x.hunger >= 1 {
        return 1.0 - f32::from(x.hunger);
    }
    if x.thirst >= 1 {
        return 2.0 - f32::from(x.thirst);
    }
    if a.promise.as_ref().is_some_and(|pr| pr.kept.is_none() && pr.to == name) {
        return 3.0;
    }
    if a.relation_with(name).is_some_and(|r| {
        matches!(
            r.kinship,
            Kinship::Partner | Kinship::Parent | Kinship::Child | Kinship::Sibling
        )
    }) {
        return 4.0;
    }
    5.0
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

/// Does the candidate serve the villager's active survival pressure? A move
/// toward food or water counts when the matching pressure is active: a
/// starving villager in an empty wood walks toward grain rather than
/// starving beside the trees.
fn serves_survival(w: &World, a: &Agent, c: &Candidate) -> bool {
    let meta = catalog::meta_for(&c.action);
    match meta.need {
        Need::Food => a.hunger >= SURVIVAL_HUNGER && a.food == 0,
        Need::Water => a.thirst >= SURVIVAL_THIRST && a.water == 0,
        Need::Health => a.ailment.is_some() || a.health < 10,
        Need::Warmth => wintery(w) && a.warmth <= SURVIVAL_WARMTH,
        Need::Rest | Need::Faith => false,
        Need::None => {
            c.verb == Verb::Move
                && ((a.hunger >= SURVIVAL_HUNGER && a.food == 0 && move_seeks_food(w, a, c))
                    || (a.thirst >= SURVIVAL_THIRST && a.water == 0 && move_seeks_water(w, a, c)))
        }
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

/// The motive derived from the action and the world (never asked).
///
/// The precedence follows the goal ladder: hunger, thirst, promise, fear,
/// kinship, duty. A move toward food or water carries the matching motive:
/// the feet are already doing hunger's work.
#[must_use]
pub fn derive_motive(w: &World, a: &Agent, d: &Decision) -> Motive {
    let meta = catalog::meta_for(&d.action);
    if matches!(meta.need, Need::Food) && (a.hunger >= 1 || a.food == 0) {
        return Motive::Hunger;
    }
    if d.verb == Verb::Move && a.hunger >= 1 && move_seeks_food(w, a, &candidate_of(d)) {
        return Motive::Hunger;
    }
    if matches!(meta.need, Need::Water) && (a.thirst >= 1 || a.water == 0) {
        return Motive::Thirst;
    }
    if d.verb == Verb::Move && a.thirst >= 1 && move_seeks_water(w, a, &candidate_of(d)) {
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

    // Tiered precedence. Tier 0 beats tier 1 beats tier 2. When nothing on
    // the menu serves survival directly, a hungry villager still walks
    // toward food rather than falling back to the full menu and their trade:
    // the fallback admits need-seeking moves, so hunger steers the feet even
    // when the shelf is a turn away.
    let survival = is_survival(w, a);
    let obligation = obligation_against(w, a);
    let tiered: Option<Vec<Candidate>> = if survival {
        let served: Vec<Candidate> = pool
            .iter()
            .filter(|c| serves_survival(w, a, c))
            .cloned()
            .collect();
        if served.is_empty() {
            Some(
                pool.iter()
                    .filter(|c| {
                        c.verb == Verb::Move
                            && ((a.hunger >= SURVIVAL_HUNGER
                                && a.food == 0
                                && move_seeks_food(w, a, c))
                                || (a.thirst >= SURVIVAL_THIRST
                                    && a.water == 0
                                    && move_seeks_water(w, a, c)))
                    })
                    .cloned()
                    .collect(),
            )
        } else {
            Some(served)
        }
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

    // Why-not: tier rejects first (never scored), then the nearest
    // outscored runners-up. Bounded at three for the record.
    let mut rejected: Vec<Rejected> = Vec::with_capacity(3);
    if hard {
        for c in pool.iter().filter(|c| !scope.iter().any(|s| s.key() == c.key())) {
            if rejected.len() >= 3 {
                break;
            }
            rejected.push(Rejected {
                candidate: c.key(),
                score: None,
                reason: "tier".to_owned(),
            });
        }
    }
    {
        let mut runners: Vec<ScoredRef<'_>> =
            scored.iter().filter(|(k, _, _)| *k != chosen_key).collect();
        runners.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (k, s, _) in runners {
            if rejected.len() >= 3 {
                break;
            }
            if rejected.iter().any(|r| r.candidate == *k) {
                continue;
            }
            rejected.push(Rejected {
                candidate: k.clone(),
                score: Some((s * 100.0).round() / 100.0),
                reason: "outscored".to_owned(),
            });
        }
    }

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
        rejected,
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

    fn mover(action: &str, dest: Place) -> Candidate {
        Candidate {
            action: action.into(),
            verb: Verb::Move,
            place: Some(dest),
            act: SocialAct::None,
        }
    }

    #[test]
    fn a_move_toward_stocked_work_scores_but_a_move_away_does_not() {
        let mut w = World::founding(11);
        w.agents.clear();
        // Bo is the farmer: tend and forage wait in the field with no input,
        // water at the well behind its own input gate. Drain the well so the
        // field is the only stocked destination.
        w.place_state_mut(Place::Well).water = 0;
        w.agents.push(villager("Bo", Place::Granary));
        let a = w.agent("Bo").unwrap();
        assert_eq!(
            term_profession_ready(&w, a, &mover("go:field", Place::Field)),
            0.4
        );
        assert_eq!(
            term_profession_ready(&w, a, &mover("go:market", Place::Market)),
            0.0
        );
        // Standing in the field, walking out recedes from the only work.
        w.agents.clear();
        w.agents.push(villager("Bo", Place::Field));
        let a = w.agent("Bo").unwrap();
        assert_eq!(
            term_profession_ready(&w, a, &mover("go:granary", Place::Granary)),
            0.0
        );
    }

    #[test]
    fn a_thirsty_villager_walks_toward_the_well_not_away() {
        let mut w = World::founding(12);
        w.agents.clear();
        let mut n = villager("Nell", Place::Market);
        n.thirst = 1;
        n.water = 0;
        w.agents.push(n);
        let a = w.agent("Nell").unwrap();
        let toward = mover("go:well", Place::Well);
        let away = mover("go:granary", Place::Granary);
        assert_eq!(term_mood_pull(&w, a, &toward), 1.0);
        assert_eq!(term_mood_pull(&w, a, &away), 0.0);
    }

    #[test]
    fn survival_keeps_a_food_seeking_move_and_drops_a_dry_one() {
        let mut w = World::founding(13);
        w.agents.clear();
        let mut b = villager("Bo", Place::Shrine);
        b.hunger = 2;
        b.food = 0;
        w.agents.push(b);
        w.place_state_mut(Place::Woods).food = 0;
        w.place_state_mut(Place::Hut).food = 0;
        let a = w.agent("Bo").unwrap();
        // The woods can be foraged; the hut holds nothing.
        assert!(serves_survival(&w, a, &mover("go:woods", Place::Woods)));
        assert!(!serves_survival(&w, a, &mover("go:hut", Place::Hut)));
    }

    fn giver(action: &str, verb: Verb) -> Candidate {
        Candidate {
            action: action.into(),
            verb,
            place: None,
            act: SocialAct::Gift,
        }
    }

    #[test]
    fn need_breaks_a_trust_tie_for_gifts() {
        // Ticket #23: trust orders, need breaks ties, name last. Bo and Cor
        // are equally trusted; Cor is hungry, so the gift goes to Cor.
        let mut w = World::founding(10);
        w.agents.clear();
        let mut ada = villager("Ada", Place::Hut);
        ada.set_trust("Bo", 3.0);
        ada.set_trust("Cor", 3.0);
        ada.food = 2;
        w.agents.push(ada);
        w.agents.push(villager("Bo", Place::Hut));
        let mut cor = villager("Cor", Place::Hut);
        cor.hunger = 2;
        w.agents.push(cor);
        let a = w.agent("Ada").unwrap();
        assert_eq!(addressee_of(&w, a, &giver("give", Verb::Give)).as_deref(), Some("Cor"));
    }

    #[test]
    fn a_villager_holding_four_measures_is_not_fed_further() {
        // Ticket #23 satiety cap: a villager holding 4+ measures is refused
        // as a give addressee however trusted; the gift goes elsewhere.
        let mut w = World::founding(10);
        w.agents.clear();
        let mut ada = villager("Ada", Place::Hut);
        ada.set_trust("Cor", 3.9);
        ada.set_trust("Bo", 1.0);
        ada.food = 2;
        w.agents.push(ada);
        let mut cor = villager("Cor", Place::Hut);
        cor.food = 5;
        w.agents.push(cor);
        w.agents.push(villager("Bo", Place::Hut));
        let a = w.agent("Ada").unwrap();
        assert_eq!(addressee_of(&w, a, &giver("give", Verb::Give)).as_deref(), Some("Bo"));
    }

    #[test]
    fn the_decision_names_what_it_did_not_take() {
        // Diagnostics item 2: the record carries the nearest runners-up so
        // the observer can ask "why didn't she do X" from the turn alone.
        let mut w = World::founding(10);
        w.agents.clear();
        w.agents.push(villager("Ada", Place::Hut));
        w.agents.push(villager("Bo", Place::Hut));
        let d = decide(&w, "Ada").expect("decision");
        assert!(d.rejected.len() <= 3, "bounded at three: {:?}", d.rejected);
        assert!(
            !d.rejected.iter().any(|r| r.candidate == d.action),
            "the winner is never its own runner-up"
        );
        assert!(
            d.rejected.iter().all(|r| matches!(r.reason.as_str(), "tier" | "outscored")),
            "every reject names its cause: {:?}",
            d.rejected
        );
        // The turn record joins the decision to the events it caused.
        let mut w2 = World::founding(10);
        let rec = crate::turn::step(&mut w2).expect("turn");
        for at in &rec.agents {
            let id = at.decision_id.as_deref().expect("decision id");
            assert_eq!(id, format!("{}:{}:{}", w2.seed, rec.turn, at.name));
            let (s, e) = at.event_range.expect("event range");
            assert!(s <= e && e <= w2.events.len(), "range {s}..{e} in bounds");
        }
    }
}
