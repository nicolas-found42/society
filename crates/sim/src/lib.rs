//! Pure simulation state machine. No IO, no network, no clock.
//!
//! Everything here is deterministic given (state, seed, inputs). The policy,
//! the speaker and the check are code in this crate; the server crate is the
//! shell around them: persistence, the control surface, the window.

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
mod catalog;
mod check;
mod grammar;
mod grammar_first;
mod grammar_second;
mod memory;
mod policy;
mod profession;
mod rng;
mod speaker;
pub mod turn;
mod world;

pub use action::{
    action_phrase, affordances, apply, apply_decision, shortlist, Intent, Outcome, SocialAct, Verb,
    NOVELTY_WINDOW,
};
pub use catalog::{meta_for, ActionMeta, Category, Need, ACTIONS, GO_META, SPEECH_MENU};
pub use check::{grounded, settle, Claim, Pred, Verdict, DISCREDITED, GROUNDED};
pub use grammar::{count_rules, missing_cells, FIRST_FLOOR, REGISTER_KEYS, SECOND_FLOOR};
pub use memory::{Belief, Fact, Memory, MemoryKind, MEMORY_CAP, REFLECTION_BUDGET};
pub use policy::{
    addressee_of, agent_rng, decide, derive_motive, is_survival, obligation_against, Candidate,
    Decision, Dilemma, Motive, DILEMMA_MARGIN, DILEMMA_RUNNERUP_MASS, OBLIGATION_GRUDGE,
    POLICY_TEMPERATURE, PROMISE_OVERDUE_TURNS, SURVIVAL_HUNGER, SURVIVAL_THIRST, SURVIVAL_WARMTH,
};
pub use profession::{
    available_orders, maker_quality, profession_for, quality, yield_multiplier, Profession,
    WorkOrder, PROFESSIONS,
};
pub use rng::Rng;
pub use speaker::{
    speak, Line, LEDGER_WINDOW, LIE_BASE_RATE, LIE_GRUDGE_MULT, LIE_HOPE, LIE_HOPE_MULT,
    LIE_HUNGER_MULT, LIE_PROMISE_MULT, LIE_RATE_CAP, LIE_THREAT_MULT, WORD_BUDGET,
};
pub use turn::{
    derived_intensity, register_label_for_mood, step, AgentTurn, BirthRecord, TurnRecord,
    TurnSummary, REGISTERS as FEED_REGISTERS, RUMOR_THRESHOLD, THREAT_MORALE_THRESHOLD,
};
pub use world::{
    mix_personas, Agent, Ailment, Event, EventKind, Kinship, Mood, Place, PlaceState, Promise,
    RelationDetail, Role, Season, Skills, TimeOfDay, WeatherKind, World, ADJACENCY, BIRTH_COST,
    BIRTH_MIN_AGE, BIRTH_TRUST, CHILD_FOOD, NEUTRAL_TRUST, PLACES, POPULATION_CAP,
};

#[cfg(test)]
mod tests;
