//! Library half of the binary: wiring shared by the window and the headless
//! runner. The engine is the pure simulation crate; this crate is the shell
//! around it: persistence, the control surface, the window.

// `App`-level wiring and JSON snapshots are bags of small fields; splitting
// them hides the shapes the control surface publishes.
#![allow(
    clippy::struct_field_names,
    reason = "snapshot structs mirror the published API shapes"
)]

pub mod devserver;
pub mod diagnostics;
pub mod persistence;

pub use devserver::{AppState, DEFAULT_PORT, build_social_fabric};
pub use society_sim::turn::{AgentTurn, BirthRecord, TurnRecord, TurnSummary};

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use society_sim::World;

/// Turns kept in memory for `/log` and the feed. A village left running for
/// days must not grow memory without limit.
pub const TURN_LOG_CAP: usize = 240;

/// High-level snapshot of village vitals for UI headers and headless monitors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VillageDigest {
    pub turn: u64,
    pub day: u64,
    pub season: String,
    pub time_of_day: String,
    pub weather_kind: String,
    pub temperature_c: i32,
    pub morale: f32,
    pub living: usize,
    pub hungry: usize,
    pub total_food_carried: u32,
    pub total_place_food: u32,
    pub active_promises: usize,
}

/// Compute a concise digest of the settlement's survival and social state.
#[must_use]
pub fn village_digest(w: &World) -> VillageDigest {
    let living_agents: Vec<_> = w.living().collect();
    VillageDigest {
        turn: w.turn,
        day: w.day,
        season: w.season().id().to_owned(),
        time_of_day: w.time_of_day().id().to_owned(),
        weather_kind: w.weather_kind.id().to_owned(),
        temperature_c: w.temperature_c,
        morale: w.village_morale,
        living: living_agents.len(),
        hungry: living_agents.iter().filter(|a| a.hunger > 0).count(),
        total_food_carried: living_agents.iter().map(|a| a.food).sum(),
        total_place_food: w.places.iter().map(|(_, st)| st.food).sum(),
        active_promises: living_agents
            .iter()
            .filter(|a| a.promise.as_ref().is_some_and(|p| p.kept.is_none()))
            .count(),
    }
}

/// Append records and drop the oldest so the log never exceeds the cap.
pub fn push_bounded(run: &mut Vec<TurnRecord>, recs: impl IntoIterator<Item = TurnRecord>) {
    run.extend(recs);
    let excess = run.len().saturating_sub(TURN_LOG_CAP);
    if excess > 0 {
        run.drain(..excess);
    }
}

/// Boot at the resolved save path. `--reset` clears the save first, then founds
/// a new world and records the discontinuity in it.
///
/// # Errors
///
/// Returns a `SaveError` if the save cannot be cleared, loaded or written.
pub fn boot(
    seed: u64,
    save_path: &Path,
    reset: bool,
) -> Result<AppState, Box<dyn std::error::Error>> {
    let (world, fresh) = if reset {
        persistence::clear(save_path)?;
        let mut w = World::founding(seed);
        persistence::mark_reset(&mut w, "the observer reset the village at startup");
        (w, true)
    } else {
        let (w, resumed) = persistence::load_or_found(save_path, seed)?;
        (w, !resumed)
    };
    // A brand-new village is written immediately, so the file the observer is
    // about to watch is the file a restart would read. A resumed village is
    // already on disk; rewriting it would only churn the file a crash could
    // catch mid-rename.
    if fresh {
        persistence::save(&world, save_path)?;
    }

    let state = AppState {
        world: Arc::new(tokio::sync::Mutex::new(world)),
        run: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        save_path: save_path.to_path_buf(),
        save_error: Arc::new(tokio::sync::Mutex::new(None)),
    };
    let turn = state.world.blocking_lock().turn;
    state.audit(
        if reset { "boot_reset" } else { "boot" },
        turn,
        serde_json::json!({"seed": seed, "fresh": fresh}),
    );
    Ok(state)
}
