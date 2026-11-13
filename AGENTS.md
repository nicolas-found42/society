# Repository Guidelines

Agent-facing guide for the `society` codebase. Domain vocabulary lives in `GLOSSARY.md`; read it before naming things.

## Project Overview

A small offline settlement simulation: a village of named villagers lives turn by turn under an in-code engine (policy → speech → grounding check → action → settlement). A desktop observer window (egui) and a localhost HTTP control API watch and drive the same engine. No runtime network, no models — a cloud judgment layer and a local voice model were replaced by code (see `docs/adr/0001-replace-the-judgment-layer-and-the-voice-model-with-code.md`).

## Architecture & Data Flow

Three crates, acyclic dependencies: `society` (desktop bin) → `society-server` (shell lib) → `society-sim` (pure engine lib).

- **`society-sim`** (`crates/sim`) — deterministic state machine. No IO, no network, no clock (`src/lib.rs:1-5`). One turn is `turn::step(&mut World)` (`src/turn.rs:177`): `policy::decide` → `speaker::speak` → `check::grounded` → `action::apply_decision` → `World::settle`. RNG is SplitMix64 with per-agent streams (`policy::agent_rng`); determinism is a hard invariant.
- **`society-server`** (`crates/server`) — the shell: persistence, JSONL journal + audit, telemetry, axum 0.8 control API on `127.0.0.1` only (`DEFAULT_PORT = 8100`). Every committed turn: save + journal append + audit entry + bounded 240-record in-memory log (`TURN_LOG_CAP`). `GET /health /state /social /agent/{name} /places/{id} /events /log /diagnostics /verify`; `POST /turn /reset /inject /halt`.
- **`society`** (`crates/desktop`) — clap CLI + egui observer window + `--headless` mode. The window is read-mostly: it polls `AppState` with `try_lock`, runs turns via `rt.spawn`, and bridges results over `std::sync::mpsc`. Startup: `main.rs` builds a tokio runtime → `society_server::boot(seed, save_path, reset)` → spawns `devserver::serve` → window or Ctrl-C wait.

Layering caution: village civic systems (festivals, council/decrees, factions) live in `crates/desktop/src/window.rs` (`apply_village_systems`), **not** in the sim. `POST /turn` and window/warmup turns therefore diverge on village evolution. Confirm the intended layer before moving or extending such mechanics.

## Key Directories

| Path | Purpose |
|---|---|
| `crates/sim` | Engine: `world`, `turn`, `policy`, `action`, `catalog`, `speaker`, `check`, `memory`, `grammar*` |
| `crates/server` | Shell: `devserver` (API), `persistence`, `diagnostics`, `telemetry`, `build.rs`, `bin/parity.rs` |
| `crates/desktop` | Bin `society`: `main.rs` (CLI/runtime), `window.rs` (UI + civic systems) |
| `docs/agents` | Agent workflow: issue tracker, triage labels, domain docs |
| `docs/adr` | Architecture decision records (ADR format: Status / Why / Considered options / Consequences / Outcome) |
| `baseline/` | Parity evidence (`corpus.jsonl`, round outputs, `PARITY_REPORT.md`) — consumed by the `parity` bin, not by `cargo test` |
| `data/` | Gitignored runtime save (`society.json` + journal/diagnostics siblings) |
| `.scratch/` | Gitignored local scratch (specs, probes, captured runs). **Not** the issue tracker |
| `tools/` | `evidence.py`: offline reader for the journal/audit/crash files; `verify-observability.py`: end-to-end retained-evidence exercise; `shot.sh`/`shot.swift`: macOS window screenshot helper (builds `shot-bin`, gitignored) |
| `models/` | Research notes (tracked) |

## Development Commands

```bash
cargo run -p society --release              # window + control API on :8100
cargo run -p society --release -- --headless
cargo run -p society --release -- --view feed --warmup 10
cargo run -p society --release -- --reset   # discard the village, start over
SOCIETY_SAVE_PATH=data/second.json cargo run -p society --release -- --port 8101

cargo test --workspace                      # offline, no models
cargo check --all-targets                   # fast gate (-D warnings applies)
cargo clippy --workspace --all-targets      # full lint gate (clippy lints need clippy)
cargo fmt                                   # rustfmt.toml, stable channel

cargo run -p society-server --bin parity -- decisions <round>   # dev-only judge harness
python3 tools/evidence.py --limit 5            # read retained evidence, no running app
python3 tools/verify-observability.py        # exercise the retained-evidence paths end to end
tools/shot.sh [out.png] [delay]             # screenshot the window (macOS, needs swiftc)
```

