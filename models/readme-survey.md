# README structure survey: what comparable projects do, and what `society` should take

Question: how do projects comparable to `society` structure their READMEs — section order, two-audience handling, GIF/asset conventions — and what should `society`'s rewrite take from them?

Short answer: the strong READMEs all open **pitch → visual → quickstart**, then diverge by depth strategy: single long file with in-file FAQ/troubleshooting (`bat`, `PMXSpan`, `partiworld`), README-as-index plus a docs site (`bottom`, `bevy`, `mesa`), or README plus one architecture doc (`ai-town` → `ARCHITECTURE.md`, `hospital-generative-agents` → `CLAUDE.md`, `ripgrep` → `GUIDE.md`/`FAQ.md`). GitHub renders in-repo images/GIFs up to 10 MB and embeddable video (`mp4`/`mov`/`webm`) in every context; the surveyed projects ship 64 KB–4.2 MB in-repo assets, one hero GIF (2.1 MB, `bottom`) or captioned screenshot grids. For `society`, the evidence points to: keep the README to pitch + hero GIF + 3-command quickstart + tour + pointers, move the architecture essay and deep numbers to the existing `docs/`/`baseline/` files, and ship two scripted, reproducible GIFs under ~3 MB each.

Method: every README below was fetched as raw text on 2026-11-13 and read end to end (line counts are from the fetched text). Asset byte sizes are from the GitHub contents API. Platform limits are from GitHub Docs, quoted verbatim. §7 lists every source URL and what is explicitly unverified.

## 1. Projects surveyed

| # | Project | Bucket | README source | Lines | Visual in README | Depth strategy |
|---|---|---|---|---|---|---|
| 1 | `joonspk-research/generative_agents` (Stanford Smallville) | agent-sim | https://raw.githubusercontent.com/joonspk-research/generative_agents/main/README.md | 135 | `cover.png`, key art at 80% width | single file, all inline |
| 2 | `a16z-infra/ai-town` | agent-sim | https://raw.githubusercontent.com/a16z-infra/ai-town/main/README.md | 719 | hero screenshot, `width="1454"` | README + `ARCHITECTURE.md` |
| 3 | `StanfordHCI/genagents` | agent-sim | https://raw.githubusercontent.com/StanfordHCI/genagents/main/README.md | 264 | `static_dir/cover3.png` | single file + full TOC |
| 4 | `nickm980/smallville` | agent-sim | https://raw.githubusercontent.com/nickm980/smallville/main/README.md | 101 | none | single thin file (negative example) |
| 5 | `georgia-max/hospital-generative-agents` | agent-sim | https://raw.githubusercontent.com/georgia-max/hospital-generative-agents/main/README.md | 235 | `assets/hospital-replay.png` + long alt text | README + `CLAUDE.md` + `plans/` |
| 6 | `projectmesa/mesa` | agent-sim | https://raw.githubusercontent.com/projectmesa/mesa/main/README.md | 142 | `docs/images/wolf_sheep.png` + online-demo link | README + readthedocs |
| 7 | `Fanaperana/partiworld` | Rust sim | https://raw.githubusercontent.com/Fanaperana/partiworld/main/README.md | 336 | `docs/screenshots/`, hero + 6 captioned | single file + `CONTRIBUTING.md` etc. |
| 8 | `AGSQ11/PMXSpan` | Rust desktop (egui) | https://raw.githubusercontent.com/AGSQ11/PMXSpan/main/README.md | 323 | `screenshots/`, 30+ captioned in tables | single file |
| 9 | `emilk/egui` | Rust desktop | https://raw.githubusercontent.com/emilk/egui/master/README.md | 374 | demo + example images (user-attachments CDN) | single file + docs.rs + `CONTRIBUTING.md` |
| 10 | `bevyengine/bevy` | Rust sim engine | https://raw.githubusercontent.com/bevyengine/bevy/main/README.md | 128 | logo only | thin README + bevy.org |
| 11 | `BurntSushi/ripgrep` | Rust CLI | https://raw.githubusercontent.com/BurntSushi/ripgrep/master/README.md | 534 | screenshot on burntsushi.net | README + `GUIDE.md` + `FAQ.md` |
| 12 | `sharkdp/bat` | Rust CLI | https://raw.githubusercontent.com/sharkdp/bat/master/README.md | 950 | 3 imgur screenshots, one per feature | single file (longest surveyed) |
| 13 | `ClementTsang/bottom` | Rust CLI/monitor | https://raw.githubusercontent.com/ClementTsang/bottom/main/README.md | 658 | `demos/demo.gif`, in-repo | README index + docs site |
| 14 | `charmbracelet/bubbletea` | onboarding exemplar (Go TUI) | https://raw.githubusercontent.com/charmbracelet/bubbletea/main/README.md | 403 | full-width GIF on stuff.charm.sh | single file + in-README tutorial |

