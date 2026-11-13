//! Standalone latency and allocation baseline for the society.
//!
//! Six paths carry the wall clock of a living village, and they are measured
//! here on this machine so no later claim about speed is an estimate:
//!
//! 1. `engine_turn` — `society_sim::turn::step`, the committed turn: the
//!    policy decides, the speaker renders the line, the check settles it, the
//!    intent is applied and the world settles. Runs once per committed turn.
//! 2. `durable_save` — `society_server::persistence::save`, the write that
//!    makes a turn durable: serialise the world, verify it loads back, write a
//!    temporary file, fsync it, rename it over the target. Runs once per
//!    committed turn, immediately after the turn.
//! 3. `observer_snapshot` — `society_server::observer::encode`, the payload
//!    every observer of the control API reads (`/state`, `/social`). Runs per
//!    observer request.
//! 4. `durable_load` — `society_server::persistence::load`, the village read
//!    back into memory. Runs once per start.
//! 5. `turn_cycle` — the engine step plus every sink `commit_turn` writes: the
//!    start audit, the save, the journal row and the completion audit. The
//!    stage paths above split this cost; a regression in their count or order
//!    only shows up here.
//! 6. `observer_health` — `society_server::diagnostics::journal_growth`, the
//!    journal window `/health` reports.
//!
//! The harness adds no dependency: the counting allocator, the percentile
//! math, the RSS sampling and the JSON fixture are all written here on top of
//! `std`. It borrows the crates under test, and nothing else.
//!
//! Run it:
//!
//! ```bash
//! tools/bench.sh                 # measure and write baseline/perf_baseline.json
//! tools/bench.sh --check         # measure and compare against the fixture
//! tools/bench.sh --breakdown     # per-stage cost, no fixture written
//! ```
//!
//! `--check` exits non-zero when a path is slower than the budget recorded in
//! the fixture, so a PR can be judged against this exact threshold.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{json, Value};
use society_sim::World;

const DEFAULT_ITERATIONS: usize = 1_000;
const DEFAULT_WARMUP: usize = 20;
const DEFAULT_SEED: u64 = 7;
const DEFAULT_TURNS: usize = 233;
const DEFAULT_FIXTURE: &str = "baseline/perf_baseline.json";
/// A path may be this much slower than the recorded baseline before the
/// check fails: 25% on p99 and on per-iteration allocation.
const TOLERANCE: f64 = 1.25;
/// A sub-millisecond path is jitter, not regression: the scheduler alone moves
/// its p99 by more than 25%. The budget is therefore never tighter than the
/// recorded p99 plus this floor.
const JITTER_FLOOR_MS: f64 = 0.50;
/// The fsync in the save path is the filesystem's own jitter, and it moves the
/// tail far more than the median: the same machine measured 13.0 ms and
/// 14.3 ms at the median across two quiet captures, and 23.6 ms at p99 on a
/// capture taken while the disk was still flushing a build. The budget is
/// therefore the loosest of three rules — 25% over p99, double the p95, and
/// the jitter floor — so a slow disk is not read as a slow village.
const P95_HEADROOM: f64 = 2.0;

// ---------------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------------

/// Tracks every byte the process allocates, so a path's allocation cost is a
/// measured number rather than a guess.
struct Counting;