Notes: the desktop package is named `society` while its directory is `crates/desktop` — always pass `-p society`. The parity harness reads `baseline/corpus.jsonl` relative to CWD, so run it from the repo root; it needs `JEV_API_URL`/`JEV_API_KEY` (optional `JEV_MODEL`, default `typesafe/jev-1.14`).

## Code Conventions & Common Patterns

Formatting (`rustfmt.toml`): edition 2021, Unix newlines, `use_try_shorthand`, `use_field_init_shorthand`, reordered imports/modules, `match_block_trailing_comma`, `force_explicit_abi`. `cargo fmt` on stable suffices.

Lints — the bar is a clippy-clean, rustdoc-clean tree (`.cargo/config.toml` sets `rustflags = ["-D", "warnings"]` and `RUSTDOCFLAGS = "-D warnings"` for all targets):

- `unsafe_code = "forbid"` everywhere, including tests. No `macro_rules!`, no generated/protobuf code in the product path.
- Workspace lints (`Cargo.toml`, opted into per crate via `[lints] workspace = true`): clippy `all` + `pedantic` + `nursery` plus a restriction set. Practical consequences: `str::to_string` over `to_owned`; no `Rc<Mutex<_>>`/`mem::forget`; no `todo!`/`unimplemented!`/`dbg!`; floats never `==`-compared outside consts.
- `unwrap_used`/`panic` are warned outside tests; `expect("invariant …")` is the sanctioned way to state invariants. `clippy.toml` allows unwrap/expect/panic/print inside tests.
- Every `#[allow]` needs `reason = "…"`; crate-level exception blocks (e.g. `crates/sim/src/lib.rs:9-14` cast lints, `crates/desktop/src/main.rs` cast/length lints) must be left in place.

Error handling — no `thiserror`/`anyhow`:

- sim: `Result<_, String>` and `Option` for "no decision / no line / no agent". `turn::step` returns `Err` as a stop signal (halted or empty population); callers treat it as terminal.
- server: `SaveError` (manual `Display` + `Error`, `persistence.rs:58`); `boot` returns `Box<dyn std::error::Error>`; HTTP handlers return `(StatusCode, Json<Value>)` with `{"error": …}` bodies.
- desktop: `eprintln!` + `exit(1|2)` in `main.rs`.

Async & state management:

- tokio multi-thread runtime built by hand in `main.rs`; shared state is `Arc<tokio::sync::Mutex<…>>` inside `AppState`. Handlers intentionally hold the world lock across awaits; `blocking_lock` is used only in `boot` (which therefore must stay outside async context).
- UI thread never awaits: `try_lock` poll + `std::sync::mpsc` for turn results.
- Correlation via `tokio::task_local!` `OPERATION` and `telemetry::Context`; mutations get an `x-society-operation-id` header.

Domain patterns:

- Names are join keys (villager name, place id, action id, `act:<id>` grammar cells). Wire enums use `#[serde(rename_all = "snake_case")]` with `const fn id(self) -> &'static str` + `from_id`.
- Game data is `const`/`static` tables (`ACTIONS` in `catalog.rs`, `PROFESSIONS`, `TERMS`, grammar cells in `grammar_first.rs`/`grammar_second.rs`): extend by adding a row, not a branch. Tests assert table↔grammar coverage.
- Every new serialized field takes `#[serde(default)]` so old saves load; incompatibility bumps `SAVE_VERSION` (`persistence.rs:22`). `f32` scores are rounded to 2 decimals at storage boundaries.
- Module doc comments cite spec/ticket numbers (`#5`, `ticket #12`) — keep this traceability habit.
- Several files are deliberately huge with sanctioned long functions (`window.rs` ~4.5k lines, `world.rs` ~2.1k); `too_many_lines`/`struct_excessive_bools` allows carry reasons. Grep for anchors before editing.

Determinism rules for `crates/sim`: no `SystemTime`, no randomness outside `rng.rs`, no iteration-order-dependent `HashMap`; timestamps belong to `diagnostics`. `World.events` grows unbounded by design — `AgentTurn.event_range` indexes into it, so do not trim events.

## Important Files

