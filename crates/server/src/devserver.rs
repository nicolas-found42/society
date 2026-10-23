//! Dev-server control API. This is how an agent (or you, from a second
//! terminal) drives and inspects a running society without touching the window.
//!
//! Bound to `127.0.0.1` only. Read-only endpoints are safe to poll; the
//! mutating ones are explicit and each one returns the resulting state hash so
//! a caller can tell a no-op from a real change.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use society_sim::{turn, Event, Place, World};

pub const DEFAULT_PORT: u16 = 8100;

#[derive(Clone, Debug)]
pub struct AppState {
    pub world: Arc<tokio::sync::Mutex<World>>,
    /// In-memory observer cache generation, including same-turn mutations.
    pub observer_revision: Arc<std::sync::atomic::AtomicU64>,
    pub run: Arc<tokio::sync::Mutex<Vec<turn::TurnRecord>>>,
    /// Where this village lives on disk. The only path the server knows.
    pub save_path: std::path::PathBuf,
    pub telemetry: Arc<crate::telemetry::Telemetry>,
    /// The last save failure, if any. Kept in memory for `/health`: the
    /// durable half of a save error lives in the audit log.
    pub save_error: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl AppState {
    /// Persist after a mutation. A failed save is reported: the caller's
    /// response says the world did not reach disk.
    ///
    /// # Errors
    ///
    /// Returns a `SaveError` if the world fails validation or cannot be written.
    pub fn save(&self, world: &World) -> Result<(), crate::persistence::SaveError> {
        self.observer_revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        crate::persistence::save(world, &self.save_path)
    }

    /// Append turn records, keeping the in-memory log bounded.
    pub async fn record(&self, recs: impl IntoIterator<Item = turn::TurnRecord>) {
        crate::push_bounded(&mut *self.run.lock().await, recs);
        self.observer_revision
            .fetch_add(1, std::sync::atomic::Ordering::Release);
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
        let started = std::time::Instant::now();
        match self.save(world) {
            Ok(()) => {
                let recovered = self.last_save_error().await.is_some();
                self.note_save_error(None).await;
                self.audit_saved(world, started, Some(recovered));
                Ok(())
            },
            Err(e) => {
                let msg = e.to_string();
                self.note_save_error(Some(msg)).await;
                self.audit_save_error(world, started, &e);
                Err(e)
            },
        }
    }

    /// Durable half of a successful save, shared by the async API path and the
    /// native observer so a recovery always has a matching failure row.
    fn audit_saved(&self, world: &World, started: std::time::Instant, recovered: Option<bool>) {
        self.audit(
            "state_saved",
            world.turn,
            json!({"recovered": recovered, "save_us": started.elapsed().as_micros(),
                "state": crate::village_digest(world), "event_count": world.events.len(),
                "halted": world.halted.is_some()}),
        );
    }

    /// Durable half of a failed save: the failing stage, never the OS text.
    fn audit_save_error(
        &self,
        world: &World,
        started: std::time::Instant,
        error: &crate::persistence::SaveError,
    ) {
        self.audit(
            "save_error",
            world.turn,
            json!({"stage": save_stage(error), "state": crate::village_digest(world),
                "event_count": world.events.len(), "save_us": started.elapsed().as_micros()}),
        );
    }

    /// Durably journal committed turns: fsynced JSONL next to the save, so
    /// history survives restarts and outlives the in-memory cap.
    ///
    /// Failures are reported to the caller.
    ///
    /// # Errors
    ///
    /// Returns a string when the journal cannot be appended to.
    pub fn journal(&self, seed: u64, recs: &[turn::TurnRecord]) -> Result<(), String> {
        let mut failure: Option<String> = None;
        for r in recs {
            if let Err(error) = crate::diagnostics::append_turn_context(
                &self.save_path,
                seed,
                r,
                &crate::diagnostics::build_version(),
                crate::persistence::SAVE_VERSION,
                self.telemetry.context(),
            ) {
                self.telemetry.outcome("journal", &Err(error.clone()));
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Durably audit one observer/shell event. The audit must never break a
    /// mutation, so it is best-effort: failures return as strings for the
    /// response body rather than failing the call.
    pub fn audit(&self, action: &str, turn: u64, detail: Value) {
        let mut context = self.telemetry.context();
        context
            .operation_id
            .get_or_insert_with(crate::telemetry::operation_id);
        self.telemetry.outcome(
            "audit",
            &crate::diagnostics::append_audit_context(
                &self.save_path,
                action,
                turn,
                detail,
                context,
            ),
        );
    }
    /// Native observers already hold the world lock; persist without awaiting
    /// another lock. Prose remains only in the existing save, never audit detail.
    #[must_use]
    pub fn save_observer(&self, world: &World, action: &str, kind: &str) -> bool {
        let started = std::time::Instant::now();
        let outcome = self.save(world);
        // The latch is a tokio mutex shared with `/health`, and this runs on the
        // UI thread under the world lock, so only `try_lock` is safe here. When
        // it is momentarily held the latch keeps its previous value and the
        // recovery is recorded as unknown instead of as a false negative.
        let recovered = self.save_error.try_lock().map_or(None, |mut slot| {
            let recovered = slot.is_some();
            *slot = outcome.as_ref().err().map(ToString::to_string);
            Some(recovered)
        });
        if let Err(e) = &outcome {
            self.audit_save_error(world, started, e);
        } else {
            self.audit_saved(world, started, recovered);
        }
        self.audit(
            action,
            world.turn,
            json!({"kind": kind, "saved": outcome.is_ok(),
            "state": crate::village_digest(world), "event_count": world.events.len()}),
        );
        outcome.is_ok()
    }

    /// Mark the pre-state before synchronous engine work, including failed steps.
    #[must_use]
    pub fn start_step(&self, world: &World, source: &str) -> std::time::Instant {
        let started = std::time::Instant::now();
        self.audit(
            "turn_started",
            world.turn,
            json!({"source": source, "state": crate::village_digest(world),
            "event_count": world.events.len()}),
        );
        started
    }

    /// Save and journal each completed step while the caller still owns the
    /// world lock. Journal failure cannot undo a committed turn or fail gameplay.
    /// The two files are independent: neither implies the other succeeded.
    ///
    /// # Errors
    /// Returns the save failure after attempting both evidence sinks.
    pub async fn commit_turn(
        &self,
        world: &World,
        rec: &turn::TurnRecord,
        source: &str,
        started: std::time::Instant,
    ) -> Result<(), crate::persistence::SaveError> {
        let save = self.save_and_track(world).await;
        let journal = self.journal(world.seed, std::slice::from_ref(rec));
        // The row's `turn` is the world state turn after the step; `record_turn`
        // names the committed turn, and the runbook joins on that field.
        self.audit("turn_completed", world.turn, json!({"source": source, "record_turn": rec.turn,
            "saved": save.is_ok(), "journaled": journal.is_ok(), "elapsed_us": started.elapsed().as_micros(),
            "state": crate::village_digest(world), "summary": rec.summary, "event_count": world.events.len()}));
        save
    }
}

/// The failing stage of a save, as a bounded audit category.
const fn save_stage(error: &crate::persistence::SaveError) -> &'static str {
    match error {
        crate::persistence::SaveError::Io { action, .. } => action,
        crate::persistence::SaveError::Corrupt { .. } => "corrupt",
        crate::persistence::SaveError::Version { .. } => "version",
        crate::persistence::SaveError::Unwritable { .. } => "validation",
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
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            observe_request,
        ))
        .with_state(state)
}

/// Bounded route categories for the audit. Known routes keep their own name so
/// the 404 of a missing villager or place is not confused with an unknown
/// endpoint; the raw path is never retained.
fn route_category(path: &str) -> &'static str {
    match path {
        "/turn" => "turn",
        "/reset" => "reset",
        "/inject" => "inject",
        "/halt" => "halt",
        "/health" | "/log" | "/diagnostics" | "/verify" | "/state" | "/events" | "/social" => {
            "read"
        },
        _ if path.starts_with("/agent/") => "agent",
        _ if path.starts_with("/places/") => "place",
        _ => "other",
    }
}

/// Retain mutations and errors; reads are deliberately unsampled only on error.
async fn observe_request(
    State(s): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let id = crate::telemetry::operation_id();
    let path = route_category(request.uri().path());
    let mutation = request.method() != axum::http::Method::GET;
    let started = std::time::Instant::now();
    crate::telemetry::OPERATION.scope(id.clone(), async {
        if mutation { s.audit("request_started", 0, json!({"route": path})); }
        let mut response = next.run(request).await;
        if mutation || response.status().is_client_error() || response.status().is_server_error() {
            s.audit("request_finished", 0, json!({"route": path, "status": response.status().as_u16(), "elapsed_us": started.elapsed().as_micros()}));
        }
        if let Ok(value) = id.parse() { response.headers_mut().insert("x-society-operation-id", value); }
        response
    }).await
}

/// Nothing here is a simulation fact; it reports that the server answers. The
/// cap and the save path are static configuration, so this is where an
/// observer tells "growth is slow" from "growth is blocked". The engine is
/// code in the process: there is no external dependency to probe, so `ok`
/// means the router is answering and nothing else.
async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let world = s.world.lock().await;
    let halted = world.halted.clone();
    let turn = world.turn;
    drop(world);
    let depth =
        crate::diagnostics::scan_journal(&s.save_path, 0, crate::diagnostics::MAX_QUERY_ENTRIES);
    let journal_oldest = depth.entries.iter().map(|e| e.turn).min();
    let journal_turns = depth.entries.len();
    let journal_bytes = crate::diagnostics::journal_bytes(&s.save_path);
    Json(json!({
        "ok": halted.is_none(),
        "engine": "code",
        "halted": halted,
        "population_cap": society_sim::POPULATION_CAP,
        "birth_cost": society_sim::BIRTH_COST,
        "build": crate::diagnostics::build_version(),
        "telemetry": s.telemetry.status(),
        "retention": {"segments": crate::diagnostics::RETAINED_SEGMENTS + 1, "journal_segment_bytes": crate::diagnostics::JOURNAL_ROTATE_BYTES, "audit_segment_bytes": crate::diagnostics::AUDIT_ROTATE_BYTES, "age_guarantee": false},
        "save_version": crate::persistence::SAVE_VERSION,
        "journal_version": crate::diagnostics::JOURNAL_VERSION,
        "save_path": s.save_path.display().to_string(),
        "log_cap": crate::TURN_LOG_CAP,
        // The reader caps at MAX_QUERY_ENTRIES, so a capped window reports the
        // oldest turn it kept; `journal_depth_limited` says the store is older.
        "journal_window_oldest_turn": journal_oldest,
        "journal_turns": journal_turns,
        "journal_bytes": journal_bytes,
        "journal_depth_limited": depth.truncated,
        "journal_quality": {"corrupt_lines": depth.corrupt_lines, "read_errors": depth.read_errors, "head_missing": depth.head_missing, "files_read": depth.files_read},
        "last_save_error": s.last_save_error().await,
        "turn": turn,
    }))
}

