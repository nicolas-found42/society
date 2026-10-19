use serde::{Deserialize, Serialize};

use crate::memory::{Memory, MemoryKind};
use crate::world::{Agent, Event, EventKind, Place, Promise, Season, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    Speak,
    Rest,
    Take,
    Give,
    Forage,
    Help,
    Steal,
    Accuse,
    Promise,
    Move,
    DrawWater,
    Chop,
    GatherHerbs,
    Craft,
    Tend,
    Heal,
    Trade,
    Pray,
    Repair,
    Console,
}

impl Verb {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Speak => "speak",
            Self::Rest => "rest",
            Self::Take => "take",
            Self::Give => "give",
            Self::Forage => "forage",
            Self::Help => "help",
            Self::Steal => "steal",
            Self::Accuse => "accuse",
            Self::Promise => "promise",
            Self::Move => "move",
            Self::DrawWater => "draw_water",
            Self::Chop => "chop",
            Self::GatherHerbs => "gather_herbs",
            Self::Craft => "craft",
            Self::Tend => "tend",
            Self::Heal => "heal",
            Self::Trade => "trade",
            Self::Pray => "pray",
            Self::Repair => "repair",
            Self::Console => "console",
        }
    }
}

/// A fully resolved intent. `action` is the stable wire id the model chooses
/// from the enumerated affordances; nothing here is inferred later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    /// Stable id: `speak`, `rest`, `take`, `give`, `forage`, `help`, `steal`,
    /// `accuse`, `promise`, or `go:<place>`.
    pub action: String,
    pub verb: Verb,
    pub place: Option<Place>,
}

impl Intent {
    #[must_use]
    pub fn parse(action: &str, from: Place) -> Option<Self> {
        if let Some(rest) = action.strip_prefix("go:") {
            let dest = Place::from_id(rest)?;
            if !from.adjacent().contains(&dest) {
                return None;
            }
            return Some(Self {
                action: action.to_owned(),
                verb: Verb::Move,
                place: Some(dest),
            });
        }
        let verb = match action {
            "speak" => Verb::Speak,
            "rest" => Verb::Rest,
            "take" => Verb::Take,
            "give" => Verb::Give,
            "forage" => Verb::Forage,
            "help" => Verb::Help,
            "steal" => Verb::Steal,
            "accuse" => Verb::Accuse,
            "promise" => Verb::Promise,
            "draw_water" => Verb::DrawWater,
            "chop" => Verb::Chop,
            "gather_herbs" => Verb::GatherHerbs,
            "craft" => Verb::Craft,
            "tend" => Verb::Tend,
            "heal" => Verb::Heal,
            "trade" => Verb::Trade,
            "pray" => Verb::Pray,
            "repair" => Verb::Repair,
            "console" => Verb::Console,
            _ => return None,
        };
        Some(Self {
            action: action.to_owned(),
            verb,
            place: None,
        })
    }
}

/// Prose rendering of a wire action id, for prompts that must not leak the id.
///
/// The voice model copies the nearest salient string in its card; a raw id like
/// `draw_water` is exactly what it copied (measured: 5 of 45 lines in a 5-turn
/// run were the action id, verbatim). Any surface the model reads — the card's
/// decision line and the actor's own memory — must therefore carry the phrase,
/// never the id.
///
/// A place id is kept verbatim (`go:market` -> "leave for market"); an unknown
/// id falls back to its own spelling with underscores turned into spaces.
#[must_use]
pub fn action_phrase(action: &str) -> String {
    if let Some(place) = action.strip_prefix("go:") {
        return format!("leave for {}", place.replace('_', " "));
    }
    let phrase = match action {
        "speak" => "speak aloud",
        "rest" => "rest",
        "take" => "take food from the store here",
        "give" => "give food to someone here",
        "forage" => "forage here for food",
        "help" => "help someone here",
        "steal" => "take what belongs to someone else",
        "accuse" => "accuse someone here",
        "promise" => "promise someone here",
        "draw_water" => "draw water from the well",
        "chop" => "chop wood",
        "gather_herbs" => "gather herbs",
        "craft" => "craft a tool",
        "tend" => "tend the field",
        "heal" => "heal the sick",
        "trade" => "trade at the market",
        "pray" => "pray at the shrine",
        "repair" => "repair what is broken",
        "console" => "console someone here",
        _ => return action.replace('_', " "),
    };
    phrase.to_owned()
}