| File | Role |
|---|---|
| `Cargo.toml` / `Cargo.lock` | Virtual workspace, `[workspace.lints]`, size-oriented release profile; lockfile v4 committed |
| `.cargo/config.toml` | `-D warnings` + `RUSTDOCFLAGS` only |
| `rustfmt.toml` / `clippy.toml` | Format and test-only lint allowances |
| `crates/sim/src/lib.rs` | Curated public API (private modules re-exported); `turn` is the only `pub mod` |
| `crates/sim/src/world.rs` | `World`, `Agent`, `Event`; `founding`, `settle`, `halt` |
| `crates/sim/src/turn.rs` | `turn::step`, `AgentTurn` (`decision_id = "{seed}:{turn}:{name}"`) |
| `crates/server/src/lib.rs` | `boot`, `push_bounded`, `TURN_LOG_CAP` |
| `crates/server/src/devserver.rs` | axum router, `serve`, observer/agent endpoints |
| `crates/server/src/persistence.rs` | Save/load, `SOCIETY_SAVE_PATH`, atomic save |
| `crates/server/src/diagnostics.rs` | Journal + `/verify` replay divergence |
| `crates/server/build.rs` | Stamps `SOCIETY_SOURCE_ID` from git HEAD + source hash — any source edit rebuilds the server crate |
| `crates/desktop/src/main.rs` | CLI flags, runtime, warmup loop |
| `crates/desktop/src/window.rs` | egui app, civic systems, headless render tests |
| `GLOSSARY.md` | Authoritative domain vocabulary (e.g. Villager, Intent) — reuse its terms, not synonyms |
| `README.md` | User-facing run commands and architecture rationale |
| `docs/agents/*.md` | Agent workflow rules (below) |

Agent workflow (details in `docs/agents/`):

- Issues and specs live as GitHub issues in `nicolas-found42/society`, managed with `gh` (create/view/list/comment/label/close; sub-issues via `gh issue create --parent` or `Part of #<n>` in the body). Repo is inferred from `git remote -v`.
- Triage labels: five canonical roles, each label string identical to its role name (`docs/agents/triage-labels.md`).
- Domain docs: `GLOSSARY.md` plus `docs/adr/`; when an ADR conflicts with code, follow the ADR and call out the divergence in the format from `docs/agents/domain.md`.
- `.scratch/` is not the issue tracker — publish tickets as GitHub issues.

## Runtime/Tooling Preferences

- Cargo-only build; no `rust-toolchain` pin (floats on installed stable, edition 2021). No node/bun, no nextest/insta, no CI, no Makefile/justfile.
- The dev server hard-binds `127.0.0.1`; default port 8100 is pinned by a test.
- Runtime is fully offline; the only outbound calls are in the dev-only `parity` bin (remote judge).
- `swiftc` (Xcode CLT) is needed only for `tools/shot.sh`, macOS only. `gh` CLI is needed for the issue workflow.
- macOS note: `window.rs` uses `with_activate_ignoring_other_apps(focus)` so the observer window never steals focus — intentional; do not simplify away.

## Testing & QA

- No `tests/` directories, no dev-dependencies, no snapshot/proptest frameworks. All tests are inline `#[cfg(test)] mod tests`; the sim suite body is `crates/sim/src/tests.rs` (~1.8k lines) declared from `lib.rs`.
- `society-sim`: synchronous behavioral unit tests + hand-rolled property checks (fixed seeds, soak = seeds × 200 turns with a per-turn time bound, save-round-trip determinism via serialized-JSON comparison).
- `society-server`: `#[tokio::test]` in `devserver.rs` (journal, audit, tamper, `/health`) plus `persistence.rs`/`diagnostics.rs` tests using pid-scoped temp dirs that self-delete.
- `society` (desktop): headless egui render tests in `window.rs` — fixed `screen_rect`, scrape `Shape::Text`; no display needed.
- Running: `cargo test --workspace`, or `cargo test -p society-sim|society-server|society`, or `cargo test -p society-sim <test_name> -- --nocapture`. Because of `-D warnings`, any warning fails test compiles; `cargo check --all-targets` is the fast gate.
- No coverage tooling and no CI — the quality bar is behavioral: tests pin the rules that are easy to get wrong and expensive to get wrong (affordance legality, no negative stores, memory caps/scars, starvation timing, broken promises permanent), table/grammar coverage floors, and determinism seams (pure sim births; save/load against temp dirs).
- Fixtures: none. Tests never read `data/` and need no env vars. `baseline/` is evidence for the parity harness, not test input.