async fn state_snapshot(State(s): State<AppState>) -> Response {
    let w = s.world.lock().await;
    json_response(crate::observer::encode(&w))
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn social_snapshot(State(s): State<AppState>) -> Response {
    let w = s.world.lock().await;
    let bytes =
        serde_json::to_vec(&crate::observer::social_fabric(&w)).expect("the social serialises");
    json_response(bytes)
}

/// The bytes of a payload, as a JSON response.
///
/// The payload is written straight to the bytes by
/// [`crate::observer::encode`], so the response never holds a second copy of
/// the village.
fn json_response(bytes: Vec<u8>) -> Response {
    Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .expect("the response builds")
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn place_detail(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let w = s.world.lock().await;
    let Some((place, st)) = w
        .places
        .iter()
        .find(|(p, _)| p.id().eq_ignore_ascii_case(&id))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown place", "place": id})),
        )
            .into_response();
    };
    let view = crate::observer::place_view(
        st,
        *place,
        w.living()
            .filter(|a| a.place == *place)
            .map(crate::observer::AgentView::living)
            .collect(),
        crate::observer::recent_place_dialogue(&w, *place, 10),
    );
    let bytes = serde_json::to_vec(&view).expect("the place serializes");
    json_response(bytes)
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the world lock is held for the whole request on purpose"
)]
async fn agent_detail(State(s): State<AppState>, Path(name): Path<String>) -> Response {
    let w = s.world.lock().await;
    let Some(a) = w.agent(&name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown villager", "name": name})),
        )
            .into_response();
    };
    // The policy's decision factors for this villager's last recorded turn:
    // what the inspector's factors panel shows, and the observable answer to
    // "why did she do that".
    let run = s.run.lock().await;
    let extra = [("factors", last_factors(&run, &a.name))];
    let bytes = serde_json::to_vec(&crate::observer::AgentView::complete(a).with_extra(&extra))
        .expect("the agent serializes");
    json_response(bytes)
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
    #[serde(flatten)]
    filter: crate::diagnostics::Filter,
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "the run lock is held while the in-memory tail is merged on purpose"
)]
async fn log_tail(State(s): State<AppState>, Query(q): Query<LogQuery>) -> Json<Value> {
    let since = q.since_turn.unwrap_or(0);
    let limit = q.limit.unwrap_or(20);
    let evidence = crate::diagnostics::query_journal(&s.save_path, since, limit, &q.filter);
    let world = s.telemetry.context().world_id;
    // Only the current world's rows can have a live counterpart in `run`.
    // Membership, not the newest turn, decides what to merge: a sink that fails
    // for one turn and recovers leaves an interior gap, not a trailing one.
    let durable: Vec<u64> = evidence
        .entries
        .iter()
        .filter(|e| e.context.world_id == world)
        .map(|e| e.turn)
        .collect();
    let oldest_durable = durable.iter().min().copied();
    let crate::diagnostics::Evidence {
        entries,
        corrupt_lines,
        read_errors,
        head_missing,
        truncated: journal_truncated,
        files_read,
    } = evidence;
    let mut rows: Vec<turn::TurnRecord> = Vec::with_capacity(entries.len());
    let mut envelopes: Vec<Value> = Vec::with_capacity(entries.len());
    let mut mine: Vec<bool> = Vec::with_capacity(entries.len());
    for entry in entries {
        mine.push(entry.context.world_id == world);
        envelopes.push(journal_envelope(&entry));
        rows.push(entry.record);
    }
    // Time and identity filters cannot be applied to a memory-only row, so they
    // exclude the merge rather than silently ignoring it.
    if q.filter.is_empty() {
        let run = s.run.lock().await;
        for record in run.iter() {
            if record.turn < since || durable.contains(&record.turn) {
                continue;
            }
            // Only a scan that clipped older matching rows makes its oldest row
            // the window's lower bound; otherwise `since` is that bound and a
            // memory-only turn above it belongs in the response.
            if journal_truncated && oldest_durable.is_some_and(|oldest| record.turn < oldest) {
                continue;
            }
            // Insert before the first later row of this incarnation, or append,
            // so both a boundary turn and an interior gap keep the read order.
            let at = (0..rows.len())
                .find(|i| mine[*i] && rows[*i].turn > record.turn)
                .unwrap_or(rows.len());
            rows.insert(at, record.clone());
            envelopes.insert(at, Value::Null);
            mine.insert(at, true);
        }
    }
    let dropped = rows.len().saturating_sub(limit);
    if dropped > 0 {
        rows.drain(..dropped);
        envelopes.drain(..dropped);
    }
    let from_memory = envelopes.iter().filter(|e| e.is_null()).count();
    let from_journal = rows.len() - from_memory;
    let truncated = journal_truncated || dropped > 0;
    Json(json!({
        "turns": rows.len(),
        "memory_cap": crate::TURN_LOG_CAP,
        "from_memory": from_memory,
        "from_journal": from_journal,
        "rows": rows,
        "envelopes": envelopes,
        "envelope_scope": "one per row: the durable envelope, or null for a turn held only in memory",
        "evidence": {"corrupt_lines": corrupt_lines, "read_errors": read_errors,
            "head_missing": head_missing, "truncated": truncated, "files_read": files_read},
        "order": "append order; most recent matching tail",
        "telemetry": s.telemetry.status(),
    }))
}

