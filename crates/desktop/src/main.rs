//! `society`: a persistent society that runs on code.
//!
//! Two run modes share one engine:
//!   `society`             native window, dev server on :8100
//!   `society --headless`  dev server only, for scripted runs
//!
//! The dev server lets an agent inspect and drive the same process the window
//! is showing.

// A drawing and UI crate. Pixel and layout math is f32 on small counts, `App`
// and the CLI are bags of independent toggles, and `egui` frame functions read
// as one top-to-bottom layout, so splitting them hides the screen's structure.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    reason = "egui layout math and top-to-bottom frame functions"
)]

use clap::Parser;
use society_server::{boot, devserver, DEFAULT_PORT};

#[derive(Parser, Debug)]
#[command(name = "society", about = "A small society that runs on code")]
struct Args {
    /// Run without opening a window.
    #[arg(long)]
    headless: bool,
    /// Control API port on 127.0.0.1.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// World seed; the same seed replays the same founding. Ignored when a
    /// save exists, so restarting never replaces a living village.
    #[arg(long, default_value_t = 7)]
    seed: u64,
    /// Discard the saved village and found a new one. Explicit, because a
    /// seed alone must not overwrite a village that already lived.
    #[arg(long)]
    reset: bool,
    /// Advance this many turns immediately, then continue live.
    #[arg(long, default_value_t = 0)]
    warmup: usize,
    /// Open on this view: world, council, feed, or inspector.
    #[arg(long, default_value = "world")]
    view: String,
    /// Inspector selection; defaults to the first villager. Lets a capture
    /// script show a particular person without clicking.
    #[arg(long)]
    select: Option<String>,
    /// Open the inspector's decision-factors disclosure. Capture aid: a
    /// screenshot cannot click a disclosure arrow.
    #[arg(long)]
    expand_factors: bool,
    /// Open with no inspector selection, which is what an empty inspector
    /// looks like before the observer picks a villager. Capture aid.
    #[arg(long)]
    no_select: bool,
    /// Window size in logical points, overriding the default 1120x760. Capture
    /// aid: the layout has to be legible at the minimum size too.
    #[arg(long, value_parser = parse_size)]
    size: Option<(f32, f32)>,
    /// Bring the observer window to the front on launch.
    #[arg(long)]
    focus: bool,
    /// Open paused instead of advancing turns on its own.
    #[arg(long)]
    paused: bool,
    /// Initial playback multiplier (one turn per second at 1x).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=100))]
    speed: u16,
    /// Print a village civic and economic digest on startup.
    #[arg(long)]
    digest: bool,
}

fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| "expected WIDTHxHEIGHT, e.g. 900x600".to_owned())?;
    let w: f32 = w.trim().parse().map_err(|_bad| "bad width".to_owned())?;
    let h: f32 = h.trim().parse().map_err(|_bad| "bad height".to_owned())?;
    Ok((w, h))
}

fn main() {
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let save_path = society_server::persistence::save_path();
    society_server::telemetry::install_panic_hook(save_path.clone());
    let state = match boot(args.seed, &save_path, args.reset) {
        Ok(s) => s,
        Err(e) => {
            // This row is the only retained record of a boot that never
            // started; if the store itself is unwritable, say so on stderr
            // instead of leaving the absence unexplained.
            if let Err(audit) = society_server::diagnostics::append_audit(
                &save_path,
                "boot_failed",
                0,
                serde_json::json!({"stage": "load_or_save"}),
            ) {
                eprintln!("cannot record boot failure: {audit}");
            }
            eprintln!("cannot start: {e}");
            std::process::exit(2);
        },
    };
    // Boot has finished and no turn has started, so the lock is free; a
    // blocked try_lock here would mean a bug, not contention, and reporting
    // nothing beats hanging before the window opens.
    let (turn, seed, pop, season, weather) = state.world.try_lock().map_or_else(
        |_| (0, args.seed, 0, "spring".into(), "clear".into()),
        |w| {
            (
                w.turn,
                w.seed,
                w.living().count(),
                w.season().id().to_owned(),
                w.weather_kind.id().to_owned(),
            )
        },
    );
    let origin = if args.reset {
        "reset"
    } else {
        "loaded or founded"
    };
    println!(
        "village: {origin} · seed {seed} · turn {turn} · pop {pop} · {season}/{weather} · save {}",
        save_path.display()
    );
    if args.digest {
        if let Ok(w) = state.world.try_lock() {
            println!("{}", window::format_village_digest(&w));
        }
    }

    state.audit(
        "launch",
        turn,
        serde_json::json!({"headless": args.headless, "port": args.port,
        "paused": args.paused, "warmup": args.warmup}),
    );
    let handle = rt.handle().clone();
    let server_state = state.clone();
    let port = args.port;
    rt.spawn(async move {
        if let Err(e) = devserver::serve(server_state.clone(), port).await {
            server_state.audit(
                "server_failed",
                0,
                serde_json::json!({"port": port, "io_kind": format!("{:?}", e.kind())}),
            );
            eprintln!("dev server failed: {e}");
        }
    });

    if args.warmup > 0 {
        let st = state.clone();
        let n = args.warmup;
        #[allow(
            clippy::significant_drop_tightening,
            reason = "the world lock spans the whole warmup"
        )]
        rt.block_on(society_server::telemetry::OPERATION.scope(society_server::telemetry::operation_id(), async move {
            let mut w = st.world.lock().await;
            let mut completed = 0_usize;
            for i in 0..n {
                let started = st.start_step(&w, "warmup");
                match society_sim::turn::step(&mut w) {
                    Ok(rec) => {
                        let civic_notes = window::apply_village_systems(&mut w);
                        let (committed, agents) = (rec.turn, rec.agents.len());
                        completed += 1;
                        let saved = st.commit_turn(&w, &rec, "warmup", started).await;
                        st.record(std::iter::once(rec)).await;
                        if let Err(e) = saved {
                            eprintln!("turn {committed} committed but not saved: {e}");
                            break;
                        }
                        println!(
                            "[warmup {}/{}] turn {}: {} agent turns ({} civic events)",
                            i + 1,
                            n,
                            committed,
                            agents,
                            civic_notes
                        );
                    },
                    Err(e) => {
                        st.audit("turn_failed", w.turn, serde_json::json!({"source": "warmup", "halted": w.halted.is_some()}));
                        eprintln!("warmup halted: {e}");
                        break;
                    },
                }
            }
            let world_turn = w.turn;
            st.audit(
                "warmup",
                world_turn,
                serde_json::json!({"committed": completed}),
            );
        }));
    }

    if args.headless {
        println!("headless society on http://127.0.0.1:{port}");
        println!("  GET  /state  /agent/:name  /events  /log  /health  /diagnostics  /verify");
        println!("  POST /turn  /reset  /inject  /halt");
        rt.block_on(async {
            tokio::signal::ctrl_c().await.ok();
        });
        state.audit("shutdown", 0, serde_json::json!({"source": "ctrl_c"}));
        return;
    }

    let window_state = state.clone();
    if let Err(e) = window::run(
        handle,
        state,
        window::Launch {
            port,
            view: &args.view,
            select: args.select.as_deref(),
            expand_factors: args.expand_factors,
            select_none: args.no_select,
            size: args.size,
            autostart: !args.paused,
            speed: args.speed,
            focus: args.focus,
        },
    ) {
        window_state.audit("window_failed", 0, serde_json::json!({"stage": "run"}));
        eprintln!("window failed: {e}");
        std::process::exit(1);
    }
    window_state.audit(
        "shutdown",
        0,
        serde_json::json!({"source": "window_closed"}),
    );
}

mod window;
