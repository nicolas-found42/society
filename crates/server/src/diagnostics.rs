//! Diagnostics: the shell's durable record of what the engine committed and
//! what the observers did. The engine stays pure (no clock, no IO); this
//! module owns every timestamp and every file write.
//!
//! Two append-only JSONL files sit next to the save:
//!
//! * `<save>.journal.jsonl` — one [`JournalEntry`] per committed turn: the
//!   full [`TurnRecord`](society_sim::turn::TurnRecord) plus an envelope of
//!   `{save_version, seed, turn, wall_time}`. Rotation is by byte size, never
//!   by memory pressure: the in-memory `/log` cap still drops old turns, the
//!   journal does not.
//! * `<save>.diagnostics.jsonl` — one [`AuditEntry`] per observer action and
//!   shell failure: boot, turns committed, resets, injects, halts, save
//!   errors. Inject payloads are summaries (kind + target), never raw text.
//!
//! Both are served back through `/log` (journal fallback) and `/diagnostics`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use society_sim::turn::TurnRecord;

/// Schema version of the envelope written around each journal entry. Bumped
/// when the envelope fields change incompatibly; the entry body is the
/// engine's [`TurnRecord`](society_sim::turn::TurnRecord) verbatim.
pub const JOURNAL_VERSION: u32 = 1;

/// Rotation threshold: 32 MiB per journal segment.
///
/// At current turn sizes this is tens of thousands of turns; rotation renames
/// the full segment aside (`journal.1.jsonl`, `journal.2.jsonl`, …) and
/// starts a fresh head.
pub const JOURNAL_ROTATE_BYTES: u64 = 32 * 1024 * 1024;

/// Rotation threshold for the audit log.
///
/// Observer actions are rare; 8 MiB is effectively unbounded while still
/// capping a runaway.
pub const AUDIT_ROTATE_BYTES: u64 = 8 * 1024 * 1024;

/// One durable turn: the envelope answers "which build, which village, when",
/// the record answers "what happened".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Envelope schema version ([`JOURNAL_VERSION`]).
    pub journal_version: u32,
    /// Crate version of the shell that committed the turn.
    pub build: String,
    /// Save format the running shell understands.
    pub save_version: u32,
    pub seed: u64,
    pub turn: u64,
    pub day: u64,
    /// Seconds since the Unix epoch, shell clock, informational only: never
    /// fed back into the engine, which stays on turn/day.
    pub wall_time: u64,
    pub record: TurnRecord,
}

/// One durable observer/shell event. `detail` is a small JSON object whose
/// shape depends on `action`; free-form observer text is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub wall_time: u64,
    pub action: String,
    #[serde(default = "default_audit_build")]
    pub build: String,
    pub turn: u64,
    #[serde(default)]
    pub detail: Value,
}

#[allow(clippy::missing_const_for_fn, reason = "String::new in const context needs newer toolchain")]
fn default_audit_build() -> String {
    String::new()
}

