//! The check (spec #5, ticket #13): the rule that compares a line's claims
//! against the world and marks the line discredited when they do not hold.
//!
//! A claim the world can settle is true or false. A claim about a past event,
//! about a third party's intent, or about a quantity nobody can see is
//! unverifiable, not false - that is deliberate, because a lie that cannot be
//! settled stays credible and can move trust (ticket #14).
//!
//! `grounded` is the check's verdict: 1.0 when every claim is true or
//! unverifiable, 0.0 when at least one is false. A line at 0.0 is
//! discredited: still spoken, moves no trust, and the failing claim is
//! written to the listener's memory.

use serde::{Deserialize, Serialize};

use crate::world::{Place, World};

/// What a line asserts (glossary: claim). The speaker produces these; the
/// check settles them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Claim {
    /// A count the speaker can see: their own carry or the store where they
    /// stand. `holder` names whose count; a count of somebody else's carry is
    /// [`Claim::OtherCount`].
    Count { resource: &'static str, value: u32 },
    /// The store at a named place.
    Store {
        place: Place,
        resource: &'static str,
        value: u32,
    },
    /// Another villager's inventory, which no one present can see.
    OtherCount,
    /// A person is present.
    PersonPresent { name: String },
    /// A place's condition: broken, lit, stocked, or empty.
    PlacePred { place: Place, pred: Pred },
    /// An obligation the speaker carries toward someone.
    Obligation { to: String, made: bool },
    /// A third party's intent. Nobody can settle it.
    Intent { who: String, what: String },
    /// Something claimed about the past. Nobody can settle it.
    PastEvent { what: String },
    /// The quality of a thing the speaker made.
    Quality { item: &'static str, value: u8 },
}

/// The predicate a place claim makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pred {
    Broken,
    Lit,
    Stocked,
    Empty,
}

/// The verdict on one claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    True,
    False,
    Unverifiable,
}

impl Claim {
    /// A human fragment naming the claim, for the listener's memory ("the
    /// claim and its failure are written to the listener's memory").
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Count { resource, value } => format!("the count of {resource} ({value})"),
            Self::Store {
                place,
                resource,
                value,
            } => {
                format!("the {} of {} at the {}", resource, value, place.id())
            },
            Self::OtherCount => "another's stores".to_owned(),
            Self::PersonPresent { name } => format!("{name} being here"),
            Self::PlacePred { place, pred } => match pred {
                Pred::Broken => format!("the {} being broken", place.id()),
                Pred::Lit => format!("the {} hearth being lit", place.id()),
                Pred::Stocked => format!("the {} being stocked", place.id()),
                Pred::Empty => format!("the {} being empty", place.id()),
            },
            Self::Obligation { to, .. } => format!("a promise to {to}"),
            Self::Intent { who, what } => format!("{who} {what}"),
            Self::PastEvent { what } => what.clone(),
            Self::Quality { item, .. } => format!("the {item}'s quality"),
        }
    }
}

/// The check's verdict for the whole line: 1.0 grounded, 0.0 discredited
/// (spec constants: the check is exact, not a score).
pub const GROUNDED: f32 = 1.0;
pub const DISCREDITED: f32 = 0.0;

/// Settle one claim against the world, from the speaker's standpoint.
#[must_use]
pub fn settle(w: &World, speaker: &str, claim: &Claim) -> Verdict {
    let Some(a) = w.agent(speaker) else {
        return Verdict::Unverifiable;
    };
    match claim {
        Claim::Count { resource, value } => match *resource {
            "food" => (a.food == *value).into(),
            "water" => (a.water == *value).into(),
            "wood" => (a.wood == *value).into(),
            "herbs" => (a.herbs == *value).into(),
            "tools" => (a.tools == *value).into(),
            "coin" => (a.coin == *value).into(),
            _ => Verdict::Unverifiable,
        },
        Claim::Store {
            place,
            resource,
            value,
        } => {
            let st = w.place_state(*place);
            let actual = match *resource {
                "food" => st.food,
                "water" => st.water,
                "wood" => st.wood,
                "herbs" => st.herbs,
                "tools" => st.tools,
                "coin" => st.coin,
                _ => return Verdict::Unverifiable,
            };
            (actual == *value).into()
        },
        // Another villager's current inventory: no one present can see it,
        // so the line stays credible even when the number is invented. The
        // same mercy covers past events and third-party intent: nobody in
        // the village can settle them, which is the point.
        Claim::OtherCount | Claim::Intent { .. } | Claim::PastEvent { .. } => Verdict::Unverifiable,
        Claim::PersonPresent { name } => {
            let present = w.agent(name).is_some_and(|p| p.alive && p.place == a.place);
            (present).into()
        },
        Claim::PlacePred { place, pred } => {
            let st = w.place_state(*place);
            let holds = match pred {
                Pred::Broken => st.condition < 5 || (*place == Place::Hut && !st.hearth_lit),
                Pred::Lit => st.hearth_lit,
                Pred::Stocked => st.food > 0,
                Pred::Empty => st.food == 0,
            };
            (holds).into()
        },
        Claim::Obligation { to, made } => {
            let exists = a
                .promise
                .as_ref()
                .is_some_and(|pr| pr.kept.is_none() && &pr.to == to);
            (exists == *made).into()
        },

        Claim::Quality { .. } => Verdict::True,
    }
}

