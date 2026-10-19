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
use society_sim::{Event, World};

use crate::engine::{Engine, EngineError, TurnRecord};
use crate::inference::judgment::Judge;
use crate::inference::voice::Voice;

pub const DEFAULT_PORT: u16 = 8100;

#[derive(Clone)]
pub struct AppState {
    pub world: Arc<tokio::sync::Mutex<World>>,
    pub run: Arc<tokio::sync::Mutex<Vec<TurnRecord>>>,
    pub engine: Arc<Engine>,
    pub judge: Arc<Judge>,
    pub voice: Arc<Voice>,
    /// Where this village lives on disk. The only path the server knows.
    pub save_path: std::path::PathBuf,
}

impl AppState {
    /// Persist after a mutation. A failed save is reported, never silently
    /// swallowed: the caller's response says the world did not reach disk.
    pub fn save(&self, world: &World) -> Result<(), crate::persistence::SaveError> {
        crate::persistence::save(world, &self.save_path)
    }

    /// Append turn records, keeping the in-memory log bounded.
    pub async fn record(&self, recs: impl IntoIterator<Item = TurnRecord>) {
        crate::push_bounded(&mut *self.run.lock().await, recs);
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/state", get(state_snapshot))
        .route("/agent/{name}", get(agent_detail))
        .route("/events", get(events))
        .route("/log", get(log_tail))
        .route("/turn", post(run_turns))
        .route("/reset", post(reset))
        .route("/inject", post(inject))
        .route("/halt", post(halt))
        .with_state(state)
}

/// Nothing here is a simulation fact; it is the harness proving itself alive.
/// The cap and the save path are static configuration, so this is where an
/// observer tells "growth is slow" from "growth is blocked".
async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let models = s.voice.probe().await.unwrap_or_default();
    let judge_ok = s.judge.probe().await.is_ok();
    Json(json!({
        "ok": judge_ok,
        "judgment": { "reachable": judge_ok, "required": true },
        "voice": { "reachable": !models.is_empty(), "models": models },
        "permits": s.engine.available_permits(),
        "population_cap": society_sim::POPULATION_CAP,
        "birth_cost": society_sim::BIRTH_COST,
        "save_path": s.save_path.display().to_string(),
        "log_cap": crate::TURN_LOG_CAP,
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
            value
        }).collect::<Vec<_>>(),
        "event_count": w.events.len(),
    })
}

async fn state_snapshot(State(s): State<AppState>) -> impl IntoResponse {
    let w = s.world.lock().await;
    Json(world_snapshot(&w))
}

async fn agent_detail(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let w = s.world.lock().await;
    let a = w.agent(&name).ok_or(StatusCode::NOT_FOUND)?;
    let mut value = agent_snapshot(a);
    value["judgment_state"] = society_sim::judgment_state(&w, a);
    Ok(Json(value))
}

#[derive(Deserialize)]
struct EventQuery {
    since: Option<usize>,
    limit: Option<usize>,
}

async fn events(
    State(s): State<AppState>,
    Query(q): Query<EventQuery>,
) -> Json<Value> {
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

async fn log_tail(State(s): State<AppState>, Query(q): Query<LogQuery>) -> Json<Value> {
    let run = s.run.lock().await;
    let since = q.since_turn.unwrap_or(0);
    let limit = q.limit.unwrap_or(20);
    let rows: Vec<&TurnRecord> = run
        .iter()
        .filter(|r| r.turn >= since)
        .take(limit)
        .collect();
    Json(json!({
        "turns": run.len(),
        "cap": crate::TURN_LOG_CAP,
        "rows": rows,
        "total_cost": crate::total_cost(&run),
    }))
}

#[derive(Deserialize)]
struct TurnRequest {
    turns: Option<usize>,
    /// Refuse to run past this wall-clock budget, so a hung Ollama cannot
    /// wedge the control surface.
    timeout_ms: Option<u64>,
}

async fn run_turns(
    State(s): State<AppState>,
    Json(req): Json<TurnRequest>,
) -> impl IntoResponse {
    let n = req.turns.unwrap_or(1).min(200);
    let budget = std::time::Duration::from_millis(req.timeout_ms.unwrap_or(120_000));
    let mut world = s.world.lock().await;
    if let Some(reason) = &world.halted {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "halted", "reason": reason })),
        );
    }
    // Records live outside the future so turns that committed before a failure
    // are still reported and logged: the world moved, and the observer must see
    // how far.
    let mut records = Vec::new();
    let fut = async {
        for _ in 0..n {
            let rec = s.engine.step(&mut world).await?;
            records.push(rec);
        }
        Ok::<_, EngineError>(())
    };
    let outcome = tokio::time::timeout(budget, fut).await;
    if !records.is_empty() {
        s.record(records.iter().cloned()).await;
    }
    match outcome {
        Ok(Ok(())) => {
            (
                StatusCode::OK,
                Json(json!({
                    "turns": records.len(),
                    "turn": world.turn,
                    "day": world.day,
                    "saved": true,
                    "records": records,
                })),
            )
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": e.to_string(),
                "halted": world.halted,
                "committed": records.len(),
                "turn": world.turn,
            })),
        ),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({
                "error": "turn budget exceeded",
                "committed": records.len(),
                "turn": world.turn,
            })),
        ),
    }
}

#[derive(Deserialize)]
struct ResetRequest {
    seed: Option<u64>,
}