/// Every action the simulator will accept for this agent right now, in the
/// exact spelling the model must choose from.
#[must_use]
pub fn affordances(w: &World, a: &Agent) -> Vec<String> {
    let here = w.place_state(a.place);
    let others = w.present(a.place, &a.name);
    // Urgency is code's business. A villager who is starving and empty-handed
    // is not offered conversation, rest, or generosity: the only actions that
    // can save them are eating here, working the ground here, or walking toward
    // food. Without this filter a small chooser reliably picks `speak` and the
    // settlement dies with a full granary.
    let desperate = a.hunger >= 2 && a.food == 0;

    let mut v = Vec::new();
    if here.food > 0 {
        v.push("take".to_owned());
        if here.owner.is_some() {
            v.push("steal".into());
        }
    }
    if a.place.can_harvest() {
        v.push("forage".into());
    }
    if desperate {
        // Only moves that lead somewhere with food, or that can be worked.
        let mut moves: Vec<String> = a
            .place
            .adjacent()
            .iter()
            .filter(|p| w.place_state(**p).food > 0 || p.can_harvest())
            .map(|p| format!("go:{}", p.id()))
            .collect();
        if moves.is_empty() {
            moves = a
                .place
                .adjacent()
                .iter()
                .map(|p| format!("go:{}", p.id()))
                .collect();
        }
        v.extend(moves);
        // Nothing edible anywhere in reach: they may still try to talk someone
        // into sharing, which is the last honest option.
        if v.is_empty() {
            v.push("speak".into());
        }
        return v;
    }

    v.insert(0, "speak".to_owned());
    v.push("rest".into());
    if a.place.can_draw_water() && here.water > 0 {
        v.push("draw_water".into());
    }
    if a.place.can_chop() && here.wood > 0 {
        v.push("chop".into());
    }
    if a.place.can_gather_herbs() && here.herbs > 0 {
        v.push("gather_herbs".into());
    }
    if a.place.can_craft() && (a.wood > 0 || here.wood > 0) {
        v.push("craft".into());
    }
    if a.place == Place::Field {
        v.push("tend".into());
    }
    if (a.herbs > 0 || here.herbs > 0)
        && (a.ailment.is_some()
            || a.health < 10
            || others.iter().any(|o| {
                w.agent(o)
                    .is_some_and(|oa| oa.ailment.is_some() || oa.health < 10)
            }))
    {
        v.push("heal".into());
    }
    if a.place == Place::Market && (a.wood > 0 || a.herbs > 0 || a.tools > 0 || a.coin > 0) {
        v.push("trade".into());
    }
    if a.place == Place::Shrine {
        v.push("pray".into());
    }
    if (a.wood > 0 || here.wood > 0)
        && (here.condition < 5 || (a.place == Place::Hut && !here.hearth_lit))
    {
        v.push("repair".into());
    }
    if a.food > 0 && !others.is_empty() {
        v.push("give".into());
        v.push("help".into());
    }
    if !others.is_empty() {
        v.push("console".into());
        v.push("accuse".into());
        v.push("promise".into());
    }
    for p in a.place.adjacent() {
        v.push(format!("go:{}", p.id()));
    }
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialAct {
    None,
    Promise,
    Gift,
    Accusation,
    Threat,
    Gossip,
    Plea,
    Gratitude,
    Counsel,
    TradeOffer,
    Blessing,
    Forgiveness,
}

impl SocialAct {
    #[must_use]
    pub fn from_id(s: &str) -> Self {
        match s {
            "promise" => Self::Promise,
            "gift" => Self::Gift,
            "accusation" => Self::Accusation,
            "threat" => Self::Threat,
            "gossip" => Self::Gossip,
            "plea" => Self::Plea,
            "gratitude" => Self::Gratitude,
            "counsel" => Self::Counsel,
            "trade_offer" => Self::TradeOffer,
            "blessing" => Self::Blessing,
            "forgiveness" => Self::Forgiveness,
            _ => Self::None,
        }
    }

    /// Trust delta applied to every listener when the act lands.
    #[must_use]
    pub const fn trust_delta(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Promise => 0.2,
            Self::Gift => 0.5,
            Self::Accusation => -0.7,
            Self::Threat => -1.0,
            Self::Gossip => -0.2,
            Self::Plea => -0.1,
            Self::Gratitude | Self::Blessing => 0.3,
            Self::Counsel => 0.35,
            Self::TradeOffer => 0.15,
            Self::Forgiveness => 0.45,
        }
    }

    #[must_use]
    pub const fn salience(self) -> f32 {
        match self {
            Self::None => 0.2,
            Self::Promise => 0.8,
            Self::Gift => 0.7,
            Self::Accusation | Self::Threat => 0.9,
            Self::Gossip | Self::Blessing => 0.6,
            Self::Plea => 0.5,
            Self::Gratitude => 0.4,
            Self::Counsel => 0.65,
            Self::TradeOffer => 0.45,
            Self::Forgiveness => 0.75,
        }
    }
}

