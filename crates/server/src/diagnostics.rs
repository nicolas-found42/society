//! Diagnostics: the shell's durable record of what the engine committed and
//! what the observers did. The engine stays pure (no clock, no IO); this
//! module owns every timestamp and every file write.
//!
//! Two append-only JSONL files sit next to the save:
//!
//! * `<save>.journal.jsonl`, one [`JournalEntry`] per committed turn: the
//!   full [`TurnRecord`] plus an envelope of
//!   `{save_version, seed, turn, wall_time}`. Rotation is by byte size, never
//!   by memory pressure: the in-memory `/log` cap still drops old turns, the
//!   journal has its own bounded disk retention.
//! * `<save>.diagnostics.jsonl`, one [`AuditEntry`] per observer action and
//!   shell failure: boot, turns committed, resets, injects, halts, save
//!   errors. Inject payloads are summaries (kind, target, and the submitted
//!   word when the kind was rejected), never raw text.
//!
//! Both are served back through `/log` (durable records) and `/diagnostics`.

use std::fs;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

use crate::telemetry::Context;

static FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Head plus four rotated segments; no age guarantee.
///
/// Rotation renames the oldest slot aside, shifts the rest up by renaming, and
/// only deletes the renamed-aside slot once the head has moved. A chain that
/// fails partway undoes the renames it already made and puts the oldest segment
/// back, so the failure itself destroys no retained row; appends report the
/// error instead of silently appending past the budget, and the next rotation
/// retries. An undo that cannot run (an entry blocking the restored slot) is
/// best effort: the parked segment is then retired by the next successful
/// rotation, which can leave the window one segment short of the count.
pub const RETAINED_SEGMENTS: u32 = 4;
pub const MAX_QUERY_ENTRIES: usize = 1000;
const MAX_LINE_BYTES: u64 = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use society_sim::turn::TurnRecord;

/// Schema version of the envelope written around each journal entry. Bumped
/// when the envelope fields change incompatibly; the entry body is the
/// engine's [`TurnRecord`] verbatim.
pub const JOURNAL_VERSION: u32 = 1;

/// Rotation threshold: 32 MiB per journal segment.
///
/// At current turn sizes this is tens of thousands of turns; rotation renames
/// the full segment aside (`journal.1.jsonl`, `journal.2.jsonl`, …) and
/// starts a fresh head.
pub const JOURNAL_ROTATE_BYTES: u64 = 32 * 1024 * 1024;

/// Rotation threshold for the audit log.
///
/// Includes per-turn and request outcomes; head plus four rotations.
pub const AUDIT_ROTATE_BYTES: u64 = 8 * 1024 * 1024;

/// One durable turn: the envelope answers "which build, which village, when",
/// the record answers "what happened".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    #[serde(default)]
    pub context: Context,
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
    #[serde(default)]
    pub context: Context,
    pub wall_time: u64,
    pub action: String,
    #[serde(default = "default_audit_build")]
    pub build: String,
    pub turn: u64,
    #[serde(default)]
    pub detail: Value,
}

#[allow(
    clippy::missing_const_for_fn,
    reason = "String::new in const context needs newer toolchain"
)]
fn default_audit_build() -> String {
    String::new()
}

/// The shell's crate version, stamped on every envelope.
///
/// A journal written by an older binary still reads: the envelope records the
/// build that wrote it.
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
/// Bytes retained across the journal head and every rotated segment.
#[must_use]
pub fn journal_bytes(save_path: &Path) -> u64 {
    let head = journal_path(save_path);
    let mut total = fs::metadata(&head).map_or(0, |m| m.len());
    for segment in rotated_segments(&head) {
        total += fs::metadata(segment).map_or(0, |m| m.len());
    }
    total
}

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
    let mut name = save_path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    save_path.with_file_name(name)
}

/// Append one committed turn to the journal, fsynced before return.
///
/// A process crash can therefore lose at most the turns after the last
/// completed write. Rotation keeps the head segment under
/// [`JOURNAL_ROTATE_BYTES`]; the directory entries the renames create are not
/// fsynced, so power loss can still void a freshly rotated segment.
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
    append_turn_context(
        save_path,
        seed,
        record,
        build,
        save_version,
        Context::default(),
    )
}

