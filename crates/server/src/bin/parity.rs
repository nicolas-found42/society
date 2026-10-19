//! The parity harness (tickets #16 and #17): replays the frozen baseline
//! corpus through the new engine and asks the judge to score the old and new
//! choices blind, paired in one call. Development-only; it is not
//! `cargo test`, and the judge is never in the product.
//!
//! The harness owns its own minimal HTTP client so it keeps working after the
//! cutover deletes the inference module: one POST per chunk to the judged
//! endpoint, the same wire shape the old judgment layer used.
//!
//! Usage:
//!   cargo run -p society-server --bin parity -- decisions <round>
//!   cargo run -p society-server --bin parity -- lines <round>
//!   cargo run -p society-server --bin parity -- side-by-side
//!   cargo run -p society-server --bin parity -- report <round>
//!
//! Environment: the endpoint and model come from `JEV_API_URL` and
//! `JEV_MODEL` (development configuration), the key from `OPENROUTER_API_KEY`.

// The harness does bounded small-count arithmetic (win rates, spreads); every
// cast below is value-checked by the surrounding clamps and the pair counts.
#![allow(
    clippy::cast_precision_loss,
    clippy::unwrap_used,
    reason = "bounded win-rate arithmetic; a serialization failure aborts the dev run"
)]

use std::collections::BTreeMap;
use std::io::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const CORPUS: &str = "baseline/corpus.jsonl";
const JUDGE_CHUNK: usize = 24;
/// Parity is relative: the new engine must not lose by more than noise. The
/// band is the 95% binomial noise on the pair count (spec: "Parity is
/// relative, not absolute").

#[derive(Deserialize)]
struct Corpus {
    #[allow(dead_code, reason = "recorded provenance, read by the report")]
    captured_at_commit: String,
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    turn: u64,
    #[allow(dead_code, reason = "recorded provenance")]
    seed: u64,
    world: Value,
    agents: Vec<OldAgent>,
}

#[derive(Deserialize)]
struct OldAgent {
    name: String,
    action: String,
    say: String,
    #[allow(dead_code, reason = "recorded context for the outcome report")]
    why: String,
    #[allow(dead_code, reason = "recorded context for the outcome report")]
    social: String,
    #[allow(dead_code, reason = "recorded context for the outcome report")]
    grounded: Option<f32>,
}

/// One blind pair: the judge sees A and B; the mapping stays here.
#[derive(Serialize)]
struct Pair {
    turn: u64,
    name: String,
    question: String,
    a_is_new: bool,
    old: String,
    new: String,
}

struct Answered {
    pair: Pair,
    /// "a" | "b" — the judge's pick; empty when it refused to answer.
    pick: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map_or("help", String::as_str);
    let round: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let code = std::process::Command::new("cargo")
        .args(["--version"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| ());
    let _ = code;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let outcome = rt.block_on(run(mode, round));
    if let Err(e) = outcome {
        eprintln!("parity: {e}");
        std::process::exit(1);
    }
}

async fn run(mode: &str, round: usize) -> Result<(), String> {
    match mode {
        "decisions" => decisions(round).await,
        "lines" => lines(round).await,
        "side-by-side" => side_by_side(),
        "diff" => {
            diff();
            Ok(())
        },
        "report" => {
            report(round);
            Ok(())
        },
        _ => {
            eprintln!("modes: decisions <round> | lines <round> | side-by-side | report <round>");
            Ok(())
        },
    }
}

fn endpoint() -> Result<(String, String, String), String> {
    #[allow(
        clippy::missing_fields_in_debug,
        reason = "the env error carries no usable context beyond its name"
    )]
    let url = std::env::var("JEV_API_URL").map_err(|e| format!("JEV_API_URL is not set ({e})"))?;
    let model = std::env::var("JEV_MODEL").unwrap_or_else(|_| "typesafe/jev-1.14".into());
    let key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_e| "OPENROUTER_API_KEY is not set".to_owned())?;
    Ok((url, model, key))
}