/// What actually happened after the simulator validated an intent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub accepted: bool,
    pub kind: EventKind,
    pub note: Option<String>,
}

/// The validated consequence of an intent, computed before any mutation so the
/// borrow of `w` stays short and the decision is auditable on its own.
enum Plan {
    Rejected(String),
    Noop(EventKind),
    Move(Place),
    Transfer {
        food_after_carried: u32,
        food_after_here: u32,
        kind: EventKind,
        note: Option<String>,
    },
    Give {
        target: String,
    },
    Promise {
        to: String,
    },
    DrawWater {
        amount: u32,
    },
    Chop {
        amount: u32,
    },
    GatherHerbs {
        amount: u32,
    },
    CraftTool {
        use_carried_wood: bool,
    },
    TendField,
    Heal {
        target: String,
        use_carried_herb: bool,
    },
    TradeMarket,
    PrayShrine,
    RepairPlace {
        use_carried_wood: bool,
    },
    Console {
        target: String,
    },
}

#[allow(
    clippy::too_many_lines,
    reason = "one sequential rule table; splitting hides the order"
)]
fn plan(w: &World, actor: &str, intent: &Intent) -> Plan {
    let Some(a) = w.agent(actor) else {
        return Plan::Rejected("no such agent".into());
    };
    if !a.alive {
        return Plan::Rejected("dead".into());
    }
    let place = a.place;
    let here = w.place_state(place);
    let others = w.present(place, actor);

    match intent.verb {
        Verb::Speak => Plan::Noop(EventKind::Speech),
        Verb::Rest => Plan::Noop(EventKind::Rest),
        Verb::Move => match intent.place {
            Some(dest) if place.adjacent().contains(&dest) => Plan::Move(dest),
            Some(dest) => {
                Plan::Rejected(format!("{} is not adjacent to {}", dest.id(), place.id()))
            },
            None => Plan::Rejected("no destination".into()),
        },
        Verb::Take | Verb::Steal => {
            let stealing = intent.verb == Verb::Steal;
            if here.food == 0 {
                return Plan::Rejected("no food here".into());
            }
            match (&here.owner, stealing) {
                (None, true) => Plan::Rejected("nothing to steal".into()),
                // Taking from a held store is theft whether or not you say so.
                (Some(o), _) if o != actor => Plan::Transfer {
                    food_after_carried: a.food + 1,
                    food_after_here: here.food - 1,
                    kind: EventKind::Betrayal,
                    note: Some(format!("took from {o}'s store")),
                },
                _ => Plan::Transfer {
                    food_after_carried: a.food + 1,
                    food_after_here: here.food - 1,
                    kind: EventKind::Transfer,
                    note: None,
                },
            }
        },
        Verb::Forage => {
            if place.can_harvest() {
                let tool_bonus = u32::from(a.tools > 0 && a.skills.farming >= 3);
                let base = if w.weather > 1.1 { 2 } else { 1 };
                let yield_ = base + tool_bonus;
                Plan::Transfer {
                    food_after_carried: a.food + yield_,
                    food_after_here: here.food,
                    kind: EventKind::Harvest,
                    note: Some(format!("+{yield_}")),
                }
            } else {
                Plan::Rejected("nothing grows here".into())
            }
        },
        Verb::Give | Verb::Help => {
            if a.food == 0 {
                Plan::Rejected("nothing to give".into())
            } else if a.hunger >= 2 && a.food <= 1 {
                Plan::Rejected("would starve".into())
            } else if others.is_empty() {
                Plan::Rejected("nobody here".into())
            } else {
                Plan::Give {
                    target: others[0].clone(),
                }
            }
        },
        Verb::Accuse => {
            if others.is_empty() {
                Plan::Rejected("nobody here".into())
            } else {
                Plan::Noop(EventKind::Accusation)
            }
        },
        Verb::Promise => {
            if others.is_empty() {
                Plan::Rejected("nobody here".into())
            } else {
                Plan::Promise {
                    to: others[0].clone(),
                }
            }
        },
        Verb::DrawWater => {
            if !place.can_draw_water() {
                Plan::Rejected("no well here".into())
            } else if here.water == 0 {
                Plan::Rejected("the well is dry".into())
            } else {
                Plan::DrawWater {
                    amount: here.water.min(2),
                }
            }
        },
        Verb::Chop => {
            if !place.can_chop() {
                Plan::Rejected("no timber here".into())
            } else if here.wood == 0 {
                Plan::Rejected("no fallen timber left".into())
            } else {
                let amount = if a.tools > 0 || a.skills.forestry >= 2 {
                    here.wood.min(2)
                } else {
                    1
                };
                Plan::Chop { amount }
            }
        },
        Verb::GatherHerbs => {
            if !place.can_gather_herbs() {
                Plan::Rejected("no herbs grow here".into())
            } else if here.herbs == 0 {
                Plan::Rejected("herbs are depleted here".into())
            } else {
                let amount = if a.skills.herbalism >= 2 {
                    here.herbs.min(2)
                } else {
                    1
                };
                Plan::GatherHerbs { amount }
            }
        },
        Verb::Craft => {
            if !place.can_craft() {
                Plan::Rejected("cannot craft here".into())
            } else if a.wood > 0 {
                Plan::CraftTool {
                    use_carried_wood: true,
                }
            } else if here.wood > 0 {
                Plan::CraftTool {
                    use_carried_wood: false,
                }
            } else {
                Plan::Rejected("need wood to craft".into())
            }
        },
        Verb::Tend => {
            if place == Place::Field {
                Plan::TendField
            } else {
                Plan::Rejected("can only tend the field".into())
            }
        },
        Verb::Heal => {
            let use_carried_herb = a.herbs > 0;
            if !use_carried_herb && here.herbs == 0 {
                return Plan::Rejected("no herbs available".into());
            }
            if a.ailment.is_some() || a.health < 10 {
                Plan::Heal {
                    target: actor.to_owned(),
                    use_carried_herb,
                }
            } else if let Some(sick) = others.iter().find(|o| {
                w.agent(o)
                    .is_some_and(|oa| oa.ailment.is_some() || oa.health < 10)
            }) {
                Plan::Heal {
                    target: sick.clone(),
                    use_carried_herb,
                }
            } else {
                Plan::Heal {
                    target: actor.to_owned(),
                    use_carried_herb,
                }
            }
        },
        Verb::Trade => {
            if place != Place::Market {
                Plan::Rejected("trading happens at the market".into())
            } else if a.wood == 0 && a.herbs == 0 && a.tools == 0 && a.coin == 0 {
                Plan::Rejected("nothing to trade".into())
            } else {
                Plan::TradeMarket
            }
        },
        Verb::Pray => {
            if place == Place::Shrine {
                Plan::PrayShrine
            } else {
                Plan::Rejected("prayers belong at the shrine".into())
            }
        },
        Verb::Repair => {
            if a.wood > 0 {
                Plan::RepairPlace {
                    use_carried_wood: true,
                }
            } else if here.wood > 0 {
                Plan::RepairPlace {
                    use_carried_wood: false,
                }
            } else {
                Plan::Rejected("need wood to repair or stoke hearth".into())
            }
        },
        Verb::Console => {
            if others.is_empty() {
                Plan::Rejected("nobody here to console".into())
            } else {
                Plan::Console {
                    target: others[0].clone(),
                }
            }
        },
    }
}