/// Append a turn with process, village and operation correlation.
///
/// # Errors
/// Returns a storage error if evidence cannot be retained.
pub fn append_turn_context(
    save_path: &Path,
    seed: u64,
    record: &TurnRecord,
    build: &str,
    save_version: u32,
    context: Context,
) -> Result<(), String> {
    let entry = JournalEntry {
        context,
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
    append_audit_context(
        save_path,
        action,
        turn,
        detail,
        Context {
            session_id: crate::telemetry::session_id(),
            source_id: env!("SOCIETY_SOURCE_ID").into(),
            operation_id: Some(
                crate::telemetry::OPERATION
                    .try_with(Clone::clone)
                    .unwrap_or_else(|_| crate::telemetry::operation_id()),
            ),
            ..Default::default()
        },
    )
}

/// Append a shell event with correlation.
///
/// # Errors
/// Returns a storage error if evidence cannot be retained.
pub fn append_audit_context(
    save_path: &Path,
    action: &str,
    turn: u64,
    detail: Value,
    context: Context,
) -> Result<(), String> {
    let entry = AuditEntry {
        context,
        wall_time: wall_now(),
        action: action.into(),
        build: build_version(),
        turn,
        detail,
    };
    append_jsonl(
        &audit_path(save_path),
        &entry,
        AUDIT_ROTATE_BYTES,
        "diagnostics",
    )
}

/// Read journal entries back, oldest first.
///
/// Applies `since_turn` and `limit` exactly like `/log` does. The head
/// segment and every rotated segment are merged by turn order; corrupt lines
/// are counted and skipped, never fatal.
///
/// Returns `(entries, corrupt_lines)`.
#[must_use]
pub fn read_journal(save_path: &Path, since_turn: u64, limit: usize) -> (Vec<JournalEntry>, usize) {
    let evidence = scan_journal(save_path, since_turn, limit);
    (
        evidence.entries,
        evidence.corrupt_lines + evidence.read_errors,
    )
}

/// Filters apply before the retained result limit. Epoch seconds are inclusive.
#[derive(Debug, Default)]
pub struct Filter {
    pub since_time: Option<u64>,
    pub until_time: Option<u64>,
    pub world_id: Option<String>,
    pub operation_id: Option<String>,
    pub session_id: Option<String>,
}

/// Query-string filters arrive as text, and `#[serde(flatten)]` buffers every
/// value as a string before this type sees it, so a derived `Option<u64>`
/// field can never parse. Accept both the buffered text and a real JSON number
/// (the offline reader and tests pass numbers), and ignore keys that belong to
/// the surrounding query struct.
impl<'de> Deserialize<'de> for Filter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FilterVisitor;

        impl<'de> serde::de::Visitor<'de> for FilterVisitor {
            type Value = Filter;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("evidence filters")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Filter, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut filter = Filter::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "since_time" => {
                            filter.since_time = epoch(&key, map.next_value::<Value>()?)?;
                        },
                        "until_time" => {
                            filter.until_time = epoch(&key, map.next_value::<Value>()?)?;
                        },
                        "world_id" => filter.world_id = text(map.next_value()?),
                        "operation_id" => filter.operation_id = text(map.next_value()?),
                        "session_id" => filter.session_id = text(map.next_value()?),
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        },
                    }
                }
                Ok(filter)
            }
        }

        deserializer.deserialize_map(FilterVisitor)
    }
}

fn epoch<E: serde::de::Error>(key: &str, value: Value) -> Result<Option<u64>, E> {
    let invalid = || E::custom(format!("{key} must be epoch seconds as an integer"));
    match value {
        Value::Null => Ok(None),
        Value::Number(n) => n.as_u64().map(Some).ok_or_else(invalid),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => s.trim().parse().map(Some).map_err(|error| {
            let _ = error;
            invalid()
        }),
        _ => Err(invalid()),
    }
}

/// An empty identity query is the absence of a filter, not a request for
/// contextless legacy rows: a script that defaults a variable to `""` must not
/// silently invert the selection.
fn text(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

impl Filter {
    /// Whether this filter selects every retained row. Time and identity
    /// filters cannot be applied to in-memory turns, so callers that merge such
    /// turns must treat a non-empty filter as opt-out.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.since_time.is_none()
            && self.until_time.is_none()
            && self.world_id.is_none()
            && self.operation_id.is_none()
            && self.session_id.is_none()
    }

    fn matches(&self, time: u64, c: &Context) -> bool {
        self.since_time.is_none_or(|t| time >= t)
            && self.until_time.is_none_or(|t| time <= t)
            && self
                .world_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .is_none_or(|id| id == c.world_id)
            && self
                .operation_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .is_none_or(|id| c.operation_id.as_deref() == Some(id))
            && self
                .session_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .is_none_or(|id| id == c.session_id)
    }
}

#[must_use]
pub fn query_journal(
    save: &Path,
    since: u64,
    limit: usize,
    filter: &Filter,
) -> Evidence<JournalEntry> {
    scan(&journal_path(save), limit, |e: &JournalEntry| {
        e.turn >= since && filter.matches(e.wall_time, &e.context)
    })
}

#[must_use]
pub fn query_audit(save: &Path, limit: usize, filter: &Filter) -> Evidence<AuditEntry> {
    scan(&audit_path(save), limit, |e: &AuditEntry| {
        filter.matches(e.wall_time, &e.context)
    })
}

/// Evidence quality is separate from absence of matching rows.
#[derive(Debug, Serialize)]
pub struct Evidence<T> {
    pub entries: Vec<T>,
    pub corrupt_lines: usize,
    pub read_errors: usize,
    pub head_missing: bool,
    pub truncated: bool,
    pub files_read: usize,
}

#[must_use]
pub fn scan_journal(save: &Path, since: u64, limit: usize) -> Evidence<JournalEntry> {
    scan(&journal_path(save), limit, |e: &JournalEntry| {
        e.turn >= since
    })
}

#[must_use]
pub fn scan_audit(save: &Path, limit: usize) -> Evidence<AuditEntry> {
    scan(&audit_path(save), limit, |_| true)
}

#[must_use]
pub fn read_audit(save: &Path, limit: usize) -> (Vec<AuditEntry>, usize) {
    let evidence = scan_audit(save, limit);
    (
        evidence.entries,
        evidence.corrupt_lines + evidence.read_errors,
    )
}