static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                let grown = (new_size - layout.size()) as u64;
                ALLOC_BYTES.fetch_add(grown, Ordering::Relaxed);
                LIVE_BYTES.fetch_add(grown, Ordering::Relaxed);
            } else {
                LIVE_BYTES.fetch_sub((layout.size() - new_size) as u64, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/// What one path cost, over `iterations` measured runs.
struct Measurement {
    nanos: Vec<u64>,
    alloc_bytes: u64,
    alloc_calls: u64,
    live_before: u64,
    live_after: u64,
    rss_before_kb: i64,
    rss_after_kb: i64,
    rss_peak_kb: i64,
}

/// Run `body` `warmup + iterations` times and keep the last `iterations`
/// measurements. `prepare` runs before the clock starts and before the
/// allocation counters are read, so setup (a cloned world, a scratch file)
/// never lands in the numbers. `finish` runs after the last iteration and
/// before the closing resident-memory reading, so the harness's own scratch
/// state is not counted as the path's growth.
fn measure<P, F, X>(
    mut prepare: P,
    mut body: F,
    mut finish: X,
    iterations: usize,
    warmup: usize,
) -> Measurement
where
    P: FnMut(),
    F: FnMut(),
    X: FnMut(),
{
    let mut m = Measurement {
        nanos: Vec::with_capacity(iterations),
        alloc_bytes: 0,
        alloc_calls: 0,
        live_before: LIVE_BYTES.load(Ordering::Relaxed),
        live_after: 0,
        rss_before_kb: rss_kb(),
        rss_after_kb: -1,
        rss_peak_kb: -1,
    };
    let total = warmup + iterations;
    for i in 0..total {
        prepare();
        let bytes_before = ALLOC_BYTES.load(Ordering::Relaxed);
        let calls_before = ALLOC_CALLS.load(Ordering::Relaxed);
        let start = Instant::now();
        body();
        let elapsed = start.elapsed();
        let bytes_after = ALLOC_BYTES.load(Ordering::Relaxed);
        let calls_after = ALLOC_CALLS.load(Ordering::Relaxed);
        if i >= warmup {
            m.nanos.push(elapsed.as_nanos() as u64);
            m.alloc_bytes += bytes_after - bytes_before;
            m.alloc_calls += calls_after - calls_before;
        }
        if i % 100 == 0 {
            let rss = rss_kb();
            m.rss_peak_kb = m.rss_peak_kb.max(rss);
        }
    }
    finish();
    m.live_after = LIVE_BYTES.load(Ordering::Relaxed);
    m.rss_after_kb = rss_kb();
    m.rss_peak_kb = m.rss_peak_kb.max(m.rss_after_kb);
    m
}

/// Resident set size of this process, in kilobytes. Sampled from `ps` so the
/// number is the kernel's, not the allocator's opinion.
fn rss_kb() -> i64 {
    let pid = std::process::id().to_string();
    let Ok(out) = Command::new("ps").args(["-o", "rss=", "-p", &pid]).output() else {
        return -1;
    };
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .unwrap_or(-1)
}

/// Nearest-rank percentile over a sorted slice.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

/// One path's report.
struct PathReport {
    name: &'static str,
    description: &'static str,
    runs: &'static str,
    iterations: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    min_ms: f64,
    max_ms: f64,
    mean_ms: f64,
    alloc_bytes_per_iter: u64,
    alloc_calls_per_iter: u64,
    live_growth_bytes: i64,
    output_bytes: u64,
    rss_before_kb: i64,
    rss_after_kb: i64,
    rss_peak_kb: i64,
}

fn report(
    name: &'static str,
    description: &'static str,
    runs: &'static str,
    output_bytes: u64,
    m: &Measurement,
) -> PathReport {
    let mut sorted = m.nanos.clone();
    sorted.sort_unstable();
    let n = m.nanos.len().max(1) as u64;
    let total: u64 = m.nanos.iter().sum();
    PathReport {
        name,
        description,
        runs,
        iterations: m.nanos.len(),
        p50_ms: ms(percentile(&sorted, 0.50)),
        p95_ms: ms(percentile(&sorted, 0.95)),
        p99_ms: ms(percentile(&sorted, 0.99)),
        min_ms: ms(sorted.first().copied().unwrap_or(0)),
        max_ms: ms(sorted.last().copied().unwrap_or(0)),
        mean_ms: ms(total / n),
        alloc_bytes_per_iter: m.alloc_bytes / n,
        alloc_calls_per_iter: m.alloc_calls / n,
        live_growth_bytes: m.live_after as i64 - m.live_before as i64,
        output_bytes,
        rss_before_kb: m.rss_before_kb,
        rss_after_kb: m.rss_after_kb,
        rss_peak_kb: m.rss_peak_kb,
    }
}

fn to_json(r: &PathReport) -> Value {
    json!({
        "name": r.name,
        "description": r.description,
        "runs_when": r.runs,
        "iterations": r.iterations,
        "latency_ms": {
            "p50": round3(r.p50_ms),
            "p95": round3(r.p95_ms),
            "p99": round3(r.p99_ms),
            "min": round3(r.min_ms),
            "max": round3(r.max_ms),
            "mean": round3(r.mean_ms),
        },
        "memory": {
            "alloc_bytes_per_iter": r.alloc_bytes_per_iter,
            "alloc_calls_per_iter": r.alloc_calls_per_iter,
            "live_growth_bytes": r.live_growth_bytes,
            "output_bytes": r.output_bytes,
            "rss_before_kb": r.rss_before_kb,
            "rss_after_kb": r.rss_after_kb,
            "rss_peak_kb": r.rss_peak_kb,
        },
        "budget": {
            "p99_ms": round3(
                (r.p99_ms * TOLERANCE)
                    .max(r.p95_ms * P95_HEADROOM)
                    .max(r.p99_ms + JITTER_FLOOR_MS),
            ),
            "alloc_bytes_per_iter": (r.alloc_bytes_per_iter as f64 * TOLERANCE) as u64,
        },
    })
}

fn round3(v: f64) -> f64 {
    (v * 1_000.0).round() / 1_000.0
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

fn cmd_output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn cpu_model() -> String {
    if let Some(m) = cmd_output("sysctl", &["-n", "machdep.cpu.brand_string"]) {
        if !m.is_empty() {
            return m;
        }
    }
    if let Some(text) = cmd_output("uname", &["-p"]) {
        if !text.is_empty() {
            return text;
        }
    }
    "unknown".to_owned()
}

fn machine_json() -> Value {
    json!({
        "cpu": cpu_model(),
        "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "kernel": cmd_output("uname", &["-r"]).unwrap_or_else(|| "unknown".to_owned()),
        "rustc": cmd_output("rustc", &["-V"]).unwrap_or_else(|| "unknown".to_owned()),
        "profile": "release",
        "git_commit": cmd_output("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned()),
    })
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

struct Args {
    iterations: usize,
    warmup: usize,
    seed: u64,
    turns: usize,
    fixture: String,
    source: Option<String>,
    check: bool,
    breakdown: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        iterations: DEFAULT_ITERATIONS,
        warmup: DEFAULT_WARMUP,
        seed: DEFAULT_SEED,
        turns: DEFAULT_TURNS,
        fixture: DEFAULT_FIXTURE.to_owned(),
        source: None,
        check: false,
        breakdown: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--iterations" => {
                args.iterations = it
                    .next()
                    .ok_or("--iterations needs a number")?
                    .parse()
                    .map_err(|_| "--iterations needs a number")?;
            },
            "--warmup" => {
                args.warmup = it
                    .next()
                    .ok_or("--warmup needs a number")?
                    .parse()
                    .map_err(|_| "--warmup needs a number")?;
            },
            "--seed" => {
                args.seed = it
                    .next()
                    .ok_or("--seed needs a number")?
                    .parse()
                    .map_err(|_| "--seed needs a number")?;
            },
            "--turns" => {
                args.turns = it
                    .next()
                    .ok_or("--turns needs a number")?
                    .parse()
                    .map_err(|_| "--turns needs a number")?;
            },
            "--out" => {
                args.fixture = it.next().ok_or("--out needs a path")?;
            },
            "--source" => {
                args.source = Some(it.next().ok_or("--source needs a path")?);
            },
            "--check" => args.check = true,
            "--breakdown" => args.breakdown = true,
            "-h" | "--help" => {
                println!(
                    "society-bench [--iterations N] [--warmup N] [--seed N] [--turns N] [--out PATH] [--source SAVE] [--check]\n\
                     \n\
                     Measures the three latency-critical paths of the society and writes\n\
                     the baseline to baseline/perf_baseline.json. Without --source, builds a\n\
                     deterministic village from --seed (default 7) stepped --turns (default 233).\n\
                     --check compares the run against the recorded budgets and exits non-zero on a regression.\n\
                     --breakdown prints the per-stage cost inside the save, journal, audit and load\n\
                     paths instead of writing the baseline."
                );
                std::process::exit(0);
            },
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(args)
}

/// The scratch directory the save path writes into. It is removed when the
/// harness ends, however it ends.
struct Scratch {
    path: std::path::PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("society-bench: {e}");
            std::process::exit(2);
        },
    };

    let (world, source_label, source_bytes) = if let Some(src) = &args.source {
        let source_path = std::path::PathBuf::from(src);
        let w = match society_server::persistence::load(&source_path) {
            Ok(Some(w)) => w,
            Ok(None) => {
                eprintln!("society-bench: no village at `{}`", source_path.display());
                std::process::exit(2);
            },
            Err(e) => {
                eprintln!(
                    "society-bench: cannot read `{}`: {e}",
                    source_path.display()
                );
                std::process::exit(2);
            },
        };
        let bytes = source_path.metadata().map_or(0, |m| m.len());
        (w, source_path.to_string_lossy().into_owned(), bytes)
    } else {
        let mut w = World::founding(args.seed);
        for _ in 0..args.turns {
            if society_sim::turn::step(&mut w).is_err() {
                break;
            }
        }
        (
            w,
            format!("deterministic:seed={}:turns={}", args.seed, args.turns),
            0,
        )
    };
    println!(
        "society-bench: {} iterations per path, {} warmup, world at turn {} ({} events, {} agents)",
        args.iterations,
        args.warmup,
        world.turn,
        world.events.len(),
        world.agents.len(),
    );

    if args.breakdown {
        let scratch = Scratch {
            path: std::env::temp_dir()
                .join(format!("society-bench-breakdown-{}", std::process::id())),
        };
        breakdown(&world, args.iterations, args.warmup, &scratch.path);
        return;
    }

    // 1. The committed turn.
    let turn_world = world.clone();
    let turn_output_bytes = {
        let mut probe = world.clone();
        let record = society_sim::turn::step(&mut probe).expect("the turn runs");
        serde_json::to_vec(&record).map_or(0, |b| b.len() as u64)
    };
    let slot = RefCell::new(None::<World>);
    let turn_measurement = measure(
        || {
            // Clone shrinks every Vec capacity to len; pre-reserve steady-state
            // turn headroom outside the timed/counted body so we do not charge
            // a full-history Vec reallocation on every single iteration.
            let mut w = turn_world.clone();
            w.events.reserve(64);
            w.agents.reserve(4);
            for a in &mut w.agents {
                a.facts.reserve(16);
                a.memories.reserve(8);
                a.beliefs.reserve(8);
                a.line_ledger.reserve(8);
                a.trust.reserve(4);
                a.relations.reserve(4);
            }
            *slot.borrow_mut() = Some(w);
        },
        || {
            let mut w = slot.borrow_mut().take().expect("prepared world");
            let record = society_sim::turn::step(&mut w).expect("the turn runs");
            std::hint::black_box(&record);
            *slot.borrow_mut() = Some(w);
        },
        || {
            *slot.borrow_mut() = None;
        },
        args.iterations,
        args.warmup,
    );
    let turn_report = report(
        "engine_turn",
        "society_sim::turn::step: policy, speaker, check, apply, settle",
        "once per committed turn",
        turn_output_bytes,
        &turn_measurement,
    );

    // 2. The durable save that follows every committed turn.
    let scratch = Scratch {
        path: std::env::temp_dir().join(format!("society-bench-{}", std::process::id())),
    };
    let scratch_path = scratch.path.join("village.json");
    let save_measurement = measure(
        || {},
        || {
            society_server::persistence::save(&world, &scratch_path).expect("the save lands");
        },
        || {},
        args.iterations,
        args.warmup,
    );
    let save_report = report(
        "durable_save",
        "society_server::persistence::save: serialise, verify, write, fsync, rename",
        "once per committed turn, right after it",
        std::fs::metadata(&scratch_path).map_or(0, |m| m.len()),
        &save_measurement,
    );

    // 3. The snapshot every observer reads.
    let snapshot_measurement = measure(
        || {},
        || {
            let bytes = society_server::observer::encode(&world);
            std::hint::black_box(bytes.len());
        },
        || {},
        args.iterations,
        args.warmup,
    );
    let snapshot_report = report(
        "observer_snapshot",
        "society_server::observer::encode: the village as the bytes on the wire",
        "per observer request (/state, /social)",
        society_server::observer::encode(&world).len() as u64,
        &snapshot_measurement,
    );

    // 4. The load a restart performs: the save that was just written, read back.
    let load_measurement = measure(
        || {},
        || {
            let world = society_server::persistence::load(&scratch_path)
                .expect("the save reads back")
                .expect("the save holds a village");
            std::hint::black_box(world.turn);
        },
        || {},
        args.iterations,
        args.warmup,
    );
    let load_report = report(
        "durable_load",
        "society_server::persistence::load: read the village back into memory",
        "once per start, and after --reset clears the save",
        std::fs::metadata(&scratch_path).map_or(0, |m| m.len()),
        &load_measurement,
    );

    // 5. The turn as the user pays for it: the engine step plus every durable
    // sink `commit_turn` writes. The three paths above split this cost; a
    // regression in their ordering or count only shows up here.
    let cycle_world = world.clone();
    let cycle_slot = RefCell::new(None::<World>);
    let cycle_measurement = measure(
        || {
            let mut w = cycle_world.clone();
            w.events.reserve(64);
            for a in &mut w.agents {
                a.facts.reserve(16);
                a.memories.reserve(8);
                a.beliefs.reserve(8);
                a.line_ledger.reserve(8);
                a.trust.reserve(4);
                a.relations.reserve(4);
            }
            *cycle_slot.borrow_mut() = Some(w);
        },
        || {
            let mut w = cycle_slot.borrow_mut().take().expect("prepared world");
            let record = society_sim::turn::step(&mut w).expect("the turn runs");
            let context = society_server::telemetry::Context::default();
            society_server::diagnostics::append_audit_context(
                &scratch_path,
                "turn_started",
                record.turn,
                json!({"source": "bench"}),
                context.clone(),
            )
            .expect("the start audit lands");
            society_server::persistence::save(&w, &scratch_path).expect("the save lands");
            society_server::diagnostics::append_turn_context(
                &scratch_path,
                w.seed,
                &record,
                "bench",
                society_server::persistence::SAVE_VERSION,
                context.clone(),
            )
            .expect("the journal lands");
            society_server::diagnostics::append_audit_context(
                &scratch_path,
                "turn_completed",
                w.turn,
                json!({"source": "bench"}),
                context,
            )
            .expect("the completion audit lands");
            *cycle_slot.borrow_mut() = Some(w);
        },
        || {
            *cycle_slot.borrow_mut() = None;
        },
        args.iterations,
        args.warmup,
    );
    let cycle_report = report(
        "turn_cycle",
        "turn::step plus the audit, save, journal and completion audit commit_turn writes",
        "once per committed turn (window and --headless warmup)",
        std::fs::metadata(&scratch_path).map_or(0, |m| m.len()),
        &cycle_measurement,
    );

    // 6. What /health answers: the journal window without its records.
    let health_measurement = measure(
        || {},
        || {
            let growth = society_server::diagnostics::journal_growth(
                &scratch_path,
                society_server::diagnostics::MAX_QUERY_ENTRIES,
            );
            std::hint::black_box(growth.turns + growth.corrupt_lines);
        },
        || {},
        args.iterations,
        args.warmup,
    );
    let health_report = report(
        "observer_health",
        "society_server::diagnostics::journal_growth: the journal window /health reports",
        "per /health request",
        std::fs::metadata(society_server::diagnostics::journal_path(&scratch_path))
            .map_or(0, |m| m.len()),
        &health_measurement,
    );

    let reports = [
        turn_report,
        save_report,
        snapshot_report,
        load_report,
        health_report,
        cycle_report,
    ];
    for r in &reports {
        println!(
            "  {:<18} p50 {:>8.3} ms  p95 {:>8.3} ms  p99 {:>8.3} ms  {:>9} B/iter",
            r.name, r.p50_ms, r.p95_ms, r.p99_ms, r.alloc_bytes_per_iter
        );
    }

    let save_bytes = if source_bytes > 0 {
        source_bytes
    } else {
        reports[1].output_bytes
    };
    let world_meta = json!({
        "source": source_label,
        "seed": world.seed,
        "turn": world.turn,
        "day": world.day,
        "living": world.living().count(),
        "agents": world.agents.len(),
        "events": world.events.len(),
        "save_bytes": save_bytes,
    });

    let fixture = json!({
        "schema": "society.perf-baseline/1",
        "generated_at": cmd_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| "unknown".to_owned()),
        "iterations": args.iterations,
        "warmup": args.warmup,
        "tolerance": TOLERANCE,
        "jitter_floor_ms": JITTER_FLOOR_MS,
        "p95_headroom": P95_HEADROOM,
        "machine": machine_json(),
        "world": world_meta,
        "paths": reports.iter().map(to_json).collect::<Vec<_>>(),
    });

    if args.check {
        let code = check_against(&reports, &args.fixture);
        std::process::exit(code);
    }

    let out = std::path::PathBuf::from(&args.fixture);
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("society-bench: cannot create `{}`: {e}", dir.display());
            std::process::exit(2);
        }
    }
    let text = match serde_json::to_string_pretty(&fixture) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("society-bench: cannot render the fixture: {e}");
            std::process::exit(2);
        },
    };
    if let Err(e) = std::fs::write(&out, format!("{text}\n")) {
        eprintln!("society-bench: cannot write `{}`: {e}", out.display());
        std::process::exit(2);
    }
    println!("society-bench: baseline written to `{}`", out.display());
    print_table(&reports);
}