Ranges read: #1, #3–#6 in full; #2 lines 1–300, 300–520, 520–719 (full); #7 lines 1–300, 301–336 (full); #8 lines 1–300, 301–323 (full); #9 lines 1–300, 301–374 (full); #10 lines 1–80, 84–128 (full); #11 lines 1–300, 450–534 (middle install-table rows skimmed, not re-read); #12 lines 1–300, 620–950 (install-table middle skimmed); #13 lines 1–300, 400–658 (install-table middle covered by TOC); #14 lines 1–80, 280–403 (tutorial body skimmed). Section-presence claims below rest on the read ranges plus each file's own TOC where present.

## 2. How the best ones sequence sections

### 2.1 The near-universal opening: title → pitch → visual → run

Every strong README puts a 1–3 sentence pitch and a visual above any install step:

- `bottom`: centered title + one-line pitch + badges, then `demos/demo.gif` with a caption naming the theme, font and terminal. Source: https://raw.githubusercontent.com/ClementTsang/bottom/main/README.md
- `partiworld`: bold one-liner ("A top-down colony sim where the whole world is made of particles."), one pitch paragraph, then a hero screenshot plus a 6-image captioned grid. Source: https://raw.githubusercontent.com/Fanaperana/partiworld/main/README.md
- `hospital-generative-agents`: three-sentence pitch (port map: villagers → patients, Smallville → hospital), then the replay screenshot with a full-sentence alt text, then a "Why this exists" section. Source: https://raw.githubusercontent.com/georgia-max/hospital-generative-agents/main/README.md
- `bat`: centered logo + "A `cat(1)` clone with syntax highlighting and Git integration.", then one screenshot per headline feature (highlighting, git, non-printables). Source: https://raw.githubusercontent.com/sharkdp/bat/master/README.md
- `bubbletea`: pitch paragraph, then a `width="100%"` example GIF, then a production-features paragraph. Source: https://raw.githubusercontent.com/charmbracelet/bubbletea/main/README.md
- `mesa`: pitch paragraph ("the Python-based alternative to NetLogo, Repast, or MASON"), then the WolfSheep screenshot with a caption that links an online demo. Source: https://raw.githubusercontent.com/projectmesa/mesa/main/README.md
- `ai-town`: Live Demo + Discord links, a 1454 px screenshot, then the pitch. Source: https://raw.githubusercontent.com/a16z-infra/ai-town/main/README.md

