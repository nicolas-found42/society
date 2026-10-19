//! Library half of the binary: engine wiring shared by the window and the
//! headless runner.

pub mod devserver;
pub mod engine;
pub mod inference;
pub mod persistence;

pub use devserver::{AppState, DEFAULT_PORT};
pub use engine::{Engine, EngineError, TurnRecord};
pub use inference::{Judge, Judgment, JudgmentError, Voice, VoiceError};
pub use persistence::{SaveError, SAVE_VERSION};

use std::path::Path;
use std::sync::Arc;

use society_sim::World;

/// Turns kept in memory for `/log` and the feed. A village left running for
/// days must not grow memory without limit.
pub const TURN_LOG_CAP: usize = 240;

/// Append records and drop the oldest so the log never exceeds the cap.
pub fn push_bounded(run: &mut Vec<TurnRecord>, recs: impl IntoIterator<Item = TurnRecord>) {
    run.extend(recs);
    let excess = run.len().saturating_sub(TURN_LOG_CAP);
    if excess > 0 {
        run.drain(..excess);
    }
}

/// Judgment spend for one committed turn: the intent and gate calls together.
pub fn turn_cost(rec: &TurnRecord) -> f64 {
    rec.intent_call.as_ref().map_or(0.0, |c| c.cost)
        + rec.gate_call.as_ref().map_or(0.0, |c| c.cost)
}

/// Session spend for the retained log. `/log` and the window's top bar both
/// read this, so the number an observer sees is the number the API reports.
pub fn total_cost(run: &[TurnRecord]) -> f64 {
    run.iter().map(turn_cost).sum()
}

/// Boot at the resolved save path. `--reset` clears the save first, then founds
/// a new world and records the discontinuity in it.
pub fn boot(
    seed: u64,
    permits: usize,
    save_path: &Path,
    reset: bool,
) -> Result<AppState, Box<dyn std::error::Error>> {
    let judge = Arc::new(Judge::from_env()?);
    let voice = Arc::new(Voice::new(Default::default()));
    let engine = Arc::new(Engine::new(
        judge.clone(),
        voice.clone(),
        permits,
        save_path.to_path_buf(),
    ));

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

    Ok(AppState {
        world: Arc::new(tokio::sync::Mutex::new(world)),
        run: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        engine,
        judge,
        voice,
        save_path: save_path.to_path_buf(),
    })
}
