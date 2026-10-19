//! The turn loop. Owns ordering; borrows judgment and voice; never invents
//! state.
//!
//! Turn shape (each step is one batched Jev call, not one per agent):
//!   1. code enumerates legal actions per living agent
//!   2. Jev chooses intent + register + addressee for all agents at once
//!   3. voice renders one line per agent, concurrently under an AIMD cap
//!   4. Jev gates all lines at once: fidelity, grounding, social act
//!   5. code applies effects in a fixed order and settles the turn

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use society_sim::{
    affordances, agent_card, apply, judgment_state, Intent, SocialAct, World,
};

use crate::inference::judgment::{action_question, Answer, Judge};
use crate::inference::voice::{Line, Voice};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: u64,
    pub day: u64,
    pub intent_call: Option<CallRecord>,
    pub gate_call: Option<CallRecord>,
    pub agents: Vec<AgentTurn>,
    /// Children born during this turn's settlement. Carried on the record
    /// because the feed renders records, not the world event log.
    #[serde(default)]
    pub births: Vec<BirthRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BirthRecord {
    pub child: String,
    pub parents: Vec<String>,
    pub generation: u8,
    pub place: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub latency_ms: u64,
    pub cost: f64,
    pub input_tokens: u64,
    pub model: Option<String>,
}