Counter-examples: `nickm980/smallville` ships no visual at all (https://raw.githubusercontent.com/nickm980/smallville/main/README.md), and `generative_agents`' `cover.png` is key art, not the product UI (https://raw.githubusercontent.com/joonspk-research/generative_agents/main/README.md).

### 2.2 Quickstart placement and shape

| Project | Quickstart form | Notable |
|---|---|---|
| `bat` | "How to use" code samples *before* "Installation" | shows value before asking for install effort; source: bat README link in §1 |
| `partiworld` | "Play": one prereq line + `git clone` + `cargo run --release -- 1337` + seed note; optional model download after | seed-as-argument documented inline, like `society --seed` should be |
| `PMXSpan` | Install table → "Build from source" → "First run" 3-step numbered checklist ending in a connected state | the only surveyed README with an explicit first-run checklist |
| `bottom` | Per-platform install, then 3-line "Usage" (`btm`, `btm -h`, `?` key) with docs link | minimal usage, maximal install matrix |
| `ripgrep` | Screenshot + benchmark tables + why/why-not first, then per-platform install, then "Building" | proof before install; playground link ("try before installing") |
| `hospital-generative-agents` | "What it does" pipeline diagram, then 2-step "Quick start" ending in a localhost URL | pipeline diagram doubles as concepts intro |
| `ai-town` | Numbered overall steps, 4 backend paths (standard/Docker/Pinokio/Fly.io), per-LLM config, ends with "visit http://localhost:5173" | most install paths; success check is the URL line |
| `generative_agents` | Two servers in two terminals, `run <step-count>`; success check is a quoted browser message | no one-command path; the weakest quickstart surveyed |

### 2.3 Section-presence matrix

✓ = in README, → = linked out, ~ = partial/stub, ✗ = absent. "Arch" = architecture/layout; "Perf" = numbers or benchmarks; "Trouble" = troubleshooting/FAQ/debugging.

| Project | Pitch | Visual | TOC | Prereqs | Quickstart | Concepts | Arch | Config | API/Usage | Perf | Trouble | Roadmap | Contrib | License |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| generative_agents | ✓ | ✓ art | ✗ | ✓ inline | ~ manual | ✗ | ✗ | ✓ | ✗ | ✗ | ~ Tips | ✗ | ✗ | ✗ |
| ai-town | ✓ | ✓ | ✓ emoji | ✓ | ✓ | ~ | → file | ✓ | ~ cmds | ✗ | ✓ 6§ | ✗ | ✗ | ✗ |
| genagents | ✓ | ✓ | ✓ full | ✓ | ✓ | ✓ | ✓ tree | ✓ | ✓ | ✗ | ✗ | ✗ | ✓ | ✓ |
| nickm980 | ✓ | ✗ | ✗ | ✗ | ✓ | ✗ | ✗ | ~ stub | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| hospital | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ diagram | ✓+→ | ✓ tables | n/a | ✗ | ✗ | ~ ptr | ✗ | ✗ |
| mesa | ✓ | ✓ | ✗ | ✓ | ✓ | → | → | ✗ | → | ✗ | ✗ | ✗ | ✓ | ✗ |
| partiworld | ✓ | ✓ ×7 | ✗ | ✓ | ✓ | ✓ | ✓ crates | ✓ | ~ ctrls | ~ probe | ✗ | ✓ 1-line | ✓→ | ✓ |
| PMXSpan | ✓ | ✓ ×30 | ✗ | ✓ | ✓+checklist | ✓ table | ✓ tree | ✓ | n/a | ✗ | ~ embed | ✓ "Not yet" | ✗ | ✗ |
| egui | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ essay | ✓ | ✓ FAQ | ✓ | ✓ claims | ✓ FAQ | ~ State | ✓→ | ✓ line |
| bevy | ✓ | logo | ✗ | ✓→ | ✓→ | → | → | ✓→ | ✓ sample | ✗ | ✗ | ✗ | ✓ | ✓ |
| ripgrep | ✓ | ✓ | ✓ links | ✓ | ✓ | ✓→ | ✗ | ✓→ | ✓ | ✓ 6 tables | →FAQ | ~ CHANGELOG | ✗ | ✓ line |
| bat | ✓ | ✓ ×3 | ✓ nav | ✓ | ✓ first | ✓ | ✗ | ✓ file | ✓+integ | ✗ | ✓ § | ✗ | ✓→ | ✓ |
| bottom | ✓ | ✓ GIF | ✓ full | ✓ | ✓ | → | → | → | ~ 3-line | ✗ | → | ✗ | ✓→ | ✗ |
| bubbletea | ✓ | ✓ GIF | ✗ | ✓ note | ✓ tutor | ✓ tutor | ~ | ✗ | ✓ | ✓ claims | ✓ debug | ✗ | ✓ link | ✓ |

Patterns worth naming:

- **Proof before install** (`ripgrep`, `egui`): benchmark tables / example output precede installation. ripgrep's six benchmark tables sit between the screenshot and "Why should I use ripgrep?"; egui's code example + dark/light renders precede "Quick start".
- **Usage before install** (`bat` only): "How to use" with per-task samples comes before "Installation".
- **Why before what** (`hospital`, `PMXSpan`, `egui`): a "Why this exists" / "Why PMXSpan" / "Goals + Non-goals" section precedes the feature tour. egui's non-goals ("not native-looking, not yet stable") are the honesty analogue of ripgrep's "Why shouldn't I use ripgrep?".
- **Concepts via diagram, not prose** (`hospital` cognitive loop, `partiworld` particle-layer table + crate table, `ai-town` layer list in `ARCHITECTURE.md`): the turn/loop fits in ~10 lines plus a table.
- **Roadmap is usually one line or absent**: `partiworld` ("Next on the list: …"), `PMXSpan` ("Not included yet"), `mesa` (banner note). Nobody surveyed maintains a roadmap *section* with dates.
- **License is often one line or absent from the README**: `ripgrep` ("Dual-licensed under MIT or the UNLICENSE"), `egui` (license line inside Credits), `bevy`/`bat`/`partiworld` (short dual-license blocks); `bottom`, `PMXSpan`, `mesa`, `ai-town`, `hospital` carry no license section in the README itself.

### 2.4 Full orders of the best (evidence)

- hospital: pitch → visual → why → what (pipeline) → quickstart → modes → layout → concepts → notes. 235 lines.
- partiworld: pitch → visuals → Play → What-you-can-do tour → Controls → How-it-works → Modding → Tests → Roadmap → Contributing → License. 336 lines.
- PMXSpan: logo/tagline → pitch → hero shots → Why → Features → Screenshots → Install → Build → First-run → Data-locations → guides → mock-cluster trial → layout → stats. 323 lines.
- bottom: pitch → GIF → TOC → Features → Support → Install → Usage → Config → Troubleshooting → Docs → Contributing → Thanks. 658 lines.
- bat: logo/pitch → feature screenshots → How-to-use → Integrations → Installation → Customization → Config-file → Windows → Troubleshooting → Development → Contributing → License. 950 lines.
- ripgrep: pitch → doc links → screenshot → benchmarks → why/why-not → Playground → Installation → Building → Tests → Related → Vuln-reporting → Translations. 534 lines.

## 3. Beginner vs advanced: single file vs linked docs

Three stable patterns, with the surveyed line counts as the cost of each:

**A. Single long file, progressive disclosure by heading depth (bat 950, bottom 658, PMXSpan 323, partiworld 336, egui 374).** The beginner reads the first three screens; the advanced reader keeps scrolling into FAQ/troubleshooting/customization. `bat` is the extreme: usage, integrations, per-OS install, pager internals, config file, Windows/Cygwin notes, troubleshooting, and dev instructions all in-file. Works when the whole thing is skimmable via TOC/nav links (`bat`'s nav bar, `bottom`'s full TOC, `egui`'s Sections list).

**B. README as index plus a docs site (bevy 128, mesa 142, bottom 658 for install only).** `bevy` keeps 128 lines and links every deep topic to bevy.org/docs.rs; `mesa` keeps 142 lines and links tutorials, examples, and API docs to readthedocs. Requires a site worth linking to; without one it degrades into pattern "thin" (see §4).

**C. README plus one architecture/concepts doc (ai-town 719 + 18.7 KB `ARCHITECTURE.md`; hospital 235 + `CLAUDE.md` + `plans/`; ripgrep 534 + `GUIDE.md` + `FAQ.md`; genagents 264 + papers).** The README carries pitch, run, tour, and pointers; the second doc carries the mental model. `ai-town`'s `ARCHITECTURE.md` exists at https://github.com/a16z-infra/ai-town/blob/main/ARCHITECTURE.md (18,712 bytes per the contents API) and holds layers, data model, inputs, engine stepping, and agent loop — none of which is in the README. `hospital` states the split explicitly: "The full design rationale lives in `hospital_gen_agent/CLAUDE.md`".

Where beginner and advanced READMEs diverge: beginners get pitch + visual + 1–3 commands + success check + tour; advanced readers get concepts + architecture + config + perf + troubleshooting — either below the fold (A) or one click away (B, C). `society` already owns the pattern-C assets (`docs/observability.md`, `docs/adr/`, `GLOSSARY.md`, `baseline/PERF_BASELINE.md`, `tools/`), so pattern C is the fit: the README should shrink toward pointers, not grow toward `bat`.

## 4. What the weak ones bury or omit

- `nickm980/smallville` (101 lines) is the full catalogue of omissions: no visual, an "Example" section that admits it "isn't finished yet", a two-line "Configuration" stub, no prerequisites, no architecture, no troubleshooting, no roadmap, no contributing guide, license only as a badge. Source: https://raw.githubusercontent.com/nickm980/smallville/main/README.md
- `generative_agents` buries prerequisites mid-prose (the Python 3.9.12 version sits in a parenthetical inside Step 2), offers no one-command run, and reduces troubleshooting to a "Tips" paragraph about API rate limits. No architecture overview, no contributing, no license section. Source: https://raw.githubusercontent.com/joonspk-research/generative_agents/main/README.md
- `genagents` omits troubleshooting and roadmap entirely and shows no product UI (cover image only). Its strength — usage code samples for every API — is also its ceiling: nothing for the operator. Source: https://raw.githubusercontent.com/StanfordHCI/genagents/main/README.md
- Commonly buried even in good READMEs: prerequisites inside install prose rather than a list (`generative_agents`, parts of `ai-town`); no "what success looks like" check after install (the good exceptions: `PMXSpan` "First run", `ai-town`'s localhost line, `hospital`'s "open http://localhost:8000/hospital.html").

## 5. GIF/asset conventions

### 5.1 Platform constraints (GitHub Docs, quoted verbatim)

Source for all rows: https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files (fetched 2026-11-13; Keenable extraction verified against the full page text).

| Claim | GitHub Docs wording |
|---|---|
| Max size, images and GIFs | "10MB for images and gifs" |
| Max size, video on free plan | "10MB for videos uploaded to a repository owned by a user or organization on a free GitHub plan" |
| Max size, video on paid plan | "100MB for videos uploaded to a repository owned by a user or organization on a paid GitHub plan" |
| Max size, all other files | "25MB for all other files" |
| Image formats supported "in all contexts" | "PNG (`.png`)", "GIF (`.gif`)", "JPEG (`.jpg`, `.jpeg`)", "SVG (`.svg`)" |
| Video formats supported "in all contexts" | "Video (`.mp4`, `.mov`, `.webm`)" — i.e. video **is** embeddable in rendered markdown |
| Codec guidance | "Video codec compatibility is browser specific … At the moment we recommend using H.264 for greatest compatibility." |
| Autoplay / loop behaviour | **Not documented** — the page contains no statement about autoplay (verified absence) |
| Recommended widths | **Not documented** — the page contains no statement about image widths (verified absence) |

Repo-side file limits, from https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-large-files-on-github: Git warns above 50 MiB, **blocks files larger than 100 MiB**, browser-added files cap at 25 MiB, and large binaries should ship as release artifacts instead of tracked files. In-repo GIFs in the low single-digit MB are therefore comfortably inside every limit.

Observed behaviour the docs do not pin down: GIFs render inline and play as motion — every project that ships one (`bottom`, `bubbletea`) places it as the primary visual and relies on that. Treat autoplay/loop as observed, not contracted.

### 5.2 What comparable projects actually ship

Byte sizes via the GitHub contents API, 2026-11-13. Pixel dimensions and frame counts were **not** determined (API returns bytes only; no image decode available here) and are listed as unknown rather than guessed.

| Project | Asset | Bytes | Location | Markup / width | Placement |
|---|---|---|---|---|---|
| `bottom` | `demos/demo.gif` | 2,165,443 (~2.1 MB) | in-repo `demos/` | `<img>` natural size | immediately after pitch, captioned (theme/font/terminal) |
| `generative_agents` | `cover.png` | 4,186,900 (~4.0 MB) | repo root | `<img>` 80% width, min 300 px | first element after title |
| `genagents` | `static_dir/cover3.png` | 1,051,114 (~1.0 MB) | in-repo `static_dir/` | plain markdown image | after title |
| `hospital` | `assets/hospital-replay.png` | 375,369 (~367 KB) | in-repo `assets/` | plain markdown + full-sentence alt | after pitch |
| `partiworld` | `docs/screenshots/autumn.png` (1 of 7) | 228,634 (~223 KB) | in-repo `docs/screenshots/` | hero + captioned `<table>` grid | after pitch |
| `PMXSpan` | `screenshots/dashboard.png` (1 of 30+) | 65,055 (~64 KB) | in-repo `screenshots/` | `width="49%"` pairs, grouped tables | hero pair + gallery section |
| `mesa` | `docs/images/wolf_sheep.png` | 163,201 (~159 KB) | in-repo `docs/images/` | raw-URL markdown + caption + demo link | after pitch |
| `ai-town` | hero screenshot | unknown (CDN) | user-attachments `…/assets/…` URL | `<img width="1454">` | before pitch |
| `egui` | demo/example images | unknown (CDN) | user-attachments URLs | `width="400"` / `width="50%"` | demo block + example |
| `bubbletea` | example GIF | unknown (CDN) | stuff.charm.sh (project CDN) | `width="100%"` | after pitch |
| `bat` | 3 feature PNGs | unknown (external) | imgur (`imgur.com`, `i.imgur.com`) | plain markdown images | one per feature |
| `ripgrep` | sample-search PNG | unknown (external) | burntsushi.net (personal domain) | linked markdown image | "Screenshot of search results" |

Conventions to take:

- **In-repo beats external.** 7 of 12 visual projects keep assets in the repo (`demos/`, `screenshots/`, `docs/screenshots/`, `assets/`, `static_dir/`). External hosts (imgur, personal domains, even project CDNs) rot or move; user-attachments URLs are stable but opaque and unreviewable in-tree.
- **Widths are author-chosen, not platform-set.** Observed: natural size (`bottom`, `hospital`, `mesa`), `width="49%"` pairs (`PMXSpan`), `width="50%"` (`egui`), `width="100%"` (`bubbletea`), `width="1454"` (`ai-town`), 80% (`generative_agents`). No docs guidance exists (§5.1).
- **Captions carry the context.** `bottom` (theme/font/terminal), `mesa` (what model + demo link), `PMXSpan` (per-shot one-liners), `partiworld` (per-shot subtitles), `hospital` (alt text as full sentence).
- **Reproducible screenshots are a named practice.** `partiworld` regenerates its shots from scripted env vars (`PW_AUTOSHOT_*`: seed, weather, speed, tribes, needs — see its README "Tests" tail); `PMXSpan` notes every screenshot "was taken against the bundled mock cluster". `society` already has the equivalent lever: `tools/shot.sh` + `--seed`/`--warmup`/`--view` flags.

## 6. Recommendations for `society`

### 6.1 Proposed section order

1. **Title + one-line pitch + hero GIF.** Keep "A living settlement" energy, but lead with the verb: watch a deterministic village live turn by turn. GIF immediately after (§2.1: `bottom`, `partiworld`, `hospital`).
2. **Quickstart: 3 commands.** `git clone`, `cargo run --release`, one variant (`--headless` or `--view feed`). Then the "no API key, no daemon" line and the success check (window appears at 1×; API on :8100). Follows `partiworld` "Play" and `PMXSpan` "First run".
3. **What you can do / observe.** Observer controls (pause, speed 1–100×, views, inspector), interventions (whisper/gift/weather + divine-event note), screenshot tool. This is `partiworld`'s "What you can do" and `PMXSpan`'s "Features", compressed to one screen.
4. **The turn, in ~10 lines.** The existing policy → speaker → check → settle diagram plus one sentence per stage, then a pointer to `docs/` for the essay. Follows `hospital`'s pipeline diagram and `partiworld`'s "How it works".
5. **Control API, short table + one curl.** Keep the endpoint table (it is already the right shape), cut prose around it; full semantics live in `docs/observability.md`.
6. **Performance, 3 numbers + pointer.** Committed turn ~20 ms, engine step ~0.2 ms, `/state` <0.1 ms → `baseline/PERF_BASELINE.md`. Follows `ripgrep` (proof early) without its six tables.
7. **Project layout.** 3-crate table (`society` → `society-server` → `society-sim`) + key dirs. Follows `partiworld`'s crate table and `PMXSpan`'s layout tree.
8. **Tests.** One `cargo test` line + what the suite covers in one line; benchmarks/probes by name only.
9. **Docs map.** One line each: `docs/observability.md`, `docs/adr/`, `GLOSSARY.md`, `baseline/`, `tools/`. This is the pattern-C pointer block (§3).
10. **Roadmap** (one line, `partiworld`-style), **Contributing** (pointer or 3 lines), **License** (one line, `ripgrep`-style).

Target length: 200–300 lines — the `hospital`/`partiworld`/`PMXSpan` band, down from today's ~450.

### 6.2 What belongs in the README vs linked docs

| Stays in README | Moves to linked docs |
|---|---|
| Pitch, hero GIF, quickstart, success check | — |
| Observer tour (controls, views, interventions) in brief | Full observer semantics → `docs/observability.md` |
| Turn diagram + one sentence per stage | "Why the architecture looks like this" essay → `docs/` architecture note or ADR index (pattern C: `ai-town` → `ARCHITECTURE.md`, `hospital` → `CLAUDE.md`) |
| API endpoint table + one curl | Payload shapes, envelopes, evidence quality → `docs/observability.md` |
| 3 perf numbers | Method, full tables, probes → `baseline/PERF_BASELINE.md` (already exists; README should only cite it) |
| Crate table, tests one-liner, docs map, roadmap, contributing, license | Terminology → `GLOSSARY.md` (link on first use of turn/policy/grounding); decisions → `docs/adr/` |

### 6.3 Demo GIF spec

| Item | Spec | Basis |
|---|---|---|
| Count | **2 GIFs**: (1) hero — village running at 1×, then speed ramp toward 100× with feed + ring visible; (2) intervention — whisper/gift via `curl`, divine event landing in the feed | `bottom` proves one hero GIF carries a README; `bat` proves one visual per headline feature; two covers observe + intervene with nothing extra |
| Content, hero | Default view 3–4 s at 1× (turns visibly advancing), then speed ramp, end on inspector or feed close-up; must show the ring, villagers, and turn counter | shows the "living settlement" claim the pitch makes |
| Content, intervention | Terminal `curl …/inject` + window side by side (or sequential), divine event in feed, villager memory note if visible | shows observer-not-god, the second headline claim |
| Pixel size | Record at the default window size (≤1280×800); ship at recorded size, no upscale | `bottom` ships natural size; `PMXSpan`'s 64 KB shots prove small reads fine |
| File size | **≤3 MB each**, hard ceiling 10 MB | GitHub renders images/GIFs to 10 MB (§5.1); `bottom`'s 2.1 MB hero is the in-repo precedent; stays trivially inside the 50/100 MiB git limits |
| Frame rate / length | 10–12 fps capture, 10–20 s loop each | keeps the 3 MB budget at 1280×800 with flat UI colours; GIFs loop (observed, §5.1) |
| Location | `docs/screenshots/demo*.gif` (or `demos/`) in-repo, referenced by relative path | in-repo convention: `bottom` `demos/`, `partiworld` `docs/screenshots/`, `PMXSpan` `screenshots/` |
| Generation | Scripted and reproducible: fixed `--seed`, `--warmup`, `--view`, driven capture via `tools/shot.sh` or equivalent; record the exact command next to the GIF or in `tools/` | `partiworld` `PW_AUTOSHOT_*`, `PMXSpan` mock-cluster note |
| Caption | One line under each: what it shows + the command that reproduces it | `bottom`/`mesa` caption practice |
| Fallback | If a GIF exceeds budget, ship a captioned PNG still + link, not a degraded GIF; avoid video in-repo (mp4 in git bloats clones; every surveyed sim README uses PNG/GIF, none in-repo video) | size discipline; all surveyed sim projects use PNG/GIF |

### 6.4 Explicit non-recommendations

- Do not copy `nickm980/smallville`'s shape (no visual, stub sections) or `generative_agents`' setup-before-result ordering.
- Do not copy `bevy`'s 128-line extreme — it works because bevy.org exists; `society` has no site.
- Do not grow toward `bat`'s 950 lines — pattern A at that length needs `bat`'s install-matrix justification; `society` builds one way (`cargo run`).
- Do not host images on imgur/personal domains (`bat`, `ripgrep`) — in-repo relative paths survive forks and renames.

## 7. Sources and verification status

Primary sources only, all fetched 2026-11-13: raw README text via `read` on `raw.githubusercontent.com` URLs (discovered via Keenable `search_web_pages`), GitHub Docs via Keenable `fetch_page_content` extraction verified against the full page text read, asset sizes via the GitHub contents API (`https://api.github.com/repos/<owner>/<repo>/contents/<path>`).

| Claim | Source |
|---|---|
| All README structure/section-order claims, §§1–4 | the 14 raw README URLs in §1, read in the ranges stated there |
| `ai-town` architecture lives outside the README | https://github.com/a16z-infra/ai-town/blob/main/ARCHITECTURE.md (18,712 bytes per contents API); README text contains no architecture section |
| Image/GIF/video size caps, formats, H.264 note | https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files |
| 50/100 MiB git limits, 25 MiB browser cap | https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-large-files-on-github |
| Asset byte sizes, §5.2 | contents API: `ClementTsang/bottom` `demos/demo.gif` (2,165,443); `joonspk-research/generative_agents` `cover.png` (4,186,900); `StanfordHCI/genagents` `static_dir/cover3.png` (1,051,114); `Fanaperana/partiworld` `docs/screenshots/autumn.png` (228,634); `AGSQ11/PMXSpan` `screenshots/dashboard.png` (65,055); `georgia-max/hospital-generative-agents` `assets/hospital-replay.png` (375,369); `mesa/mesa` `docs/images/wolf_sheep.png` (163,201) |
| Width attributes and markup | the raw README source of each project (§1) |
| `society` current README shape (pitch, API table, perf lines, turn diagram) | local `README.md` lines 1–300 of 453; existing `docs/`, `baseline/`, `GLOSSARY.md`, `tools/` per assignment context |

Explicitly **unverified** (could not verify from here — no shell to download/decode binaries): pixel dimensions, frame counts, and durations of any surveyed GIF/PNG; GIF autoplay/loop behaviour as a platform contract (GitHub Docs is silent; §5.1 records the absence); exact install-table middle rows of `ripgrep`/`bat`/`bottom` beyond the read ranges (covered instead by each file's own TOC/headers); whether `PMXSpan`/`bottom` license text lives outside their READMEs (only the in-README absence is claimed).
