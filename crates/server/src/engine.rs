//! The turn loop. Owns ordering; borrows judgment and voice; never invents
//! state.
//!
//! Turn shape (each step is one batched Jev call, not one per agent):
//!   1. code enumerates legal actions per living agent
//!   2. Jev chooses intent for all agents at once; code derives the register
//!      from mood and the card's addressee from trust
//!   3. voice renders one line per agent, concurrently under an AIMD cap
//!   4. Jev gates all lines at once: fidelity, grounding, social act
//!   5. code reads the addressee and the intensity out of the line, applies
//!      effects in a fixed order and settles the turn

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use society_sim::{
    affordances, agent_card, apply, judgment_state, Event, EventKind, Intent, Memory, MemoryKind,
    Mood, Outcome, Place, Role, SocialAct, World,
};

use crate::copy::{is_copy, is_paraphrase};
use crate::inference::judgment::{
    action_question, choice_question, noul_question, Answer, Dilemma, Judge,
};
use crate::inference::voice::{Line, Voice, VoiceError};

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
    /// Whom the line addressed at the villager's location, if anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressee: Option<String>,
    /// Vocal register derived from the villager's mood for this turn.
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

/// One villager's resolved choice: name, intent, the judge's confidence in it,
/// the register derived from mood, the addressee (the card's choice until the
/// line exists, then the line's), and any dilemma it raised.
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

/// The register a label the table does not hold falls back to. "plain and
/// neutral" is the register the measurement actually observed (38 of 45 lines).
const NEUTRAL_REGISTER: &str = "plain and neutral";

/// The register index for `mood`, looked up in [`REGISTERS`].
///
/// Derived in code rather than asked: the five-tone question came back "plain
/// and neutral" for 38 of 45 lines — mode-collapsed — while the answer still
/// drove the card's "Register:" line, a ±0.05 trust delta and the witness
/// memory text, so it was load-bearing and uninformative at once. Mood already
/// carries the tone the question was trying to recover.
fn register_for_mood(mood: Mood) -> u8 {
    let label = match mood {
        Mood::Desperate => "urgent and raw",
        Mood::Anxious | Mood::Feverish => "wary and guarded",
        Mood::Bitter | Mood::Grieving => "cold and clipped",
        Mood::Weary | Mood::Determined | Mood::Content => "plain and neutral",
        Mood::Hopeful => "warm and open",
    };
    register_index(label)
}

/// The label for a register ordinal. The single clamped lookup, so no caller
/// indexes the table itself.
fn register_label(register: u8) -> &'static str {
    REGISTERS[register.min(4) as usize]
}

/// Index of `label` in [`REGISTERS`], so a label and its ordinal cannot drift
/// apart the way a hand-written index would. A label the table does not hold
/// falls back to the neutral entry, which is the register the measurement
/// actually observed (38 of 45 lines).
fn register_index(label: &str) -> u8 {
    let index = REGISTERS
        .iter()
        .position(|r| *r == label)
        .or_else(|| REGISTERS.iter().position(|r| *r == NEUTRAL_REGISTER))
        .unwrap_or(0);
    // The table holds five entries, so this cannot truncate.
    index as u8
}

