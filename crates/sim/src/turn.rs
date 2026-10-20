//! The pure turn seam (spec #5, ticket #8).
//!
//! One function over the world whose seeded RNG is part of it: it enumerates
//! affordances, shortlists, decides, speaks, checks, applies and settles,
//! returning the mutated world's record. No IO, no clock, no network, no async
//! runtime.
//!
//! Turn shape (each stage pure, per villager in roster order):
//!   1. the policy decides from the shortlist
//!   2. the speaker renders the line from the decision, or stays silent
//!   3. the check settles the line's claims against the world
//!   4. code applies the intent, gates the act's trust on the verdict,
//!      derives the intensity, and remembers a discredited claim
//!   5. the world settles, and the record carries the summary

use serde::{Deserialize, Serialize};

use crate::action::{self, SocialAct};
use crate::check;
use crate::policy::{self, Decision, Dilemma};
use crate::speaker::{self, Line};
use crate::world::{EventKind, Mood, World};

/// The five registers, full labels for the feed. Derived from mood, never
/// asked (measuring showed "plain and neutral" 38 of 45 times while trust
/// still moved).
pub const REGISTERS: [&str; 5] = [
    "cold and clipped",
    "wary and guarded",
    "plain and neutral",
    "warm and open",
    "urgent and raw",
];

/// The full register label for a mood. Kept in step with
/// [`crate::grammar::register_for_mood`], which owns the mapping.
#[must_use]
pub fn register_label_for_mood(mood: Mood) -> &'static str {
    crate::grammar::register_label(crate::grammar::register_for_mood(mood))
}

/// Base dramatic pressure per social act, before the register adjustment.
/// Accusation (3.2) and threat (3.4) sit at or above the world's rumor (3.2)
/// and threat-morale (3.0) thresholds, so the drama branches are reachable.
#[must_use]
pub(crate) const fn base_intensity(social: SocialAct) -> f32 {
    match social {
        SocialAct::Accusation => 3.2,
        SocialAct::Threat => 3.4,
        SocialAct::Promise | SocialAct::Blessing | SocialAct::Plea => 2.6,
        SocialAct::Gift | SocialAct::Gossip | SocialAct::Forgiveness => 2.4,
        SocialAct::Counsel | SocialAct::Gratitude => 2.2,
        SocialAct::TradeOffer => 2.0,
        SocialAct::None => 1.0,
    }
}

/// How far the register pushes the act's base pressure.
fn register_adjustment(register: &str) -> f32 {
    match register {
        "urgent" => 0.2,
        "cold" => -0.2,
        _ => 0.0,
    }
}

/// The turn's dramatic pressure: the act's base plus the register's push,
/// clamped and rounded. Derived, never asked.
#[must_use]
pub fn derived_intensity(social: SocialAct, register: &str) -> f32 {
    let v = (base_intensity(social) + register_adjustment(register)).clamp(0.0, 4.0);
    (v * 100.0).round() / 100.0
}

/// The rumor threshold: an accusation at or above this intensity echoes.
pub const RUMOR_THRESHOLD: f32 = 3.2;
/// The threat-morale threshold: a threat at or above this intensity bites.
pub const THREAT_MORALE_THRESHOLD: f32 = 3.0;

/// One villager's committed turn, shrunk to the contract (ticket #18): no
/// model, no latency, no raw state, no fidelity, no call records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurn {
    pub name: String,
    pub action: String,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub say: String,
    /// The motive behind the action, in one word.
    pub why: String,
    pub social: String,
    /// Whom the line addressed, if anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressee: Option<String>,
    /// The register the line was spoken in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register: Option<String>,
    /// Inner hesitation when the villager was nearly pulled two ways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dilemma: Option<Dilemma>,
    /// The act's dramatic pressure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f32>,
    /// The check's verdict: 1.0 or 0.0. The feed derives the discredited
    /// label from this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounded: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// The chosen candidate's term contributions.
    #[serde(default)]
    pub factors: Vec<(String, f32)>,
    /// The work order advanced, when the action was work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// True when the speaker entered lie mode; a lie may still land.
    #[serde(default)]
    pub lie: bool,
}

/// A child born during the turn's settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BirthRecord {
    pub child: String,
    pub parents: Vec<String>,
    pub generation: u8,
    pub place: String,
}

/// What the turn produced, counted once: the summary is the at-a-glance
/// digest the feed and `/log` carry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TurnSummary {
    pub lines: u32,
    pub silences: u32,
    pub discredited: u32,
    pub lies: u32,
    pub acts: Vec<(String, u32)>,
    pub motives: Vec<(String, u32)>,
}

/// The turn record: the turn, the day, the agents, the births, the summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: u64,
    pub day: u64,
    pub agents: Vec<AgentTurn>,
    #[serde(default)]
    pub births: Vec<BirthRecord>,
    #[serde(default)]
    pub summary: TurnSummary,
}