fn scan<T: serde::de::DeserializeOwned>(
    head: &Path,
    limit: usize,
    include: impl Fn(&T) -> bool,
) -> Evidence<T> {
    let _guard = FILE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let limit = limit.min(MAX_QUERY_ENTRIES);
    let mut tail = std::collections::VecDeque::new();
    let mut tail_bytes = 0;
    let mut truncated = false;
    let reads = read_segments(head, |bytes| {
        match serde_json::from_slice::<T>(bytes) {
            Ok(e) if include(&e) => {
                tail_bytes += bytes.len();
                tail.push_back((e, bytes.len()));
                while tail.len() > limit || tail_bytes > MAX_RESULT_BYTES {
                    if let Some((_, size)) = tail.pop_front() {
                        tail_bytes -= size;
                    }
                    truncated = true;
                }
            },
            Ok(_) => {},
            Err(_) => return Line::Corrupt,
        }
        Line::Parsed
    });
    Evidence {
        entries: tail.into_iter().map(|(entry, _)| entry).collect(),
        corrupt_lines: reads.corrupt_lines,
        read_errors: reads.read_errors,
        head_missing: reads.head_missing,
        truncated,
        files_read: reads.files_read,
    }
}

/// What [`read_segments`] saw of the store itself, separate from any rows it
/// parsed: absence and damage that a caller must not read as an empty answer.
#[derive(Debug, Default, Clone, Copy)]
struct ReadStats {
    corrupt_lines: usize,
    read_errors: usize,
    head_missing: bool,
    files_read: usize,
}

/// What a caller wants done with one line.
enum Line {
    /// A row of the stream, or a line the caller chose to ignore.
    Parsed,
    /// Not a row of this stream; counted against the store's quality.
    Corrupt,
}

/// Read the retained evidence stream `head` and its rotated segments, oldest
/// first, handing every non-blank line to `on_line`.
///
/// A line over [`MAX_LINE_BYTES`] counts as corrupt and is discarded without
/// buffering the rest of it. The line buffer is reused across lines: a scan of
/// a long journal would otherwise allocate once per row.
fn read_segments(head: &Path, mut on_line: impl FnMut(&[u8]) -> Line) -> ReadStats {
    let mut stats = ReadStats::default();
    let parent = head
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if let Err(e) = fs::read_dir(parent) {
        if e.kind() != std::io::ErrorKind::NotFound {
            stats.read_errors += 1;
        }
    }
    let mut paths = rotated_segments(head);
    paths.push(head.to_path_buf());
    let mut line = Vec::new();
    for path in paths {
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                if path == head && e.kind() == std::io::ErrorKind::NotFound {
                    stats.head_missing = true;
                } else {
                    stats.read_errors += 1;
                }
                continue;
            },
        };
        stats.files_read += 1;
        let mut reader = std::io::BufReader::new(file);
        loop {
            line.clear();
            match reader
                .by_ref()
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line)
            {
                Ok(0) => break,
                Err(_) => {
                    stats.read_errors += 1;
                    break;
                },
                Ok(_) => {},
            }
            if line.len() as u64 > MAX_LINE_BYTES {
                stats.corrupt_lines += 1;
                if line.last() != Some(&b'\n') && discard_line(&mut reader).is_err() {
                    stats.read_errors += 1;
                    break;
                }
                continue;
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match on_line(&line) {
                Line::Parsed => {},
                Line::Corrupt => stats.corrupt_lines += 1,
            }
        }
    }
    stats
}

/// What the retained journal looks like without materialising its rows.
///
/// `/health` reports growth, not history: it needs the count and the turn
/// range of the retained window, not the recorded turns themselves. A journal
/// row costs ~9 KB in this village, five times the whole save, so building the
/// typed records to read two numbers off them made one health poll the most
/// expensive read in the shell. Rows are validated as journal rows (the JSON of
/// the line and its `turn`) and skipped otherwise; [`scan_journal`] remains the
/// typed reader for callers that need the records.
#[derive(Debug, Default)]
pub struct JournalGrowth {
    /// Rows retained inside the window.
    pub turns: usize,
    /// Lowest turn among them.
    pub oldest_turn: Option<u64>,
    pub truncated: bool,
    pub corrupt_lines: usize,
    pub read_errors: usize,
    pub head_missing: bool,
    pub files_read: usize,
}

/// The one field of a journal row that says where the window starts.
#[derive(Deserialize)]
struct JournalRowTurn {
    turn: u64,
}

/// Summarise the retained journal window. See [`JournalGrowth`].
#[must_use]
pub fn journal_growth(save: &Path, limit: usize) -> JournalGrowth {
    let _guard = FILE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let limit = limit.min(MAX_QUERY_ENTRIES);
    let mut window = std::collections::VecDeque::new();
    let mut window_bytes = 0;
    let mut truncated = false;
    let reads = read_segments(&journal_path(save), |bytes| {
        serde_json::from_slice::<JournalRowTurn>(bytes).map_or(Line::Corrupt, |row| {
            // The same oldest-out retention [`scan`] applies, so the reported
            // window is the newest tail either way.
            window_bytes += bytes.len();
            window.push_back((row.turn, bytes.len()));
            while window.len() > limit || window_bytes > MAX_RESULT_BYTES {
                if let Some((_, size)) = window.pop_front() {
                    window_bytes -= size;
                }
                truncated = true;
            }
            Line::Parsed
        })
    });
    JournalGrowth {
        turns: window.len(),
        oldest_turn: window.iter().map(|(turn, _)| *turn).min(),
        truncated,
        corrupt_lines: reads.corrupt_lines,
        read_errors: reads.read_errors,
        head_missing: reads.head_missing,
        files_read: reads.files_read,
    }
}