/// Validate and apply one intent. Returns what was committed. Speech is passed
/// in because rendering already happened; this only moves state.
///
/// # Panics
///
/// Panics if `actor` is not a known agent after `plan` accepted the intent.
#[allow(
    clippy::too_many_lines,
    reason = "one sequential rule table; splitting hides the order"
)]
pub fn apply(
    w: &mut World,
    actor: &str,
    intent: &Intent,
    speech: &str,
    social: SocialAct,
) -> Outcome {
    let turn = w.turn;
    let p = plan(w, actor, intent);

    let (accepted, kind, note) = match p {
        Plan::Rejected(why) => (false, EventKind::Confusion, Some(why)),
        Plan::Noop(kind) => {
            if kind == EventKind::Rest {
                if let Some(a) = w.agent_mut(actor) {
                    a.energy = (a.energy + 3).min(10);
                    a.stress = (a.stress - 0.3).max(0.0);
                    if a.place == Place::Hut {
                        a.warmth = (a.warmth + 1).min(4);
                    }
                }
            }
            (true, kind, None)
        },
        Plan::Move(dest) => {
            if let Some(a) = w.agent_mut(actor) {
                a.place = dest;
                a.energy = a.energy.saturating_sub(1);
            }
            (true, EventKind::Move, None)
        },
        Plan::Transfer {
            food_after_carried,
            food_after_here,
            kind,
            note,
        } => {
            let place = w.agent(actor).expect("actor exists").place;
            if let Some(a) = w.agent_mut(actor) {
                a.food = food_after_carried;
                if kind == EventKind::Harvest {
                    a.skills.farming = (a.skills.farming + 1).min(5);
                    a.energy = a.energy.saturating_sub(1);
                }
            }
            w.place_state_mut(place).food = food_after_here;
            (true, kind, note)
        },
        Plan::Give { target } => {
            let place = w.agent(actor).expect("actor exists").place;
            if let Some(a) = w.agent_mut(actor) {
                a.food -= 1;
                a.adjust_trust(&target, 0.25);
                a.standing = (a.standing + 0.15).min(4.0);
            }
            if let Some(t) = w.agent_mut(&target) {
                t.food += 1;
                t.hunger = t.hunger.saturating_sub(1);
                t.adjust_trust(actor, 0.6);
                t.stress = (t.stress - 0.3).max(0.0);
                t.remember(Memory::new(
                    turn,
                    format!("{actor} gave you food."),
                    0.75,
                    MemoryKind::Witnessed,
                ));
            }
            let _ = place;
            (true, EventKind::Transfer, Some(format!("to {target}")))
        },
        Plan::Promise { to } => {
            if let Some(a) = w.agent_mut(actor) {
                a.promise = Some(Promise {
                    to: to.clone(),
                    text: speech.to_owned(),
                    made_turn: turn,
                    kept: None,
                });
            }
            (true, EventKind::Promise, Some(format!("to {to}")))
        },
        Plan::DrawWater { amount } => {
            if let Some(a) = w.agent_mut(actor) {
                a.water += amount;
                a.thirst = 0;
            }
            let st = w.place_state_mut(Place::Well);
            st.water = st.water.saturating_sub(amount);
            (true, EventKind::Harvest, Some(format!("+{amount} water")))
        },
        Plan::Chop { amount } => {
            let place = w.agent(actor).expect("actor exists").place;
            if let Some(a) = w.agent_mut(actor) {
                a.wood += amount;
                a.skills.forestry = (a.skills.forestry + 1).min(5);
                a.energy = a.energy.saturating_sub(1);
            }
            let st = w.place_state_mut(place);
            st.wood = st.wood.saturating_sub(amount);
            (true, EventKind::Harvest, Some(format!("+{amount} wood")))
        },
        Plan::GatherHerbs { amount } => {
            let place = w.agent(actor).expect("actor exists").place;
            if let Some(a) = w.agent_mut(actor) {
                a.herbs += amount;
                a.skills.herbalism = (a.skills.herbalism + 1).min(5);
                a.energy = a.energy.saturating_sub(1);
            }
            let st = w.place_state_mut(place);
            st.herbs = st.herbs.saturating_sub(amount);
            (true, EventKind::Harvest, Some(format!("+{amount} herbs")))
        },
        Plan::CraftTool { use_carried_wood } => {
            let place = w.agent(actor).expect("actor exists").place;
            if use_carried_wood {
                if let Some(a) = w.agent_mut(actor) {
                    a.wood = a.wood.saturating_sub(1);
                }
            } else {
                let st = w.place_state_mut(place);
                st.wood = st.wood.saturating_sub(1);
            }
            if let Some(a) = w.agent_mut(actor) {
                a.tools += 1;
                a.skills.craft = (a.skills.craft + 1).min(5);
                a.standing = (a.standing + 0.1).min(4.0);
            }
            (true, EventKind::Craft, Some("crafted tool".into()))
        },
        Plan::TendField => {
            let has_water = w.agent(actor).is_some_and(|a| a.water > 0);
            if let Some(a) = w.agent_mut(actor) {
                if has_water {
                    a.water -= 1;
                }
                a.skills.farming = (a.skills.farming + 1).min(5);
                a.energy = a.energy.saturating_sub(1);
            }
            let st = w.place_state_mut(Place::Field);
            st.fertility = (st.fertility + 0.25).min(2.0);
            st.food = (st.food + 1).min(12);
            (true, EventKind::Harvest, Some("tended soil".into()))
        },
        Plan::Heal {
            target,
            use_carried_herb,
        } => {
            let place = w.agent(actor).expect("actor exists").place;
            if use_carried_herb {
                if let Some(a) = w.agent_mut(actor) {
                    a.herbs = a.herbs.saturating_sub(1);
                }
            } else {
                let st = w.place_state_mut(place);
                st.herbs = st.herbs.saturating_sub(1);
            }
            if let Some(a) = w.agent_mut(actor) {
                a.skills.herbalism = (a.skills.herbalism + 1).min(5);
                if target != actor {
                    a.standing = (a.standing + 0.2).min(4.0);
                    a.adjust_trust(&target, 0.25);
                }
            }
            if let Some(t) = w.agent_mut(&target) {
                t.ailment = None;
                t.health = (t.health + 4).min(10);
                t.stress = (t.stress - 0.5).max(0.0);
                if target != actor {
                    t.adjust_trust(actor, 0.65);
                }
            }
            (true, EventKind::Heal, Some(format!("healed {target}")))
        },
        Plan::TradeMarket => {
            if let Some(a) = w.agent_mut(actor) {
                if a.wood > 0 {
                    a.wood -= 1;
                    a.coin += 1;
                    a.food += 1;
                } else if a.herbs > 0 {
                    a.herbs -= 1;
                    a.coin += 2;
                } else if a.coin > 0 {
                    a.coin -= 1;
                    a.food += 1;
                } else if a.tools > 1 {
                    a.tools -= 1;
                    a.coin += 2;
                    a.food += 1;
                }
                a.skills.stewardship = (a.skills.stewardship + 1).min(5);
            }
            let st = w.place_state_mut(Place::Market);
            st.coin = (st.coin + 1).min(20);
            (true, EventKind::Trade, Some("bartered at market".into()))
        },
        Plan::PrayShrine => {
            if let Some(a) = w.agent_mut(actor) {
                a.stress = (a.stress - 0.8).max(0.0);
                a.hope = (a.hope + 0.5).min(4.0);
                a.devotion = (a.devotion + 0.3).min(4.0);
            }
            (true, EventKind::Prayer, Some("prayed at the shrine".into()))
        },
        Plan::RepairPlace { use_carried_wood } => {
            let place = w.agent(actor).expect("actor exists").place;
            if use_carried_wood {
                if let Some(a) = w.agent_mut(actor) {
                    a.wood = a.wood.saturating_sub(1);
                }
            } else {
                let st = w.place_state_mut(place);
                st.wood = st.wood.saturating_sub(1);
            }
            let st = w.place_state_mut(place);
            st.condition = (st.condition + 1).min(5);
            st.hearth_lit = true;
            if let Some(a) = w.agent_mut(actor) {
                a.warmth = (a.warmth + 2).min(4);
                a.standing = (a.standing + 0.15).min(4.0);
            }
            (
                true,
                EventKind::Repair,
                Some(format!("repaired {}", place.id())),
            )
        },
        Plan::Console { target } => {
            if let Some(a) = w.agent_mut(actor) {
                a.skills.counsel = (a.skills.counsel + 1).min(5);
                a.adjust_trust(&target, 0.3);
            }
            if let Some(t) = w.agent_mut(&target) {
                t.stress = (t.stress - 0.6).max(0.0);
                t.hope = (t.hope + 0.4).min(4.0);
                t.adjust_trust(actor, 0.45);
            }
            (true, EventKind::Bond, Some(format!("consoled {target}")))
        },
    };

    if accepted {
        let place = w.agent(actor).expect("actor exists").place;
        let st_snapshot = w.place_state(place).clone();
        let season = Season::from_day(w.day);
        if let Some(a) = w.agent_mut(actor) {
            a.observe_place(place, &st_snapshot, turn);
            a.refresh_mood_and_goal(season);
        }

        // Social acts colour trust for everyone in the room, whatever the verb.
        if social != SocialAct::None {
            let listeners = w.present(place, actor);
            let delta = social.trust_delta();
            let salience = social.salience();
            let text = if speech.is_empty() {
                format!("{actor} did something to you.")
            } else {
                format!("{actor} said: \"{speech}\"")
            };
            for l in listeners {
                if let Some(x) = w.agent_mut(&l) {
                    x.adjust_trust(actor, delta);
                    x.record_fact(actor, text.clone(), turn, 0.9);
                    x.remember(Memory::new(
                        turn,
                        text.clone(),
                        salience,
                        MemoryKind::Witnessed,
                    ));
                }
            }
        }

        w.push_event(Event {
            turn,
            actor: actor.to_owned(),
            place,
            kind,
            action: intent.action.clone(),
            text: speech.to_owned(),
            social: Some(social.id().to_owned()),
            intensity: None,
            grounded: None,
            fidelity: None,
            divine: false,
        });

        // The actor remembers their own act.
        if let Some(x) = w.agent_mut(actor) {
            let text = if speech.is_empty() {
                format!("You {}.", action_phrase(&intent.action))
            } else {
                format!("You said: \"{speech}\"")
            };
            x.remember(Memory::new(
                turn,
                text,
                social.salience().max(0.3),
                MemoryKind::Own,
            ));
        }
    } else {
        w.push_event(Event {
            turn,
            actor: actor.to_owned(),
            place: w.agent(actor).map_or(Place::Granary, |a| a.place),
            kind: EventKind::Confusion,
            action: intent.action.clone(),
            text: note.clone().unwrap_or_else(|| "confused".into()),
            social: None,
            intensity: None,
            grounded: None,
            fidelity: None,
            divine: false,
        });
    }

    Outcome {
        accepted,
        kind,
        note,
    }
}