/// One turn through the seam. A halted world or an empty village halts
/// instead of acting.
///
/// # Errors
///
/// A halted world reports its reason; the caller says so rather than acting.
pub fn step(w: &mut World) -> Result<TurnRecord, String> {
    if let Some(reason) = w.halted.clone() {
        return Err(reason);
    }
    let names: Vec<String> = w.living().map(|a| a.name.clone()).collect();
    if names.is_empty() {
        w.halt("every villager is dead");
        return Err("every villager is dead".into());
    }
    let turn = w.turn;
    let day = w.day;
    let register_keys: Vec<(&str, String)> = names
        .iter()
        .map(|n| {
            let mood = w.agent(n).map_or(Mood::Content, |a| a.mood);
            (crate::grammar::register_for_mood(mood), n.clone())
        })
        .collect();

    // Decide, speak, check - all against the turn-start state, in roster
    // order, so the same seed and save replay identically.
    let mut turns: Vec<(String, Decision, Option<Line>)> = Vec::with_capacity(names.len());
    for name in &names {
        let Some(decision) = policy::decide(w, name) else {
            continue;
        };
        let line = speaker::speak(w, name, &decision);
        turns.push((name.clone(), decision, line));
    }

    // Apply, in the same order, so effects commit deterministically.
    let mut agents: Vec<AgentTurn> = Vec::with_capacity(turns.len());
    for (name, decision, line) in turns {
        let register_key = register_keys
            .iter()
            .find(|(_, n)| *n == name)
            .map_or("plain", |(k, _)| *k);
        let intensity = derived_intensity(decision.act, register_key);
        let lie = line.as_ref().is_some_and(|l| l.lie);
        let grounded = line
            .as_ref()
            .map_or(check::GROUNDED, |l| check::grounded(w, &name, &l.claims));
        let text = line.as_ref().map_or(String::new(), |l| l.text.clone());
        let failing = line
            .as_ref()
            .and_then(|l| check::failing(w, &name, &l.claims).map(check::Claim::describe));
        let outcome = action::apply_decision(
            w,
            &name,
            &decision,
            text.as_str(),
            decision.act,
            grounded,
            intensity,
            failing.as_deref(),
        );
        if let Some(l) = &line {
            speaker::ledger_push(w, &name, &l.text);
        }
        agents.push(AgentTurn {
            name: name.clone(),
            action: decision.action.clone(),
            accepted: outcome.accepted,
            note: outcome.note,
            say: text,
            why: decision.motive.id().to_owned(),
            social: decision.act.id().to_owned(),
            addressee: decision.addressee.clone(),
            register: Some(crate::grammar::register_label(register_key).to_owned()),
            dilemma: decision.dilemma.clone(),
            intensity: Some(intensity),
            grounded: Some(grounded),
            confidence: Some(decision.confidence),
            factors: decision
                .factors
                .iter()
                .map(|(t, c)| ((*t).to_owned(), *c))
                .collect(),
            job: decision.job.map(str::to_owned),
            lie,
        });
    }

    // Settle, then harvest the record.
    w.settle();
    let births: Vec<BirthRecord> = w
        .events
        .iter()
        .filter(|e| e.turn == turn && e.kind == EventKind::Birth)
        .map(|e| BirthRecord {
            child: e.actor.clone(),
            parents: e
                .text
                .split(" born to ")
                .nth(1)
                .map(|rest| {
                    rest.trim_end_matches('.')
                        .split(" and ")
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            generation: w.agent(&e.actor).map_or(0, |a| a.generation),
            place: e.place.id().to_owned(),
        })
        .collect();

    Ok(record_of(turn, day, agents, births))
}

/// The summary and record assembly.
fn record_of(turn: u64, day: u64, agents: Vec<AgentTurn>, births: Vec<BirthRecord>) -> TurnRecord {
    let mut lines = 0_u32;
    let mut silences = 0_u32;
    let mut discredited_count = 0_u32;
    let mut lie_count = 0_u32;
    let mut acts: Vec<(String, u32)> = Vec::new();
    let mut motives: Vec<(String, u32)> = Vec::new();
    for a in &agents {
        let spoke = !a.say.is_empty();
        if spoke {
            lines += 1;
        } else {
            silences += 1;
        }
        if a.grounded == Some(0.0) {
            discredited_count += 1;
        }
        if a.lie {
            lie_count += 1;
        }
        bump(&mut acts, a.social.clone());
        bump(&mut motives, a.why.clone());
    }
    acts.sort();
    motives.sort();
    let summary = TurnSummary {
        lines,
        silences,
        discredited: discredited_count,
        lies: lie_count,
        acts,
        motives,
    };
    TurnRecord {
        turn,
        day,
        agents,
        births,
        summary,
    }
}

fn bump(v: &mut Vec<(String, u32)>, k: String) {
    if let Some(slot) = v.iter_mut().find(|(key, _)| *key == k) {
        slot.1 += 1;
    } else {
        v.push((k, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::DISCREDITED;

    #[test]
    fn a_turn_through_the_seem_needs_no_network_and_produces_a_record() {
        let mut w = World::founding(21);
        let rec = step(&mut w).expect("turn");
        assert_eq!(rec.turn, 0);
        assert!(!rec.agents.is_empty());
        assert_eq!(w.turn, 1);
        // The summary counts every villager once.
        let counted: u32 = rec.summary.lines + rec.summary.silences;
        assert_eq!(counted, rec.agents.len() as u32);
    }

    #[test]
    fn the_intensity_reaches_the_drama_thresholds() {
        assert!(derived_intensity(SocialAct::Accusation, "plain") >= RUMOR_THRESHOLD);
        assert!(derived_intensity(SocialAct::Threat, "plain") >= THREAT_MORALE_THRESHOLD);
        // The cold register pulls the accusation to 3.0: at the morale
        // threshold, below the rumor one.
        assert!((derived_intensity(SocialAct::Accusation, "cold") - 3.0).abs() < 1e-6);
    }

    #[test]
    fn the_register_is_derived_from_the_mood() {
        assert_eq!(register_label_for_mood(Mood::Desperate), "urgent and raw");
        assert_eq!(register_label_for_mood(Mood::Content), "plain and neutral");
        assert_eq!(register_label_for_mood(Mood::Bitter), "cold and clipped");
    }

    #[test]
    fn discredited_is_the_check_verdict_not_a_label() {
        assert_eq!(DISCREDITED, 0.0);
    }
}