/// The shell's crate version, stamped on every envelope.
///
/// A journal written by an older binary is still readable — the envelope
/// just says so.
#[must_use]
pub fn build_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Seconds since the Unix epoch on the shell clock.
///
/// Zero when the clock is unavailable; diagnostics timestamps are
/// informational, never engine input.
#[must_use]
#[allow(clippy::missing_const_for_fn, reason = "calls SystemTime::now")]
pub fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Journal path for a save: `<save>.journal.jsonl` (`data/society.json` →
/// `data/society.json.journal.jsonl`).
#[must_use]
pub fn journal_path(save_path: &Path) -> PathBuf {
    sibling_suffixed(save_path, "journal.jsonl")
}

/// Audit path for a save: `<save>.diagnostics.jsonl`.
#[must_use]
pub fn audit_path(save_path: &Path) -> PathBuf {
    sibling_suffixed(save_path, "diagnostics.jsonl")
}

fn sibling_suffixed(save_path: &Path, suffix: &str) -> PathBuf {
    let mut name = save_path
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    name.push(".");
    name.push(suffix);
    save_path.with_file_name(name)
}

/// Append one committed turn to the journal, fsynced before return so a crash
/// can lose at most the turns after the last completed write. Rotation keeps
/// the head segment under [`JOURNAL_ROTATE_BYTES`].
///
/// # Errors
///
/// Returns an IO error string when the journal cannot be written.
pub fn append_turn(
    save_path: &Path,
    seed: u64,
    record: &TurnRecord,
    build: &str,
    save_version: u32,
) -> Result<(), String> {
    let entry = JournalEntry {
        journal_version: JOURNAL_VERSION,
        build: build.to_owned(),
        save_version,
        seed,
        turn: record.turn,
        day: record.day,
        wall_time: wall_now(),
        record: record.clone(),
    };
    append_jsonl(
        &journal_path(save_path),
        &entry,
        JOURNAL_ROTATE_BYTES,
        "journal",
    )
}

/// Append one observer/shell event to the audit log.
///
/// # Errors
///
/// Returns an IO error string when the audit log cannot be written.
pub fn append_audit(
    save_path: &Path,
    action: &str,
    turn: u64,
    detail: Value,
) -> Result<(), String> {
    let entry = AuditEntry {
        wall_time: wall_now(),
        action: action.into(),
        build: build_version(),
        turn,
        detail,
    };
    append_jsonl(&audit_path(save_path), &entry, AUDIT_ROTATE_BYTES, "audit")
}

/// Read journal entries back, oldest first.
///
/// Applies `since_turn` and `limit` exactly like `/log` does. The head
/// segment and every rotated segment are merged by turn order; corrupt lines
/// are counted and skipped, never fatal.
///
/// Returns `(entries, corrupt_lines)`.
#[must_use]
pub fn read_journal(
    save_path: &Path,
    since_turn: u64,
    limit: usize,
) -> (Vec<JournalEntry>, usize) {
    let mut entries = Vec::new();
    let mut corrupt = 0_usize;
    let mut paths = vec![journal_path(save_path)];
    paths.extend(rotated_segments(&journal_path(save_path), "journal.jsonl"));
    paths.sort();
    for path in paths {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<JournalEntry>(line) {
                Ok(e) => {
                    if e.turn >= since_turn {
                        entries.push(e);
                    }
                },
                Err(_) => corrupt += 1,
            }
        }
    }
    entries.sort_by_key(|e| (e.turn, e.wall_time));
    entries.dedup_by_key(|e| (e.seed, e.turn));
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    (entries, corrupt)
}

/// Read audit entries back, oldest first, bounded by `limit`. Corrupt lines
/// are counted and skipped like the journal.
#[must_use]
pub fn read_audit(save_path: &Path, limit: usize) -> (Vec<AuditEntry>, usize) {
    let mut entries = Vec::new();
    let mut corrupt = 0_usize;
    let mut paths = vec![audit_path(save_path)];
    paths.extend(rotated_segments(&audit_path(save_path), "diagnostics.jsonl"));
    paths.sort();
    for path in paths {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<AuditEntry>(line) {
                Ok(e) => entries.push(e),
                Err(_) => corrupt += 1,
            }
        }
    }
    entries.sort_by_key(|e| (e.turn, e.wall_time));
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    (entries, corrupt)
}

/// Oldest turn present in the journal head, for `/health` depth reporting.
#[must_use]
pub fn journal_depth(save_path: &Path) -> (Option<u64>, usize, u64) {
    let path = journal_path(save_path);
    let mut oldest: Option<u64> = None;
    let mut count = 0_usize;
    for segment in std::iter::once(path.clone()).chain(rotated_segments(&path, "journal.jsonl")) {
        let Ok(bytes) = fs::read(&segment) else {
            continue;
        };
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_slice::<JournalEntry>(line) {
                oldest = Some(oldest.map_or(e.turn, |o: u64| o.min(e.turn)));
                count += 1;
            }
        }
    }
    let bytes = fs::metadata(&path).map_or(0, |m| m.len());
    (oldest, count, bytes)
}

