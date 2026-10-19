//! Pure simulation state machine. No IO, no network, no clock.
//!
//! Everything here is deterministic given (state, seed, inputs). The server
//! owns inference; this crate owns legality, arithmetic, and commit.

// The sim does bounded arithmetic on small counts and unit-range f32 scores;
// every cast below is value-checked by the surrounding clamps and tests.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "bounded small-count and unit-range arithmetic"
)]

mod action;
mod card;
mod memory;
mod rng;
mod world;

pub use action::{action_phrase, affordances, apply, Intent, Outcome, SocialAct, Verb};
pub use card::{
    agent_card, judgment_state, render_place_map, render_social_matrix, render_village_ui,
    render_villager_inspector, world_brief,
};
pub use memory::{Belief, Fact, Memory, MemoryKind, MEMORY_CAP, REFLECTION_BUDGET};
pub use rng::Rng;
pub use world::{
    mix_personas, Agent, Ailment, Event, EventKind, Kinship, Mood, Place, PlaceState, Promise,
    RelationDetail, Role, Season, Skills, TimeOfDay, WeatherKind, World, ADJACENCY, BIRTH_COST,
    BIRTH_MIN_AGE, BIRTH_TRUST, CHILD_FOOD, NEUTRAL_TRUST, PLACES, POPULATION_CAP,
};

#[cfg(test)]
mod tests;
