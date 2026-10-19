//! `society` — a persistent society of small local models.
//!
//! Two run modes share one engine:
//!   `society`            — native window, dev server on :8100
//!   `society --headless` — dev server only, for scripted runs
//!
//! The dev server is the harness: it lets an agent inspect and drive the same
//! process the window is showing.

use clap::Parser;
use society_server::{boot, devserver, DEFAULT_PORT};

#[derive(Parser, Debug)]
#[command(name = "society", about = "A small society of local models")]
struct Args {
    /// Run without opening a window.
    #[arg(long)]
    headless: bool,
    /// Control-surface port on 127.0.0.1.
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
    /// Concurrent voice calls. 16 measured clean on qwen3:0.6b.
    #[arg(long, default_value_t = 8)]
    permits: usize,
    /// Advance this many turns immediately, then continue live.
    #[arg(long, default_value_t = 0)]
    warmup: usize,
    /// Open on this view: world, feed, or inspector.
    #[arg(long, default_value = "world")]
    view: String,
    /// Inspector selection; defaults to the first villager. Lets a capture
    /// script show a particular person without clicking.
    #[arg(long)]
    select: Option<String>,
    /// Open the inspector's judgment-state disclosure. Capture aid: a
    /// screenshot cannot click a disclosure arrow.
    #[arg(long)]
    expand_state: bool,
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
}

fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| "expected WIDTHxHEIGHT, e.g. 900x600".to_string())?;
    let w: f32 = w.trim().parse().map_err(|_| "bad width".to_string())?;
    let h: f32 = h.trim().parse().map_err(|_| "bad height".to_string())?;
    Ok((w, h))
}

fn main() {
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let save_path = society_server::persistence::save_path();
    let state = match boot(args.seed, args.permits, &save_path, args.reset) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot start: {e}");
            if std::env::var_os("OPENROUTER_API_KEY").is_none() {
                eprintln!("the judgment layer is required; set OPENROUTER_API_KEY");
            }
            std::process::exit(2);
        }
    };
    // Boot has finished and no turn has started, so the lock is free; a
    // blocked try_lock here would mean a bug, not contention, and reporting
    // nothing beats hanging before the window opens.
    let (turn, seed) = state
        .world
        .try_lock()
        .map(|w| (w.turn, w.seed))
        .unwrap_or((0, args.seed));
    let origin = if args.reset {
        "reset"
    } else {
        "loaded or founded"
    };
    println!(
        "village: {origin} · seed {seed} · turn {turn} · save {}",
        save_path.display()
    );

    let handle = rt.handle().clone();
    let server_state = state.clone();
    let port = args.port;
    rt.spawn(async move {
        if let Err(e) = devserver::serve(server_state, port).await {
            eprintln!("dev server failed: {e}");
        }
    });

    if args.warmup > 0 {
        let st = state.clone();
        let n = args.warmup;
        rt.block_on(async move {
            let mut w = st.world.lock().await;
            for i in 0..n {
                match st.engine.step(&mut w).await {
                    Ok(rec) => {
                        println!(
                            "[warmup {}/{}] turn {} — {} agent turns",
                            i + 1,
                            n,
                            rec.turn,
                            rec.agents.len()
                        );
                        st.record(std::iter::once(rec)).await;
                    }
                    Err(e) => {
                        eprintln!("warmup halted: {e}");
                        break;
                    }
                }
            }
        });
    }

    if args.headless {
        println!("headless society on http://127.0.0.1:{port}");
        println!("  GET  /state  /agent/:name  /events  /log  /health");
        println!("  POST /turn  /reset  /inject  /halt");
        rt.block_on(async {
            tokio::signal::ctrl_c().await.ok();
        });
        return;
    }

    if let Err(e) = window::run(
        handle,
        state,
        port,
        &args.view,
        args.select.as_deref(),
        args.expand_state,
        args.no_select,
        args.size,
        !args.paused,
        args.focus,
    ) {
        eprintln!("window failed: {e}");
        std::process::exit(1);
    }
}

mod window;