/// Compare this run against the recorded budgets. Returns the process exit
/// code: 0 when every path is inside its budget, 1 when one is not.
fn check_against(reports: &[PathReport], fixture_path: &str) -> i32 {
    let path = std::path::PathBuf::from(fixture_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "society-bench: no baseline at `{}` to compare against",
            path.display()
        );
        return 2;
    };
    let Ok(baseline) = serde_json::from_str::<Value>(&text) else {
        eprintln!(
            "society-bench: `{}` is not readable as a baseline",
            path.display()
        );
        return 2;
    };
    let Some(paths) = baseline.get("paths").and_then(Value::as_array) else {
        eprintln!("society-bench: `{}` has no paths", path.display());
        return 2;
    };

    println!("\n| path | p50 now | p50 base | p99 now | p99 budget | alloc now | alloc budget | verdict |");
    println!("|---|---|---|---|---|---|---|---|");
    let mut failed = false;
    for r in reports {
        let base = paths
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(r.name));
        let (base_p50, p99_budget, alloc_budget) = match base {
            Some(b) => (
                b.pointer("/latency_ms/p50")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                b.pointer("/budget/p99_ms")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                b.pointer("/budget/alloc_bytes_per_iter")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            ),
            None => (f64::NAN, f64::NAN, 0),
        };
        let over_p99 = r.p99_ms > p99_budget;
        let over_alloc = r.alloc_bytes_per_iter > alloc_budget;
        let verdict = if over_p99 || over_alloc {
            failed = true;
            "REGRESSION"
        } else {
            "ok"
        };
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} | {} |",
            r.name,
            r.p50_ms,
            base_p50,
            r.p99_ms,
            p99_budget,
            r.alloc_bytes_per_iter,
            alloc_budget,
            verdict
        );
    }
    if failed {
        println!("\nsociety-bench: a path is outside its budget.");
        1
    } else {
        println!("\nsociety-bench: every path is inside its budget.");
        0
    }
}