/// The failing claim of a line, if any: the first claim that settles false.
#[must_use]
pub(crate) fn failing<'c>(w: &World, speaker: &str, claims: &'c [Claim]) -> Option<&'c Claim> {
    claims
        .iter()
        .find(|c| settle(w, speaker, c) == Verdict::False)
}

/// The line's groundedness: 1.0 when every claim is true or unverifiable,
/// 0.0 when at least one is false.
#[must_use]
pub fn grounded(w: &World, speaker: &str, claims: &[Claim]) -> f32 {
    if failing(w, speaker, claims).is_some() {
        DISCREDITED
    } else {
        GROUNDED
    }
}

impl From<bool> for Verdict {
    fn from(holds: bool) -> Self {
        if holds {
            Self::True
        } else {
            Self::False
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Agent;

    #[test]
    fn own_counts_settle_against_the_carry() {
        let w = World::founding(1);
        let mut ada = Agent::new("Ada", "plain", Place::Granary, 3, 0, 0, 0, vec![], &[]);
        ada.set_trust("Bo", 2.0);
        let mut w = w;
        w.agents.clear();
        w.agents.push(ada);
        w.agents.push(Agent::new(
            "Bo",
            "plain",
            Place::Granary,
            0,
            0,
            0,
            0,
            vec![],
            &[],
        ));
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::Count {
                    resource: "food",
                    value: 3
                }
            ),
            Verdict::True
        );
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::Count {
                    resource: "food",
                    value: 5
                }
            ),
            Verdict::False
        );
    }

    #[test]
    fn another_villagers_inventory_is_unverifiable_and_past_events_never_false() {
        let w = World::founding(2);
        assert_eq!(settle(&w, "Ada", &Claim::OtherCount), Verdict::Unverifiable);
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::PastEvent {
                    what: "gave grain yesterday".into()
                }
            ),
            Verdict::Unverifiable
        );
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::Intent {
                    who: "Tam".into(),
                    what: "meant to cheat".into()
                }
            ),
            Verdict::Unverifiable
        );
    }

    #[test]
    fn a_false_claim_discredits_and_a_line_without_one_is_grounded() {
        let mut w = World::founding(3);
        // Ada speaks where Bo stands, so the presence claim is true.
        let bo = w.agents.iter().find(|a| a.name == "Bo").unwrap().place;
        w.agent_mut("Ada").unwrap().place = bo;
        let claims = vec![
            Claim::PersonPresent { name: "Bo".into() },
            Claim::PastEvent {
                what: "lost a tool".into(),
            },
        ];
        assert!((grounded(&w, "Ada", &claims) - GROUNDED).abs() < 1e-6);
        let claims = vec![
            Claim::Count {
                resource: "food",
                value: 9,
            },
            Claim::PersonPresent { name: "Bo".into() },
        ];
        assert!((grounded(&w, "Ada", &claims) - DISCREDITED).abs() < 1e-6);
        assert!(failing(&w, "Ada", &claims).is_some());
    }

    #[test]
    fn place_predictions_settle_from_the_world() {
        let w = World::founding(4);
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::PlacePred {
                    place: Place::Hut,
                    pred: Pred::Lit
                }
            ),
            Verdict::True
        );
        assert_eq!(
            settle(
                &w,
                "Ada",
                &Claim::PlacePred {
                    place: Place::Hut,
                    pred: Pred::Broken
                }
            ),
            Verdict::False
        );
    }
}