fn append_jsonl<T: Serialize>(
    path: &Path,
    value: &T,
    rotate_bytes: u64,
    stem: &str,
) -> Result<(), String> {
    rotate_if_full(path, rotate_bytes, stem).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    line.push(b'\n');
    append_synced(path, &line).map_err(|e| format!("{stem} append {}: {e}", path.display()))
}

fn append_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.flush()?;
    f.sync_all()
}

fn rotate_if_full(path: &Path, rotate_bytes: u64, stem: &str) -> std::io::Result<()> {
    let len = fs::metadata(path).map_or(0, |m| m.len());
    if len < rotate_bytes {
        return Ok(());
    }
    // Find the first free segment number: `save.journal.1.jsonl`, `.2`, ….
    let mut n = 1_u32;
    loop {
        let candidate = segment_path(path, stem, n);
        if !candidate.exists() {
            fs::rename(path, candidate)?;
            return Ok(());
        }
        n += 1;
        if n > 9999 {
            return Ok(());
        }
    }
}

fn segment_path(path: &Path, stem: &str, n: u32) -> PathBuf {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let seg = name.replace(stem, &format!("{stem}.{n}"));
    path.with_file_name(seg)
}

fn rotated_segments(head: &Path, stem: &str) -> Vec<PathBuf> {
    let dir = head.parent().filter(|d| !d.as_os_str().is_empty());
    let head_name = head
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let prefix = head_name.replace(stem, &format!("{stem}."));
    let mut out = Vec::new();
    let Ok(entries) = dir.map_or_else(
        || fs::read_dir("."),
        fs::read_dir,
    ) else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name != head_name {
            out.push(e.path());
        }
    }
    out
}

/// Summarise an inject request for the audit log: kind + target + turn, never
/// observer prose.
#[must_use]
pub fn inject_summary(kind: &str, target: Option<&str>, to: Option<&str>, place: Option<&str>) -> Value {
    json!({
        "kind": kind,
        "target": target,
        "to": to,
        "place": place,
    })
}

/// Verify a journal slice against the engine.
///
/// Re-step from the founding seed through the slice's turns and compare the
/// committed records field by field. Returns the first divergence, or `None`
/// when the slice replays identically. This is the automatic half of
/// diagnosis: hand it a journal range and it names the turn where the world
/// stopped matching its history.
#[must_use]
pub fn verify_journal(entries: &[JournalEntry]) -> Option<VerifyDivergence> {
    if entries.is_empty() {
        return None;
    }
    let seed = entries[0].seed;
    let mut w = society_sim::World::founding(seed);
    // Fast-forward: the slice may start past turn 0 only if the world it was
    // read from had already run there. Replaying from founding still reaches
    // the same states deterministically, so step until the slice's first turn.
    let first = entries[0].turn;
    for _ in 0..first {
        if society_sim::turn::step(&mut w).is_err() {
            return Some(VerifyDivergence {
                turn: w.turn,
                reason: "replay halted before the slice".into(),
            });
        }
    }
    for e in entries {
        if w.turn != e.turn {
            return Some(VerifyDivergence {
                turn: e.turn,
                reason: format!("replay at turn {} but journal at {}", w.turn, e.turn),
            });
        }
        let rec = match society_sim::turn::step(&mut w) {
            Ok(r) => r,
            Err(reason) => {
                return Some(VerifyDivergence {
                    turn: e.turn,
                    reason: format!("replay halted: {reason}"),
                });
            },
        };
        if let Some(reason) = compare_records(&rec, &e.record) {
            return Some(VerifyDivergence {
                turn: e.turn,
                reason,
            });
        }
    }
    None
}

/// Where a replay stopped matching its journal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyDivergence {
    pub turn: u64,
    pub reason: String,
}