impl CallRecord {
    fn of(a: &Answer) -> Self {
        Self {
            latency_ms: a.latency_ms,
            cost: a.cost,
            input_tokens: a.input_tokens,
            model: a.model.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurn {
    pub name: String,
    pub action: String,
    pub accepted: bool,
    pub note: Option<String>,
    pub say: String,
    pub why: String,
    pub social: String,
    pub fidelity: Option<f32>,
    pub grounded: Option<f32>,
    pub intensity: Option<f32>,
    pub confidence: Option<f32>,
    pub latency_ms: u64,
    pub model: String,
    /// The exact questions Jev saw, for the inspector.
    pub state: Value,
}

#[derive(Debug)]
pub enum EngineError {
    Judgment(String),
    Halted(String),
    /// The turn committed in memory but could not be written to disk. Loud,
    /// because an unwritten world is a reset waiting to happen.
    Persistence(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Judgment(e) => write!(f, "{e}"),
            EngineError::Halted(e) => write!(f, "society halted: {e}"),
            EngineError::Persistence(e) => write!(f, "world not saved: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

pub struct Engine {
    pub judge: Arc<Judge>,
    pub voice: Arc<Voice>,
    /// Where committed turns are written. `None` is test-only.
    save_path: std::path::PathBuf,
    /// AIMD permit ceiling; adjusted by observed latency.
    permits: Arc<tokio::sync::Semaphore>,
}

const MIN_PERMITS: usize = 2;
const MAX_PERMITS: usize = 24;
const SLOW_MS: u64 = 4_000;

impl Engine {
    /// Every committed turn is written to `save_path` before `step` returns Ok,
    /// so a crash loses at most the turn in flight. There is no engine without
    /// a save path: a village that only exists in memory is a reset waiting to
    /// happen.
    pub fn new(
        judge: Arc<Judge>,
        voice: Arc<Voice>,
        permits: usize,
        save_path: std::path::PathBuf,
    ) -> Self {
        Self {
            judge,
            voice,
            save_path,
            permits: Arc::new(tokio::sync::Semaphore::new(
                permits.clamp(MIN_PERMITS, MAX_PERMITS),
            )),
        }
    }

    pub fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }

    /// Slow calls shrink the window; fast ones grow it back by one. TCP-style
    /// AIMD, minus the congestion signal Ollama does not give us.
    fn adapt(&self, latency_ms: u64) {
        if latency_ms > SLOW_MS {
            let free = self.permits.available_permits();
            if free > MIN_PERMITS {
                let _ = self.permits.try_acquire_many(free as u32 - MIN_PERMITS as u32);
            }
        } else if self.permits.available_permits() < MAX_PERMITS {
            self.permits.add_permits(1);
        }
    }

    /// A halt is part of the world and must survive a restart, so it is written
    /// as soon as it happens — the turn itself is not committed, but the fact
    /// that the society stopped is. A failed write is folded into the reason so
    /// the caller sees both facts rather than a lost halt.
    fn halt_now(&self, world: &mut World, reason: String) -> String {
        world.halt(reason.clone());
        if let Err(e) = crate::persistence::save(world, &self.save_path) {
            return format!("{reason} (halt not saved: {e})");
        }
        reason
    }

    /// One complete turn. Mutates `world` only after every judgment has landed,
    /// so a mid-turn failure leaves the world untouched.
    pub async fn step(&self, world: &mut World) -> Result<TurnRecord, EngineError> {
        if let Some(reason) = &world.halted {
            return Err(EngineError::Halted(reason.clone()));
        }
        let turn = world.turn;
        let day = world.day;
        let names: Vec<String> = world.living().map(|a| a.name.clone()).collect();
        if names.is_empty() {
            let reason = self.halt_now(world, "every villager is dead".into());
            return Err(EngineError::Halted(reason));
        }

        // ---- 1. intent: one batched request ----
        let mut state = Map::new();
        let mut questions = Map::new();
        for n in &names {
            let a = world.agent(n).expect("living agent");
            let acts = affordances(world, a);
            state.insert(n.clone(), judgment_state(world, a));
            questions.insert(
                format!("intent_{n}"),
                action_question(
                    &format!(
                        "As {n}, reading `{n}`, which single action do you take this turn? \
                         Only the actions in `{n}.legal_actions` are possible, and the choice must be one of them. \
                         Choose the action {n} would defend afterwards."
                    ),
                    &acts,
                ),
            );
            questions.insert(
                format!("register_{n}"),
                json!({
                    "type": "score",
                    "instructions": format!("In what register does {n} speak this turn?"),
                    "criteria": ["Cold and clipped","Wary and guarded","Plain and neutral","Warm and open","Urgent and raw"]
                }),
            );
            let present = world.present(a.place, n);
            if !present.is_empty() {
                let mut crit = Map::new();
                for p in &present {
                    crit.insert(p.clone(), json!(p));
                }
                crit.insert("nobody".into(), json!("nobody"));
                questions.insert(
                    format!("address_{n}"),
                    json!({
                        "type": "choice",
                        "instructions": format!("Whom does {n} address?"),
                        "criteria": crit
                    }),
                );
            }
        }
        let intent_answer = self
            .judge
            .ask(Value::Object(state.clone()), questions)
            .await
            .map_err(|e| {
                let reason = self.halt_now(world, format!("judgment layer unavailable: {e}"));
                EngineError::Judgment(reason)
            })?;

        // Resolve every intent in code. A judge answer that is not one of the
        // offered actions is a hard error, not a fallback.
        let mut intents: Vec<(String, Intent, f32, u8, Option<String>)> = Vec::new();
        for n in &names {
            let a = world.agent(n).expect("living agent");
            let legal = affordances(world, a);
            let j = intent_answer
                .get(&format!("intent_{n}"))
                .ok_or_else(|| {
                    EngineError::Judgment(format!("missing intent answer for {n}"))
                })?;
            let chosen = j.choice().ok_or_else(|| {
                EngineError::Judgment(format!("intent for {n} was not a choice"))
            })?;
            if !legal.iter().any(|l| l == chosen) {
                return Err(EngineError::Judgment(format!(
                    "{n} chose `{chosen}`, which is not among the offered actions {legal:?}"
                )));
            }
            let intent = Intent::parse(chosen, a.place).ok_or_else(|| {
                EngineError::Judgment(format!("{n} chose an unparseable action `{chosen}`"))
            })?;
            let register = intent_answer
                .get(&format!("register_{n}"))
                .and_then(|j| j.score())
                .unwrap_or(2.0)
                .round()
                .clamp(0.0, 4.0) as u8;
            let addressee = intent_answer
                .get(&format!("address_{n}"))
                .and_then(|j| j.choice().map(str::to_string))
                .filter(|s| s != "nobody");
            intents.push((n.clone(), intent, j.confidence().unwrap_or(0.0), register, addressee));
        }

        // ---- 2. voice: concurrent, AIMD-capped ----
        let mut tasks = tokio::task::JoinSet::new();
        for (name, intent, _, register, addressee) in &intents {
            let a = world.agent(name).expect("agent");
            let card = agent_card_with(world, a, &intent.action, *register, addressee.as_deref());
            let voice = self.voice.clone();
            let permits = self.permits.clone();
            let name_for_call = name.clone();
            let name = name.clone();
            tasks.spawn(async move {
                let permit = permits.acquire_owned().await.expect("semaphore open");
                let res = voice.speak_actor(&card, &name_for_call).await;
                drop(permit);
                (name, res)
            });
        }
        let mut lines: Vec<(String, Line)> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            let (name, res) = joined.map_err(|e| {
                EngineError::Judgment(format!("voice task failed: {e}"))
            })?;
            match res {
                Ok(line) => {
                    self.adapt(line.latency_ms);
                    lines.push((name, line));
                }
                Err(e) => {
                    // A malformed local reply is not a judgment failure; the
                    // society survives it and the turn is simply not committed.
                    return Err(EngineError::Judgment(format!(
                        "voice failed for {name}: {e}"
                    )));
                }
            }
        }
        // Deterministic ordering when committing, whatever order the calls
        // completed in.
        lines.sort_by(|a, b| a.0.cmp(&b.0));

        // ---- 3. gate: one batched request ----
        let mut gate_state = Map::new();
        let mut gate_q = Map::new();
        for (name, line) in &lines {
            let a = world.agent(name).expect("agent");
            let (_, intent, _, _, _) = intents
                .iter()
                .find(|(n, _, _, _, _)| n == name)
                .expect("intent exists");
            gate_state.insert(
                name.clone(),
                json!({
                    "said": line.say,
                    "why": line.why,
                    "decided": intent.action,
                    "place": a.place.id(),
                    "food_carried": a.food,
                    "food_here": world.place_state(a.place).food,
                    "people_present": world.present(a.place, name),
                    "world_state": judgment_state(world, a),
                }),
            );
            gate_q.insert(
                format!("fidelity_{name}"),
                json!({
                    "type": "noul",
                    "instructions": format!(
                        "Does `{name}.said` carry out `{name}.decided` without contradicting it?"
                    )
                }),
            );
            gate_q.insert(
                format!("grounded_{name}"),
                json!({
                    "type": "noul",
                    "instructions": format!(
                        "Are the concrete facts in `{name}.said` (amounts, who is present, who has what) \
                         consistent with `{name}`? Invented or contradicted facts make this false."
                    )
                }),
            );
            gate_q.insert(
                format!("social_{name}"),
                json!({
                    "type": "choice",
                    "instructions": format!(
                        "What social act does `{name}.said` perform toward someone present, if any?"
                    ),
                    "criteria": {
                        "none": "No social act",
                        "promise": "Commits to a future action",
                        "gift": "Gives or offers something",
                        "accusation": "Blames someone",
                        "threat": "Warns of harm",
                        "gossip": "Reports a third party's behaviour",
                        "plea": "Asks for help",
                        "gratitude": "Thanks someone",
                        "counsel": "Offers advice or reassurance",
                        "trade_offer": "Proposes an exchange of goods",
                        "blessing": "Offers a blessing or invokes shared faith",
                        "forgiveness": "Forgives a wrong or releases a grudge"
                    }
                }),
            );
            gate_q.insert(
                format!("intensity_{name}"),
                json!({
                    "type": "score",
                    "instructions": format!("How much dramatic pressure does `{name}.said` create?"),
                    "criteria": ["Flat restatement","Mild preference","Clear want or worry","Strong need or accusation","Crisis or turn"]
                }),
            );
        }
        let gate_answer = self
            .judge
            .ask(Value::Object(gate_state), gate_q)
            .await
            .map_err(|e| {
                let reason = self.halt_now(world, format!("judgment layer unavailable: {e}"));
                EngineError::Judgment(reason)
            })?;

        // ---- 4. commit ----
        let mut agent_turns = Vec::new();
        for (name, intent, confidence, _register, _addressee) in intents {
            let line = lines
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, l)| l.clone())
                .unwrap_or(Line {
                    say: String::new(),
                    why: String::new(),
                    model: String::new(),
                    latency_ms: 0,
                });
            let social = gate_answer
                .get(&format!("social_{name}"))
                .and_then(|j| j.choice().map(SocialAct::from_id))
                .unwrap_or(SocialAct::None);
            let q = |t: Option<f32>| t.map(|x| (x * 100.0).round() / 100.0);
            let fidelity = q(gate_answer.get(&format!("fidelity_{name}")).and_then(|j| j.noul()));
            let grounded = q(gate_answer.get(&format!("grounded_{name}")).and_then(|j| j.noul()));
            let intensity = q(gate_answer.get(&format!("intensity_{name}")).and_then(|j| j.score()));

            // A line that contradicts its own decision or the world is spoken
            // but not credited: it cannot move trust.
            let credible = fidelity.is_none_or(|f| f >= 0.5) && grounded.is_none_or(|g| g >= 0.4);
            let effective_social = if credible { social } else { SocialAct::None };

            let before = world.agent(&name).map(|a| a.place);
            let outcome = apply(world, &name, &intent, &line.say, effective_social);
            if let Some(e) = world.events.last_mut() {
                e.intensity = intensity;
                e.grounded = grounded;
                e.fidelity = fidelity;
            }
            let _ = before;

            agent_turns.push(AgentTurn {
                name,
                action: intent.action,
                accepted: outcome.accepted,
                note: outcome.note,
                say: line.say,
                why: line.why,
                social: if credible { social.id().to_string() } else { "discredited".into() },
                fidelity,
                grounded,
                intensity,
                confidence: Some(confidence),
                latency_ms: line.latency_ms,
                model: line.model,
                state: Value::Null,
            });
        }

        let events_before = world.events.len();
        world.settle();

        // A birth is an event the observer must see without clicking: the feed
        // renders turn records, so the settlement's births ride along here.
        let births = world.events[events_before..]
            .iter()
            .filter(|e| e.kind == society_sim::EventKind::Birth)
            .filter_map(|e| {
                world.agent(&e.actor).map(|a| BirthRecord {
                    child: a.name.clone(),
                    parents: a.parents.clone(),
                    generation: a.generation,
                    place: a.place.id().to_string(),
                })
            })
            .collect();

        // Commit to disk before reporting success. The save is the observer's
        // only protection against losing a village, so it is not fire-and-
        // forget: an unwritten turn is a failure the caller must see.
        crate::persistence::save(world, &self.save_path)
            .map_err(|e| EngineError::Persistence(e.to_string()))?;

        Ok(TurnRecord {
            turn,
            day,
            intent_call: Some(CallRecord::of(&intent_answer)),
            gate_call: Some(CallRecord::of(&gate_answer)),
            agents: agent_turns,
            births,
        })
    }
}

/// Card plus the decision and register, which is what the voice needs and the
/// chooser must not see (the split that keeps 0.6B from doing two jobs).
fn agent_card_with(
    w: &World,
    a: &society_sim::Agent,
    decision: &str,
    register: u8,
    addressee: Option<&str>,
) -> String {
    const REGISTERS: [&str; 5] = ["cold and clipped", "wary and guarded", "plain and neutral", "warm and open", "urgent and raw"];
    let mut card = agent_card(w, a, &decision.replace("go:", "leave for "));
    card.push_str(&format!(
        "Register: {}.\n",
        REGISTERS[register.min(4) as usize]
    ));
    if let Some(t) = addressee {
        card.push_str(&format!("You are speaking to {t}.\n"));
    }
    card
}

/// Run `n` turns, returning each record. Stops on the first hard failure.
pub async fn run(
    engine: &Engine,
    world: &mut World,
    turns: usize,
) -> Result<Vec<TurnRecord>, EngineError> {
    let mut out = Vec::with_capacity(turns);
    for _ in 0..turns {
        out.push(engine.step(world).await?);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_clamped() {
        const R: [&str; 5] = ["a", "b", "c", "d", "e"];
        assert_eq!(R[9.min(4)], "e");
        assert_eq!(R[0.min(4)], "a");
    }
}
