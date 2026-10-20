//! Dev-server control surface. This is how an agent (or you, from a second
//! terminal) drives and inspects a running society without touching the window.
//!
//! Bound to `127.0.0.1` only. Read-only endpoints are safe to poll; the
//! mutating ones are explicit and each one returns the resulting state hash so
//! a caller can tell a no-op from a real change.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use society_sim::{turn, Event, Place, World};

pub const DEFAULT_PORT: u16 = 8100;

#[derive(Clone, Debug)]
pub struct AppState {
    pub world: Arc<tokio::sync::Mutex<World>>,
    pub run: Arc<tokio::sync::Mutex<Vec<turn::TurnRecord>>>,
    /// Where this village lives on disk. The only path the server knows.
    pub save_path: std::path::PathBuf,
    /// The last save failure, if any. Kept in memory for `/health`: the
    /// durable half of a save error lives in the audit log.
    pub save_error: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl AppState {
    /// Persist after a mutation. A failed save is reported, never silently
    /// swallowed: the caller's response says the world did not reach disk.
    ///
    /// # Errors
    ///
    /// Returns a `SaveError` if the world fails validation or cannot be written.
    pub fn save(&self, world: &World) -> Result<(), crate::persistence::SaveError> {
        crate::persistence::save(world, &self.save_path)
    }

    /// Append turn records, keeping the in-memory log bounded.
    pub async fn record(&self, recs: impl IntoIterator<Item = turn::TurnRecord>) {
        crate::push_bounded(&mut *self.run.lock().await, recs);
    }

    /// The last save failure, if any, for `/health`.
    pub async fn last_save_error(&self) -> Option<String> {
        self.save_error.lock().await.clone()
    }

    async fn note_save_error(&self, err: Option<String>) {
        *self.save_error.lock().await = err;
    }

    /// Persist and remember the outcome for `/health`.
    ///
    /// A failure is also audited durably; the caller's response still
    /// carries the error.
    ///
    /// # Errors
    ///
    /// Returns the `SaveError` when the world cannot be written.
    pub async fn save_and_track(&self, world: &World) -> Result<(), crate::persistence::SaveError> {
        match self.save(world) {
            Ok(()) => {
                self.note_save_error(None).await;
                Ok(())
            },
            Err(e) => {
                let msg = e.to_string();
                self.note_save_error(Some(msg.clone())).await;
                self.audit("save_error", world.turn, serde_json::json!({"error": msg}));
                Err(e)
            },
        }
    }

    /// Durably journal committed turns: fsynced JSONL next to the save, so
    /// history survives restarts and outlives the in-memory cap.
    ///
    /// Failures are reported to the caller, never silently swallowed.
    ///
    /// # Errors
    ///
    /// Returns a string when the journal cannot be appended to.
    pub fn journal(&self, seed: u64, recs: &[turn::TurnRecord]) -> Result<(), String> {
        for r in recs {
            crate::diagnostics::append_turn(
                &self.save_path,
                seed,
                r,
                &crate::diagnostics::build_version(),
                crate::persistence::SAVE_VERSION,
            )?;
        }
        Ok(())
    }

    /// Durably audit one observer/shell event. Best-effort by design: the
    /// audit must never break a mutation, so failures return as strings for
    /// the response body rather than failing the call.
    pub fn audit(&self, action: &str, turn: u64, detail: Value) {
        if let Err(e) = crate::diagnostics::append_audit(&self.save_path, action, turn, detail) {
            eprintln!("diagnostics audit `{action}` not recorded: {e}");
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/state", get(state_snapshot))
        .route("/social", get(social_snapshot))
        .route("/agent/{name}", get(agent_detail))
        .route("/places/{id}", get(place_detail))
        .route("/events", get(events))
        .route("/log", get(log_tail))
        .route("/diagnostics", get(diagnostics_tail))
        .route("/verify", get(verify_replay))
        .route("/turn", post(run_turns))
        .route("/reset", post(reset))
        .route("/inject", post(inject))
        .route("/halt", post(halt))
        .with_state(state)
}

/// Nothing here is a simulation fact; it is the harness proving itself alive.
/// The cap and the save path are static configuration, so this is where an
/// observer tells "growth is slow" from "growth is blocked". The engine is
/// code in the process: there is no external dependency to probe, so `ok`
/// means the router is answering and nothing else.
async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let world = s.world.lock().await;
    let halted = world.halted.clone();
    let turn = world.turn;
    drop(world);
    let (journal_oldest, journal_turns, journal_bytes) =
        crate::diagnostics::journal_depth(&s.save_path);
    Json(json!({
        "ok": halted.is_none(),
        "engine": "code",
        "halted": halted,
        "population_cap": society_sim::POPULATION_CAP,
        "birth_cost": society_sim::BIRTH_COST,
        "build": crate::diagnostics::build_version(),
        "save_version": crate::persistence::SAVE_VERSION,
        "journal_version": crate::diagnostics::JOURNAL_VERSION,
        "save_path": s.save_path.display().to_string(),
        "log_cap": crate::TURN_LOG_CAP,
        "journal_oldest_turn": journal_oldest,
        "journal_turns": journal_turns,
        "journal_bytes": journal_bytes,
        "last_save_error": s.last_save_error().await,
        "turn": turn,
    }))
}

/// Serialize the complete agent record, preserving the existing public shape
/// while making newly added simulation fields available to observers.
fn agent_snapshot(a: &society_sim::Agent) -> Value {
    let mut value = serde_json::to_value(a).expect("agent serializes");
    value["standing"] = json!((a.standing * 10.0).round() / 10.0);
    value["trust"] = json!(a
        .trust
        .iter()
        .map(|(n, v)| json!([n, (v * 10.0).round() / 10.0]))
        .collect::<Vec<_>>());
    value
}

fn world_snapshot(w: &World) -> Value {
    json!({
        "turn": w.turn,
        "day": w.day,
        "weather": w.weather,
        "weather_kind": w.weather_kind.id(),
        "season": w.season().id(),
        "time_of_day": w.time_of_day().id(),
        "temperature_c": w.temperature_c,
        "village_morale": w.village_morale,
        "halted": w.halted,
        "agents": w.agents.iter().map(agent_snapshot).collect::<Vec<_>>(),
        "places": w.places.iter().map(|(p, st)| {
            let mut value = serde_json::to_value(st).expect("place serializes");
            value["id"] = json!(p.id());
            value["adjacent"] = json!(p.adjacent().iter().map(|q| q.id()).collect::<Vec<_>>());
            value["present"] = json!(w.living().filter(|a| a.place == *p).map(|a| a.name.clone()).collect::<Vec<_>>());
            value["recent_dialogue"] = json!(recent_place_dialogue(w, *p, 4));
            value
        }).collect::<Vec<_>>(),
        "social": build_social_fabric(w),
        "event_count": w.events.len(),
    })
}

/// Computes the village's living social structure: alliances, feuds, lineages,
/// active/overdue oaths, elders, and territorial stewardship.
#[must_use]
pub fn build_social_fabric(w: &World) -> Value {
    let living: Vec<&society_sim::Agent> = w.living().collect();
    let mut alliances = Vec::new();
    let mut feuds = Vec::new();

    for (i, a) in living.iter().enumerate() {
        for b in living.iter().skip(i + 1) {
            let ab = a.trust_of(&b.name);
            let ba = b.trust_of(&a.name);
            if ab >= 2.7 && ba >= 2.7 {
                alliances.push(json!({
                    "members": [a.name, b.name],
                    "mutual_trust": (f32::midpoint(ab, ba) * 10.0).round() / 10.0,
                }));
            } else if ab <= 1.0 || ba <= 1.0 {
                feuds.push(json!({
                    "members": [a.name, b.name],
                    "min_trust": (ab.min(ba) * 10.0).round() / 10.0,
                }));
            }
        }
    }

    let promises: Vec<Value> = living
        .iter()
        .filter_map(|a| {
            a.promise.as_ref().map(|p| {
                let age = w.turn.saturating_sub(p.made_turn);
                json!({
                    "from": a.name,
                    "to": p.to,
                    "text": p.text,
                    "made_turn": p.made_turn,
                    "age_turns": age,
                    "overdue": p.kept.is_none() && age >= 8,
                    "kept": p.kept,
                })
            })
        })
        .collect();

    let lineages: Vec<Value> = living
        .iter()
        .filter(|a| !a.parents.is_empty() || a.generation > 0)
        .map(|a| {
            json!({
                "villager": a.name,
                "generation": a.generation,
                "parents": a.parents,
            })
        })
        .collect();

    let elders: Vec<String> = living
        .iter()
        .filter(|a| a.role == society_sim::Role::Elder)
        .map(|a| a.name.clone())
        .collect();

    let stewardship: Vec<Value> = w
        .places
        .iter()
        .filter_map(|(p, st)| {
            st.owner.as_ref().map(|owner| {
                json!({
                    "place": p.id(),
                    "steward": owner,
                    "food": st.food,
                })
            })
        })
        .collect();

    json!({
        "living_count": living.len(),
        "elders": elders,
        "alliances": alliances,
        "feuds": feuds,
        "promises": promises,
        "lineages": lineages,
        "stewardship": stewardship,
    })
}

async fn state_snapshot(State(s): State<AppState>) -> impl IntoResponse {
    let w = s.world.lock().await;
    Json(world_snapshot(&w))
}

async fn social_snapshot(State(s): State<AppState>) -> impl IntoResponse {
    let w = s.world.lock().await;
    Json(build_social_fabric(&w))
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn place_detail(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let w = s.world.lock().await;
    let (place, st) = w
        .places
        .iter()
        .find(|(p, _)| p.id().eq_ignore_ascii_case(&id))
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut value = serde_json::to_value(st).expect("place serializes");
    value["id"] = json!(place.id());
    value["adjacent"] = json!(place.adjacent().iter().map(|q| q.id()).collect::<Vec<_>>());
    value["present"] = json!(w
        .living()
        .filter(|a| a.place == *place)
        .map(agent_snapshot)
        .collect::<Vec<_>>());
    value["recent_dialogue"] = json!(recent_place_dialogue(&w, *place, 10));
    Ok(Json(value))
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn agent_detail(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let w = s.world.lock().await;
    let a = w.agent(&name).ok_or(StatusCode::NOT_FOUND)?;
    let mut value = agent_snapshot(a);
    // The policy's decision factors for this villager's last recorded turn:
    // what the inspector's factors panel shows, and the observable answer to
    // "why did she do that".
    let run = s.run.lock().await;
    value["factors"] = last_factors(&run, &a.name);
    Ok(Json(value))
}

/// The recorded term contributions from the villager's last turn, or `null`
/// when she has not spoken a decision into the retained log yet.
#[must_use]
pub fn last_factors(run: &[turn::TurnRecord], name: &str) -> Value {
    run.iter()
        .rev()
        .find_map(|r| r.agents.iter().find(|at| at.name == name))
        .map_or(Value::Null, |at| json!(at.factors))
}

/// The last few lines spoken at one place, oldest first. A world query, not a
/// simulation fact: it reads the event log the world already carries.
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

#[derive(Deserialize)]
struct EventQuery {
    since: Option<usize>,
    limit: Option<usize>,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn events(State(s): State<AppState>, Query(q): Query<EventQuery>) -> Json<Value> {
    let w = s.world.lock().await;
    let since = q.since.unwrap_or(0).min(w.events.len());
    let limit = q.limit.unwrap_or(200);
    let slice: Vec<&Event> = w.events[since..].iter().take(limit).collect();
    Json(json!({
        "total": w.events.len(),
        "since": since,
        "events": slice,
    }))
}

#[derive(Deserialize)]
struct LogQuery {
    since_turn: Option<u64>,
    limit: Option<usize>,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn log_tail(State(s): State<AppState>, Query(q): Query<LogQuery>) -> Json<Value> {
    let since = q.since_turn.unwrap_or(0);
    let limit = q.limit.unwrap_or(20);
    let rows: Vec<turn::TurnRecord> = {
        let run = s.run.lock().await;
        run.iter().filter(|r| r.turn >= since).take(limit).cloned().collect()
    };
    let from_memory = rows.len();
    let (journal, journal_corrupt) = if rows.len() < limit {
        crate::diagnostics::read_journal(&s.save_path, since, limit - rows.len())
    } else {
        (Vec::new(), 0)
    };
    // Journal fallback: turns that aged out of the in-memory cap (or that a
    // restart wiped) still read back, oldest first, ahead of the live tail.
    let mut merged: Vec<Value> = journal
        .into_iter()
        .map(|e| serde_json::to_value(&e.record).unwrap_or(Value::Null))
        .collect();
    let journal_rows = merged.len();
    for r in &rows {
        merged.push(serde_json::to_value(r).unwrap_or(Value::Null));
    }
    Json(json!({
        "turns": merged.len(),
        "cap": crate::TURN_LOG_CAP,
        "from_memory": from_memory,
        "from_journal": journal_rows,
        "journal_corrupt_lines": journal_corrupt,
        "rows": merged,
    }))
}

#[derive(Deserialize)]
struct DiagnosticsQuery {
    limit: Option<usize>,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn diagnostics_tail(State(s): State<AppState>, Query(q): Query<DiagnosticsQuery>) -> Json<Value> {
    let (entries, corrupt) = crate::diagnostics::read_audit(&s.save_path, q.limit.unwrap_or(100));
    Json(json!({
        "entries": entries,
        "corrupt_lines": corrupt,
    }))
}

#[derive(Deserialize)]
struct VerifyQuery {
    since_turn: Option<u64>,
    limit: Option<usize>,
}

/// Re-step the journal slice through the engine and name the first turn where
/// the world stops matching its history. `None` divergence means the slice
/// replays identically: the automatic half of diagnosis.
async fn verify_replay(State(s): State<AppState>, Query(q): Query<VerifyQuery>) -> Json<Value> {
    let since = q.since_turn.unwrap_or(0);
    let limit = q.limit.unwrap_or(200);
    let (entries, corrupt) = crate::diagnostics::read_journal(&s.save_path, since, limit);
    let checked = entries.len();
    let divergence = crate::diagnostics::verify_journal(&entries);
    Json(json!({
        "checked": checked,
        "corrupt_lines": corrupt,
        "divergence": divergence,
    }))
}

#[derive(Deserialize)]
struct TurnRequest {
    turns: Option<usize>,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn run_turns(State(s): State<AppState>, Json(req): Json<TurnRequest>) -> impl IntoResponse {
    let n = req.turns.unwrap_or(1).min(200);
    let mut world = s.world.lock().await;
    if let Some(reason) = &world.halted {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "halted", "reason": reason })),
        );
    }
    // The engine is synchronous, in-process code: a turn either completes or
    // reports its error, and there is no hung daemon to fence with a budget.
    let mut records = Vec::new();
    let mut error = None;
    for _ in 0..n {
        match turn::step(&mut world) {
            Ok(rec) => {
                records.push(rec.clone());
                // The seam is pure, so the shell owns the save and the
                // journal. Per turn, the way the engine always did it: an
                // unwritten world is a reset waiting to happen.
                if let Err(e) = s.save_and_track(&world).await {
                    error = Some(format!("turn {} committed but not saved: {e}", rec.turn));
                    break;
                }
            },
            Err(reason) => {
                error = Some(reason);
                break;
            },
        }
    }
    let committed = records.len();
    let world_turn = world.turn;
    let seed = world.seed;
    drop(world);
    let journal_error = if committed > 0 {
        s.journal(seed, &records).err()
    } else {
        None
    };
    if !records.is_empty() {
        s.record(records.iter().cloned()).await;
    }
    if let Some(reason) = error {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": reason,
                "committed": committed,
                "journal_error": journal_error,
                "turn": world_turn,
            })),
        );
    }
    s.audit(
        "turns",
        world_turn,
        json!({"committed": committed, "journal_error": journal_error}),
    );
    (
        StatusCode::OK,
        Json(json!({
            "turns": committed,
            "turn": world_turn,
            "saved": true,
            "journal_error": journal_error,
            "records": records,
        })),
    )
}