/// Field-by-field comparison of two committed turn records.
///
/// The compared fields are the engine's observable contract: roster, actions,
/// lines, verdicts, motives and the summary counts. `decision_id`,
/// `event_range` and floating-point factor noise are informational, not
/// compared.
#[must_use]
pub fn compare_records(a: &TurnRecord, b: &TurnRecord) -> Option<String> {
    if a.turn != b.turn {
        return Some(format!("turn {} vs {}", a.turn, b.turn));
    }
    if a.agents.len() != b.agents.len() {
        return Some(format!(
            "turn {}: {} agents vs {}",
            a.turn,
            a.agents.len(),
            b.agents.len()
        ));
    }
    for (x, y) in a.agents.iter().zip(b.agents.iter()) {
        if x.name != y.name {
            return Some(format!("turn {}: {} vs {}", a.turn, x.name, y.name));
        }
        for (field, xv, yv) in [
            ("action", x.action.as_str(), y.action.as_str()),
            ("say", x.say.as_str(), y.say.as_str()),
            ("why", x.why.as_str(), y.why.as_str()),
            ("social", x.social.as_str(), y.social.as_str()),
        ] {
            if xv != yv {
                return Some(format!(
                    "turn {} {}: {field} {xv:?} vs {yv:?}",
                    a.turn, x.name
                ));
            }
        }
        if x.accepted != y.accepted
            || x.addressee != y.addressee
            || x.grounded != y.grounded
            || x.lie != y.lie
        {
            return Some(format!("turn {} {}: outcome differs", a.turn, x.name));
        }
    }
    if a.summary != b.summary {
        return Some(format!("turn {}: summary differs", a.turn));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "society-diag-{}-{}",
            std::process::id(),
            prefix
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rec(turn: u64) -> TurnRecord {
        TurnRecord {
            turn,
            day: turn / 12,
            agents: Vec::new(),
            births: Vec::new(),
            summary: society_sim::turn::TurnSummary::default(),
        }
    }

    #[test]
    fn journal_round_trips_and_reads_back_bounded() {
        let dir = tmp("roundtrip");
        let save = dir.join("v.json");
        for t in 0..5_u64 {
            append_turn(&save, 7, &rec(t), "0.1.0", 2).unwrap();
        }
        let (entries, corrupt) = read_journal(&save, 2, 2);
        assert_eq!(corrupt, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].turn, 3);
        assert_eq!(entries[1].turn, 4);
        assert_eq!(entries[0].seed, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_lines_are_skipped_and_counted() {
        let dir = tmp("corrupt");
        let save = dir.join("v.json");
        append_turn(&save, 7, &rec(0), "0.1.0", 2).unwrap();
        {
            use std::io::Write as _;
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(journal_path(&save))
                .unwrap();
            writeln!(f, "not json{{").unwrap();
        }
        append_turn(&save, 7, &rec(1), "0.1.0", 2).unwrap();
        let (entries, corrupt) = read_journal(&save, 0, 10);
        assert_eq!(corrupt, 1);
        assert_eq!(entries.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_passes_on_a_real_replay() {
        let mut w = society_sim::World::founding(7);
        let mut records = Vec::new();
        for _ in 0..5 {
            records.push(society_sim::turn::step(&mut w).unwrap());
        }
        let entries: Vec<JournalEntry> = records
            .iter()
            .map(|r| JournalEntry {
                journal_version: JOURNAL_VERSION,
                build: "test".into(),
                save_version: 2,
                seed: 7,
                turn: r.turn,
                day: r.day,
                wall_time: 0,
                record: r.clone(),
            })
            .collect();
        assert_eq!(verify_journal(&entries), None);
    }

    #[test]
    fn verify_names_the_divergent_turn() {
        let mut w = society_sim::World::founding(7);
        let r0 = society_sim::turn::step(&mut w).unwrap();
        let r1 = society_sim::turn::step(&mut w).unwrap();
        let mut bad = r1.clone();
        bad.agents[0].say = "tampered line".into();
        let entries: Vec<JournalEntry> = [r0, bad]
            .iter()
            .map(|r| JournalEntry {
                journal_version: JOURNAL_VERSION,
                build: "test".into(),
                save_version: 2,
                seed: 7,
                turn: r.turn,
                day: r.day,
                wall_time: 0,
                record: r.clone(),
            })
            .collect();
        let div = verify_journal(&entries).expect("divergence");
        assert_eq!(div.turn, 1);
        assert!(div.reason.contains("say"), "{}", div.reason);
    }
}