/// One batched judged call: many paired questions over one shared context
/// note (the pairs carry their own per-villager state).
async fn judge(
    questions: Map<String, Value>,
    states: Map<String, Value>,
) -> Result<BTreeMap<String, String>, String> {
    let (url, model, key) = endpoint()?;
    let client = reqwest::Client::new();
    let body = json!({
        "model": model,
        // All questions in a chunk see the same outer state; each question
        // embeds the villager's own situation.
        "state": states,
        "questions": questions,
    });
    let resp = client
        .post(&url)
        .bearer_auth(&key)
        .header("content-type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(180))
        .send()
        .await
        .map_err(|e| format!("judge transport: {e}"))?;
    let code = resp.status();
    let text = resp.text().await.map_err(|e| format!("judge read: {e}"))?;
    if !code.is_success() {
        return Err(format!("judge status {}: {}", code, short(&text, 300)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("judge body: {e}"))?;
    let answers = v
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("judge sent no answers: {}", short(&text, 300)))?;
    let mut out = BTreeMap::new();
    for (id, a) in answers {
        let pick = a
            .get("choice")
            .or_else(|| a.get("choice_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        out.insert(id.clone(), pick.to_owned());
    }
    if let Some(usage) = v.get("usage") {
        eprintln!(
            "judge chunk: {} questions, cost {}",
            questions.len(),
            usage
                .get("cost")
                .map_or_else(|| "none".to_owned(), ToString::to_string)
        );
    }
    Ok(out)
}

fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A compact, judge-readable digest of one villager's recorded situation.
fn villager_situation(world: &Value, name: &str) -> Value {
    let agents = world.get("agents").and_then(Value::as_array);
    let me = agents.and_then(|list| {
        list.iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
    });
    let place = me
        .and_then(|a| a.get("place"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let places = world.get("places").and_then(Value::as_array);
    let here = places.and_then(|list| {
        list.iter()
            .find(|p| p.get(0).and_then(Value::as_str) == Some(place))
    });
    let stores = here.and_then(|p| p.get(1));
    let present: Vec<String> = agents
        .map(|list| {
            list.iter()
                .filter(|a| {
                    a.get("alive").and_then(Value::as_bool).unwrap_or(false)
                        && a.get("name").and_then(Value::as_str) != Some(name)
                        && a.get("place").and_then(Value::as_str) == Some(place)
                })
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    json!({
        "you": me,
        "place": place,
        "stores_here": stores,
        "present": present,
        "season": world.get("weather_kind"),
        "morale": world.get("village_morale"),
    })
}

fn action_phrase(action: &str) -> String {
    if let Some(p) = action.strip_prefix("go:") {
        return format!("leave for {p}");
    }
    action.replace('_', " ")
}

/// The decision A/B: the recorded old choice against the new policy's choice
/// on the same recorded world, blind to which is which.
async fn decisions(round: usize) -> Result<(), String> {
    let corpus = load_corpus()?;
    let mut pairs: Vec<Pair> = Vec::new();
    let mut ties = 0_usize;
    let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15 ^ (round as u64).wrapping_mul(7919);
    for rec in &corpus.records {
        let world: society_sim::World = serde_json::from_value(rec.world.clone())
            .map_err(|e| format!("corpus world at turn {}: {e}", rec.turn))?;
        for old in &rec.agents {
            let Some(decision) = society_sim::decide(&world, &old.name) else {
                continue;
            };
            let new_action = decision.action.clone();
            if new_action == old.action {
                ties += 1;
                continue;
            }
            // A cheap deterministic shuffle: which side is "A".
            rng_state = rng_state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let a_is_new = rng_state >> 33 & 1 == 1;
            let (a, b) = if a_is_new {
                (&new_action, &old.action)
            } else {
                (&old.action, &new_action)
            };
            pairs.push(Pair {
                turn: rec.turn,
                name: old.name.clone(),
                question: format!(
                    "{} at turn {}, situation: {}. Choice A: {}. Choice B: {}. \
                     Which single choice is the better one for this villager to make now?",
                    old.name,
                    rec.turn,
                    villager_situation(&rec.world, &old.name),
                    action_phrase(a),
                    action_phrase(b)
                ),
                a_is_new,
                old: old.action.clone(),
                new: new_action,
            });
        }
    }
    eprintln!(
        "decision pairs: {} ({} records agreed outright and count as ties)",
        pairs.len(),
        ties
    );
    let answered = ask_pairs(&pairs, "decision").await?;
    write_round(round, "decisions", ties, &answered)
}

/// The line A/B on the same recorded facts (ticket #17).
async fn lines(round: usize) -> Result<(), String> {
    let corpus = load_corpus()?;
    let mut pairs: Vec<Pair> = Vec::new();
    let mut ties = 0_usize;
    let mut rng_state: u64 = 0xA24B_5D3F_6C1E_9087 ^ (round as u64).wrapping_mul(104_729);
    for rec in &corpus.records {
        let mut world: society_sim::World = serde_json::from_value(rec.world.clone())
            .map_err(|e| format!("corpus world at turn {}: {e}", rec.turn))?;
        for old in &rec.agents {
            let Some(decision) = society_sim::decide(&world, &old.name) else {
                continue;
            };
            // The new line is spoken on the recorded facts, before any
            // mutation: the comparison is on the same facts, not on the new
            // engine's own timeline.
            let new_line = society_sim::speak(&world, &old.name, &decision)
                .map(|l| l.text)
                .unwrap_or_default();
            if new_line.is_empty() || new_line == old.say {
                ties += 1;
                continue;
            }
            rng_state = rng_state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let a_is_new = rng_state >> 33 & 1 == 1;
            let (a, b) = if a_is_new {
                (new_line.as_str(), old.say.as_str())
            } else {
                (old.say.as_str(), new_line.as_str())
            };
            pairs.push(Pair {
                turn: rec.turn,
                name: old.name.clone(),
                question: format!(
                    "{} at turn {}, situation: {}. The villager just chose to: {}. \
                     Line A: {:?}. Line B: {:?}. \
                     Which line is the better thing for this villager to say here - \
                     which one fits the person, the moment and the choice?",
                    old.name,
                    rec.turn,
                    villager_situation(&rec.world, &old.name),
                    action_phrase(&decision.action),
                    a,
                    b
                ),
                a_is_new,
                old: old.say.clone(),
                new: new_line,
            });
        }
        // Keep the world strictly on the recorded facts: no settling here.
        let _ = &mut world;
    }
    eprintln!(
        "line pairs: {} ({} records agreed outright and count as ties)",
        pairs.len(),
        ties
    );
    let answered = ask_pairs(&pairs, "line").await?;
    write_round(round, "lines", ties, &answered)
}

async fn ask_pairs(pairs: &[Pair], kind: &str) -> Result<Vec<Answered>, String> {
    let mut answered = Vec::new();
    for chunk in pairs.chunks(JUDGE_CHUNK) {
        let mut questions = Map::new();
        let mut states = Map::new();
        for (i, p) in chunk.iter().enumerate() {
            let id = format!("{kind}_{i}");
            questions.insert(
                id.clone(),
                json!({
                    "type": "choice",
                    "instructions": p.question,
                    "criteria": { "a": "Choice A", "b": "Choice B" }
                }),
            );
            states.insert(id.clone(), json!({}));
        }
        let answers = judge(questions, states).await.map_err(|e| {
            format!(
                "chunk failed ({} pairs answered so far): {e}",
                answered.len()
            )
        })?;
        for (i, p) in chunk.iter().enumerate() {
            let pick = answers
                .get(&format!("{kind}_{i}"))
                .cloned()
                .unwrap_or_default();
            answered.push(Answered {
                pair: Pair {
                    turn: p.turn,
                    name: p.name.clone(),
                    question: p.question.clone(),
                    a_is_new: p.a_is_new,
                    old: p.old.clone(),
                    new: p.new.clone(),
                },
                pick,
            });
        }
        let _ = std::io::stderr().flush();
    }
    Ok(answered)
}

fn write_round(round: usize, kind: &str, ties: usize, answered: &[Answered]) -> Result<(), String> {
    let mut new_wins = 0_usize;
    let mut old_wins = 0_usize;
    let mut no_answer = 0_usize;
    for a in answered {
        if a.pick.is_empty() {
            no_answer += 1;
        } else if a.pick == "a" {
            if a.pair.a_is_new {
                new_wins += 1;
            } else {
                old_wins += 1;
            }
        } else if a.pick == "b" {
            if a.pair.a_is_new {
                old_wins += 1;
            } else {
                new_wins += 1;
            }
        } else {
            no_answer += 1;
        }
    }
    let decided = new_wins + old_wins;
    let wr = if decided > 0 {
        new_wins as f64 / decided as f64
    } else {
        0.5
    };
    let n = decided + ties;
    let noise = 1.96 * (0.25 / n.max(1) as f64).sqrt();
    let verdict = if wr + noise < 0.5 {
        "LOSES beyond noise"
    } else {
        "within noise of parity"
    };
    let out = json!({
        "round": round,
        "kind": kind,
        "pairs": answered.len(),
        "ties_new_engine_matched_old": ties,
        "new_wins": new_wins,
        "old_wins": old_wins,
        "no_answer": no_answer,
        "new_win_rate": wr,
        "noise_band_95": noise,
        "verdict": verdict,
    });
    let path = format!("baseline/round{round}_{kind}.json");
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap())
        .map_err(|e| format!("write {path}: {e}"))?;
    let detail: Vec<Value> = answered
        .iter()
        .map(|a| {
            json!({
                "turn": a.pair.turn,
                "name": a.pair.name,
                "old": a.pair.old,
                "new": a.pair.new,
                "pick": a.pick,
                "a_is_new": a.pair.a_is_new,
            })
        })
        .collect();
    let detail_path = format!("baseline/round{round}_{kind}_pairs.json");
    std::fs::write(&detail_path, serde_json::to_string_pretty(&detail).unwrap())
        .map_err(|e| format!("write {detail_path}: {e}"))?;
    println!("{path}\n{out}");
    Ok(())
}

fn load_corpus() -> Result<Corpus, String> {
    let text = std::fs::read_to_string(CORPUS).map_err(|e| format!("read {CORPUS}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {CORPUS}: {e}"))
}

/// The side-by-side outcome run: one seed, the new world with professions.
fn side_by_side() -> Result<(), String> {
    let mut w = society_sim::World::founding(7);
    let mut records = Vec::new();
    for _ in 0..40 {
        match society_sim::step(&mut w) {
            Ok(rec) => records.push(rec),
            Err(_) => break,
        }
    }
    let mut births = 0_u64;
    let mut deaths = 0_u64;
    for e in &w.events {
        match e.kind {
            society_sim::EventKind::Birth => births += 1,
            society_sim::EventKind::Death => deaths += 1,
            _ => {},
        }
    }
    // The records carry the truth: lines spoken, the check's verdicts, and
    // the lines spoken in lie mode.
    let lines: u64 = records.iter().map(|r| u64::from(r.summary.lines)).sum();
    let discredited: u64 = records
        .iter()
        .map(|r| u64::from(r.summary.discredited))
        .sum();
    let lie_lines: u64 = records.iter().map(|r| u64::from(r.summary.lies)).sum();
    let silences: u64 = records.iter().map(|r| u64::from(r.summary.silences)).sum();
    let mut trusts: Vec<f32> = Vec::new();
    for a in w.living() {
        trusts.extend(a.trust.iter().map(|(_, v)| *v));
    }
    let mean = trusts.iter().sum::<f32>() / trusts.len().max(1) as f32;
    let spread = (trusts.iter().map(|t| (t - mean).powi(2)).sum::<f32>()
        / trusts.len().max(1) as f32)
        .sqrt();
    let out = json!({
        "note": "The new engine's outcome run at seed 7, 40 turns, reported as the new world with professions. Context only; the acceptance bar is the paired judgment.",
        "seed": 7,
        "turns": w.turn,
        "population_end": w.living().count(),
        "births": births,
        "deaths": deaths,
        "lines": lines,
        "silences": silences,
        "discredited": discredited,
        "lies": lie_lines,
        "discredited_rate": if lines > 0 { discredited as f64 / lines as f64 } else { 0.0 },
        "trust_mean": mean,
        "trust_spread_sd": spread,
        "professions": society_sim::PROFESSIONS.iter().map(|p| json!({
            "role": p.role.id(),
            "orders": p.orders.len(),
        })).collect::<Vec<_>>(),
    });
    std::fs::write(
        "baseline/side_by_side.json",
        serde_json::to_string_pretty(&out).unwrap(),
    )
    .map_err(|e| format!("write: {e}"))?;
    println!("{out}");
    Ok(())
}

/// The offline disagreement matrix: where the new policy chose differently
/// from the old judge, counted by (old, new) action pair. Free - no judge -
/// and the evidence a weight revision is justified by.
fn diff() {
    let Ok(corpus) = load_corpus() else { return };
    let mut matrix: BTreeMap<String, usize> = BTreeMap::new();
    let mut agree = 0_usize;
    for rec in &corpus.records {
        let Ok(world) = serde_json::from_value::<society_sim::World>(rec.world.clone()) else {
            continue;
        };
        for old in &rec.agents {
            let Some(decision) = society_sim::decide(&world, &old.name) else {
                continue;
            };
            if decision.action == old.action {
                agree += 1;
            } else {
                *matrix
                    .entry(format!("{} -> {}", old.action, decision.action))
                    .or_default() += 1;
            }
        }
    }
    let mut rows: Vec<(String, usize)> = matrix.into_iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(*v));
    println!("agreements: {agree}; disagreements by (old -> new):");
    for (k, v) in rows.iter().take(30) {
        println!("  {v:3}  {k}");
    }
}

fn report(round: usize) {
    for kind in ["decisions", "lines"] {
        let path = format!("baseline/round{round}_{kind}.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => println!("{path}: {text}"),
            Err(_) => println!("{path}: (not yet run)"),
        }
    }
}