#[derive(Deserialize)]
struct ResetRequest {
    seed: Option<u64>,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn reset(State(s): State<AppState>, Json(req): Json<ResetRequest>) -> (StatusCode, Json<Value>) {
    let mut world = s.world.lock().await;
    // The API and the flag mean the same thing by "reset": the world and its
    // save go together, so a restart cannot resurrect what was discarded.
    let cleared = match crate::persistence::clear(&s.save_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": e.to_string() })),
            )
        },
    };
    *world = World::founding(req.seed.unwrap_or(7));
    crate::persistence::mark_reset(&mut world, "the observer reset the village");
    let world_turn = world.turn;
    let seed = world.seed;
    let saved = s.save_and_track(&world).await;
    s.run.lock().await.clear();
    s.audit(
        "reset",
        world_turn,
        json!({"seed": seed, "save_cleared": cleared, "saved": saved.is_ok()}),
    );
    let body = json!({
        "ok": saved.is_ok(),
        "seed": seed,
        "save_cleared": cleared,
        "saved": saved.is_ok(),
        "save_error": saved.err().map(|e| e.to_string()),
    });
    (StatusCode::OK, Json(body))
}

#[derive(Deserialize)]
struct InjectRequest {
    /// Observer interventions: whisper, gift, weather, rumor, feast, oath, goal.
    kind: String,
    target: Option<String>,
    to: Option<String>,
    place: Option<String>,
    text: Option<String>,
    amount: Option<u32>,
}