/// The durable envelope around one turn record, without the record itself.
fn journal_envelope(e: &crate::diagnostics::JournalEntry) -> Value {
    json!({
        "journal_version": e.journal_version,
        "build": e.build,
        "save_version": e.save_version,
        "seed": e.seed,
        "turn": e.turn,
        "day": e.day,
        "wall_time": e.wall_time,
        "context": e.context,
    })
}

#[derive(Deserialize)]
struct DiagnosticsQuery {
    limit: Option<usize>,
    #[serde(flatten)]
    filter: crate::diagnostics::Filter,
}

async fn diagnostics_tail(
    State(s): State<AppState>,
    Query(q): Query<DiagnosticsQuery>,
) -> Json<Value> {
    let evidence = crate::diagnostics::query_audit(&s.save_path, q.limit.unwrap_or(100), &q.filter);
    Json(
        json!({"entries": evidence.entries, "corrupt_lines": evidence.corrupt_lines,
        "read_errors": evidence.read_errors, "head_missing": evidence.head_missing, "truncated": evidence.truncated,
        "files_read": evidence.files_read, "telemetry": s.telemetry.status()}),
    )
}

#[derive(Deserialize)]
struct VerifyQuery {
    since_turn: Option<u64>,
    limit: Option<usize>,
    #[serde(flatten)]
    filter: crate::diagnostics::Filter,
}

