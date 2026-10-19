use serde::{Deserialize, Serialize};

/// A memory the agent carries. `salience` is written once, by the judgment
/// layer, at the moment the memory is formed; age decay happens in code.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub turn: u64,
    pub text: String,
    pub salience: f32,
    pub kind: MemoryKind,
    /// Scars never decay and never leave the buffer (Q19c).
    pub permanent: bool,
    /// Retrieval count and last access drive rank-based recency, not wall time
    /// (Generative Agents `retrieve.py:145`: `0.99**i` over a last-access list).
    #[serde(default)]
    pub accesses: u32,
    #[serde(default)]
    pub last_access: u64,
    /// Emotional valence in [-1.0, 1.0]: negative for trauma/betrayal, positive for joy/aid.
    #[serde(default)]
    pub valence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// The agent did the thing.
    Own,
    /// The agent saw or was told.
    Witnessed,
    /// First-person consolidation produced by the agent.
    Reflection,
    /// Death, betrayal, disappearance.
    Scar,
    /// Shared village custom, seasonal rite, or proverb.
    Tradition,
    /// Life milestone: birth of a child, masterwork crafted, survival of winter.
    Milestone,
}

fn round2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

impl Memory {
    pub fn new(turn: u64, text: impl Into<String>, salience: f32, kind: MemoryKind) -> Self {
        Self {
            turn,
            text: text.into(),
            // Quantise at writing time: an f32 that reaches JSON as
            // 0.8999999761581421 is noise in every log and every prompt.
            salience: round2(salience.clamp(0.0, 1.0)),
            kind,
            permanent: false,
            accesses: 0,
            last_access: turn,
            valence: 0.0,
            tags: Vec::new(),
        }
    }

    pub fn scar(turn: u64, text: impl Into<String>) -> Self {
        Self {
            turn,
            text: text.into(),
            salience: 1.0,
            kind: MemoryKind::Scar,
            permanent: true,
            accesses: 0,
            last_access: turn,
            valence: -1.0,
            tags: vec!["scar".into()],
        }
    }

    #[must_use]
    pub fn with_valence(mut self, valence: f32) -> Self {
        self.valence = round2(valence.clamp(-1.0, 1.0));
        self
    }

    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        let t = tag.into();
        if !self.tags.contains(&t) {
            self.tags.push(t);
        }
        self
    }

    /// Salience after age decay. Permanent memories never decay.
    #[must_use]
    pub fn weight(&self, now: u64, half_life: f32) -> f32 {
        if self.permanent {
            return self.salience;
        }
        let age = now.saturating_sub(self.turn) as f32;
        round2(self.salience * 0.5_f32.powf(age / half_life))
    }

    /// Rank-based recency in `(0,1]`, matching the implementation rather than
    /// the paper: `0.99^rank` with rank taken from the last-access ordering.
    #[must_use]
    pub fn recency(rank: usize) -> f32 {
        0.99_f32.powi(rank as i32 + 1)
    }

    /// Three-term blend. Weights are the shipped GA values, not the paper's
    /// all-ones: relevance dominates, importance next, recency last.
    #[must_use]
    pub fn blend(recency: f32, relevance: f32, importance: f32) -> f32 {
        2.0_f32.mul_add(importance, 3.0_f32.mul_add(relevance, 0.5 * recency))
    }
}

/// A belief the agent holds about the world. Divergence from truth and
/// staleness are *derived*, never stored (`ToMi` `oracle.py`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Belief {
    pub about: String,
    pub value: i32,
    pub as_of_tick: u64,
    /// Who told them, if anyone. `None` means they saw it.
    pub source: Option<String>,
}

impl Belief {
    pub fn observed(about: impl Into<String>, value: i32, tick: u64) -> Self {
        Self {
            about: about.into(),
            value,
            as_of_tick: tick,
            source: None,
        }
    }

    pub fn told(about: impl Into<String>, value: i32, tick: u64, by: impl Into<String>) -> Self {
        Self {
            about: about.into(),
            value,
            as_of_tick: tick,
            source: Some(by.into()),
        }
    }

    #[must_use]
    pub const fn is_stale(&self, now: u64, max_age: u64) -> bool {
        now.saturating_sub(self.as_of_tick) > max_age
    }
}

/// One remembered fact about a person or place, with bi-temporal stamps
/// (Graphiti `edges.py:263`). `invalid_at` is a write, never a delete.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    pub id: u64,
    pub subject: String,
    pub text: String,
    pub valid_at: u64,
    pub invalid_at: Option<u64>,
    pub expired_at: Option<u64>,
    pub confidence: f32,
}

impl Fact {
    #[must_use]
    pub fn live(&self, now: u64) -> bool {
        self.invalid_at.is_none_or(|t| t > now) && self.expired_at.is_none()
    }
}

/// Recent memories kept verbatim; older ones compete for the remaining slots.
pub const MEMORY_CAP: usize = 8;

/// Reflection fires on accumulated poignancy, not a clock. Each stored event
/// spends its salience from this budget; at zero the agent consolidates.
pub const REFLECTION_BUDGET: f32 = 150.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_decreases_with_rank() {
        let a = Memory::recency(0);
        let b = Memory::recency(10);
        assert!(a > b, "rank 0 must be more recent-weighty than rank 10");
        assert!((a - 0.99).abs() < 1e-6);
    }

    #[test]
    fn blend_prefers_relevance() {
        // Same inputs: relevance must move the score more than recency.
        let by_recency = Memory::blend(1.0, 0.0, 0.0);
        let by_relevance = Memory::blend(0.0, 1.0, 0.0);
        assert!(by_relevance > by_recency);
    }

    #[test]
    fn belief_diverges_from_truth_without_storing_it() {
        let b = Belief::observed("granary", 2, 0);
        assert_eq!(b.value, 2); // truth is 1; divergence is computed by the caller
        assert!(b.is_stale(20, 5));
        assert!(!b.is_stale(3, 5));
    }

    #[test]
    fn fact_invalidation_closes_the_interval() {
        let mut f = Fact {
            id: 1,
            subject: "Bo".into(),
            text: "Bo has grain".into(),
            valid_at: 3,
            invalid_at: None,
            expired_at: None,
            confidence: 0.9,
        };
        assert!(f.live(10));
        f.invalid_at = Some(8);
        assert!(f.live(7));
        assert!(!f.live(8));
    }
}