/// Base dramatic pressure per social act, before the register adjustment.
///
/// Accusation (3.2) and threat (3.4) are deliberately at or above the world's
/// rumor (3.2) and threat-morale (3.0) thresholds: those branches were dead
/// while the value came from the judge, which never went above 2.21 in 45
/// lines.
const fn base_intensity(social: SocialAct) -> f32 {
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

/// How far the vocal register pushes the act's base pressure.
fn register_adjustment(register: u8) -> f32 {
    match register_label(register) {
        "urgent and raw" => 0.2,
        "cold and clipped" => -0.2,
        _ => 0.0,
    }
}

/// Clamp to the ordinal's range and round to two decimals.
fn clamp_intensity(value: f32) -> f32 {
    (value.clamp(0.0, 4.0) * 100.0).round() / 100.0
}

/// The turn's dramatic pressure: the social act's base plus the register's
/// push, clamped and rounded.
///
/// Derived in code rather than asked: the judge's ordinal never exceeded 2.21
/// in 45 lines, below the rumor (3.2) and threat-morale (3.0) checks, so those
/// world branches never fired. The act and the tone are both already decided,
/// and they are the two things the ordinal was meant to express.
fn derived_intensity(social: SocialAct, register: u8) -> f32 {
    clamp_intensity(base_intensity(social) + register_adjustment(register))
}

/// The present villager `a` trusts most, ties broken by name (the smaller name
/// wins, deterministically). `None` when nobody else is at the place.
///
/// This is the card's addressee. The card is built before the line exists, so
/// it can only name a person the speaker is predisposed to address; the record
/// resolves the line's actual addressee afterwards.
fn most_trusted_present(world: &World, a: &society_sim::Agent) -> Option<String> {
    most_trusted(a, world.present(a.place, &a.name).into_iter())
}

/// The candidate `a` trusts most, ties broken by the smaller name.
fn most_trusted(
    a: &society_sim::Agent,
    candidates: impl Iterator<Item = String>,
) -> Option<String> {
    // `min_by` over reversed trust: the smallest comparator value is the
    // highest trust, and equal trust compares by name ascending, so the
    // smallest name wins. Names are unique, so no pair compares equal.
    candidates.min_by(|x, y| {
        a.trust_of(y)
            .total_cmp(&a.trust_of(x))
            .then_with(|| x.cmp(y))
    })
}

/// The villager the line names, restricted to the villagers present at the
/// speaker's place; the card's addressee when it names nobody.
///
/// Derived in code after the line exists: the intent batch asked this before
/// the voice call, so the answer was a guess about a line that had not been
/// written. A whole-word, case-insensitive mention is the only evidence the
/// line itself carries, and several mentions fall back to the speaker's
/// strongest tie. When the line names nobody the card's choice stands, so a
/// silent-on-names line still lands on the person the speaker meant to reach.
fn addressee_from_line(
    world: &World,
    speaker: &society_sim::Agent,
    say: &str,
    card_addressee: Option<String>,
) -> Option<String> {
    let named: Vec<String> = world
        .present(speaker.place, &speaker.name)
        .into_iter()
        .filter(|name| names_whole_word(say, name))
        .collect();
    if named.is_empty() {
        return card_addressee;
    }
    most_trusted(speaker, named.into_iter())
}

/// True when `haystack` contains `name` as a whole word, ignoring case. A name
/// inside a longer word ("Bo" in "body") is not a mention.
fn names_whole_word(haystack: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let hay = haystack.to_lowercase();
    let needle = name.to_lowercase();
    let mut from = 0;
    while let Some(offset) = hay[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = hay[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = hay[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = start + needle.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Turns before an unfulfilled promise becomes a public breach of faith.
const PROMISE_OVERDUE_TURNS: u64 = 8;

/// The observer-facing explanation for a villager the copy filter silenced.
const DROPPED_LINE_NOTE: &str = "dropped a line copied verbatim from the card";

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
            // No register, addressee or intensity question here: all three are
            // meaning-free for this judge and are derived in code below. The
            // tone question returned "plain and neutral" for 38 of 45 lines,
            // the pressure ordinal never passed 2.21 (so the world's 3.2 rumor
            // and 3.0 morale branches could not fire), and the addressee was
            // asked before the line existed, so it guessed.
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
            // The register is a function of mood, not a judgment: see
            // `register_for_mood` for why the question was dropped.
            let register = register_for_mood(a.mood);
            // The card is rendered before the line exists, so it names the
            // present villager the speaker trusts most. The record's addressee
            // is resolved from the line itself after the voice tasks join.
            let addressee = most_trusted_present(world, a);
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
            // Assembled here, while `world` is borrowed, from the same card the
            // model sees; the task owns the strings.
            let sources = CopySources::for_agent(world, a, &card);
            let voice = Arc::clone(&self.voice);
            let permits = Arc::clone(&self.permits);
            let name_for_call = name.clone();
            let name = name.clone();
            tasks.spawn(async move {
                let permit = permits.acquire_owned().await.expect("semaphore open");
                // The copy check and its one retry stay under this permit: a
                // retry is a second call in the same AIMD slot, not a permit of
                // its own, so the window still bounds the backend's queue.
                let res = speak_checked(&voice, elder, &card, &name_for_call, &sources).await;
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

        // The line exists now, so the record's addressee is read out of it
        // instead of guessed. The card was rendered from the trust-based choice
        // above and keeps it: the card had to be built before the voice call.
        // A dropped or lost line has no text and keeps the card's choice.
        for entry in &mut intents {
            let say = lines
                .iter()
                .find(|(n, _)| *n == entry.0)
                .map_or("", |(_, l)| l.say.as_str());
            let card_addressee = entry.4.clone();
            let a = world.agent(&entry.0).expect("living agent");
            entry.4 = addressee_from_line(world, a, say, card_addressee);
        }

        // ---- 3. gate: one batched request ----
        let mut gate_state = Map::new();
        let mut gate_q = Map::new();
        for (name, line) in &lines {
            // A dropped line was never spoken, so there is nothing to ask
            // about: gating it would spend a call on an empty string and, worse,
            // a missing gate answer reads as credible.
            if line.say.is_empty() {
                continue;
            }
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
                    "register": register_label(*register),
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
                // Consistency, not alignment: the old "does it carry out the
                // decision" wording rewarded parroting the action id (measured:
                // the identical string "draw_water" scored 0.96 when the
                // decision was `draw_water`, 0.25 when it was `speak`) and
                // punished a line that simply did not mention the action.
                //
                // Asked in the positive, because the answer is the probability
                // that the proposition holds and the credibility rule reads a
                // high score as faithful. Asking "does it contradict" inverts
                // the scale: measured, 43 of 45 lines then scored below 0.5 and
                // every social effect was discredited.
                noul_question(&format!(
                    "Is `{name}.said` consistent with what `{name}` decided or did this turn \
                     (`{name}.decided`)? A line that merely fails to mention the decision is \
                     consistent with it; a line that states the opposite of it is not."
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
            // No intensity question: it never exceeded 2.21 in 45 lines, so
            // the rumor (3.2) and threat-morale (3.0) branches were
            // unreachable. `derived_intensity` computes it from the social act
            // the judge did answer plus the register.
        }
        // A turn where every line was dropped has nothing left to gate; asking
        // an empty batch would be a network round trip whose failure would fail
        // a turn that already decided to commit.
        let gate_asked = !gate_q.is_empty();
        let gate_answer = if gate_asked {
            self.judge
                .ask(Value::Object(gate_state), gate_q)
                .await
                .map_err(|e| {
                    let reason = self.halt_now(world, format!("judgment layer unavailable: {e}"));
                    EngineError::Judgment(reason)
                })?
        } else {
            Answer {
                answers: Map::new(),
                model: None,
                cost: 0.0,
                input_tokens: 0,
                latency_ms: 0,
                state: Value::Null,
                questions: Value::Null,
            }
        };

        // ---- 4. commit ----
        let mut agent_turns = Vec::new();
        for (name, intent, confidence, register, addressee, dilemma) in intents {
            let found = lines.iter().find(|(n, _)| *n == name).map(|(_, l)| l);
            let line = found.cloned().unwrap_or_else(|| Line {
                say: String::new(),
                why: String::new(),
                model: String::new(),
                latency_ms: 0,
            });
            // A line the engine dropped is present with an empty `say`; the
            // voice module errors rather than return one. A villager absent from
            // `lines` lost the call (a 429 or a malformed reply) and keeps the
            // old silent path, which is not a copy.
            let dropped = found.is_some_and(|l| l.say.is_empty());
            let verdict = turn_verdict(&gate_answer, &name, dropped, found.is_some(), register);

            let place_before = world.agent(&name).map(|a| a.place);
            let outcome = apply(world, &name, &intent, &line.say, verdict.social);
            if let Some(e) = world.events.last_mut() {
                e.intensity = verdict.intensity;
                e.grounded = verdict.grounded;
                e.fidelity = verdict.fidelity;
            }

            if verdict.credible {
                settle_social_echoes(
                    world,
                    turn,
                    &name,
                    place_before,
                    &intent.action,
                    &line.say,
                    &line.why,
                    verdict.social,
                    register,
                    addressee.as_deref(),
                    verdict.intensity.unwrap_or(1.0),
                );
            }

            let agent_state = state.get(&name).cloned().unwrap_or(Value::Null);
            agent_turns.push(agent_turn_record(
                name,
                &intent,
                &outcome,
                &verdict,
                line,
                confidence,
                register,
                addressee,
                dilemma,
                agent_state,
            ));
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
            gate_call: gate_asked.then(|| CallRecord::of(&gate_answer)),
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

/// The strings one villager's line must not reproduce.
///
/// The voice model is small and copies the nearest salient string in its card;
/// measured on a 5-turn run, 5 of 45 lines were a verbatim action id or card
/// string, and the stored line then reappeared in the next card. The lists are
/// built once per speaking villager from the same card the model read, so they
/// hold exactly what that card taught it. With the goal, the villager's own
/// past lines and the recent dialogue in the card, 10 of 18 sampled lines were
/// copies of card text; with all three gone, 0 of 18. Keeping them as sources
/// now would drop lines for copying words the model never saw.
struct CopySources {
    /// Exact text: equality or a contiguous word-run, either direction. The
    /// card, the legal action ids (so a one-word id like "help" is caught),
    /// the action phrases, and the memory lines the card printed.
    strict: Vec<String>,
    /// Short phrases where a content-word paraphrase is still a copy. The card
    /// is deliberately absent: its common words would match almost anything.
    phrases: Vec<String>,
}

impl CopySources {
    /// Assemble the lists for `a` from `card`, the exact text sent to the
    /// voice model.
    fn for_agent(world: &World, a: &society_sim::Agent, card: &str) -> Self {
        let legal = affordances(world, a);
        let action_phrases: Vec<String> = legal
            .iter()
            .map(|id| society_sim::action_phrase(id))
            .collect();
        let memories = card_memories(card);

        let mut strict =
            Vec::with_capacity(1 + legal.len() + action_phrases.len() + memories.len());
        strict.push(card.to_owned());
        strict.extend(legal);
        strict.extend(action_phrases.iter().cloned());
        strict.extend(memories.iter().cloned());

        let mut phrases = action_phrases;
        phrases.extend(memories);

        Self { strict, phrases }
    }

    /// True when either copy test fires.
    fn copies(&self, line: &str) -> bool {
        is_copy(line, &self.strict) || is_paraphrase(line, &self.phrases)
    }
}

/// The memory lines the card printed, taken from the card itself.
///
/// Reading them back out of the rendered card rather than recomputing the
/// memory slice keeps the filter honest: a string the model did not see cannot
/// become a source. The card prefixes memory lines with "- " and nothing else
/// it prints does, so the marker is the slice boundary.
fn card_memories(card: &str) -> Vec<String> {
    card.lines()
        .filter_map(|line| line.strip_prefix("- "))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// One voice call plus, if the line is a copy, one retry with the rejected
/// words appended to the card.
///
/// A dropped line comes back as a [`Line`] with an empty `say`: the voice
/// module refuses to return one, so an empty `say` in `lines` means the engine
/// dropped it. A villager absent from `lines` lost the call to an error instead.
///
/// # Errors
///
/// Returns the first call's error, or the retry's rate-limit error so the turn
/// keeps its backpressure path. Any other retry error is swallowed into a drop:
/// the first call already succeeded, so the turn still commits.
async fn speak_checked(
    voice: &Voice,
    elder: bool,
    card: &str,
    name: &str,
    sources: &CopySources,
) -> Result<Line, VoiceError> {
    async fn speak(voice: &Voice, elder: bool, card: &str, name: &str) -> Result<Line, VoiceError> {
        if elder {
            voice.speak_elder(card, name).await
        } else {
            voice.speak_actor(card, name).await
        }
    }

    let first = speak(voice, elder, card, name).await?;
    if !sources.copies(&first.say) {
        return Ok(first);
    }
    // The same call with a longer card, not a new protocol.
    let retry_card = format!(
        "{card}\n\nYou already said \"{}\". Say something different.",
        first.say
    );
    // The permit was held across both calls, so the retry's cost is the sum:
    // reporting only the second call would hide half the work from `adapt`.
    let latency_ms = first.latency_ms;
    match speak(voice, elder, &retry_card, name).await {
        Ok(line) if !sources.copies(&line.say) => Ok(Line {
            latency_ms: latency_ms + line.latency_ms,
            ..line
        }),
        Ok(line) => Ok(Line {
            say: String::new(),
            why: String::new(),
            model: line.model,
            latency_ms: latency_ms + line.latency_ms,
        }),
        Err(e) if e.retry_after().is_some() => Err(e),
        Err(_) => Ok(Line {
            say: String::new(),
            why: String::new(),
            model: first.model,
            latency_ms,
        }),
    }
}

/// One villager's gate readings and what they mean for the record.
struct TurnVerdict {
    fidelity: Option<f32>,
    grounded: Option<f32>,
    intensity: Option<f32>,
    /// The social act credited to the turn; `None` when it is discredited.
    social: SocialAct,
    /// The record's `social` string: the act's id, or "discredited".
    social_label: String,
    /// Whether trust, memory and social echoes may move.
    credible: bool,
    /// The line was dropped by the copy filter, never gated.
    dropped: bool,
}

impl TurnVerdict {
    /// The record's note. A dropped line's note wins: what the observer needs
    /// explained is why the villager said nothing, not what the action did.
    fn note(&self, action_note: Option<String>) -> Option<String> {
        if self.dropped {
            Some(DROPPED_LINE_NOTE.to_owned())
        } else {
            action_note
        }
    }
}

/// Read the gate answer for one villager. `spoken` is false when no line
/// exists (a lost call), and `register` is the mood-derived tone the intensity
/// derivation needs.
///
/// A dropped line was never asked about, and a missing answer is `None`, which
/// the credibility test would accept, so the drop pins `fidelity` to `Some(0.0)`
/// explicitly.
fn turn_verdict(
    gate: &Answer,
    name: &str,
    dropped: bool,
    spoken: bool,
    register: u8,
) -> TurnVerdict {
    if dropped {
        return TurnVerdict {
            fidelity: Some(0.0),
            grounded: None,
            intensity: None,
            social: SocialAct::None,
            social_label: "discredited".into(),
            credible: false,
            dropped: true,
        };
    }
    let social = gate
        .get(&format!("social_{name}"))
        .and_then(|j| j.choice().map(SocialAct::from_id))
        .unwrap_or(SocialAct::None);
    let q = |t: Option<f32>| t.map(|x| (x * 100.0).round() / 100.0);
    let fidelity = q(gate.get(&format!("fidelity_{name}")).and_then(|j| j.noul()));
    let grounded = q(gate.get(&format!("grounded_{name}")).and_then(|j| j.noul()));

    // A line that contradicts its own decision or the world is spoken but not
    // credited: it cannot move trust.
    let credible = fidelity.is_none_or(|f| f >= 0.5) && grounded.is_none_or(|g| g >= 0.4);
    // The intensity follows the act the judge answered plus the tone code
    // already knows; it is not a separate judgment. It is read before the
    // credibility substitution so the record and the event describe the line
    // that was spoken even when the gate refuses to credit its act. A line that
    // was never spoken (a 429 or a malformed reply) has no pressure to report.
    let intensity = spoken.then(|| derived_intensity(social, register));
    let social = if credible { social } else { SocialAct::None };
    TurnVerdict {
        fidelity,
        grounded,
        intensity,
        social,
        social_label: if credible {
            social.id().to_owned()
        } else {
            "discredited".into()
        },
        credible,
        dropped: false,
    }
}

/// The record `step` writes for one villager. Split out so the dropped-line
/// record can be tested without a network round trip.
#[allow(
    clippy::too_many_arguments,
    reason = "one field per judgment, one per line, one per resolved intent"
)]
fn agent_turn_record(
    name: String,
    intent: &Intent,
    outcome: &Outcome,
    verdict: &TurnVerdict,
    line: Line,
    confidence: f32,
    register: u8,
    addressee: Option<String>,
    dilemma: Option<Dilemma>,
    state: Value,
) -> AgentTurn {
    AgentTurn {
        name,
        action: intent.action.clone(),
        accepted: outcome.accepted,
        note: verdict.note(outcome.note.clone()),
        say: line.say,
        why: line.why,
        social: verdict.social_label.clone(),
        fidelity: verdict.fidelity,
        grounded: verdict.grounded,
        intensity: verdict.intensity,
        confidence: Some(confidence),
        latency_ms: line.latency_ms,
        model: line.model,
        state,
        addressee,
        register: Some(register_label(register).to_owned()),
        dilemma,
    }
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

    let reg_name = register_label(register);
    let salience = intensity.mul_add(0.14, 0.35).clamp(0.35, 0.95);

    // 2. Direct addressee memory & relational trust response.
    if let Some(target) = addressee {
        let trust_delta: f32 = match social.id() {
            "gift" | "gratitude" | "blessing" | "forgiveness" => 0.25,
            "counsel" | "promise" | "trade_offer" => 0.12,
            "plea" => 0.05,
            "accusation" => -0.25,
            "threat" => -0.40,
            _ if reg_name == "warm and open" => 0.05,
            _ if reg_name == "cold and clipped" => -0.05,
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
    // The promise text is quoted, not framed: an unkept promise is the one
    // short phrase the voice should be answering, and the chooser has already
    // used it. No goal line here: the sim card dropped it because the model
    // returns the phrase's opening word instead of a line.
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
    let _ = writeln!(card, "Register: {}.", register_label(register));
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
    // No recent-dialogue block. This was the strongest copy source in the whole
    // card: with the goal, the own past lines and these quoted lines, 10 of 18
    // sampled lines were copies; with all three out, 0 of 18, and the lines
    // were full sentences instead of one-word fragments ("taken", "Grain").
    // Tam said one line four times in six samples while it was quoted back.
    // `enrich_judgment_state` still gives the dialogue to the chooser.
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
        assert_eq!(register_label(0), REGISTERS[0]);
        assert_eq!(register_label(4), REGISTERS[4]);
        // An out-of-range ordinal from an old record or a future table still
        // renders a tone instead of panicking.
        assert_eq!(register_label(u8::MAX), REGISTERS[4]);
    }

    #[test]
    fn the_register_label_follows_the_mood() {
        // Every mood the sim crate defines, with the mapping the slice fixed.
        // The label is looked up in the shared table, so a reordering of
        // `REGISTERS` cannot silently change what a mood sounds like.
        let expected = [
            (Mood::Content, "plain and neutral"),
            (Mood::Hopeful, "warm and open"),
            (Mood::Determined, "plain and neutral"),
            (Mood::Anxious, "wary and guarded"),
            (Mood::Weary, "plain and neutral"),
            (Mood::Bitter, "cold and clipped"),
            (Mood::Grieving, "cold and clipped"),
            (Mood::Feverish, "wary and guarded"),
            (Mood::Desperate, "urgent and raw"),
        ];
        assert_eq!(expected.len(), 9, "one case per Mood variant");
        for (mood, label) in expected {
            let index = register_for_mood(mood);
            assert_eq!(register_label(index), label, "{mood:?}");
            assert_eq!(REGISTERS[index as usize], label, "{mood:?}");
        }
    }

    #[test]
    fn the_register_still_moves_trust_by_a_twentieth() {
        let mut w = World::founding(7);
        w.agent_mut("Ada").unwrap().place = Place::Well;
        w.agent_mut("Bo").unwrap().place = Place::Well;
        let warm = register_index("warm and open");
        let cold = register_index("cold and clipped");

        settle_social_echoes(
            &mut w,
            1,
            "Ada",
            Some(Place::Well),
            "speak",
            "Good morning, Bo.",
            "",
            SocialAct::None,
            warm,
            Some("Bo"),
            1.0,
        );
        let up = w.agent("Bo").unwrap().trust_of("Ada");
        settle_social_echoes(
            &mut w,
            2,
            "Ada",
            Some(Place::Well),
            "speak",
            "Move aside, Bo.",
            "",
            SocialAct::None,
            cold,
            Some("Bo"),
            1.0,
        );
        let down = w.agent("Bo").unwrap().trust_of("Ada");

        assert!((up - 2.05).abs() < 1e-4, "{up}");
        assert!((down - 2.00).abs() < 1e-4, "{down}");
        assert!((up - down - 0.05).abs() < 1e-4, "{up} {down}");
    }

    /// Ada shares the well with Bo and Cor, trusting Cor most. Everyone else is
    /// moved away so the founding seed's random placements cannot join them.
    fn addressee_world() -> World {
        let mut w = World::founding(7);
        let names: Vec<String> = w.living().map(|a| a.name.clone()).collect();
        for name in names {
            w.agent_mut(&name).unwrap().place = Place::Woods;
        }
        w.agent_mut("Ada").unwrap().place = Place::Well;
        w.agent_mut("Bo").unwrap().place = Place::Well;
        w.agent_mut("Cor").unwrap().place = Place::Well;
        w.agent_mut("Ada").unwrap().set_trust("Bo", 1.0);
        w.agent_mut("Ada").unwrap().set_trust("Cor", 3.0);
        w
    }

    #[test]
    fn a_line_naming_one_present_villager_addresses_that_villager() {
        let w = addressee_world();
        let ada = w.agent("Ada").unwrap();
        // Naming the less-trusted villager still wins: the line decides, not
        // the card's preference.
        assert_eq!(
            addressee_from_line(&w, ada, "Bo, the well is low.", None).as_deref(),
            Some("Bo")
        );
        // Case-insensitive and whole-word: the name in lower case is a mention.
        assert_eq!(
            addressee_from_line(&w, ada, "cor, wait for me.", None).as_deref(),
            Some("Cor")
        );
    }

    #[test]
    fn a_line_naming_several_present_villagers_addresses_the_most_trusted() {
        let mut w = addressee_world();
        let ada = w.agent("Ada").unwrap();
        assert_eq!(
            addressee_from_line(&w, ada, "Bo, Cor, listen to me.", None).as_deref(),
            Some("Cor")
        );

        // Equal trust: the smaller name wins, so the choice is deterministic.
        w.agent_mut("Ada").unwrap().set_trust("Bo", 3.0);
        let ada = w.agent("Ada").unwrap();
        assert_eq!(
            addressee_from_line(&w, ada, "Cor and Bo, listen.", None).as_deref(),
            Some("Bo")
        );
    }

    #[test]
    fn a_name_inside_a_longer_word_is_not_a_mention() {
        let w = addressee_world();
        let ada = w.agent("Ada").unwrap();
        // "Bo" appears inside "body" and "nobody"; neither is the villager.
        assert_eq!(
            addressee_from_line(&w, ada, "Nobody moves a body today.", Some("Cor".into()))
                .as_deref(),
            Some("Cor")
        );
    }

    #[test]
    fn a_line_naming_nobody_keeps_the_cards_addressee() {
        let w = addressee_world();
        let ada = w.agent("Ada").unwrap();
        assert_eq!(
            addressee_from_line(&w, ada, "The well is low.", Some("Cor".into())).as_deref(),
            Some("Cor")
        );
    }

    #[test]
    fn the_cards_addressee_is_the_most_trusted_present_villager() {
        let w = addressee_world();
        let ada = w.agent("Ada").unwrap();
        let addressee = most_trusted_present(&w, ada);
        assert_eq!(addressee.as_deref(), Some("Cor"));

        let register = register_for_mood(ada.mood);
        let card = agent_card_with(&w, ada, "speak", register, addressee.as_deref(), None);
        assert!(card.contains("You are speaking directly to Cor"), "{card}");
        assert!(!card.contains("You are speaking directly to Bo"), "{card}");
    }

    #[test]
    fn a_villager_alone_has_no_card_addressee() {
        let mut w = World::founding(7);
        let names: Vec<String> = w.living().map(|a| a.name.clone()).collect();
        for name in names.iter().filter(|n| *n != "Ada") {
            w.agent_mut(name).unwrap().place = Place::Woods;
        }
        w.agent_mut("Ada").unwrap().place = Place::Well;
        let ada = w.agent("Ada").unwrap();
        assert_eq!(most_trusted_present(&w, ada), None);
    }

    #[test]
    fn an_accusation_reaches_the_rumor_threshold() {
        // The judge's ordinal never passed 2.21 in 45 lines, so the world's
        // rumor branch (intensity >= 3.2) was dead. The act's base is the
        // threshold; the tone can push it up or, only when cold, back down to
        // 3.0 — where the act's own rumor arm still fires.
        assert!(base_intensity(SocialAct::Accusation) >= 3.2);
        let plain = register_index("plain and neutral");
        let warm = register_index("warm and open");
        let urgent = register_index("urgent and raw");
        for register in [plain, warm, urgent] {
            assert!(
                derived_intensity(SocialAct::Accusation, register) >= 3.2,
                "register {register}"
            );
        }
        let cold = register_index("cold and clipped");
        assert!((derived_intensity(SocialAct::Accusation, cold) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn a_threat_reaches_the_morale_threshold() {
        // The morale branch (intensity >= 3.0) was unreachable for the same
        // reason; a threat's base is 3.4 and the coldest register only takes
        // 0.2 off, so every register clears it.
        for register in 0..=4 {
            assert!(
                derived_intensity(SocialAct::Threat, register) > 3.0,
                "register {register}"
            );
        }
    }

    #[test]
    fn the_register_shifts_the_intensity_by_the_stated_amount() {
        let plain = register_index("plain and neutral");
        let warm = register_index("warm and open");
        let urgent = register_index("urgent and raw");
        let cold = register_index("cold and clipped");

        let base = derived_intensity(SocialAct::Promise, plain);
        assert!((base - 2.6).abs() < 1e-6, "{base}");
        assert!((derived_intensity(SocialAct::Promise, warm) - base).abs() < 1e-6);
        assert!((derived_intensity(SocialAct::Promise, urgent) - base - 0.2).abs() < 1e-6);
        assert!((base - derived_intensity(SocialAct::Promise, cold) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn the_intensity_is_clamped_and_rounded() {
        assert!((clamp_intensity(6.0) - 4.0).abs() < 1e-6);
        assert!(clamp_intensity(-1.0).abs() < 1e-6);
        assert!((clamp_intensity(2.345) - 2.35).abs() < 1e-6);
        // Every act/register pair the derivation can produce stays in range.
        for register in 0..=4 {
            for social in [
                SocialAct::None,
                SocialAct::Promise,
                SocialAct::Gift,
                SocialAct::Accusation,
                SocialAct::Threat,
                SocialAct::Gossip,
                SocialAct::Plea,
                SocialAct::Gratitude,
                SocialAct::Counsel,
                SocialAct::TradeOffer,
                SocialAct::Blessing,
                SocialAct::Forgiveness,
            ] {
                let value = derived_intensity(social, register);
                assert!((0.0..=4.0).contains(&value), "{social:?} {register}");
                let rounded = (value * 100.0).round() / 100.0;
                assert!((rounded - value).abs() < 1e-5, "{value}");
            }
        }
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
    fn enriched_state_keeps_local_dialogue_while_the_card_drops_it() {
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
        assert!(card.contains("Unkept promise to Bo"));
        assert!(card.contains("Inner conflict"));
        // The chooser still sees the local dialogue in its state; the card no
        // longer quotes it, because quoted dialogue was the top copy source
        // (measured: 10 of 18 sampled lines with it, 0 of 18 without).
        assert_eq!(
            enriched["village_context"]["recent_place_dialogue"][0],
            "Turn 11 Bo: You promised me grain before the frost."
        );
        assert!(
            !card.contains("You promised me grain before the frost."),
            "{card}"
        );
        // The goal lives in the judgment state, not in the prompt.
        assert_eq!(enriched["current_goal"], "Secure winter grain");
        assert!(!card.contains("Secure winter grain"), "{card}");
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
    fn the_card_has_no_recent_dialogue_block() {
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
        // The card quoting a line is the copy loop: the model returns it, the
        // line is stored in memory, and the next card quotes it again.
        assert!(!card.contains("Recent speech at this place"), "{card}");
        assert!(!card.contains("The well is low."), "{card}");
    }

    #[test]
    fn the_card_still_carries_the_decision_as_prose_and_the_register() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "draw_water", 4, None, None);
        assert!(
            card.contains("You have decided to draw water from the well."),
            "{card}"
        );
        assert!(card.contains("Register: "), "{card}");
        assert!(card.contains(REGISTERS[4]), "{card}");
    }

    #[test]
    fn the_goal_is_not_in_the_card() {
        let w = card_world();
        let ada = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, ada, "draw_water", 4, None, None);
        assert!(!card.contains("Secure winter grain"), "{card}");
    }

    /// The copy sources as `step` assembles them for one villager.
    fn sources_for(w: &World, name: &str, decision: &str) -> CopySources {
        let a = w.agent(name).unwrap();
        let card = agent_card_with(w, a, decision, 4, None, None);
        CopySources::for_agent(w, a, &card)
    }

    #[test]
    fn a_card_decision_makes_its_words_a_copy_but_a_sentence_does_not() {
        // Card-level test, as the rule set specifies: the action phrase is also
        // a strict source, so the same sentence is caught there; what this test
        // pins is that the card alone does not swallow an elaboration.
        let card = vec!["You have decided to draw water from the well.".to_owned()];
        assert!(is_copy("draw water", &card));
        assert!(!is_copy("I will draw water from the well tomorrow", &card));
    }

    #[test]
    fn a_paraphrase_of_the_goal_is_not_a_copy() {
        let mut w = card_world();
        w.agent_mut("Ada").unwrap().current_goal =
            "keep the settlement stocked with timber and firewood".into();
        let copy = sources_for(&w, "Ada", "draw_water");
        // The goal is not in the card, so a line that echoes it copies nothing
        // the model saw. Keeping it as a source would drop legitimate lines.
        assert!(!copy
            .strict
            .iter()
            .any(|s| s.contains("keep the settlement")));
        assert!(!copy
            .phrases
            .iter()
            .any(|s| s.contains("keep the settlement")));
        assert!(!copy.copies("keep stocked"));
    }

    #[test]
    fn a_line_that_opens_with_the_goals_word_is_not_a_copy() {
        let mut w = card_world();
        w.agent_mut("Ada").unwrap().current_goal =
            "counsel the anxious and preserve village memory".into();
        let copy = sources_for(&w, "Ada", "draw_water");
        // The head-copy rule had the goal as its only source. With the goal out
        // of the card it would drop any legitimate line opening with "counsel".
        assert!(!copy.copies("counsel the anxious"));
    }

    #[test]
    fn a_villagers_own_previous_line_is_not_a_copy_any_more() {
        let mut w = card_world();
        w.agent_mut("Ada").unwrap().memories.push(Memory::new(
            11,
            "You said: \"The barley is mine to count.\"",
            0.9,
            MemoryKind::Own,
        ));
        w.push_event(Event {
            turn: 11,
            actor: "Ada".into(),
            place: Place::Market,
            kind: EventKind::Speech,
            action: "speak".into(),
            text: "The barley is mine to count.".into(),
            social: None,
            intensity: Some(2.0),
            grounded: Some(1.0),
            fidelity: Some(1.0),
            divine: false,
        });
        let a = w.agent("Ada").unwrap();
        let card = agent_card_with(&w, a, "draw_water", 4, None, None);
        let copy = CopySources::for_agent(&w, a, &card);
        // The sim card drops the villager's own spoken line and the engine no
        // longer adds it back, so repeating it copies nothing the model read.
        assert!(!card.contains("barley"), "{card}");
        assert!(!copy.strict.iter().any(|s| s.contains("barley")));
        assert!(!copy.phrases.iter().any(|s| s.contains("barley")));
        assert!(!copy.copies("The barley is mine to count."));
    }

    #[test]
    fn the_copy_sources_hold_the_strings_the_card_shows() {
        let mut w = card_world();
        w.agent_mut("Ada").unwrap().memories.push(Memory::new(
            0,
            "Odo gave you food.",
            0.75,
            MemoryKind::Witnessed,
        ));
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
        let copy = sources_for(&w, "Ada", "draw_water");
        // A card string is still a copy, and a memory line the card printed is
        // still a source...
        assert!(copy.copies("You have decided to draw water from the well."));
        assert!(copy.copies("Odo gave you food."));
        // ...while the three strings the card no longer shows are not.
        assert!(!copy.phrases.iter().any(|s| s == "Secure winter grain"));
        assert!(!copy.strict.iter().any(|s| s.contains("The well is low.")));
        assert!(!copy.copies("The well is low."));
    }

    /// The gate answer a dropped line never receives.
    fn empty_gate() -> Answer {
        Answer {
            answers: Map::new(),
            model: None,
            cost: 0.0,
            input_tokens: 0,
            latency_ms: 0,
            state: Value::Null,
            questions: Value::Null,
        }
    }

    #[test]
    fn a_dropped_line_is_committed_silent_and_discredited() {
        let mut w = World::founding(7);
        w.agent_mut("Ada").unwrap().place = Place::Well;
        w.agent_mut("Bo").unwrap().place = Place::Well;
        let trust_before = w.agent("Ada").unwrap().trust_of("Bo");

        let verdict = turn_verdict(&empty_gate(), "Ada", true, true, 2);
        let intent = Intent::parse("draw_water", Place::Well).unwrap();
        let outcome = apply(&mut w, "Ada", &intent, "", verdict.social);
        let event = w.events.last_mut().unwrap();
        event.fidelity = verdict.fidelity;
        let event_text = event.text.clone();
        let event_fidelity = event.fidelity;

        let record = agent_turn_record(
            "Ada".into(),
            &intent,
            &outcome,
            &verdict,
            Line {
                say: String::new(),
                why: String::new(),
                model: "a".into(),
                latency_ms: 0,
            },
            0.8,
            2,
            None,
            None,
            Value::Null,
        );

        assert_eq!(record.say, "");
        assert_eq!(record.fidelity, Some(0.0));
        assert_eq!(record.social, "discredited");
        assert_eq!(record.note.as_deref(), Some(DROPPED_LINE_NOTE));
        assert_eq!(record.action, "draw_water");
        // The drop note replaces the action's own note: the observer needs the
        // silence explained, not the water counted.
        assert_eq!(outcome.note.as_deref(), Some("+2 water"));
        assert_ne!(record.note, outcome.note);

        // The act happened, silently: empty event text and an own memory in
        // prose, not a line.
        assert!(record.accepted);
        assert_eq!(event_text, "");
        assert_eq!(event_fidelity, Some(0.0));
        assert!(w
            .agent("Ada")
            .unwrap()
            .memories
            .iter()
            .any(|m| m.text == "You draw water from the well."));
        assert!(!w
            .agent("Ada")
            .unwrap()
            .memories
            .iter()
            .any(|m| m.text.contains("You said:")));

        // Not credited: no speech reached Bo and no trust moved.
        assert!(!w
            .agent("Bo")
            .unwrap()
            .memories
            .iter()
            .any(|m| m.text.contains("said")));
        let trust_after = w.agent("Ada").unwrap().trust_of("Bo");
        assert!((trust_before - trust_after).abs() < f32::EPSILON);
    }
}