/// Compare a bounded journal slice against the current pure founding engine.
/// Evidence quality and comparison scope accompany every result.
async fn verify_replay(State(s): State<AppState>, Query(q): Query<VerifyQuery>) -> Json<Value> {
    let evidence = crate::diagnostics::query_journal(
        &s.save_path,
        q.since_turn.unwrap_or(0),
        q.limit.unwrap_or(200),
        &q.filter,
    );
    let checked = evidence.entries.len();
    let incomplete = evidence.corrupt_lines > 0
        || evidence.read_errors > 0
        || evidence.head_missing
        || evidence.truncated;
    let divergence = if incomplete {
        None
    } else {
        crate::diagnostics::verify_journal(&evidence.entries)
    };
    // A journal that stops before the live world has committed a turn the
    // engine never retained; the compared prefix still matches, so say so.
    // Only this incarnation's rows count, and only for an unfiltered read.
    let world_turn = s.world.lock().await.turn;
    let world_id = s.telemetry.context().world_id;
    // A clipped window or a read that starts after turn 0 cannot witness a hole
    // outside it, so it declines the claim instead of answering `true`.
    let complete_window = q.since_turn.unwrap_or(0) == 0 && !evidence.truncated;
    let journal_current = (q.filter.is_empty() && complete_window).then(|| {
        let mut turns: Vec<u64> = evidence
            .entries
            .iter()
            .filter(|e| e.context.world_id == world_id)
            .map(|e| e.turn)
            .collect();
        turns.sort_unstable();
        turns.dedup();
        // Reaching the live turn is not enough: a sink that failed for one
        // turn and recovered leaves a hole a max-only check calls complete.
        turns.last().is_some_and(|newest| *newest + 1 == world_turn)
            && turns.windows(2).all(|pair| pair[1] == pair[0] + 1)
    });
    Json(
        json!({"checked": checked, "evidence": evidence, "divergence": divergence,
        "world_turn": world_turn, "journal_current": journal_current,
        "status": if checked == 0 {"no_evidence"} else if incomplete {"incomplete_evidence"} else if divergence.is_some() {"baseline_diverged"} else {"baseline_matches"},
        "scope": "comparison with founding seed under CURRENT pure engine; excludes observer interventions and desktop civic systems; matching records do not prove historical state equivalence"}),
    )
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
    let waiting = std::time::Instant::now();
    let mut world = s.world.lock().await;
    s.audit(
        "world_lock_acquired",
        world.turn,
        json!({"wait_us": waiting.elapsed().as_micros()}),
    );
    if let Some(reason) = &world.halted {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "halted", "reason": reason })),
        );
    }
    // The engine is synchronous, in-process code, so a turn either completes
    // or reports its error. There is no hung daemon to fence with a budget.
    let mut records = Vec::new();
    let mut error = None;
    for _ in 0..n {
        let started = s.start_step(&world, "api");
        match turn::step(&mut world) {
            Ok(rec) => {
                records.push(rec.clone());
                // The turn function is pure, so the shell owns the save and the
                // journal. Per turn, the way the engine always did it: an
                // unwritten world is a reset waiting to happen.
                if let Err(e) = s.commit_turn(&world, &rec, "api", started).await {
                    error = Some(format!("turn {} committed but not saved: {e}", rec.turn));
                    break;
                }
            },
            Err(reason) => {
                s.audit("turn_failed", world.turn, json!({"source": "api", "halted": world.halted.is_some(), "elapsed_us": started.elapsed().as_micros()}));
                error = Some(reason);
                break;
            },
        }
    }
    let committed = records.len();
    let world_turn = world.turn;
    let telemetry = s.telemetry.status();
    if !records.is_empty() {
        s.record(records.iter().cloned()).await;
    }
    drop(world);
    if let Some(reason) = error {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": reason,
                "committed": committed,
                "telemetry": telemetry,
                "turn": world_turn,
            })),
        );
    }
    s.audit(
        "turns",
        world_turn,
        json!({"committed": committed, "telemetry": telemetry}),
    );
    (
        StatusCode::OK,
        Json(json!({
            "turns": committed,
            "turn": world_turn,
            "saved": !records.is_empty(),
            "telemetry": telemetry,
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
async fn reset(
    State(s): State<AppState>,
    Json(req): Json<ResetRequest>,
) -> (StatusCode, Json<Value>) {
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
    s.telemetry.reset(&s.save_path);
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
    let ok = saved.is_ok();
    let body = json!({
        "ok": ok,
        "seed": seed,
        "save_cleared": cleared,
        "saved": ok,
        "save_error": saved.err().map(|e| e.to_string()),
    });
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(body))
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
    let mut w = s.world.lock().await;
    let turn = w.turn;
    let summary = crate::diagnostics::inject_summary(
        &req.kind,
        req.target.as_deref().filter(|name| w.agent(name).is_some()),
        req.to.as_deref().filter(|name| w.agent(name).is_some()),
        // Match the case-insensitive lookup the event arms use, so the audit
        // names the place the intervention actually reached.
        req.place.as_deref().and_then(|id| {
            Place::ALL
                .into_iter()
                .find(|p| p.id().eq_ignore_ascii_case(id))
                .map(Place::id)
        }),
    );
    s.audit("inject_attempt", turn, summary.clone());
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
                spoken: false,
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
                spoken: false,
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
                spoken: false,
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
                spoken: false,
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
            // A feast writes the bias, not the level (ticket #3): settlement
            // folds it into the hope/stress baseline, so the lift survives
            // the turn and then decays instead of vanishing at once.
            w.morale_bias = (w.morale_bias + 0.15).clamp(-4.0, 4.0);
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
                // Feasts narrate; they speak nothing (ticket #4).
                spoken: false,
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
                spoken: false,
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
                // The only birth of an explicit goal (ticket #2): the next
                // refresh picks it up unless survival or a promise borrows
                // the turn, and neither clears it.
                a.adopt_explicit_goal(text.clone());
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
                spoken: false,
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
async fn halt(State(s): State<AppState>, Json(req): Json<Value>) -> (StatusCode, Json<Value>) {
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
            s.audit("halt", world_turn, json!({"observer_requested": true}));
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "halted": halted })),
            )
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "halted": halted, "save_error": e.to_string() })),
        ),
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
    fn observer_api_exposes_the_living_village_and_the_whole_memory_of_one_villager() {
        let mut w = World::founding(7);
        w.temperature_c = -8;
        w.village_morale = 1.25;
        w.agent_mut("Ada").unwrap().water = 7;
        w.agent_mut("Ada").unwrap().current_goal = "Find herbs".into();
        let value = serde_json::to_value(crate::observer::snapshot(&w)).expect("the snapshot");
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
        // Every serialized simulation field must survive the observer API,
        // except the claims a villager holds: those are the villager's own
        // history, and they are served by `/agent/{name}`.
        for key in serde_json::to_value(w.agent("Ada").unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
        {
            if key == "facts" {
                assert!(
                    ada.get(key).is_none(),
                    "the living village carries no claims"
                );
                continue;
            }
            assert!(ada.get(key).is_some(), "missing agent field {key}");
        }
        // The villager's own endpoint carries the whole memory.
        let whole = serde_json::to_value(crate::observer::AgentView::complete(
            w.agent("Ada").unwrap(),
        ))
        .expect("the agent serialises");
        assert!(
            whole.get("facts").is_some(),
            "the whole memory carries the claims"
        );
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
        let neutral = serde_json::to_value(crate::observer::social_fabric(&w)).expect("the social");
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

        let social = serde_json::to_value(crate::observer::social_fabric(&w)).expect("the social");
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
            observer_revision: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            world: Arc::new(tokio::sync::Mutex::new(World::founding(7))),
            run: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            telemetry: Arc::new(crate::telemetry::Telemetry::open(&save, true)),
            save_path: save.clone(),
            save_error: Arc::new(tokio::sync::Mutex::new(None)),
        };
        (state, root)
    }

    #[tokio::test]
    async fn observer_revision_tracks_failed_saves_and_history_publication() {
        let (state, root) = test_state("observer-revision");
        let before = state
            .observer_revision
            .load(std::sync::atomic::Ordering::Acquire);
        // A directory at the destination forces save failure after an in-memory mutation.
        std::fs::create_dir(&state.save_path).unwrap();
        {
            let mut world = state.world.lock().await;
            world.agent_mut("Ada").unwrap().water += 1;
            let failed = state.save(&world).is_err();
            drop(world);
            assert!(failed);
        }
        assert!(
            state
                .observer_revision
                .load(std::sync::atomic::Ordering::Acquire)
                > before
        );
        let before = state
            .observer_revision
            .load(std::sync::atomic::Ordering::Acquire);
        state.record(Vec::new()).await;
        assert!(
            state
                .observer_revision
                .load(std::sync::atomic::Ordering::Acquire)
                > before
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn audit_failure_is_visible_without_breaking_application_mutations() {
        let (s, root) = test_state("audit-failure");
        std::fs::create_dir(crate::diagnostics::audit_path(&s.save_path)).unwrap();
        let response = run_turns(State(s.clone()), Json(TurnRequest { turns: Some(1) }))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(s.telemetry.status()["write_failures"].as_u64().unwrap() > 0);
        assert_eq!(
            crate::persistence::load(&s.save_path)
                .unwrap()
                .unwrap()
                .turn,
            1
        );
        assert_eq!(
            crate::diagnostics::scan_journal(&s.save_path, 0, 10)
                .entries
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_journal_does_not_fail_a_saved_turn_and_reports_degradation() {
        let (s, root) = test_state("journal-failure");
        std::fs::create_dir(crate::diagnostics::journal_path(&s.save_path)).unwrap();
        let mut w = s.world.lock().await;
        let started = s.start_step(&w, "test");
        let rec = turn::step(&mut w).unwrap();
        s.commit_turn(&w, &rec, "test", started).await.unwrap();
        drop(w);
        assert_eq!(
            crate::persistence::load(&s.save_path)
                .unwrap()
                .unwrap()
                .turn,
            1
        );
        assert_eq!(s.telemetry.status()["write_failures"], 1);
        let evidence = crate::diagnostics::scan_audit(&s.save_path, 100);
        let outcome = evidence
            .entries
            .iter()
            .find(|e| e.action == "turn_completed")
            .unwrap();
        assert_eq!(outcome.detail["saved"], true);
        assert_eq!(outcome.detail["journaled"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn save_failure_retains_record_and_recovery_is_observable() {
        let (s, root) = test_state("save-failure");
        let mut w = s.world.lock().await;
        std::fs::create_dir(&s.save_path).unwrap();
        let started = s.start_step(&w, "test");
        let rec = turn::step(&mut w).unwrap();
        assert!(s.commit_turn(&w, &rec, "test", started).await.is_err());
        assert!(s.last_save_error().await.is_some());
        assert_eq!(
            crate::diagnostics::scan_journal(&s.save_path, 0, 10)
                .entries
                .len(),
            1
        );
        std::fs::remove_dir(&s.save_path).unwrap();
        s.save_and_track(&w).await.unwrap();
        drop(w);
        assert!(s.last_save_error().await.is_none());
        let evidence = crate::diagnostics::scan_audit(&s.save_path, 100);
        assert!(evidence
            .entries
            .iter()
            .any(|e| e.action == "state_saved" && e.detail["recovered"] == true));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_observer_save_survives_reload_without_a_later_turn() {
        let (s, root) = test_state("native-save");
        let mut w = World::founding(7);
        w.place_state_mut(Place::Granary).food += 19;
        assert!(s.save_observer(&w, "intervention", "test"));
        // The observer path must describe its persistence the same way the API
        // path does, so a recovery is still explainable from the audit log.
        let (rows, corrupt) = crate::diagnostics::read_audit(&s.save_path, 10);
        assert_eq!(corrupt, 0);
        assert!(rows.iter().any(|e| e.action == "state_saved"
            && e.detail["recovered"] == false
            && e.detail["halted"] == false));
        let loaded = crate::persistence::load(&s.save_path).unwrap().unwrap();
        assert_eq!(
            loaded.place_state(Place::Granary).food,
            w.place_state(Place::Granary).food
        );
        let id = s.telemetry.context().world_id;
        let resumed = crate::telemetry::Telemetry::open(&s.save_path, false);
        assert_eq!(resumed.context().world_id, id);
        resumed.reset(&s.save_path);
        assert_ne!(resumed.context().world_id, id);
        // A failed observer save is audited with its stage, not only a flag.
        std::fs::remove_file(&s.save_path).unwrap();
        std::fs::create_dir_all(&s.save_path).unwrap();
        assert!(!s.save_observer(&w, "intervention", "test"));
        let (rows, _) = crate::diagnostics::read_audit(&s.save_path, 10);
        assert!(rows.iter().any(|e| e.action == "save_error"
            && e.detail["stage"].is_string()
            && e.detail["stage"] != ""));
        assert!(s.save_error.try_lock().is_ok_and(|slot| slot.is_some()));
        std::fs::remove_dir_all(root).unwrap();
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

    #[cfg(unix)]
    #[tokio::test]
    async fn log_includes_a_turn_the_journal_missed() {
        // A degraded journal sink must not hide a committed turn: the record
        // is saved and still held in memory, so `/log` serves it with a null
        // envelope instead of reporting a shorter history.
        let (s, root) = test_state("log-memory-merge");
        let mut w = s.world.lock().await;
        let rec = turn::step(&mut w).unwrap();
        let seed = w.seed;
        s.commit_turn(&w, &rec, "test", s.start_step(&w, "test"))
            .await
            .unwrap();
        s.record([rec.clone()]).await;
        drop(w);
        // Make the next journal append fail without erasing the retained row.
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                crate::diagnostics::journal_path(&s.save_path),
                std::fs::Permissions::from_mode(0o400),
            )
            .unwrap();
        }
        let mut w = s.world.lock().await;
        let missed = turn::step(&mut w).unwrap();
        s.commit_turn(&w, &missed, "test", s.start_step(&w, "test"))
            .await
            .unwrap();
        s.record([missed.clone()]).await;
        drop(w);
        let body = log_tail(
            State(s.clone()),
            Query(LogQuery {
                since_turn: None,
                limit: Some(10),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(body["turns"], 2, "{body}");
        assert_eq!(body["from_memory"], 1, "{body}");
        assert_eq!(body["from_journal"], 1, "{body}");
        assert_eq!(body["rows"][1]["turn"], missed.turn);
        assert!(body["envelopes"][0].is_object(), "{body}");
        assert!(body["envelopes"][1].is_null(), "{body}");
        assert_eq!(seed, body["envelopes"][0]["seed"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    async fn step_once(s: &AppState) {
        let response = run_turns(State(s.clone()), Json(TurnRequest { turns: Some(1) }))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn log_serves_a_turn_an_interior_journal_gap_missed() {
        use std::os::unix::fs::PermissionsExt as _;
        // A sink that fails for one turn and recovers leaves a hole, not a
        // trailing gap; the committed turn is still in memory and still served.
        let (s, root) = test_state("log-gap");
        step_once(&s).await;
        let journal = crate::diagnostics::journal_path(&s.save_path);
        // Unwritable, not replaced: the existing row must survive the outage.
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o400)).unwrap();
        step_once(&s).await;
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
        step_once(&s).await;
        let body = log_tail(
            State(s.clone()),
            Query(LogQuery {
                since_turn: None,
                limit: Some(10),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(body["turns"], 3, "{body}");
        assert_eq!(body["from_memory"], 1, "{body}");
        assert_eq!(body["from_journal"], 2, "{body}");
        assert_eq!(body["rows"][1]["turn"], 1, "{body}");
        assert!(body["envelopes"][1].is_null(), "{body}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn log_since_turn_keeps_an_in_window_memory_turn() {
        use std::os::unix::fs::PermissionsExt as _;
        // `since_turn` bounds the window when the journal read did not clip older
        // rows, so a memory-only turn at or above it belongs in the response even
        // though it sits below the oldest durable row.
        let (s, root) = test_state("log-since");
        step_once(&s).await;
        let journal = crate::diagnostics::journal_path(&s.save_path);
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o400)).unwrap();
        step_once(&s).await;
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
        step_once(&s).await;
        let body = log_tail(
            State(s.clone()),
            Query(LogQuery {
                since_turn: Some(1),
                limit: Some(10),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(body["turns"], 2, "{body}");
        assert_eq!(body["from_memory"], 1, "{body}");
        assert_eq!(body["from_journal"], 1, "{body}");
        assert_eq!(body["rows"][0]["turn"], 1, "{body}");
        assert!(body["envelopes"][0].is_null(), "{body}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn log_keeps_the_clipped_tail_tighter_than_a_memory_turn() {
        use std::os::unix::fs::PermissionsExt as _;
        // When the journal read clipped older matching rows, its oldest row is the
        // window's lower bound: memory-only turns below it are outside the tail.
        let (s, root) = test_state("log-clipped");
        step_once(&s).await;
        let journal = crate::diagnostics::journal_path(&s.save_path);
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o400)).unwrap();
        step_once(&s).await;
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
        step_once(&s).await;
        let body = log_tail(
            State(s.clone()),
            Query(LogQuery {
                since_turn: None,
                limit: Some(1),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(body["turns"], 1, "{body}");
        assert_eq!(body["from_memory"], 0, "{body}");
        assert_eq!(body["rows"][0]["turn"], 2, "{body}");
        assert!(body["evidence"]["truncated"].as_bool().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn log_limit_smaller_than_the_memory_tail_still_answers() {
        // The response limit trims the oldest rows after the merge; the
        // in-memory count must describe what is served, not what was merged.
        let (s, root) = test_state("log-limit");
        std::fs::create_dir(crate::diagnostics::journal_path(&s.save_path)).unwrap();
        for _ in 0..3 {
            step_once(&s).await;
        }
        let body = log_tail(
            State(s.clone()),
            Query(LogQuery {
                since_turn: None,
                limit: Some(1),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(body["turns"], 1, "{body}");
        assert_eq!(body["from_memory"], 1, "{body}");
        assert_eq!(body["from_journal"], 0, "{body}");
        assert!(body["evidence"]["truncated"].as_bool().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn zero_turns_does_not_claim_a_save() {
        let (s, root) = test_state("zero-turns");
        let response = run_turns(State(s.clone()), Json(TurnRequest { turns: Some(0) }))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["saved"], false, "{body}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_reset_and_halt_answer_with_a_status() {
        let (s, root) = test_state("failed-mutations");
        std::fs::create_dir(&s.save_path).unwrap();
        let (status, body) = reset(State(s.clone()), Json(ResetRequest { seed: None })).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{:?}", body.0);
        assert_eq!(body.0["ok"], false);
        let (status, body) = halt(State(s.clone()), Json(serde_json::json!({}))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body:?}");
        assert_eq!(body.0["ok"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn missing_agent_and_place_answer_with_the_shared_error_shape() {
        let (s, root) = test_state("not-found-shape");
        let resp = agent_detail(State(s.clone()), Path("Nobody".into())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "unknown villager");
        let resp = place_detail(State(s.clone()), Path("nowhere".into())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "unknown place");
        std::fs::remove_dir_all(root).unwrap();
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
        s.audit(
            "inject",
            3,
            crate::diagnostics::inject_summary("gift", Some("Ada"), None, None),
        );
        let (entries, corrupt) = crate::diagnostics::read_audit(&s.save_path, 10);
        assert_eq!(corrupt, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "turns");
        assert_eq!(entries[1].detail["kind"], "gift");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn verify_declines_currency_for_a_windowed_read() {
        // Only a complete read can call the journal current: a window that starts
        // late or clips older rows cannot see a hole outside itself.
        let (s, root) = test_state("verify-window");
        for _ in 0..3 {
            step_once(&s).await;
        }
        let full = verify_replay(
            State(s.clone()),
            Query(VerifyQuery {
                since_turn: None,
                limit: None,
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert_eq!(full["journal_current"], true, "{full}");
        let clipped = verify_replay(
            State(s.clone()),
            Query(VerifyQuery {
                since_turn: None,
                limit: Some(1),
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert!(clipped["journal_current"].is_null(), "{clipped}");
        let late = verify_replay(
            State(s.clone()),
            Query(VerifyQuery {
                since_turn: Some(1),
                limit: None,
                filter: crate::diagnostics::Filter::default(),
            }),
        )
        .await
        .0;
        assert!(late["journal_current"].is_null(), "{late}");
        std::fs::remove_dir_all(root).unwrap();
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

    fn dialogue_event(
        w: &World,
        turn: u64,
        actor: &str,
        kind: society_sim::EventKind,
        action: &str,
        text: &str,
        spoken: bool,
    ) -> Event {
        Event {
            turn,
            actor: actor.into(),
            place: w.agent(actor).map_or(Place::Granary, |a| a.place),
            kind,
            action: action.into(),
            text: text.into(),
            spoken,
            social: None,
            intensity: None,
            grounded: None,
            divine: false,
        }
    }

    #[test]
    fn dialogue_excludes_narration_and_keeps_spoken_lines() {
        // Ticket #4: a mixed market chronicle (a spoken promise, a death, a
        // birth, an empty line, a rejection note) yields only the spoken
        // line, oldest first, capped by the limit.
        use society_sim::EventKind;
        let mut w = World::founding(4);
        let ada = w.agents[0].name.clone();
        let bo = w.agents[1].name.clone();
        let market = Place::Market;
        w.agent_mut(&ada).unwrap().place = market;
        w.agent_mut(&bo).unwrap().place = market;
        w.push_event(dialogue_event(
            &w,
            1,
            &ada,
            EventKind::Promise,
            "promise",
            "I will feed you.",
            true,
        ));
        w.push_event(dialogue_event(
            &w,
            2,
            &bo,
            EventKind::Death,
            "death",
            "Bo died of hunger.",
            false,
        ));
        w.push_event(dialogue_event(
            &w,
            3,
            &ada,
            EventKind::Birth,
            "birth",
            "Ash was born to Ada and Bo.",
            false,
        ));
        w.push_event(dialogue_event(
            &w,
            4,
            &ada,
            EventKind::Confusion,
            "give",
            "no food to give",
            false,
        ));
        w.push_event(dialogue_event(
            &w,
            5,
            &ada,
            EventKind::Transfer,
            "give",
            "   ",
            true,
        ));
        w.push_event(dialogue_event(
            &w,
            6,
            &bo,
            EventKind::System,
            "halt",
            "halted",
            false,
        ));
        let lines = crate::observer::recent_place_dialogue(&w, market, 10);
        assert_eq!(lines, vec![format!("Turn 1 {ada}: I will feed you.")]);
        // The cap counts spoken lines, not narrative: a limit of one still
        // returns the single spoken line even with six events behind it.
        assert_eq!(
            crate::observer::recent_place_dialogue(&w, market, 1).len(),
            1
        );
    }

    #[test]
    fn dialogue_counts_only_spoken_lines_within_the_limit() {
        // Ticket #4: the limit bounds eligible spoken lines, so two spoken
        // lines amid narration return the two newest-first-capped lines in
        // oldest-first order.
        use society_sim::EventKind;
        let mut w = World::founding(4);
        let ada = w.agents[0].name.clone();
        w.agent_mut(&ada).unwrap().place = Place::Market;
        for (t, text, spoken) in [
            (1_u64, "first words", true),
            (2, "a death in the market", false),
            (3, "second words", true),
            (4, "third words", true),
        ] {
            w.push_event(dialogue_event(
                &w,
                t,
                &ada,
                EventKind::Speech,
                "speak",
                text,
                spoken,
            ));
        }
        let lines = crate::observer::recent_place_dialogue(&w, Place::Market, 2);
        assert_eq!(
            lines,
            vec![
                format!("Turn 3 {ada}: second words"),
                format!("Turn 4 {ada}: third words"),
            ]
        );
    }
}