fn discard_line(reader: &mut impl std::io::BufRead) -> std::io::Result<()> {
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(());
        }
        let newline = bytes.iter().position(|b| *b == b'\n');
        let n = newline.map_or(bytes.len(), |n| n + 1);
        reader.consume(n);
        if newline.is_some() {
            return Ok(());
        }
    }
}

fn append_jsonl<T: Serialize>(
    path: &Path,
    value: &T,
    rotate_bytes: u64,
    stem: &str,
) -> Result<(), String> {
    let _guard = FILE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut line = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if line.len() as u64 >= MAX_LINE_BYTES {
        return Err(format!("{stem} entry exceeds evidence limit"));
    }
    line.push(b'\n');
    rotate_if_full(path, rotate_bytes, stem).map_err(|e| e.to_string())?;
    append_synced(path, &line).map_err(|e| format!("{stem} append {}: {e}", path.display()))
}

fn append_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    // One handle serves the tail check and the append: the previous byte can be
    // read from the same descriptor, so an append does not open the file twice.
    // `O_APPEND` still places the write at the end whatever the read cursor is.
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    // A crash can leave a partial tail. Separate it before the next entry so
    // one damaged record cannot consume the following successful write.
    let len = f.metadata()?.len();
    if len > 0 {
        use std::io::{Read as _, Seek as _, SeekFrom};
        f.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            f.write_all(b"\n")?;
        }
    }
    f.write_all(bytes)?;
    f.flush()?;
    f.sync_all()
}

/// Remove one log path, whatever a foreign writer left there. A directory can be
/// renamed but never removed by [`fs::remove_file`], so it would otherwise climb
/// the slots and wedge every append.
fn retire(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn rotate_if_full(path: &Path, rotate_bytes: u64, stem: &str) -> std::io::Result<()> {
    let len = fs::metadata(path).map_or(0, |m| m.len());
    if len < rotate_bytes {
        return Ok(());
    }
    let oldest = segment_path(path, stem, RETAINED_SEGMENTS);
    let parked = slot_path(path, stem, "parked");
    // Clear foreign directories before any rename: they read as segments and
    // would be shifted into the retained slots untouched.
    for slot in rotated_segments(path)
        .into_iter()
        .chain(std::iter::once(parked.clone()))
    {
        if slot.is_dir() {
            retire(&slot)?;
        }
    }
    // A rotation that failed after parking left the segment aside; put it back
    // so a retry does not retire retained evidence it never replaced.
    if parked.exists() {
        if oldest.exists() {
            retire(&parked)?;
        } else {
            fs::rename(&parked, &oldest)?;
        }
    }
    // Older installations could hold more segments than the retained count.
    for old in rotated_segments(path) {
        if segment_number(path, &old).is_some_and(|n| n > RETAINED_SEGMENTS) {
            retire(&old)?;
        }
    }
    // Free the oldest slot by renaming, not deleting: a rename chain that
    // fails partway can then put every segment back.
    let parked_ok = if oldest.exists() {
        fs::rename(&oldest, &parked)?;
        true
    } else {
        false
    };
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let shift = (|| {
        for n in (1..RETAINED_SEGMENTS).rev() {
            let old = segment_path(path, stem, n);
            if old.exists() {
                let next = segment_path(path, stem, n + 1);
                fs::rename(&old, &next)?;
                moved.push((old, next));
            }
        }
        let head = segment_path(path, stem, 1);
        fs::rename(path, &head)?;
        moved.push((path.to_path_buf(), head));
        Ok(())
    })();
    match shift {
        Ok(()) => {
            // The head has moved, so the record still has to be appended even
            // if this cleanup fails; the next rotation retires the leftover.
            if parked_ok {
                let _ = retire(&parked);
            }
            Ok(())
        },
        Err(error) => {
            unwind(&moved);
            if parked_ok && !oldest.exists() {
                let _ = fs::rename(&parked, &oldest);
            }
            Err(error)
        },
    }
}

/// One slot beside the head: `{save}.{stem}.{slot}.jsonl`.
///
/// The slot name is derived from the head's own name bytes, so a save whose
/// name is not UTF-8 writes and discovers segments under exactly that name
/// instead of a lossy rendering another save could also claim.
fn slot_path(path: &Path, stem: &str, slot: &str) -> PathBuf {
    slot_name(path, stem, slot).map_or_else(
        || path.with_file_name(format!("{stem}.{slot}.jsonl")),
        |name| path.with_file_name(name),
    )
}

#[cfg(unix)]
fn slot_name(head: &Path, stem: &str, slot: &str) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    let name = head.file_name()?.as_bytes();
    let suffix = format!("{stem}.jsonl");
    let prefix = name.strip_suffix(suffix.as_bytes())?;
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(format!("{stem}.{slot}.jsonl").as_bytes());
    Some(std::ffi::OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn slot_name(head: &Path, stem: &str, slot: &str) -> Option<std::ffi::OsString> {
    let name = head.file_name()?.to_string_lossy();
    let suffix = format!("{stem}.jsonl");
    let prefix = name.strip_suffix(&suffix)?;
    Some(std::ffi::OsString::from(format!(
        "{prefix}{stem}.{slot}.jsonl"
    )))
}

/// Put every rename a failed rotation made back, newest first.
fn unwind(moved: &[(PathBuf, PathBuf)]) {
    for (old, next) in moved.iter().rev() {
        let _ = fs::rename(next, old);
    }
}

fn segment_path(path: &Path, stem: &str, n: u32) -> PathBuf {
    slot_path(path, stem, &n.to_string())
}

#[cfg(unix)]
fn segment_number(head: &Path, candidate: &Path) -> Option<u32> {
    use std::os::unix::ffi::OsStrExt as _;
    let head_name = head.file_name()?.as_bytes();
    let head_name = head_name.strip_suffix(b".jsonl")?;
    let candidate_name = candidate.file_name()?.as_bytes();
    let rest = candidate_name.strip_prefix(head_name)?.strip_prefix(b".")?;
    let digits = rest.strip_suffix(b".jsonl")?;
    if digits.is_empty() || (digits.len() > 1 && digits[0] == b'0') {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(not(unix))]
fn segment_number(head: &Path, candidate: &Path) -> Option<u32> {
    let head_name = head.file_name()?.to_string_lossy();
    let head_name = head_name.strip_suffix(".jsonl")?;
    let candidate_name = candidate.file_name()?.to_str()?;
    let rest = candidate_name.strip_prefix(head_name)?.strip_prefix('.')?;
    let digits = rest.strip_suffix(".jsonl")?;
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    digits.parse().ok()
}

fn rotated_segments(head: &Path) -> Vec<PathBuf> {
    let dir = head
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| segment_number(head, p).is_some())
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(segment_number(head, p)));
    out
}

/// Summarise an inject request for the audit log: kind, target and the
/// submitted word when the kind was rejected, never
/// observer prose.
#[must_use]
pub fn inject_summary(
    kind: &str,
    target: Option<&str>,
    to: Option<&str>,
    place: Option<&str>,
) -> Value {
    let (kind, submitted) = match kind {
        "whisper" | "gift" | "weather" | "rumor" | "feast" | "oath" | "goal" => (kind, None),
        // A rejected typo is still something an observer did; keep the word
        // they sent, bounded, so the audit can answer what was attempted.
        other => ("unknown", Some(bounded_word(other))),
    };
    let mut summary = json!({
        "kind": kind,
        "target": target,
        "to": to,
        "place": place,
    });
    if let Some(submitted) = submitted.filter(|word| !word.is_empty()) {
        summary["submitted_kind"] = Value::from(submitted);
    }
    summary
}

/// The observable word an observer sent, capped to a short printable run.
fn bounded_word(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii_graphic)
        .take(24)
        .collect()
}