impl SocialAct {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Promise => "promise",
            Self::Gift => "gift",
            Self::Accusation => "accusation",
            Self::Threat => "threat",
            Self::Gossip => "gossip",
            Self::Plea => "plea",
            Self::Gratitude => "gratitude",
            Self::Counsel => "counsel",
            Self::TradeOffer => "trade_offer",
            Self::Blessing => "blessing",
            Self::Forgiveness => "forgiveness",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    #[test]
    fn action_phrase_matches_the_contract_table() {
        let table = [
            ("speak", "speak aloud"),
            ("rest", "rest"),
            ("take", "take food from the store here"),
            ("give", "give food to someone here"),
            ("forage", "forage here for food"),
            ("help", "help someone here"),
            ("steal", "take what belongs to someone else"),
            ("accuse", "accuse someone here"),
            ("promise", "promise someone here"),
            ("draw_water", "draw water from the well"),
            ("chop", "chop wood"),
            ("gather_herbs", "gather herbs"),
            ("craft", "craft a tool"),
            ("tend", "tend the field"),
            ("heal", "heal the sick"),
            ("trade", "trade at the market"),
            ("pray", "pray at the shrine"),
            ("repair", "repair what is broken"),
            ("console", "console someone here"),
            ("go:market", "leave for market"),
        ];
        for (id, phrase) in table {
            assert_eq!(action_phrase(id), phrase, "{id}");
        }
    }