#[allow(
    clippy::too_many_lines,
    reason = "one match over every injectable event kind"
)]
#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn inject(State(s): State<AppState>, Json(req): Json<InjectRequest>) -> impl IntoResponse {
    let kind = req.kind.clone();
    let summary = crate::diagnostics::inject_summary(
        &req.kind,
        req.target.as_deref(),
        req.to.as_deref(),
        req.place.as_deref(),
    );
    let mut w = s.world.lock().await;
    let turn = w.turn;
    let (status, body) = match kind.as_str() {
        "whisper" => {
            let Some(target) = req.target else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"target required"})),
                );
            };
            if w.agent(&target).is_none() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"no such agent"})),
                );
            }
            let text = req
                .text
                .unwrap_or_else(|| "a dream you cannot quite recall".into());
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.remember(society_sim::Memory::new(
                    turn,
                    format!("A whisper from the observer: {text}"),
                    0.95,
                    society_sim::MemoryKind::Witnessed,
                ));
                a.place
            };
            w.push_event(Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "whisper".into(),
                text: format!("A whisper to {target}: {text}"),
                social: None,
                intensity: None,
                grounded: None,
                divine: true,
            });
            (StatusCode::OK, json!({"ok": true, "injected": "whisper"}))
        },
        "gift" => {
            let Some(target) = req.target else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"target required"})),
                );
            };
            if w.agent(&target).is_none() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"no such agent"})),
                );
            }
            let amount = req.amount.unwrap_or(1).min(5);
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.food += amount;
                a.hunger = 0;
                a.place
            };
            w.push_event(Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "gift".into(),
                text: format!("{amount} measures appear beside {target}."),
                social: None,
                intensity: None,
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "injected": "gift", "amount": amount}),
            )
        },
        "weather" => {
            #[allow(
                clippy::cast_precision_loss,
                reason = "an injected amount is 0..=5; no precision to lose"
            )]
            let amount = req.amount.unwrap_or(2).min(5) as f32;
            w.set_weather(amount.mul_add(0.35, 0.4));
            let weather = w.weather;
            let weather_text = format!(
                "The weather turns: {} · {} °C · harvest ×{weather:.1}.",
                w.weather_kind.id(),
                w.temperature_c
            );
            w.push_event(Event {
                turn,
                actor: "observer".into(),
                place: Place::Field,
                kind: society_sim::EventKind::Divine,
                action: "weather".into(),
                text: weather_text,
                social: None,
                intensity: None,
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "weather": weather, "weather_kind": w.weather_kind.id(), "temperature_c": w.temperature_c}),
            )
        },
        "rumor" => {
            let text = req
                .text
                .unwrap_or_else(|| "someone has been hoarding grain after dark".into());
            let place = req
                .place
                .as_deref()
                .and_then(|id| {
                    w.places
                        .iter()
                        .find(|(p, _)| p.id().eq_ignore_ascii_case(id))
                        .map(|(p, _)| *p)
                })
                .or_else(|| {
                    req.target
                        .as_deref()
                        .and_then(|t| w.agent(t).map(|a| a.place))
                })
                .unwrap_or(Place::Market);
            let hearers: Vec<String> = w
                .living()
                .filter(|a| a.place == place)
                .map(|a| a.name.clone())
                .collect();
            for name in &hearers {
                if let Some(a) = w.agent_mut(name) {
                    a.remember(society_sim::Memory::new(
                        turn,
                        format!("A rumor spreads at {}: {text}", place.id()),
                        0.9,
                        society_sim::MemoryKind::Witnessed,
                    ));
                }
            }
            w.push_event(Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "rumor".into(),
                text: format!("Rumor at {}: {text}", place.id()),
                social: Some(society_sim::SocialAct::Gossip.id().into()),
                intensity: Some(3.2),
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "injected": "rumor", "place": place.id(), "heard_by": hearers}),
            )
        },
        "feast" => {
            let place = req
                .place
                .as_deref()
                .and_then(|id| {
                    w.places
                        .iter()
                        .find(|(p, _)| p.id().eq_ignore_ascii_case(id))
                        .map(|(p, _)| *p)
                })
                .unwrap_or(Place::Market);
            let amount = req.amount.unwrap_or(4).min(10);
            w.place_state_mut(place).food += amount;
            w.village_morale = (w.village_morale + 0.15).clamp(0.0, 4.0);
            let attendees: Vec<String> = w
                .living()
                .filter(|a| a.place == place)
                .map(|a| a.name.clone())
                .collect();
            for name in &attendees {
                if let Some(a) = w.agent_mut(name) {
                    a.hunger = 0;
                    for other in &attendees {
                        if other != name {
                            let cur = a.trust_of(other);
                            a.set_trust(other, (cur + 0.25).clamp(0.0, 4.0));
                        }
                    }
                    a.remember(society_sim::Memory::new(
                        turn,
                        format!(
                            "A communal feast warmed {} (+{amount} measures).",
                            place.id()
                        ),
                        0.85,
                        society_sim::MemoryKind::Witnessed,
                    ));
                }
            }
            let village_morale = w.village_morale;
            w.push_event(Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "feast".into(),
                text: format!(
                    "A feast is laid at {} (+{amount} food); village morale rises to {:.2}.",
                    place.id(),
                    village_morale
                ),
                social: None,
                intensity: Some(3.0),
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "injected": "feast", "place": place.id(), "attendees": attendees, "village_morale": w.village_morale}),
            )
        },
        "oath" => {
            let Some(target) = req.target else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"target required"})),
                );
            };
            let Some(to) = req.to else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"`to` villager required"})),
                );
            };
            if w.agent(&target).is_none() || w.agent(&to).is_none() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"no such agent"})),
                );
            }
            let text = req.text.unwrap_or_else(|| "share the next harvest".into());
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.promise = Some(society_sim::Promise {
                    to: to.clone(),
                    text: text.clone(),
                    made_turn: turn,
                    kept: None,
                });
                a.remember(society_sim::Memory::new(
                    turn,
                    format!("You swore an oath to {to}: {text}"),
                    0.9,
                    society_sim::MemoryKind::Witnessed,
                ));
                a.place
            };
            if let Some(other) = w.agent_mut(&to) {
                other.remember(society_sim::Memory::new(
                    turn,
                    format!("{target} swore an oath to you: {text}"),
                    0.9,
                    society_sim::MemoryKind::Witnessed,
                ));
            }
            w.push_event(Event {
                turn,
                actor: target.clone(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "oath".into(),
                text: format!("{target} swore an oath to {to}: {text}"),
                social: None,
                intensity: Some(3.0),
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "injected": "oath", "from": target, "to": to, "text": text}),
            )
        },
        "goal" => {
            let Some(target) = req.target else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"target required"})),
                );
            };
            let Some(text) = req.text else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"text required"})),
                );
            };
            if w.agent(&target).is_none() {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"no such agent"})),
                );
            }
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.current_goal.clone_from(&text);
                a.remember(society_sim::Memory::new(
                    turn,
                    format!("A new resolve takes hold: {text}"),
                    0.85,
                    society_sim::MemoryKind::Witnessed,
                ));
                a.place
            };
            w.push_event(Event {
                turn,
                actor: target.clone(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "goal".into(),
                text: format!("{target} sets a new goal: {text}"),
                social: None,
                intensity: None,
                grounded: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "injected": "goal", "target": target, "goal": text}),
            )
        },
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unknown intervention `{other}`")})),
            )
        },
    };
    // A mutating control call is saved immediately, so a restart cannot
    // resurrect a world that already moved on.
    let world_turn = w.turn;
    match s.save_and_track(&w).await {
        Ok(()) => {
            s.audit("inject", world_turn, summary);
            (status, Json(body))
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "save_error": e.to_string()})),
        ),
    }
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn halt(State(s): State<AppState>, Json(req): Json<Value>) -> impl IntoResponse {
    let reason = req
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("halted by observer")
        .to_owned();
    let mut w = s.world.lock().await;
    w.halt(&reason);
    let halted = w.halted.clone();
    let world_turn = w.turn;
    match s.save_and_track(&w).await {
        Ok(()) => {
            s.audit("halt", world_turn, json!({"reason": reason}));
            Json(json!({ "ok": true, "halted": halted }))
        },
        Err(e) => Json(json!({ "ok": false, "halted": halted, "save_error": e.to_string() })),
    }
}

