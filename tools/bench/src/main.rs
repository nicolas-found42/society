//! Standalone latency and allocation baseline for the society.
//!
//! Three paths carry the wall clock of a living village, and they are measured
//! here on this machine so no later claim about speed is an estimate:
//!
//! 1. `engine_turn` — `society_sim::turn::step`, the committed turn: the
//!    policy decides, the speaker renders the line, the check settles it, the
//!    intent is applied and the world settles. Runs once per committed turn.
//! 2. `durable_save` — `society_server::persistence::save`, the write that
//!    makes a turn durable: serialise the world, verify it loads back, write a
//!    temporary file, fsync it, rename it over the target. Runs once per
//!    committed turn, immediately after the turn.
//! 3. `observer_snapshot` — `society_server::devserver::world_snapshot` plus
//!    the wire bytes, the payload every observer of the control API reads
//!    (`/state`, `/social`). Runs per observer request.
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
const DEFAULT_FIXTURE: &str = "baseline/perf_baseline.json";
/// A path may be this much slower than the recorded baseline before the
/// check fails: 25% on p99 and on per-iteration allocation.
const TOLERANCE: f64 = 1.25;
/// A sub-millisecond path is jitter, not regression: the scheduler alone moves
/// its p99 by more than 25%. The budget is therefore never tighter than the
/// recorded p99 plus this floor.
const JITTER_FLOOR_MS: f64 = 0.25;
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
    fixture: String,
    source: String,
    check: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        iterations: DEFAULT_ITERATIONS,
        warmup: DEFAULT_WARMUP,
        fixture: DEFAULT_FIXTURE.to_owned(),
        source: society_server::persistence::save_path()
            .to_string_lossy()
            .into_owned(),
        check: false,
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
            "--out" => {
                args.fixture = it.next().ok_or("--out needs a path")?;
            },
            "--source" => {
                args.source = it.next().ok_or("--source needs a path")?;
            },
            "--check" => args.check = true,
            "-h" | "--help" => {
                println!(
                    "society-bench [--iterations N] [--warmup N] [--out PATH] [--source SAVE] [--check]\n\
                     \n\
                     Measures the three latency-critical paths of the society and writes\n\
                     the baseline to baseline/perf_baseline.json. --check compares the\n\
                     run against the recorded budgets and exits non-zero on a regression."
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

    let source_path = std::path::PathBuf::from(&args.source);
    let world = match society_server::persistence::load(&source_path) {
        Ok(Some(w)) => w,
        Ok(None) => {
            eprintln!(
                "society-bench: no village at `{}`, founding a fresh one from seed 7",
                source_path.display()
            );
            World::founding(7)
        },
        Err(e) => {
            eprintln!(
                "society-bench: cannot read `{}`: {e}",
                source_path.display()
            );
            std::process::exit(2);
        },
    };

    let world_meta = json!({
        "source": source_path.to_string_lossy(),
        "seed": world.seed,
        "turn": world.turn,
        "day": world.day,
        "living": world.living().count(),
        "agents": world.agents.len(),
        "events": world.events.len(),
        "save_bytes": source_path.metadata().map_or(0, |m| m.len()),
    });

    println!(
        "society-bench: {} iterations per path, {} warmup, world at turn {} ({} events, {} agents)",
        args.iterations,
        args.warmup,
        world.turn,
        world.events.len(),
        world.agents.len(),
    );

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
            *slot.borrow_mut() = Some(turn_world.clone());
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

    let reports = [turn_report, save_report, snapshot_report];
    for r in &reports {
        println!(
            "  {:<18} p50 {:>8.3} ms  p95 {:>8.3} ms  p99 {:>8.3} ms  {:>9} B/iter",
            r.name, r.p50_ms, r.p95_ms, r.p99_ms, r.alloc_bytes_per_iter
        );
    }

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
