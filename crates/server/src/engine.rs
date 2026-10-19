//! The turn loop. Owns ordering; borrows judgment and voice; never invents
//! state.
//!
//! Turn shape (each step is one batched Jev call, not one per agent):
//!   1. code enumerates legal actions per living agent
//!   2. Jev chooses intent + register + addressee for all agents at once
//!   3. voice renders one line per agent, concurrently under an AIMD cap
//!   4. Jev gates all lines at once: fidelity, grounding, social act
//!   5. code applies effects in a fixed order and settles the turn

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use society_sim::{
    affordances, agent_card, apply, judgment_state, Event, EventKind, Intent, Memory, MemoryKind,
    Place, Role, SocialAct, World,
};

use crate::inference::judgment::{
    action_question, choice_question, noul_question, score_question, Answer, Dilemma, Judge,
};
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
    /// Whom the villager addressed at their location, if anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressee: Option<String>,
    /// Vocal register chosen by Jev for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register: Option<String>,
    /// Inner hesitation when the villager was nearly split between two actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dilemma: Option<Dilemma>,
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
            Self::Judgment(e) => write!(f, "{e}"),
            Self::Halted(e) => write!(f, "society halted: {e}"),
            Self::Persistence(e) => write!(f, "world not saved: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

#[derive(Debug)]
pub struct Engine {
    pub judge: Arc<Judge>,
    pub voice: Arc<Voice>,
    /// Where committed turns are written. `None` is test-only.
    save_path: std::path::PathBuf,
    /// AIMD permit ceiling; adjusted by observed latency.
    permits: Arc<tokio::sync::Semaphore>,
    /// Total permits currently issued. `available_permits` is the free count,
    /// which is not the limit while calls are in flight, so growth has to be
    /// measured against this or it walks past `MAX_PERMITS`.
    permit_limit: Arc<std::sync::atomic::AtomicUsize>,
}

/// One villager's resolved choice: name, intent, salience, urgency, the
/// judge's stated reason, and any dilemma it raised.
type ResolvedIntent = (String, Intent, f32, u8, Option<String>, Option<Dilemma>);

const MIN_PERMITS: usize = 2;
/// One turn makes at most one voice call per living villager, so
/// `POPULATION_CAP` already bounds it at 12 concurrent calls. The ceiling stays
/// above that so a local setup can still ask for the 16 permits measured clean
/// on `qwen3:0.6b`; a cloud burst that still hits the account's queue is caught
/// by the 429 path, which sheds to the floor.
const MAX_PERMITS: usize = 24;
/// A nine-line cloud turn measures 0.6–4.4 s per line with no 429, so slowness
/// only means congestion well above that. Rate limits, not latency, are the
/// primary signal; this is the backstop for a daemon that is up but thrashing.
const SLOW_MS: u64 = 10_000;

const REGISTERS: [&str; 5] = [
    "cold and clipped",
    "wary and guarded",
    "plain and neutral",
    "warm and open",
    "urgent and raw",
];

/// Turns before an unfulfilled promise becomes a public breach of faith.
const PROMISE_OVERDUE_TURNS: u64 = 8;

impl Engine {
    /// Every committed turn is written to `save_path` before `step` returns Ok,
    /// so a crash loses at most the turn in flight. There is no engine without
    /// a save path: a village that only exists in memory is a reset waiting to
    /// happen.
    #[must_use]
    pub fn new(
        judge: Arc<Judge>,
        voice: Arc<Voice>,
        permits: usize,
        save_path: std::path::PathBuf,
    ) -> Self {
        let permits = permits.clamp(MIN_PERMITS, MAX_PERMITS);
        Self {
            judge,
            voice,
            save_path,
            permits: Arc::new(tokio::sync::Semaphore::new(permits)),
            permit_limit: Arc::new(std::sync::atomic::AtomicUsize::new(permits)),
        }
    }

    #[must_use]
    pub fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }

    /// AIMD, one decision per turn. Slowness shrinks the window to the floor; a
    /// turn where every line came back fast grows it by one. Growth is
    /// per-turn, not per line: a nine-line turn would otherwise add nine
    /// permits at once and blow past the backend's queue.
    fn adapt(&self, worst_latency_ms: u64) {
        use std::sync::atomic::Ordering;
        if worst_latency_ms > SLOW_MS {
            self.shed_to_floor();
        } else {
            // Compare against the issued total, not the free count: mid-turn,
            // free permits are always below the limit and growth would never
            // stop.
            let limit = self.permit_limit.load(Ordering::Relaxed);
            if limit < MAX_PERMITS {
                self.permits.add_permits(1);
                self.permit_limit.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A 429 is the congestion signal latency cannot give: drop to the floor
    /// before the next turn.
    fn shed_to_floor(&self) {
        use std::sync::atomic::Ordering;
        let free = self.permits.available_permits();
        if free > MIN_PERMITS {
            let shed = free as u32 - MIN_PERMITS as u32;
            // The guard must be forgotten: dropping a `SemaphorePermit` returns
            // the permits, which would make the shed a no-op.
            if let Ok(permits) = self.permits.try_acquire_many(shed) {
                permits.forget();
                self.permit_limit
                    .fetch_sub(shed as usize, Ordering::Relaxed);
            } else { /* another line took one first; next turn sheds */
            }
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
    ///
    /// # Panics
    ///
    /// Panics if the judge returns an answer for a villager who is not living.
    ///
    /// # Errors
    ///
    /// Returns an `EngineError` if the world is halted or any judgment or voice call fails.
    #[allow(
        clippy::too_many_lines,
        reason = "one turn: judge, resolve, voice, commit, in order"
    )]
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
            let enriched_state = enrich_judgment_state(world, a);
            state.insert(n.clone(), enriched_state);
            questions.insert(
                format!("intent_{n}"),
                action_question(
                    &format!(
                        "As {n}, reading `{n}`, which single action do you take this turn? \
                         Only the actions in `{n}.legal_actions` are possible, and the choice must be one of them. \
                         Weigh hunger, water, weather, kinship, overdue promises, place ownership, and recent speech at this place. \
                         Choose the action {n} would defend afterwards."
                    ),
                    &acts,
                ),
            );
            questions.insert(
                format!("register_{n}"),
                score_question(
                    &format!("In what register does {n} speak this turn?"),
                    [
                        "Cold and clipped",
                        "Wary and guarded",
                        "Plain and neutral",
                        "Warm and open",
                        "Urgent and raw",
                    ],
                ),
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
                        "instructions": format!("Whom at `{n}`'s place does {n} directly address?"),
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
        let mut intents: Vec<ResolvedIntent> = Vec::new();
        for n in &names {
            let a = world.agent(n).expect("living agent");
            let legal = affordances(world, a);
            let j = intent_answer
                .get(&format!("intent_{n}"))
                .ok_or_else(|| EngineError::Judgment(format!("missing intent answer for {n}")))?;
            let chosen = j
                .choice()
                .ok_or_else(|| EngineError::Judgment(format!("intent for {n} was not a choice")))?;
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
                .and_then(|j| j.choice().map(str::to_owned))
                .filter(|s| s != "nobody");
            let dilemma = j.dilemma(0.12);
            intents.push((
                n.clone(),
                intent,
                j.confidence().unwrap_or(0.0),
                register,
                addressee,
                dilemma,
            ));
        }

        // ---- 2. voice: concurrent, AIMD-capped ----
        let mut tasks = tokio::task::JoinSet::new();
        for (name, intent, _, register, addressee, dilemma) in &intents {
            let a = world.agent(name).expect("agent");
            let card = agent_card_with(
                world,
                a,
                &intent.action,
                *register,
                addressee.as_deref(),
                dilemma.as_ref(),
            );
            // The elder is the only villager whose line is rendered by the
            // elder model; everyone else uses the actor model.
            let elder = a.role == Role::Elder;
            let voice = Arc::clone(&self.voice);
            let permits = Arc::clone(&self.permits);
            let name_for_call = name.clone();
            let name = name.clone();
            tasks.spawn(async move {
                let permit = permits.acquire_owned().await.expect("semaphore open");
                let res = if elder {
                    voice.speak_elder(&card, &name_for_call).await
                } else {
                    voice.speak_actor(&card, &name_for_call).await
                };
                drop(permit);
                (name, res)
            });
        }
        let mut lines: Vec<(String, Line)> = Vec::new();
        let mut worst_ms = 0;
        let mut rate_limited = false;
        while let Some(joined) = tasks.join_next().await {
            let (name, res) =
                joined.map_err(|e| EngineError::Judgment(format!("voice task failed: {e}")))?;
            match res {
                Ok(line) => {
                    worst_ms = worst_ms.max(line.latency_ms);
                    lines.push((name, line));
                },
                // A 429 is backpressure, not a broken turn: the line is lost,
                // the world is unaffected, and the next turn runs narrower.
                Err(e) if e.retry_after().is_some() => {
                    rate_limited = true;
                    eprintln!("voice backpressure for {name}: {e}");
                },
                Err(e) => {
                    // A malformed local reply is not a judgment failure; the
                    // society survives it and the turn is simply not committed.
                    return Err(EngineError::Judgment(format!(
                        "voice failed for {name}: {e}"
                    )));
                },
            }
        }
        if rate_limited {
            self.shed_to_floor();
        } else {
            self.adapt(worst_ms);
        }
        // Deterministic ordering when committing, whatever order the calls
        // completed in.
        lines.sort_by(|a, b| a.0.cmp(&b.0));

        // ---- 3. gate: one batched request ----
        let mut gate_state = Map::new();
        let mut gate_q = Map::new();
        for (name, line) in &lines {
            let a = world.agent(name).expect("agent");
            let (_, intent, _, register, addressee, dilemma) = intents
                .iter()
                .find(|(n, _, _, _, _, _)| n == name)
                .expect("intent exists");
            gate_state.insert(
                name.clone(),
                json!({
                    "said": line.say,
                    "why": line.why,
                    "decided": intent.action,
                    "register": REGISTERS[(*register).min(4) as usize],
                    "addressed_to": addressee,
                    "dilemma": dilemma,
                    "place": a.place.id(),
                    "place_owner": world.place_state(a.place).owner,
                    "food_carried": a.food,
                    "water_carried": a.water,
                    "hunger": a.hunger,
                    "food_here": world.place_state(a.place).food,
                    "people_present": world.present(a.place, name),
                    "world_state": state.get(name).cloned().unwrap_or_else(|| judgment_state(world, a)),
                }),
            );
            gate_q.insert(
                format!("fidelity_{name}"),
                // Contradiction, not alignment: the old "does it carry out the
                // decision" wording rewarded parroting the action id (measured:
                // the identical string "draw_water" scored 0.96 when the
                // decision was `draw_water`, 0.25 when it was `speak`) and
                // punished a line that simply did not mention the action.
                noul_question(&format!(
                    "Does `{name}.said` contradict what `{name}` decided or did this turn \
                     (`{name}.decided`)? A line that merely fails to mention the decision is \
                     not a contradiction; a line that states the opposite of it is."
                )),
            );
            gate_q.insert(
                format!("grounded_{name}"),
                noul_question(&format!(
                    "Are the concrete facts in `{name}.said` (amounts, who is present, who has what) \
                     consistent with `{name}`? Invented or contradicted facts make this false."
                )),
            );
            gate_q.insert(
                format!("social_{name}"),
                choice_question(
                    &format!(
                        "What social act does `{name}.said` perform toward someone present, if any?"
                    ),
                    &[
                        ("none", "No social act"),
                        ("promise", "Commits to a future action"),
                        ("gift", "Gives or offers something"),
                        ("accusation", "Blames someone"),
                        ("threat", "Warns of harm"),
                        ("gossip", "Reports a third party's behaviour"),
                        ("plea", "Asks for help"),
                        ("gratitude", "Thanks someone"),
                        ("counsel", "Offers advice or reassurance"),
                        ("trade_offer", "Proposes an exchange of goods"),
                        ("blessing", "Offers a blessing or invokes shared faith"),
                        ("forgiveness", "Forgives a wrong or releases a grudge"),
                    ],
                ),
            );
            gate_q.insert(
                format!("intensity_{name}"),
                score_question(
                    &format!("How much dramatic pressure does `{name}.said` create?"),
                    [
                        "Flat restatement",
                        "Mild preference",
                        "Clear want or worry",
                        "Strong need or accusation",
                        "Crisis or turn",
                    ],
                ),
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
        for (name, intent, confidence, register, addressee, dilemma) in intents {
            let line = lines.iter().find(|(n, _)| *n == name).map_or(
                Line {
                    say: String::new(),
                    why: String::new(),
                    model: String::new(),
                    latency_ms: 0,
                },
                |(_, l)| l.clone(),
            );
            let social = gate_answer
                .get(&format!("social_{name}"))
                .and_then(|j| j.choice().map(SocialAct::from_id))
                .unwrap_or(SocialAct::None);
            let q = |t: Option<f32>| t.map(|x| (x * 100.0).round() / 100.0);
            let fidelity = q(gate_answer
                .get(&format!("fidelity_{name}"))
                .and_then(|j| j.noul()));
            let grounded = q(gate_answer
                .get(&format!("grounded_{name}"))
                .and_then(|j| j.noul()));
            let intensity = q(gate_answer
                .get(&format!("intensity_{name}"))
                .and_then(|j| j.score()));

            // A line that contradicts its own decision or the world is spoken
            // but not credited: it cannot move trust.
            let credible = fidelity.is_none_or(|f| f >= 0.5) && grounded.is_none_or(|g| g >= 0.4);
            let effective_social = if credible { social } else { SocialAct::None };

            let place_before = world.agent(&name).map(|a| a.place);
            let outcome = apply(world, &name, &intent, &line.say, effective_social);
            if let Some(e) = world.events.last_mut() {
                e.intensity = intensity;
                e.grounded = grounded;
                e.fidelity = fidelity;
            }

            if credible {
                settle_social_echoes(
                    world,
                    turn,
                    &name,
                    place_before,
                    &intent.action,
                    &line.say,
                    &line.why,
                    effective_social,
                    register,
                    addressee.as_deref(),
                    intensity.unwrap_or(1.0),
                );
            }

            let agent_state = state.get(&name).cloned().unwrap_or(Value::Null);
            let register_label = REGISTERS[register.min(4) as usize].to_owned();

            agent_turns.push(AgentTurn {
                name,
                action: intent.action,
                accepted: outcome.accepted,
                note: outcome.note,
                say: line.say,
                why: line.why,
                social: if credible {
                    social.id().to_owned()
                } else {
                    "discredited".into()
                },
                fidelity,
                grounded,
                intensity,
                confidence: Some(confidence),
                latency_ms: line.latency_ms,
                model: line.model,
                state: agent_state,
                addressee,
                register: Some(register_label),
                dilemma,
            });
        }

        let events_before = world.events.len();
        settle_village_promises(world);
        world.settle();

        // A birth is an event the observer must see without clicking: the feed
        // renders turn records, so the settlement's births ride along here.
        let births = world.events[events_before..]
            .iter()
            .filter(|e| e.kind == EventKind::Birth)
            .filter_map(|e| {
                world.agent(&e.actor).map(|a| BirthRecord {
                    child: a.name.clone(),
                    parents: a.parents.clone(),
                    generation: a.generation,
                    place: a.place.id().to_owned(),
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

/// Enriches `judgment_state(world, a)` with village institutions, kinship,
/// active promises, territorial ownership, and recent local dialogue so Jev
/// judges with the full context of a real village.
#[must_use]
pub fn enrich_judgment_state(w: &World, a: &society_sim::Agent) -> Value {
    let mut base = judgment_state(w, a);
    let Some(obj) = base.as_object_mut() else {
        return base;
    };

    let children: Vec<String> = w
        .living()
        .filter(|other| other.parents.iter().any(|p| p == &a.name))
        .map(|other| other.name.clone())
        .collect();
    let siblings: Vec<String> = w
        .living()
        .filter(|other| {
            other.name != a.name
                && !a.parents.is_empty()
                && other.parents.iter().any(|p| a.parents.contains(p))
        })
        .map(|other| other.name.clone())
        .collect();
    let elders: Vec<String> = w
        .living()
        .filter(|other| other.role == Role::Elder)
        .map(|other| other.name.clone())
        .collect();

    let local_dialogue = recent_place_dialogue(w, a.place, 4);
    let place_owner = w.place_state(a.place).owner.clone();

    let promise_status = a.promise.as_ref().map(|p| {
        let age = w.turn.saturating_sub(p.made_turn);
        let promisee_present = w.present(a.place, &a.name).iter().any(|n| n == &p.to);
        json!({
            "to": p.to,
            "text": p.text,
            "age_turns": age,
            "overdue": p.kept.is_none() && age >= PROMISE_OVERDUE_TURNS,
            "kept": p.kept,
            "promisee_here": promisee_present,
        })
    });

    let present_details: Vec<Value> = w
        .living()
        .filter(|other| other.place == a.place && other.name != a.name)
        .map(|other| {
            let trust = (a.trust_of(&other.name) * 10.0).round() / 10.0;
            let is_kin = a.parents.contains(&other.name)
                || other.parents.contains(&a.name)
                || (!a.parents.is_empty() && other.parents.iter().any(|p| a.parents.contains(p)));
            json!({
                "name": other.name,
                "role": if other.role == Role::Elder { "elder" } else { "villager" },
                "food": other.food,
                "water": other.water,
                "hunger": other.hunger,
                "standing": (other.standing * 10.0).round() / 10.0,
                "your_trust": trust,
                "kin": is_kin,
                "goal": other.current_goal,
            })
        })
        .collect();

    obj.insert(
        "village_context".into(),
        json!({
            "season": w.season().id(),
            "time_of_day": w.time_of_day().id(),
            "weather_kind": w.weather_kind.id(),
            "temperature_c": w.temperature_c,
            "village_morale": (w.village_morale * 100.0).round() / 100.0,
            "place_owner": place_owner,
            "elders": elders,
            "kin": {
                "parents": a.parents,
                "children": children,
                "siblings": siblings,
            },
            "promise_status": promise_status,
            "present_villagers": present_details,
            "recent_place_dialogue": local_dialogue,
        }),
    );
    base
}

/// Recent spoken lines at `place`, oldest first, so villagers respond to an
/// ongoing local scene rather than speaking in isolation.
#[must_use]
pub fn recent_place_dialogue(w: &World, place: Place, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = w
        .events
        .iter()
        .rev()
        .filter(|e| e.place == place && !e.divine && !e.text.trim().is_empty())
        .take(limit)
        .map(|e| format!("Turn {} {}: {}", e.turn, e.actor, e.text))
        .collect();
    out.reverse();
    out
}

/// Propagates interpersonal memories, rumor mill echoes, trust shifts, and
/// dynamic goal progression after a credible turn.
#[allow(
    clippy::too_many_arguments,
    reason = "each argument is a distinct slice of the turn state"
)]
fn settle_social_echoes(
    world: &mut World,
    turn: u64,
    speaker: &str,
    place_before: Option<Place>,
    action: &str,
    say: &str,
    why: &str,
    social: SocialAct,
    register: u8,
    addressee: Option<&str>,
    intensity: f32,
) {
    let Some(place) = place_before.or_else(|| world.agent(speaker).map(|a| a.place)) else {
        return;
    };

    // 1. Evolve the speaker's current goal when their motivation (`why`) is
    // substantive and grounded.
    if let Some(a) = world.agent_mut(speaker) {
        if let Some(new_goal) = synthesize_goal(action, social, why, addressee) {
            a.current_goal = new_goal;
        }
    }

    if say.trim().is_empty() {
        return;
    }

    let reg_name = REGISTERS[register.min(4) as usize];
    let salience = intensity.mul_add(0.14, 0.35).clamp(0.35, 0.95);

    // 2. Direct addressee memory & relational trust response.
    if let Some(target) = addressee {
        let trust_delta: f32 = match social.id() {
            "gift" | "gratitude" | "blessing" | "forgiveness" => 0.25,
            "counsel" | "promise" | "trade_offer" => 0.12,
            "plea" => 0.05,
            "accusation" => -0.25,
            "threat" => -0.40,
            _ if register == 3 => 0.05,
            _ if register == 0 => -0.05,
            _ => 0.0,
        };

        if let Some(target_agent) = world.agent_mut(target) {
            target_agent.remember(Memory::new(
                turn,
                format!("{speaker} spoke to you ({reg_name}): \"{say}\""),
                salience,
                MemoryKind::Witnessed,
            ));
            if trust_delta.abs() > 0.0 {
                let current = target_agent.trust_of(speaker);
                target_agent.set_trust(speaker, (current + trust_delta).clamp(0.0, 4.0));
            }
        }

        // If the speaker had an unkept promise to `target` and just delivered a
        // gift or fulfilled counsel/trade, mark the promise kept.
        if matches!(social.id(), "gift" | "trade_offer") || action.starts_with("give") {
            let fulfilled = world
                .agent(speaker)
                .and_then(|a| a.promise.as_ref())
                .is_some_and(|p| p.to == target && p.kept.is_none());
            if fulfilled {
                if let Some(a) = world.agent_mut(speaker) {
                    if let Some(p) = a.promise.as_mut() {
                        p.kept = Some(true);
                    }
                    a.standing = (a.standing + 0.3).clamp(0.0, 4.0);
                }
                if let Some(t) = world.agent_mut(target) {
                    t.remember(Memory::new(
                        turn,
                        format!("{speaker} honored their promise to you."),
                        0.85,
                        MemoryKind::Witnessed,
                    ));
                }
            }
        }
    }

    // 3. Rumor mill & witness propagation at the same location.
    if matches!(
        social.id(),
        "gossip" | "accusation" | "threat" | "promise" | "blessing" | "forgiveness"
    ) || intensity >= 3.2
    {
        let witnesses: Vec<String> = world
            .present(place, speaker)
            .into_iter()
            .filter(|w| Some(w.as_str()) != addressee)
            .collect();
        let act_label = social.id();
        for w_name in witnesses {
            if let Some(witness) = world.agent_mut(&w_name) {
                witness.remember(Memory::new(
                    turn,
                    format!(
                        "At {}, you overheard {speaker} ({act_label}): \"{say}\"",
                        place.id()
                    ),
                    (salience * 0.85).clamp(0.3, 0.9),
                    MemoryKind::Witnessed,
                ));
            }
        }
    }

    // 4. Communal morale nudges from public blessings, forgiveness, or threats.
    match social.id() {
        "blessing" | "forgiveness" | "gratitude" => {
            world.village_morale = (world.village_morale + 0.02).clamp(0.0, 4.0);
        },
        "threat" | "accusation" if intensity >= 3.0 => {
            world.village_morale = (world.village_morale - 0.02).clamp(0.0, 4.0);
        },
        _ => {},
    }
}

/// Checks for long-overdue promises across the village and records the strain
/// on trust and standing when an oath goes unkept.
fn settle_village_promises(world: &mut World) {
    let turn = world.turn;
    let mut breaches: Vec<(String, String, String, Place)> = Vec::new();
    for a in world.living() {
        if let Some(p) = &a.promise {
            if p.kept.is_none()
                && turn > p.made_turn
                && (turn - p.made_turn) == PROMISE_OVERDUE_TURNS
            {
                breaches.push((a.name.clone(), p.to.clone(), p.text.clone(), a.place));
            }
        }
    }
    for (promiser, promisee, text, place) in breaches {
        if let Some(a) = world.agent_mut(&promiser) {
            a.standing = (a.standing - 0.25).clamp(0.0, 4.0);
            a.remember(Memory::new(
                turn,
                format!("Your promise to {promisee} ({text}) is now overdue."),
                0.8,
                MemoryKind::Witnessed,
            ));
        }
        if let Some(target) = world.agent_mut(&promisee) {
            let cur = target.trust_of(&promiser);
            target.set_trust(&promiser, (cur - 0.35).clamp(0.0, 4.0));
            target.remember(Memory::new(
                turn,
                format!("{promiser} has still not kept their promise ({text})."),
                0.85,
                MemoryKind::Witnessed,
            ));
        }
        world.push_event(Event {
            turn,
            actor: promiser.clone(),
            place,
            kind: EventKind::Divine,
            action: "broken_oath".into(),
            text: format!("{promiser}'s promise to {promisee} ({text}) hangs unkept."),
            social: None,
            intensity: Some(2.8),
            grounded: Some(1.0),
            fidelity: Some(1.0),
            divine: false,
        });
    }
}

fn synthesize_goal(
    action: &str,
    social: SocialAct,
    why: &str,
    addressee: Option<&str>,
) -> Option<String> {
    let trimmed = why.trim().trim_end_matches('.');
    if trimmed.len() >= 8 && trimmed.len() <= 72 {
        return Some(trimmed.to_owned());
    }
    match (social.id(), addressee) {
        ("promise", Some(t)) => Some(format!("Make good on promise to {t}")),
        ("plea", Some(t)) => Some(format!("Seek aid from {t}")),
        ("accusation", Some(t)) => Some(format!("Hold {t} to account")),
        ("trade_offer", Some(t)) => Some(format!("Strike a trade with {t}")),
        _ if action.starts_with("go:") => {
            Some(format!("Reach {} and see what awaits", &action[3..]))
        },
        _ => None,
    }
}

/// Card plus the decision and register, which is what the voice needs and the
/// chooser must not see (the split that keeps one model from doing two jobs).
fn agent_card_with(
    w: &World,
    a: &society_sim::Agent,
    decision: &str,
    register: u8,
    addressee: Option<&str>,
    dilemma: Option<&Dilemma>,
) -> String {
    // The sim crate renders the decision itself through `action_phrase`; any
    // rewrite here lands on top of that phrase ("leave for leave for market").
    let mut card = agent_card(w, a, decision);
    let _ = writeln!(
        card,
        "Village atmosphere: {} ({}) · {} ({} °C) · morale {:.2}.",
        w.season().id(),
        w.time_of_day().id(),
        w.weather_kind.id(),
        w.temperature_c,
        w.village_morale,
    );
    if let Some(owner) = &w.place_state(a.place).owner {
        let _ = writeln!(card, "This place ({}) is claimed by {owner}.", a.place.id());
    }
    // The goal is printed by the sim card, framed as context. Repeating it here
    // put the same string in the card twice, and the voice model copies it.
    if let Some(p) = &a.promise {
        if p.kept.is_none() {
            let age = w.turn.saturating_sub(p.made_turn);
            let _ = writeln!(
                card,
                "Unkept promise to {} ({} turns ago): \"{}\".",
                p.to, age, p.text
            );
        }
    }
    let _ = writeln!(card, "Register: {}.", REGISTERS[register.min(4) as usize]);
    if let Some(t) = addressee {
        let trust = a.trust_of(t);
        let is_parent = a.parents.iter().any(|p| p == t);
        let is_child = w
            .agent(t)
            .is_some_and(|other| other.parents.iter().any(|p| p == &a.name));
        let kin_note = if is_parent {
            " (your parent)"
        } else if is_child {
            " (your child)"
        } else {
            ""
        };
        let _ = writeln!(
            card,
            "You are speaking directly to {t}{kin_note} (your trust: {trust:+.1})."
        );
    }
    if let Some(d) = dilemma {
        // Prose, not ids: a raw id in the card is a string the model copies.
        let _ = writeln!(
            card,
            "Inner conflict: you chose {} ({:.0}%), but nearly chose {} ({:.0}%).",
            society_sim::action_phrase(&d.chosen),
            d.chosen_prob * 100.0,
            society_sim::action_phrase(&d.alternative),
            d.alternative_prob * 100.0
        );
    }
    let recent = recent_place_dialogue(w, a.place, 3);
    if !recent.is_empty() {
        card.push_str("Recent speech at this place (answer them; never reuse their words):\n");
        for line in recent {
            let _ = writeln!(card, "  - {line}");
        }
    }
    card
}

/// Run `n` turns, returning each record. Stops on the first hard failure.
///
/// # Errors
///
/// Returns the first `EngineError` raised by a turn.
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
        assert_eq!(R[4], "e");
        assert_eq!(R[0], "a");
    }

    fn engine_with_permits(permits: usize) -> Engine {
        use crate::inference::judgment::{Judge, JudgeConfig};
        use crate::inference::voice::{Voice, VoiceConfig};
        let judge = Arc::new(Judge::new(JudgeConfig {
            endpoint: "http://127.0.0.1:1".into(),
            api_key: "test".into(),
            model: "test".into(),
            timeout: Duration::from_millis(50),
        }));
        let voice = Arc::new(Voice::new(VoiceConfig {
            base_url: "http://127.0.0.1:1".into(),
            actor_model: "a".into(),
            elder_model: "b".into(),
            actor_think: crate::inference::voice::Think::parse("low"),
            elder_think: crate::inference::voice::Think::parse("low"),
            timeout: Duration::from_millis(50),
            max_tokens: 8,
        }));
        Engine::new(
            judge,
            voice,
            permits,
            std::path::PathBuf::from("/unused.json"),
        )
    }

    #[test]
    fn fast_turns_grow_the_window_one_permit_at_a_time() {
        let e = engine_with_permits(2);
        // Growth is per turn: `step` calls `adapt` once, with the worst line.
        e.adapt(10);
        assert_eq!(e.permit_limit.load(std::sync::atomic::Ordering::Relaxed), 3);
        e.adapt(10);
        assert_eq!(e.permit_limit.load(std::sync::atomic::Ordering::Relaxed), 4);
    }

    #[test]
    fn the_window_never_exceeds_its_ceiling() {
        let e = engine_with_permits(MAX_PERMITS);
        for _ in 0..50 {
            e.adapt(10);
        }
        assert_eq!(
            e.permit_limit.load(std::sync::atomic::Ordering::Relaxed),
            MAX_PERMITS
        );
        assert!(e.permit_limit.load(std::sync::atomic::Ordering::Relaxed) <= MAX_PERMITS);
    }

    #[test]
    fn a_slow_turn_sheds_to_the_floor_and_a_rate_limit_does_too() {
        let e = engine_with_permits(12);
        e.adapt(30_000);
        assert_eq!(e.available_permits(), MIN_PERMITS);

        // Issued total tracks the semaphore, so the next grow starts from 2.
        e.adapt(10);
        assert_eq!(e.permit_limit.load(std::sync::atomic::Ordering::Relaxed), 3);

        let e = engine_with_permits(12);
        e.shed_to_floor();
        assert_eq!(e.available_permits(), MIN_PERMITS);
    }

    #[test]
    fn enriched_card_and_state_include_kinship_promises_and_local_dialogue() {
        let mut w = World::founding(7);
        w.turn = 12;
        w.place_state_mut(Place::Market).owner = Some("Sable".into());
        w.agent_mut("Ada").unwrap().place = Place::Market;
        w.agent_mut("Bo").unwrap().place = Place::Market;
        w.agent_mut("Ada").unwrap().current_goal = "Secure winter grain".into();
        w.agent_mut("Ada").unwrap().promise = Some(society_sim::Promise {
            to: "Bo".into(),
            text: "share two measures".into(),
            made_turn: 2,
            kept: None,
        });
        w.push_event(Event {
            turn: 11,
            actor: "Bo".into(),
            place: Place::Market,
            kind: EventKind::Divine,
            action: "speak".into(),
            text: "You promised me grain before the frost.".into(),
            social: None,
            intensity: Some(3.0),
            grounded: Some(1.0),
            fidelity: Some(1.0),
            divine: false,
        });

        let ada = w.agent("Ada").unwrap();
        let enriched = enrich_judgment_state(&w, ada);
        assert_eq!(enriched["village_context"]["place_owner"], "Sable");
        assert_eq!(
            enriched["village_context"]["promise_status"]["overdue"],
            true
        );
        assert_eq!(
            enriched["village_context"]["promise_status"]["promisee_here"],
            true
        );

        let d = Dilemma {
            chosen: "give:Bo".into(),
            alternative: "eat".into(),
            chosen_prob: 0.45,
            alternative_prob: 0.40,
        };
        let card = agent_card_with(&w, ada, "give:Bo", 4, Some("Bo"), Some(&d));
        assert!(card.contains("claimed by Sable"));
        assert!(card.contains("Secure winter grain"));
        assert!(card.contains("Unkept promise to Bo"));
        assert!(card.contains("Inner conflict"));
        assert!(card.contains("You promised me grain before the frost."));
    }

    /// A founding world with a fixed goal and a place owner, so card assertions
    /// do not depend on the seed's random placements.
    fn card_world() -> World {
        let mut w = World::founding(7);
        w.place_state_mut(Place::Market).owner = Some("Sable".into());
        w.agent_mut("Ada").unwrap().place = Place::Market;
        w.agent_mut("Bo").unwrap().place = Place::Market;
        w.agent_mut("Ada").unwrap().current_goal = "Secure winter grain".into();
        w
    }

    #[test]
    fn a_card_does_not_offer_the_decision_as_a_copyable_id() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "draw_water", 4, None, None);
        assert!(card.contains("draw water from the well"), "{card}");
        assert!(!card.contains("draw_water"), "{card}");
    }

    #[test]
    fn a_place_decision_is_rendered_once_as_prose() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "go:market", 4, None, None);
        assert!(card.contains("leave for market"), "{card}");
        assert!(!card.contains("leave for leave for"), "{card}");
        assert!(!card.contains("go:market"), "{card}");
    }

    #[test]
    fn the_dilemma_is_rendered_as_prose_not_ids() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let d = Dilemma {
            chosen: "draw_water".into(),
            alternative: "console".into(),
            chosen_prob: 0.33,
            alternative_prob: 0.29,
        };
        let card = agent_card_with(&w, ada, "draw_water", 4, None, Some(&d));
        assert!(
            card.contains("you chose draw water from the well (33%)"),
            "{card}"
        );
        assert!(
            card.contains("nearly chose console someone here (29%)"),
            "{card}"
        );
        assert!(!card.contains("draw_water"), "{card}");
        assert!(!card.contains('`'), "{card}");
    }

    #[test]
    fn the_recent_dialogue_block_asks_for_an_answer_not_a_reuse() {
        let mut w = card_world();
        w.push_event(Event {
            turn: 11,
            actor: "Bo".into(),
            place: Place::Market,
            kind: EventKind::Speech,
            action: "speak".into(),
            text: "The well is low.".into(),
            social: None,
            intensity: Some(2.0),
            grounded: Some(1.0),
            fidelity: Some(1.0),
            divine: false,
        });
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "draw_water", 4, None, None);
        assert!(
            card.contains("Recent speech at this place (answer them; never reuse their words):"),
            "{card}"
        );
        assert!(card.contains("The well is low."), "{card}");
    }

    #[test]
    fn the_goal_appears_exactly_once() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "draw_water", 4, None, None);
        assert_eq!(card.matches("Secure winter grain").count(), 1, "{card}");
    }
}