    #[test]
    fn action_phrase_spaces_an_unknown_id() {
        assert_eq!(action_phrase("foo_bar"), "foo bar");
    }

    #[test]
    fn no_action_phrase_leaks_an_underscore() {
        // Every id the wire vocabulary can carry, including the `move` verb id
        // (the model never chooses it; `go:<place>` replaces it) and every
        // place form. A prompt that shows an underscore shows the wire id.
        let verbs = [
            Verb::Speak,
            Verb::Rest,
            Verb::Take,
            Verb::Give,
            Verb::Forage,
            Verb::Help,
            Verb::Steal,
            Verb::Accuse,
            Verb::Promise,
            Verb::Move,
            Verb::DrawWater,
            Verb::Chop,
            Verb::GatherHerbs,
            Verb::Craft,
            Verb::Tend,
            Verb::Heal,
            Verb::Trade,
            Verb::Pray,
            Verb::Repair,
            Verb::Console,
        ];
        let mut ids: Vec<String> = verbs.iter().map(|v| v.id().to_owned()).collect();
        for p in Place::ALL {
            ids.push(format!("go:{}", p.id()));
        }
        for id in ids {
            let phrase = action_phrase(&id);
            assert!(
                !phrase.contains('_'),
                "{id} rendered as {phrase}, which still leaks the id"
            );
        }
    }

    #[test]
    fn a_silent_acts_own_memory_holds_prose_not_the_id() {
        let mut w = World::founding(307);
        let name = w.agents[0].name.clone();
        w.agent_mut(&name).unwrap().place = Place::Well;
        let intent = Intent::parse("draw_water", Place::Well).unwrap();
        let out = apply(&mut w, &name, &intent, "", SocialAct::None);
        assert!(out.accepted);
        let own = w
            .agent(&name)
            .unwrap()
            .memories
            .iter()
            .find(|m| m.kind == MemoryKind::Own)
            .expect("the actor records their own act");
        assert_eq!(own.text, "You draw water from the well.");
        assert!(!own.text.contains("draw_water"));
    }
}
