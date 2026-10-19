//! One-off baseline capture (ticket #7). Runs the old engine at the freeze
//! commit and records the full turn-start world plus every villager's decision
//! and line, so the parity harness can replay those worlds through the new
//! policy after the old engine is deleted.
//!
//! Captured from a tree at `c4467d5`. The corpus must NOT be recaptured: every
//! later slice changes the legacy engine's behaviour, because it shares the
//! simulation crate. See `baseline/HOW_CAPTURED.md`.
//!
//! Usage: `cargo run -p society-server --bin capture -- <seed> <turns> <out_dir>`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use society_server::engine::{Engine, EngineError};
use society_server::inference::judgment::{Judge, JudgeConfig};
use society_server::inference::voice::{Voice, VoiceConfig};
use society_sim::World;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seed: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(20_261_019);
    let turns: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(30);
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "baseline".into()));

    std::fs::create_dir_all(&out_dir)?;

    // The engine refuses to start without a key, by design; capture runs the
    // old engine, so it needs the same.
    let judge = Arc::new(Judge::new(JudgeConfig::from_env()?));
    let voice = Arc::new(Voice::new(VoiceConfig::default()));
    // The engine persists after every committed turn; the corpus does not need
    // the save, so it goes to a throwaway file.
    let throwaway = std::env::temp_dir().join("society-capture-save.json");
    let engine = Engine::new(judge, voice, 8, throwaway);

    let corpus_path = out_dir.join("corpus.jsonl");
    let mut w = World::founding(seed);
    let mut lines = Vec::new();
    for _ in 0..turns {
        if w.halted.is_some() || w.living().count() == 0 {
            break;
        }
        // The old engine decides and speaks for every villager against the
        // turn-start state, then applies effects; this snapshot is therefore
        // the input for that villager's decision, for all of them at once.
        let world_json = serde_json::to_value(&w)?;
        let turn = w.turn;
        let rec = match engine.step(&mut w).await {
            Ok(r) => r,
            Err(EngineError::Halted(reason)) => {
                eprintln!("halted at turn {turn}: {reason}");
                break;
            },
            Err(e @ EngineError::Judgment(_)) => return Err(e.into()),
            Err(e @ EngineError::Persistence(_)) => return Err(e.into()),
        };
        lines.push(json!({
            "turn": turn,
            "seed": seed,
            "world": world_json,
            "agents": rec.agents,
        }));
        eprintln!("captured turn {turn} ({} agents)", rec.agents.len());
    }
    serde_json::to_writer_pretty(std::fs::File::create(&corpus_path)?, &json!({
        "captured_at_commit": "c4467d5",
        "seed": seed,
        "turns": lines.len(),
        "note": "The world is the turn-start state; every villager in a turn decided and spoke against it. Do not recapture.",
        "records": lines,
    }))?;
    eprintln!("wrote {}", corpus_path.display());

    // Outcome metrics from a side-by-side seed run, recorded for context: the
    // parity bar is the paired judgment, not these numbers.
    let metrics = side_by_side(7, 40).await?;
    let metrics_path = out_dir.join("metrics.json");
    serde_json::to_writer_pretty(std::fs::File::create(&metrics_path)?, &metrics)?;
    eprintln!("wrote {}", metrics_path.display());
    Ok(())
}

async fn side_by_side(seed: u64, turns: usize) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let judge = Arc::new(Judge::new(JudgeConfig::from_env()?));
    let voice = Arc::new(Voice::new(VoiceConfig::default()));
    let engine = Engine::new(judge, voice, 8, std::env::temp_dir().join("society-metrics-save.json"));
    let mut w = World::founding(seed);
    let mut births = 0_u64;
    let mut deaths = 0_u64;
    let mut lines = 0_u64;
    let mut discredited = 0_u64;
    for _ in 0..turns {
        if w.halted.is_some() || w.living().count() == 0 {
            break;
        }
        let turn = w.turn;
        match engine.step(&mut w).await {
            Ok(rec) => {
                births += rec.births.len() as u64;
                for a in &rec.agents {
                    if let Some(g) = a.grounded {
                        lines += 1;
                        if g == 0.0 {
                            discredited += 1;
                        }
                    }
                }
            },
            Err(EngineError::Halted(reason)) => {
                eprintln!("metrics run halted at turn {turn}: {reason}");
                break;
            },
            Err(e) => return Err(e.into()),
        }
    }
    for e in &w.events {
        if e.kind == society_sim::EventKind::Death {
            deaths += 1;
        }
    }
    let mut trusts: Vec<f32> = Vec::new();
    for a in w.living() {
        trusts.extend(a.trust.iter().map(|(_, v)| *v));
    }
    let n = trusts.len() as f32;
    let mean = trusts.iter().sum::<f32>() / n.max(1.0);
    let spread = (trusts.iter().map(|t| (t - mean).powi(2)).sum::<f32>() / n.max(1.0)).sqrt();
    Ok(json!({
        "seed": seed,
        "turns": w.turn,
        "population_end": w.living().count(),
        "births": births,
        "deaths": deaths,
        "lines": lines,
        "discredited": discredited,
        "discredited_rate": if lines > 0 { discredited as f64 / lines as f64 } else { 0.0 },
        "trust_mean": mean,
        "trust_spread_sd": spread,
    }))
}

/// Silences the unused-import lint when Path is only used in signatures.
#[allow(dead_code)]
fn _touch(_: &Path) {}