fn print_table(reports: &[PathReport]) {
    println!("\n| path | p50 (ms) | p95 (ms) | p99 (ms) | max (ms) | alloc B/iter | payload B | live growth (B) | RSS delta (KB) |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in reports {
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {} | {} | {} |",
            r.name,
            r.p50_ms,
            r.p95_ms,
            r.p99_ms,
            r.max_ms,
            r.alloc_bytes_per_iter,
            r.output_bytes,
            r.live_growth_bytes,
            r.rss_after_kb - r.rss_before_kb
        );
    }
}

// ---------------------------------------------------------------------------
// Stage breakdown
// ---------------------------------------------------------------------------

/// The envelope as the bench sees it. `persistence::EnvelopeRef` is private to
/// the server; this mirrors its shape so a stage is timed on the bytes the
/// save actually writes.
#[derive(serde::Serialize)]
struct EnvelopeRef<'a> {
    version: u32,
    world: &'a World,
}

#[derive(serde::Deserialize)]
struct EnvelopeOwned {
    #[allow(
        dead_code,
        reason = "the field exists so the parse matches the save verification"
    )]
    version: u32,
    #[allow(dead_code, reason = "the parse must build the world the save verifies")]
    world: World,
}

fn breakdown(world: &World, iterations: usize, warmup: usize, dir: &std::path::Path) {
    let path = dir.join("village.json");
    let _ = std::fs::remove_dir_all(dir);
    let envelope = EnvelopeRef {
        version: society_server::persistence::SAVE_VERSION,
        world,
    };
    let bytes = serde_json::to_vec(&envelope).expect("the world serialises");
    // A real turn record, so the journal measures the row it writes in place.
    let record = {
        let mut w = world.clone();
        society_sim::turn::step(&mut w).expect("the turn runs")
    };
    let context = society_server::telemetry::Context::default();

    let stage = |name: &str, m: &Measurement| {
        let p50 = ms(percentile(&sorted_nanos(m), 0.50));
        let p99 = ms(percentile(&sorted_nanos(m), 0.99));
        let alloc = m.alloc_bytes / m.nanos.len().max(1) as u64;
        println!("  {name:<28} p50 {p50:>8.3} ms  p99 {p99:>8.3} ms  {alloc:>9} B/iter");
    };

    println!(
        "society-bench --breakdown: {iterations} iterations, {warmup} warmup, {} bytes of save",
        bytes.len()
    );

    let m = measure(
        || {},
        || {
            std::hint::black_box(serde_json::to_vec(&envelope).unwrap());
        },
        || {},
        iterations,
        warmup,
    );
    stage("save.serialize.grow", &m);

    // What `persistence::save` does: the buffer starts at the size of the save
    // being replaced, so the village is not copied through the growth ladder.
    let m = measure(
        || {},
        || {
            let mut bytes = Vec::with_capacity(bytes.len() + 4096);
            serde_json::to_writer(&mut bytes, &envelope).unwrap();
            std::hint::black_box(bytes.len());
        },
        || {},
        iterations,
        warmup,
    );
    stage("save.serialize.presized", &m);

    let m = measure(
        || {},
        || {
            std::hint::black_box(serde_json::from_slice::<EnvelopeOwned>(&bytes).unwrap());
        },
        || {},
        iterations,
        warmup,
    );
    stage("save.verify_parse", &m);

    let tmp = dir.join("write.json.tmp");
    let _ = std::fs::create_dir_all(dir);
    let m = measure(
        || {},
        || {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(&bytes).unwrap();
            f.flush().unwrap();
            f.sync_all().unwrap();
            drop(f);
            std::fs::rename(&tmp, &path).unwrap();
        },
        || {},
        iterations,
        warmup,
    );
    stage("save.write_fsync_rename", &m);

    let m = measure(
        || {},
        || {
            society_server::persistence::save(world, &path).expect("the save lands");
        },
        || {},
        iterations,
        warmup,
    );
    stage("save.total", &m);

    let m = measure(
        || {},
        || {
            std::hint::black_box(society_server::persistence::load(&path).unwrap().unwrap());
        },
        || {},
        iterations,
        warmup,
    );
    stage("load.total", &m);

    let m = measure(
        || {},
        || {
            society_server::diagnostics::append_turn_context(
                &path,
                world.seed,
                &record,
                "breakdown",
                society_server::persistence::SAVE_VERSION,
                context.clone(),
            )
            .expect("the journal appends");
        },
        || {},
        iterations,
        warmup,
    );
    stage("journal.append", &m);

    let m = measure(
        || {},
        || {
            society_server::diagnostics::append_audit_context(
                &path,
                "turn_completed",
                world.turn,
                json!({"breakdown": true}),
                context.clone(),
            )
            .expect("the audit appends");
        },
        || {},
        iterations,
        warmup,
    );
    stage("audit.append", &m);

    // The observer's evidence reads, which is where /health, /log and
    // /diagnostics spend their time. Both readers see the same journal, so the
    // comparison does not depend on what else the machine is doing.
    let journal = society_server::diagnostics::journal_path(&path);
    println!(
        "  journal bytes now {}",
        std::fs::metadata(&journal).map_or(0, |m| m.len())
    );
    let m = measure(
        || {},
        || {
            let evidence = society_server::diagnostics::scan_journal(&path, 0, 1000);
            std::hint::black_box(evidence.entries.len());
        },
        || {},
        iterations,
        warmup,
    );
    stage("journal.scan_typed", &m);

    let m = measure(
        || {},
        || {
            let growth = society_server::diagnostics::journal_growth(&path, 1000);
            std::hint::black_box(growth.turns);
        },
        || {},
        iterations,
        warmup,
    );
    stage("journal.growth", &m);

    let m = measure(
        || {},
        || {
            let evidence = society_server::diagnostics::scan_audit(&path, 1000);
            std::hint::black_box(evidence.entries.len());
        },
        || {},
        iterations,
        warmup,
    );
    stage("audit.scan", &m);
}

fn sorted_nanos(m: &Measurement) -> Vec<u64> {
    let mut sorted = m.nanos.clone();
    sorted.sort_unstable();
    sorted
}
