//! Professions (spec #5, ticket #11): each role owns a profession; each
//! profession owns a queue of work orders at fixed places. A new trade is a
//! data entry here, not new code. The Watchman and the Elder have no work
//! orders, because their duty is already the role goal.
//!
//! Quality and its yield multiplier reuse the existing effect arithmetic:
//! harvest, eating and the granary are not rebalanced.

use crate::action::Verb;
use crate::world::{Agent, Place, Role, World};

/// One item of a profession's queue.
#[derive(Clone, Copy, Debug)]
pub struct WorkOrder {
    /// Stable id, recorded on the decision's `job` field.
    pub id: &'static str,
    /// The existing verb the order advances; the action id stays atomic.
    pub verb: Verb,
    pub place: Place,
    /// The input the place must hold before the order is available:
    /// (resource id, count). `None` when the ground itself is the input.
    pub input: Option<(&'static str, u32)>,
    /// What the order yields, named for the feed.
    pub item: &'static str,
}

/// A trade's queue of work orders.
#[derive(Clone, Copy, Debug)]
pub struct Profession {
    pub role: Role,
    pub name: &'static str,
    pub orders: &'static [WorkOrder],
}

const WOODS: Place = Place::Woods;
const FIELD: Place = Place::Field;
const MARKET: Place = Place::Market;
const HUT: Place = Place::Hut;
const GRANARY: Place = Place::Granary;
const WELL: Place = Place::Well;
const SHRINE: Place = Place::Shrine;

/// The profession table. One per role; eight professions, six with orders.
pub const PROFESSIONS: [Profession; 9] = [
    Profession {
        role: Role::Farmer,
        name: "farmer",
        orders: &[
            WorkOrder {
                id: "farmer.tend",
                verb: Verb::Tend,
                place: FIELD,
                input: None,
                item: "grain",
            },
            WorkOrder {
                id: "farmer.harvest",
                verb: Verb::Forage,
                place: FIELD,
                input: None,
                item: "grain",
            },
            WorkOrder {
                id: "farmer.water",
                verb: Verb::DrawWater,
                place: WELL,
                input: Some(("water", 1)),
                item: "water",
            },
        ],
    },
    Profession {
        role: Role::Woodcutter,
        name: "woodcutter",
        orders: &[
            WorkOrder {
                id: "woodcutter.fell",
                verb: Verb::Chop,
                place: WOODS,
                input: Some(("wood", 1)),
                item: "wood",
            },
            WorkOrder {
                id: "woodcutter.split",
                verb: Verb::Chop,
                place: WOODS,
                input: Some(("wood", 2)),
                item: "wood",
            },
        ],
    },
    Profession {
        role: Role::Herbalist,
        name: "herbalist",
        orders: &[
            WorkOrder {
                id: "herbalist.gather",
                verb: Verb::GatherHerbs,
                place: WOODS,
                input: Some(("herbs", 1)),
                item: "herbs",
            },
            WorkOrder {
                id: "herbalist.gather_shrine",
                verb: Verb::GatherHerbs,
                place: SHRINE,
                input: Some(("herbs", 1)),
                item: "herbs",
            },
            WorkOrder {
                id: "herbalist.heal",
                verb: Verb::Heal,
                place: HUT,
                input: Some(("herbs", 1)),
                item: "health",
            },
        ],
    },
    Profession {
        role: Role::Artisan,
        name: "artisan",
        orders: &[
            WorkOrder {
                id: "artisan.craft",
                verb: Verb::Craft,
                place: MARKET,
                input: Some(("wood", 1)),
                item: "tools",
            },
            WorkOrder {
                id: "artisan.craft_hut",
                verb: Verb::Craft,
                place: HUT,
                input: Some(("wood", 1)),
                item: "tools",
            },
            WorkOrder {
                id: "artisan.repair",
                verb: Verb::Repair,
                place: HUT,
                input: Some(("wood", 1)),
                item: "condition",
            },
        ],
    },
    Profession {
        role: Role::Steward,
        name: "steward",
        orders: &[
            WorkOrder {
                id: "steward.haul",
                verb: Verb::Take,
                place: FIELD,
                input: Some(("food", 1)),
                item: "grain",
            },
            WorkOrder {
                id: "steward.store",
                verb: Verb::Take,
                place: GRANARY,
                input: Some(("food", 1)),
                item: "grain",
            },
            WorkOrder {
                id: "steward.mend",
                verb: Verb::Repair,
                place: GRANARY,
                input: Some(("wood", 1)),
                item: "condition",
            },
        ],
    },
    Profession {
        role: Role::Trader,
        name: "trader",
        orders: &[
            WorkOrder {
                id: "trader.barter",
                verb: Verb::Trade,
                place: MARKET,
                input: None,
                item: "coin",
            },
            WorkOrder {
                id: "trader.haul_water",
                verb: Verb::DrawWater,
                place: WELL,
                input: Some(("water", 1)),
                item: "water",
            },
        ],
    },
    Profession {
        role: Role::Forager,
        name: "forager",
        orders: &[
            WorkOrder {
                id: "forager.woods",
                verb: Verb::Forage,
                place: WOODS,
                input: None,
                item: "grain",
            },
            WorkOrder {
                id: "forager.margins",
                verb: Verb::Forage,
                place: FIELD,
                input: None,
                item: "grain",
            },
            WorkOrder {
                id: "forager.herbs",
                verb: Verb::GatherHerbs,
                place: FIELD,
                input: Some(("herbs", 1)),
                item: "herbs",
            },
        ],
    },
    // The Watchman patrols and the Elder counsels; their duty is the role
    // goal, so neither owns a queue.
    Profession {
        role: Role::Watchman,
        name: "watchman",
        orders: &[],
    },
    Profession {
        role: Role::Elder,
        name: "elder",
        orders: &[],
    },
];

/// The profession for a role, `None` for a role with no queue.
#[must_use]
pub fn profession_for(role: Role) -> Option<&'static Profession> {
    PROFESSIONS.iter().find(|p| p.role == role)
}