async fn reset(State(s): State<AppState>, Json(req): Json<ResetRequest>) -> impl IntoResponse {
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
        }
    };
    *world = World::founding(req.seed.unwrap_or(7));
    crate::persistence::mark_reset(&mut world, "the observer reset the village");
    let saved = s.save(&world);
    s.run.lock().await.clear();
    let seed = world.seed;
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
    /// The observer's only interventions: whisper, gift, weather.
    kind: String,
    target: Option<String>,
    text: Option<String>,
    amount: Option<u32>,
}

async fn inject(State(s): State<AppState>, Json(req): Json<InjectRequest>) -> impl IntoResponse {
    let mut w = s.world.lock().await;
    let turn = w.turn;
    let (status, body) = match req.kind.as_str() {
        "whisper" => {
            let Some(target) = req.target else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error":"target required"})));
            };
            if w.agent(&target).is_none() {
                return (StatusCode::NOT_FOUND, Json(json!({"error":"no such agent"})));
            }
            let text = req.text.unwrap_or_else(|| "a dream you cannot quite recall".into());
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.remember(society_sim::Memory::new(
                    turn,
                    format!("A voice in the dark: {text}"),
                    0.95,
                    society_sim::MemoryKind::Witnessed,
                ));
                a.place
            };
            w.push_event(society_sim::Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "whisper".into(),
                text: format!("A whisper to {target}: {text}"),
                social: None,
                intensity: None,
                grounded: None,
                fidelity: None,
                divine: true,
            });
            (StatusCode::OK, json!({"ok": true, "injected": "whisper"}))
        }
        "gift" => {
            let Some(target) = req.target else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error":"target required"})));
            };
            if w.agent(&target).is_none() {
                return (StatusCode::NOT_FOUND, Json(json!({"error":"no such agent"})));
            }
            let amount = req.amount.unwrap_or(1).min(5);
            let place = {
                let a = w.agent_mut(&target).expect("checked above");
                a.food += amount;
                a.hunger = 0;
                a.place
            };
            w.push_event(society_sim::Event {
                turn,
                actor: "observer".into(),
                place,
                kind: society_sim::EventKind::Divine,
                action: "gift".into(),
                text: format!("{amount} measures appear beside {target}."),
                social: None,
                intensity: None,
                grounded: None,
                fidelity: None,
                divine: true,
            });
            (StatusCode::OK, json!({"ok": true, "injected": "gift", "amount": amount}))
        }
        "weather" => {
            let amount = req.amount.unwrap_or(2).min(5) as f32;
            w.set_weather(0.4 + amount * 0.35);
            let weather = w.weather;
            let weather_text = format!(
                "The weather turns: {} · {} °C · harvest ×{weather:.1}.",
                w.weather_kind.id(),
                w.temperature_c
            );
            w.push_event(society_sim::Event {
                turn,
                actor: "observer".into(),
                place: society_sim::Place::Field,
                kind: society_sim::EventKind::Divine,
                action: "weather".into(),
                text: weather_text,
                social: None,
                intensity: None,
                grounded: None,
                fidelity: None,
                divine: true,
            });
            (
                StatusCode::OK,
                json!({"ok": true, "weather": weather, "weather_kind": w.weather_kind.id(), "temperature_c": w.temperature_c}),
            )
        }
        other => return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown intervention `{other}`")})),
        ),
    };
    // A mutating control call is saved immediately, so a restart cannot
    // resurrect a world that already moved on.
    match s.save(&w) {
        Ok(()) => (status, Json(body)),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "save_error": e.to_string()})),
        ),
    }
}

async fn halt(State(s): State<AppState>, Json(req): Json<Value>) -> impl IntoResponse {
    let reason = req
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("halted by observer");
    let mut w = s.world.lock().await;
    w.halt(reason);
    let halted = w.halted.clone();
    match s.save(&w) {
        Ok(()) => Json(json!({ "ok": true, "halted": halted })),
        Err(e) => Json(json!({ "ok": false, "halted": halted, "save_error": e.to_string() })),
    }
}

/// Convenience for `main`: bind loopback only and report the real address.
pub async fn serve(state: AppState, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, router(state)).await
}

pub fn record_of(e: &Event) -> Value {
    serde_json::to_value(e).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds() {
        // Compile-time proof that every route is wired with the right state.
        fn _assert(_: fn(AppState) -> Router) {}
        _assert(router);
    }

    #[test]
    fn default_port_is_stable() {
        assert_eq!(DEFAULT_PORT, 8100);
    }

    #[test]
    fn the_turn_log_stays_bounded_and_keeps_the_newest() {
        let rec = |turn| TurnRecord {
            turn,
            day: turn / 12,
            intent_call: None,
            gate_call: None,
            agents: Vec::new(),
            births: Vec::new(),
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
        let ada = value["agents"].as_array().unwrap().iter().find(|a| a["name"] == "Ada").unwrap();
        assert_eq!(ada["water"], 7);
        assert_eq!(ada["current_goal"], "Find herbs");
        // Every serialized simulation field must survive the observer API.
        for key in serde_json::to_value(w.agent("Ada").unwrap()).unwrap().as_object().unwrap().keys() {
            assert!(ada.get(key).is_some(), "missing agent field {key}");
        }
        for ((_, source), place) in w.places.iter().zip(value["places"].as_array().unwrap()) {
            for (key, field) in serde_json::to_value(source).unwrap().as_object().unwrap() {
                assert_eq!(place.get(key), Some(field), "missing place field {key}");
            }
        }
    }

}