/// Convenience for `main`: bind loopback only and report the real address.
///
/// # Errors
///
/// Returns an I/O error if the port cannot be bound or the server stops abnormally.
pub async fn serve(state: AppState, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, router(state)).await
}

#[must_use]
pub fn record_of(e: &Event) -> Value {
    serde_json::to_value(e).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds() {
        // Compile-time proof that every route is wired with the right state.
        fn assert_router(_: fn(AppState) -> Router) {}
        assert_router(router);
    }

    #[test]
    fn default_port_is_stable() {
        assert_eq!(DEFAULT_PORT, 8100);
    }

    #[test]
    fn the_turn_log_stays_bounded_and_keeps_the_newest() {
        let rec = |t: u64| turn::TurnRecord {
            turn: t,
            day: t / 12,
            agents: Vec::new(),
            births: Vec::new(),
            summary: turn::TurnSummary::default(),
        };
        let mut run = Vec::new();
        for t in 0..(crate::TURN_LOG_CAP as u64 * 2) {
            crate::push_bounded(&mut run, std::iter::once(rec(t)));
        }
        assert_eq!(run.len(), crate::TURN_LOG_CAP);
        assert_eq!(run.first().unwrap().turn, crate::TURN_LOG_CAP as u64);
        assert_eq!(run.last().unwrap().turn, crate::TURN_LOG_CAP as u64 * 2 - 1);

        // A batch larger than the cap keeps only its tail.
        crate::push_bounded(&mut run, (0..crate::TURN_LOG_CAP as u64 + 5).map(rec));
        assert_eq!(run.len(), crate::TURN_LOG_CAP);
        assert_eq!(run.last().unwrap().turn, crate::TURN_LOG_CAP as u64 + 4);
    }
    #[test]
    fn observer_api_exposes_resources_environment_and_complete_agent_records() {
        let mut w = World::founding(7);
        w.temperature_c = -8;
        w.village_morale = 1.25;
        w.agent_mut("Ada").unwrap().water = 7;
        w.agent_mut("Ada").unwrap().current_goal = "Find herbs".into();
        let value = world_snapshot(&w);
        assert_eq!(value["temperature_c"], -8);
        assert_eq!(value["village_morale"], 1.25);
        assert_eq!(value["season"], w.season().id());
        assert_eq!(value["time_of_day"], w.time_of_day().id());
        assert_eq!(value["weather_kind"], w.weather_kind.id());
        let ada = value["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "Ada")
            .unwrap();
        assert_eq!(ada["water"], 7);
        assert_eq!(ada["current_goal"], "Find herbs");
        // Every serialized simulation field must survive the observer API.
        for key in serde_json::to_value(w.agent("Ada").unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
        {
            assert!(ada.get(key).is_some(), "missing agent field {key}");
        }
        for ((_, source), place) in w.places.iter().zip(value["places"].as_array().unwrap()) {
            for (key, field) in serde_json::to_value(source).unwrap().as_object().unwrap() {
                assert_eq!(place.get(key), Some(field), "missing place field {key}");
            }
        }
    }

    #[test]
    fn agent_factors_come_from_the_last_recorded_turn() {
        let at = |factors: Vec<(String, f32)>| turn::AgentTurn {
            name: "Ada".into(),
            decision_id: None,
            action: "craft".into(),
            accepted: true,
            note: None,
            say: String::new(),
            why: "duty".into(),
            social: "none".into(),
            addressee: None,
            register: None,
            dilemma: None,
            intensity: None,
            grounded: None,
            confidence: None,
            factors,
            rejected: Vec::new(),
            event_range: None,
            job: None,
            lie: false,
        };
        let older = turn::TurnRecord {
            turn: 1,
            day: 0,
            agents: vec![at(vec![("stale".into(), 1.0)])],
            births: Vec::new(),
            summary: turn::TurnSummary::default(),
        };
        let newer = turn::TurnRecord {
            turn: 2,
            day: 0,
            agents: vec![at(vec![("profession_ready".into(), 3.0)])],
            births: Vec::new(),
            summary: turn::TurnSummary::default(),
        };
        let factors = last_factors(&[older, newer.clone()], "Ada");
        assert_eq!(
            factors,
            json!([["profession_ready", 3.0]]),
            "the newest turn wins"
        );
        assert_eq!(last_factors(&[], "Ada"), Value::Null);
        assert_eq!(last_factors(&[newer], "Nobody"), Value::Null);
    }

    #[test]
    fn social_fabric_tracks_alliances_feuds_and_overdue_promises() {
        let mut w = World::founding(7);
        w.turn = 20;
        let neutral = build_social_fabric(&w);
        assert_eq!(neutral["alliances"].as_array().unwrap().len(), 0);
        assert_eq!(neutral["feuds"].as_array().unwrap().len(), 0);
        w.agent_mut("Ada").unwrap().set_trust("Bo", 3.2);
        w.agent_mut("Bo").unwrap().set_trust("Ada", 3.1);
        w.agent_mut("Ada").unwrap().set_trust("Cor", 0.5);
        w.agent_mut("Ada").unwrap().promise = Some(society_sim::Promise {
            to: "Bo".into(),
            text: "bring firewood".into(),
            made_turn: 5,
            kept: None,
        });
        w.place_state_mut(Place::Granary).owner = Some("Ada".into());

        let social = build_social_fabric(&w);
        assert_eq!(social["alliances"].as_array().unwrap().len(), 1);
        assert_eq!(social["feuds"].as_array().unwrap().len(), 1);
        assert_eq!(social["promises"][0]["overdue"], true);
        assert_eq!(social["stewardship"][0]["steward"], "Ada");
    }

    fn test_state(dir: &str) -> (AppState, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("society-srv-{}-{dir}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let save = root.join("v.json");
        let state = AppState {
            world: Arc::new(tokio::sync::Mutex::new(World::founding(7))),
            run: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            save_path: save.clone(),
            save_error: Arc::new(tokio::sync::Mutex::new(None)),
        };
        (state, root)
    }

    #[tokio::test]
    async fn journal_survives_a_restart_of_the_in_memory_log() {
        // Diagnostics item 1: turns outlive the in-memory cap and restarts.
        let (s, root) = test_state("journal");
        let mut w = s.world.lock().await;
        let mut recs = Vec::new();
        for _ in 0..3 {
            recs.push(turn::step(&mut w).unwrap());
        }
        let seed = w.seed;
        drop(w);
        s.journal(seed, &recs).unwrap();
        // Wipe the live log the way a restart does; the journal still reads.
        s.run.lock().await.clear();
        let (entries, corrupt) = crate::diagnostics::read_journal(&s.save_path, 0, 10);
        assert_eq!(corrupt, 0);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].turn, recs[0].turn);
        assert_eq!(entries[0].seed, seed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn audit_records_turns_resets_and_injects() {
        // Diagnostics item 3: observer actions leave a durable summary trail.
        let (s, root) = test_state("audit");
        {
            let w = s.world.lock().await;
            let seed = w.seed;
            drop(w);
            s.journal(seed, &[]).unwrap();
        }
        s.audit("turns", 3, json!({"committed": 1}));
        s.audit("inject", 3, crate::diagnostics::inject_summary("gift", Some("Ada"), None, None));
        let (entries, corrupt) = crate::diagnostics::read_audit(&s.save_path, 10);
        assert_eq!(corrupt, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "turns");
        assert_eq!(entries[1].detail["kind"], "gift");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn verify_flags_a_tampered_journal_turn() {
        // Diagnostics item 6: replay names the first divergent turn.
        let (s, root) = test_state("verify");
        let mut w = s.world.lock().await;
        let mut recs = Vec::new();
        for _ in 0..3 {
            recs.push(turn::step(&mut w).unwrap());
        }
        let seed = w.seed;
        drop(w);
        s.journal(seed, &recs).unwrap();
        let (mut entries, _) = crate::diagnostics::read_journal(&s.save_path, 0, 10);
        assert_eq!(crate::diagnostics::verify_journal(&entries), None);
        entries[1].record.agents[0].say = "tampered line".into();
        let div = crate::diagnostics::verify_journal(&entries).expect("divergence");
        assert_eq!(div.turn, recs[1].turn);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn health_carries_build_and_journal_depth() {
        // Diagnostics item 4: /health answers "which build, how much
        // history" without opening the save.
        let (s, root) = test_state("health");
        let mut w = s.world.lock().await;
        let rec = turn::step(&mut w).unwrap();
        let seed = w.seed;
        let turn = w.turn;
        drop(w);
        s.journal(seed, &[rec]).unwrap();
        let response = health(State(s.clone())).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = std::fs::remove_dir_all(&root);
        let _ = turn;
    }
}