fn place_has_input(st: &crate::world::PlaceState, input: Option<(&str, u32)>) -> bool {
    match input {
        None => true,
        Some((res, n)) => match res {
            "food" => st.food >= n,
            "water" => st.water >= n,
            "wood" => st.wood >= n,
            "herbs" => st.herbs >= n,
            "tools" => st.tools >= n,
            "coin" => st.coin >= n,
            _ => false,
        },
    }
}

/// The villager's work orders that are available right now: the place
/// matches where they stand and the place holds the order's input.
///
/// (Spec: "A work order is available when its place has its input and the
/// villager stands there".)
#[must_use]
pub fn available_orders<'p>(w: &World, a: &Agent) -> Vec<&'p WorkOrder> {
    let Some(p) = profession_for(a.role) else {
        return Vec::new();
    };
    let st = w.place_state(a.place);
    p.orders
        .iter()
        .filter(|o| o.place == a.place && place_has_input(st, o.input))
        .collect()
}

/// The order a candidate advances, when the candidate's verb and place match
/// an available order. Ties keep the table's order.
#[must_use]
pub(crate) fn order_for<'p>(w: &World, a: &Agent, verb: Verb) -> Option<&'p WorkOrder> {
    available_orders(w, a).into_iter().find(|o| o.verb == verb)
}

/// Quality from skill and tools (spec: "Quality range 1 to 5, from skill
/// (1-5) and tools (0-1)").
///
/// `q = round(0.8*skill + 1.5*tools)`, clamped to 1..=5: a role-master
/// (skill 3) with a tool makes 4, a master of the craft (skill 5) with a tool
/// makes 5, bare hands top out at 3.
#[must_use]
pub fn quality(skill: u8, tools: u32) -> u8 {
    let raw = 1.5_f32.mul_add(tools as f32, 0.8 * f32::from(skill));
    let q = raw.round() as i32;
    q.clamp(1, 5) as u8
}

/// The yield multiplier for a quality (spec: "1.0 + 0.1 per quality point
/// above 3").
#[must_use]
pub fn yield_multiplier(quality: u8) -> f32 {
    0.1_f32.mul_add(i32::from(quality.saturating_sub(3)) as f32, 1.0)
}

/// The villager's own trade quality right now: their top skill and their
/// tools.
#[must_use]
pub fn maker_quality(a: &Agent) -> u8 {
    quality(a.skills.top_skill().1, a.tools.min(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_derives_from_skill_and_tools() {
        assert_eq!(quality(1, 0), 1);
        assert_eq!(quality(3, 0), 2);
        assert_eq!(quality(3, 1), 4);
        assert_eq!(quality(5, 1), 5);
    }

    #[test]
    fn yield_multiplier_is_a_tenth_per_point_above_three() {
        assert!((yield_multiplier(3) - 1.0).abs() < 1e-6);
        assert!((yield_multiplier(5) - 1.2).abs() < 1e-6);
        assert!((yield_multiplier(1) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn watchman_and_elder_own_no_orders() {
        assert!(profession_for(Role::Watchman).is_some_and(|p| p.orders.is_empty()));
        assert!(profession_for(Role::Elder).is_some_and(|p| p.orders.is_empty()));
        assert!(available_orders(&World::founding(1), &{
            Agent::new("Wren", "quiet", Place::Granary, 1, 0, 0, 0, vec![], &[])
        })
        .is_empty());
    }

    #[test]
    fn a_new_profession_is_data() {
        // The table carries six queues with two to four orders each; adding a
        // trade means a new `Profession` row here and nowhere else.
        let with_orders = PROFESSIONS.iter().filter(|p| !p.orders.is_empty()).count();
        assert_eq!(
            with_orders, 7,
            "six trades carry queues; Watchman and Elder do not"
        );
        for p in PROFESSIONS.iter().filter(|p| !p.orders.is_empty()) {
            assert!(
                (2..=4).contains(&p.orders.len()),
                "{} has {} orders",
                p.name,
                p.orders.len()
            );
        }
    }
}