/// Compare selected journal records against the current pure founding engine.
///
/// Does not replay observer inputs, desktop civic systems or historical builds.
/// A match covers only `compare_records` fields, not historical world equality.
/// Mixed village identities and excessive replay work are rejected explicitly.
#[must_use]
pub fn verify_journal(entries: &[JournalEntry]) -> Option<VerifyDivergence> {
    if entries.is_empty() {
        return None;
    }
    if entries
        .iter()
        .any(|e| e.seed != entries[0].seed || e.context.world_id != entries[0].context.world_id)
    {
        return Some(VerifyDivergence {
            turn: entries[0].turn,
            reason: "mixed villages: select one world_id".into(),
        });
    }
    if entries.windows(2).any(|pair| pair[0].turn >= pair[1].turn) {
        return Some(VerifyDivergence {
            turn: entries[0].turn,
            reason: "journal entries are not in ascending turn order".into(),
        });
    }
    if entries
        .iter()
        .map(|e| e.turn)
        .max()
        .is_some_and(|t| t > 10_000)
    {
        return Some(VerifyDivergence {
            turn: entries[0].turn,
            reason: "replay budget exceeded (10000 turns)".into(),
        });
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
/// The compared fields are the engine's observable contract: the turn and day
/// counters, roster, actions, lines, verdicts, motives, births and the summary
/// counts. Informational and not compared: `decision_id`, `event_range`,
/// `factors`, and the derived/decision-side fields (`note`, `register`,
/// `dilemma`, `intensity`, `confidence`, `rejected`, `job`) whose floats and
/// candidate lists are not part of the committed outcome contract.
#[must_use]
pub fn compare_records(a: &TurnRecord, b: &TurnRecord) -> Option<String> {
    if a.turn != b.turn {
        return Some(format!("turn {} vs {}", a.turn, b.turn));
    }
    if a.day != b.day {
        return Some(format!("turn {}: day {} vs {}", a.turn, a.day, b.day));
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
    if a.births.len() != b.births.len() {
        return Some(format!(
            "turn {}: {} births vs {}",
            a.turn,
            a.births.len(),
            b.births.len()
        ));
    }
    for (x, y) in a.births.iter().zip(b.births.iter()) {
        if x.child != y.child
            || x.parents != y.parents
            || x.generation != y.generation
            || x.place != y.place
        {
            return Some(format!("turn {}: birth of {} differs", a.turn, x.child));
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
        let dir =
            std::env::temp_dir().join(format!("society-diag-{}-{}", std::process::id(), prefix));
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

    /// A save name that is not UTF-8 must still name its own segments; matching
    /// on `to_str` returned `None` and silently narrowed history to the head.
    #[cfg(unix)]
    #[test]
    fn segment_number_matches_a_non_utf8_save_name() {
        use std::os::unix::ffi::OsStrExt as _;
        let head = Path::new(std::ffi::OsStr::from_bytes(b"v\xff.json.journal.jsonl"));
        let segment = Path::new(std::ffi::OsStr::from_bytes(b"v\xff.json.journal.2.jsonl"));
        assert_eq!(segment_number(head, segment), Some(2));
        let unrelated = Path::new(std::ffi::OsStr::from_bytes(b"notes.2.jsonl"));
        assert_eq!(segment_number(head, unrelated), None);
    }

    /// A directory where a segment belongs must not wedge rotation: it can be
    /// renamed but never removed by `remove_file`, so it would climb the slots and
    /// fail every append once it reached the parked path.
    #[test]
    fn rotation_retires_a_foreign_directory_in_a_slot_and_at_the_park() {
        let dir = tmp("rotation-directory");
        let save = dir.join("v.json");
        let head = journal_path(&save);
        let two = segment_path(&head, "journal", 2);
        let parked = slot_path(&head, "journal", "parked");
        fs::create_dir_all(two.join("nested")).unwrap();
        fs::write(two.join("nested/notes.txt"), b"foreign\n").unwrap();
        fs::create_dir_all(parked.join("nested")).unwrap();
        append_turn(&save, 7, &rec(0), "test", 1).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&head)
            .unwrap()
            .set_len(JOURNAL_ROTATE_BYTES)
            .unwrap();
        append_turn(&save, 7, &rec(1), "test", 1).unwrap();
        assert!(!two.exists(), "the foreign directory must not be shifted");
        assert!(!parked.exists(), "the parked directory must be retired");
        assert!(segment_path(&head, "journal", 1).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// A padded slot name is a foreign artifact, like the server assumes when it
    /// only ever writes canonical decimals.
    #[test]
    fn segment_number_ignores_a_padded_slot_name() {
        let head = Path::new("journal.jsonl");
        assert_eq!(segment_number(head, Path::new("journal.2.jsonl")), Some(2));
        assert_eq!(segment_number(head, Path::new("journal.02.jsonl")), None);
        assert_eq!(segment_number(head, Path::new("journal.jsonl.jsonl")), None);
    }

    /// A rotation that fails after shifting puts every renamed segment back.
    #[test]
    fn a_failed_rotation_unwinds_its_renames() {
        let dir = tmp("rotation-unwind");
        let head = dir.join("journal.jsonl");
        let one = dir.join("journal.1.jsonl");
        let two = dir.join("journal.2.jsonl");
        let three = dir.join("journal.3.jsonl");
        let four = dir.join("journal.4.jsonl");
        // The state a half-done chain leaves: 3→4, 2→3 and 1→2 already ran.
        fs::write(&four, b"old3\n").unwrap();
        fs::write(&three, b"old2\n").unwrap();
        fs::write(&two, b"old1\n").unwrap();
        fs::write(&one, b"head\n").unwrap();
        unwind(&[
            (three.clone(), four.clone()),
            (two.clone(), three.clone()),
            (one.clone(), two.clone()),
        ]);
        assert_eq!(fs::read(&one).unwrap(), b"old1\n");
        assert_eq!(fs::read(two).unwrap(), b"old2\n");
        assert_eq!(fs::read(three).unwrap(), b"old3\n");
        assert!(!four.exists());
        assert!(!head.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn journal_bytes_count_every_retained_segment() {
        let dir = tmp("journal-bytes");
        let save = dir.join("v.json");
        let head = journal_path(&save);
        fs::write(&head, b"head\n").unwrap();
        fs::write(segment_path(&head, "journal", 1), b"one\n").unwrap();
        fs::write(segment_path(&head, "journal", 2), b"two\n").unwrap();
        assert_eq!(journal_bytes(&save), 5 + 4 + 4);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A rotation that cannot rename must not delete what it was going to
    /// retire: the oldest segment stays until a rotation actually replaces it.
    #[cfg(unix)]
    #[test]
    fn failed_rotation_keeps_the_oldest_segment() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tmp("rotation-failure");
        let save = dir.join("v.json");
        let head = journal_path(&save);
        let oldest = segment_path(&head, "journal", RETAINED_SEGMENTS);
        fs::write(&oldest, b"oldest\n").unwrap();
        append_turn(&save, 7, &rec(0), "test", 1).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&head)
            .unwrap()
            .set_len(JOURNAL_ROTATE_BYTES)
            .unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let error = append_turn(&save, 7, &rec(1), "test", 1);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            error.is_err(),
            "a read-only directory must fail the rotation"
        );
        assert_eq!(fs::read(&oldest).unwrap(), b"oldest\n");
        assert!(head.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// A rotation interrupted after parking its oldest segment restores that
    /// segment on the retry instead of shedding an extra one.
    #[test]
    fn interrupted_rotation_restores_its_parked_segment() {
        let dir = tmp("rotation-parked");
        let save = dir.join("v.json");
        let head = journal_path(&save);
        for n in 1..=RETAINED_SEGMENTS {
            fs::write(segment_path(&head, "journal", n), format!("old{n}\n")).unwrap();
        }
        fs::write(slot_path(&head, "journal", "parked"), b"parked\n").unwrap();
        append_turn(&save, 7, &rec(0), "test", 1).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&head)
            .unwrap()
            .set_len(JOURNAL_ROTATE_BYTES)
            .unwrap();
        append_turn(&save, 7, &rec(1), "test", 1).unwrap();
        assert!(!slot_path(&head, "journal", "parked").exists());
        assert_eq!(
            rotated_segments(&head).len(),
            RETAINED_SEGMENTS as usize,
            "a rotation restores the parked segment before it retires one"
        );
        assert_eq!(
            fs::read(segment_path(&head, "journal", RETAINED_SEGMENTS)).unwrap(),
            b"old3\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A save whose name is not UTF-8 writes and finds its own segments, and a
    /// UTF-8 twin spelled with U+FFFD cannot claim them.
    #[cfg(unix)]
    #[test]
    fn segments_are_matched_by_exact_name_bytes() {
        use std::os::unix::ffi::OsStrExt as _;
        let head = Path::new(std::ffi::OsStr::from_bytes(b"v\xff.json.journal.jsonl"));
        let own = slot_path(head, "journal", "2");
        assert_eq!(own.as_os_str().as_bytes(), b"v\xff.json.journal.2.jsonl");
        assert_eq!(segment_number(head, &own), Some(2));
        let twin = Path::new("v\u{fffd}.json.journal.2.jsonl");
        assert_eq!(segment_number(head, twin), None);
    }

    /// Whitespace is absence, matching the epoch rule, so a stray space cannot
    /// invert an identity filter into "legacy rows only".
    #[test]
    fn a_blank_identity_filter_is_no_filter() {
        let filter: Filter =
            serde_json::from_value(json!({"world_id": " ", "session_id": "\t"})).unwrap();
        assert!(filter.is_empty());
    }

    #[test]
    fn a_rejected_inject_keeps_the_submitted_word() {
        let summary = inject_summary("whispr", Some("Ada"), None, None);
        assert_eq!(summary["kind"], "unknown");
        assert_eq!(summary["submitted_kind"], "whispr");
        assert!(inject_summary("gift", Some("Ada"), None, None)
            .get("submitted_kind")
            .is_none());
    }

    #[test]
    fn compare_records_checks_the_day_counter() {
        let a = rec(3);
        let mut b = rec(3);
        b.day += 1;
        assert!(compare_records(&a, &b).is_some_and(|reason| reason.contains("day")));
    }

    #[test]
    fn verify_rejects_a_non_ascending_slice() {
        let entries: Vec<JournalEntry> = [0, 2, 1]
            .into_iter()
            .map(|turn| JournalEntry {
                context: Context::default(),
                journal_version: JOURNAL_VERSION,
                build: "test".into(),
                save_version: crate::persistence::SAVE_VERSION,
                seed: 7,
                turn,
                day: 0,
                wall_time: 0,
                record: rec(turn),
            })
            .collect();
        let divergence = verify_journal(&entries).unwrap();
        assert!(divergence.reason.contains("ascending"), "{divergence:?}");
    }

    /// `/health` reports the journal window without materialising its rows. The
    /// window, its oldest turn and the clipping flag must be exactly what the
    /// typed reader returns, or the two answers describe different stores.
    #[test]
    fn journal_growth_reports_the_typed_reader_s_window() {
        let dir = tmp("journal-growth");
        let save = dir.join("v.json");
        for turn in 0..40 {
            append_turn(&save, 7, &rec(turn), "test", 2).unwrap();
        }
        // A torn tail is damage the health report must still count.
        let path = journal_path(&save);
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{\"turn\": 41, ");
        fs::write(&path, &bytes).unwrap();

        for limit in [1, 5, 39, 100] {
            let typed = scan_journal(&save, 0, limit);
            let growth = journal_growth(&save, limit);
            assert_eq!(growth.turns, typed.entries.len(), "limit {limit}");
            assert_eq!(
                growth.oldest_turn,
                typed.entries.iter().map(|e| e.turn).min(),
                "limit {limit}"
            );
            assert_eq!(growth.truncated, typed.truncated, "limit {limit}");
            assert_eq!(growth.corrupt_lines, typed.corrupt_lines, "limit {limit}");
            assert_eq!(growth.files_read, typed.files_read, "limit {limit}");
            assert_eq!(growth.head_missing, typed.head_missing, "limit {limit}");
        }

        fs::remove_file(&path).unwrap();
        let absent = journal_growth(&save, 100);
        assert_eq!(absent.turns, 0);
        assert_eq!(absent.oldest_turn, None);
        assert!(absent.head_missing);
    }

    /// The window spans rotated segments, so the growth reader has to walk them
    /// in the same order as the typed one and drop the same oldest rows.
    #[test]
    fn journal_growth_matches_the_typed_reader_across_rotated_segments() {
        let dir = tmp("journal-growth-rotated");
        let save = dir.join("v.json");
        let head = journal_path(&save);
        for (slot, turns) in [(1_u32, 0..3_u64), (2, 3..6)] {
            let path = slot_path(&head, "journal", &slot.to_string());
            let mut bytes = Vec::new();
            for turn in turns {
                bytes.extend_from_slice(
                    serde_json::to_string(&JournalEntry {
                        context: Context::default(),
                        journal_version: JOURNAL_VERSION,
                        build: "test".into(),
                        save_version: crate::persistence::SAVE_VERSION,
                        seed: 7,
                        turn,
                        day: 0,
                        wall_time: 0,
                        record: rec(turn),
                    })
                    .unwrap()
                    .as_bytes(),
                );
                bytes.push(b'\n');
            }
            fs::write(path, bytes).unwrap();
        }
        for turn in 6..9 {
            append_turn(&save, 7, &rec(turn), "test", 2).unwrap();
        }

        for limit in [1, 2, 4, 8, 100] {
            let typed = scan_journal(&save, 0, limit);
            let growth = journal_growth(&save, limit);
            assert_eq!(growth.turns, typed.entries.len(), "limit {limit}");
            assert_eq!(
                growth.oldest_turn,
                typed.entries.iter().map(|e| e.turn).min(),
                "limit {limit}"
            );
            assert_eq!(growth.truncated, typed.truncated, "limit {limit}");
            assert_eq!(growth.files_read, typed.files_read, "limit {limit}");
            assert_eq!(growth.corrupt_lines, typed.corrupt_lines, "limit {limit}");
        }
    }

    #[test]
    fn audit_production_writer_rotates_to_a_discoverable_path() {
        let dir = tmp("actual-audit-rotation");
        let save = dir.join("v.json");
        append_audit(&save, "before", 0, json!({})).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(audit_path(&save))
            .unwrap()
            .set_len(AUDIT_ROTATE_BYTES)
            .unwrap();
        append_audit(&save, "after", 1, json!({})).unwrap();
        assert_eq!(rotated_segments(&audit_path(&save)).len(), 1);
        let evidence = scan_audit(&save, 10);
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|e| e.action.as_str())
                .collect::<Vec<_>>(),
            vec!["before", "after"]
        );
        assert_eq!(evidence.corrupt_lines, 1); // sparse padding is deliberately invalid evidence
        assert_eq!(evidence.files_read, 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotation_is_bounded_discoverable_and_ordered() {
        let dir = tmp("rotation");
        let save = dir.join("contains-journal-name.json");
        let head = audit_path(&save);
        for t in 0..8 {
            let entry = AuditEntry {
                context: Context::default(),
                wall_time: t,
                action: "test".into(),
                build: "test".into(),
                turn: 0,
                detail: json!({}),
            };
            append_jsonl(&head, &entry, 1, "diagnostics").unwrap();
        }
        assert_eq!(rotated_segments(&head).len(), RETAINED_SEGMENTS as usize);
        let evidence = scan_audit(&save, 100);
        assert_eq!(evidence.files_read, 5);
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|e| e.wall_time)
                .collect::<Vec<_>>(),
            vec![3, 4, 5, 6, 7]
        );
        assert!(!evidence.truncated);
        assert!(scan_audit(&save, 2).truncated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn torn_tail_does_not_consume_next_successful_write() {
        let dir = tmp("torn-tail");
        let save = dir.join("v.json");
        fs::write(audit_path(&save), b"{partial").unwrap();
        append_audit(&save, "after_crash", 0, json!({})).unwrap();
        let evidence = scan_audit(&save, 100);
        assert_eq!(evidence.corrupt_lines, 1);
        assert_eq!(evidence.entries[0].action, "after_crash");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_failures_and_missing_heads_are_explicit() {
        let dir = tmp("read-error");
        let save = dir.join("v.json");
        assert!(scan_audit(&save, 10).head_missing);
        fs::create_dir(audit_path(&save)).unwrap();
        assert!(scan_audit(&save, 10).read_errors > 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filters_preserve_same_seed_reset_histories_and_limit_after_filtering() {
        let dir = tmp("identities");
        let save = dir.join("v.json");
        for world in ["first", "reset"] {
            append_turn_context(
                &save,
                7,
                &rec(0),
                "test",
                2,
                Context {
                    world_id: world.into(),
                    operation_id: Some(world.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(scan_journal(&save, 0, 10).entries.len(), 2);
        let filter = Filter {
            world_id: Some("first".into()),
            ..Default::default()
        };
        let evidence = query_journal(&save, 0, 1, &filter);
        assert_eq!(evidence.entries.len(), 1);
        assert_eq!(evidence.entries[0].context.world_id, "first");
        assert!(!evidence.truncated);
        assert!(verify_journal(&scan_journal(&save, 0, 10).entries)
            .unwrap()
            .reason
            .contains("mixed villages"));
        fs::remove_dir_all(dir).unwrap();
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
                context: Context::default(),
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
        let mut bad = r1;
        bad.agents[0].say = "tampered line".into();
        let entries: Vec<JournalEntry> = [r0, bad]
            .iter()
            .map(|r| JournalEntry {
                context: Context::default(),
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

    #[test]
    fn verify_reports_a_divergent_birth() {
        // Births are part of the committed record: a replay that settles the
        // same turn elsewhere must not verify as a match.
        let mut w = society_sim::World::founding(7);
        let rec = society_sim::turn::step(&mut w).expect("turn");
        let mut replay = rec.clone();
        replay.births = vec![society_sim::turn::BirthRecord {
            child: "Nobody".into(),
            parents: vec!["Ada".into(), "Bo".into()],
            generation: 1,
            place: "market".into(),
        }];
        let reason = compare_records(&rec, &replay).expect("births differ");
        assert!(reason.contains("birth"), "{reason}");
    }
}
