//! Inference layer. No simulation state lives here.
//!
//! Two backends with disjoint jobs, because measurement forced the split:
//!
//! * [`judgment`] — Jev via OpenRouter's Decisions API. Chooses among legal
//!   actions, labels social acts, checks grounding, scores salience. Required:
//!   if it is unreachable the society halts rather than degrading.
//! * [`voice`] — small local models via Ollama. Render one spoken line given a
//!   decision that has already been made. They never choose.

pub mod judgment;
pub mod voice;

pub use judgment::{Judgment, JudgmentError, Judge};
pub use voice::{Voice, VoiceError};
