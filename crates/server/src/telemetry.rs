//! Process/operation identity and telemetry health. No observer payloads here.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);
static SESSION: OnceLock<String> = OnceLock::new();
tokio::task_local! { pub static OPERATION: String; }

#[must_use]
pub fn session_id() -> String {
    SESSION
        .get_or_init(|| {
            format!(
                "{}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            )
        })
        .clone()
}

#[must_use]
pub fn operation_id() -> String {
    format!(
        "{}-{}",
        session_id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Context {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub world_id: String,
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub source_id: String,
}

#[derive(Debug)]
pub struct Telemetry {
    world_id: Mutex<String>,
    identity_loaded_at_boot: bool,
    failures: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Telemetry {
    #[must_use]
    pub fn open(save: &Path, fresh: bool) -> Self {
        let identity = identity_path(save);
        let old = if fresh {
            None
        } else {
            read_identity(&identity).filter(|s| {
                !s.is_empty()
                    && s.len() < 128
                    && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            })
        };
        let new_identity = old.is_none();
        let state = Self {
            identity_loaded_at_boot: !new_identity,
            world_id: Mutex::new(old.unwrap_or_else(operation_id)),
            failures: AtomicU64::new(0),
            last_error: Mutex::new(None),
        };
        if new_identity {
            state.persist_identity(save);
        }
        publish_world(&state.context().world_id);
        state
    }

    pub fn reset(&self, save: &Path) {
        let previous = {
            let mut world_id = self
                .world_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *world_id, operation_id())
        };
        if !self.persist_identity(save) {
            // The sidecar still names the previous incarnation. Roll back so
            // the process and the disk agree and a restart cannot split one
            // journal across two world ids.
            *self
                .world_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = previous;
        }
        publish_world(&self.context().world_id);
    }

    fn persist_identity(&self, save: &Path) -> bool {
        let id = self.context().world_id;
        let path = identity_path(save);
        let result = (|| -> std::io::Result<()> {
            if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(p)?;
            }
            let tmp = path.with_extension("identity.tmp");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(tmp, path)
        })()
        .map_err(|e| e.to_string());
        self.outcome("identity", &result);
        result.is_ok()
    }

    #[must_use]
    pub fn context(&self) -> Context {
        Context {
            session_id: session_id(),
            world_id: self
                .world_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            operation_id: OPERATION.try_with(Clone::clone).ok(),
            source_id: env!("SOCIETY_SOURCE_ID").into(),
        }
    }

    pub fn outcome(&self, sink: &str, result: &Result<(), String>) {
        let Err(error) = result else { return };
        self.failures.fetch_add(1, Ordering::Relaxed);
        // The retained field stays a category: the sink it would describe may
        // be unreadable, and raw OS text must not become the durable summary.
        // Stderr is not retained evidence, so the detail belongs there.
        let msg = format!("{sink}_write_failed");
        *self
            .last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(msg.clone());
        eprintln!("telemetry: {msg}: {error}");
    }

    #[must_use]
    pub fn status(&self) -> Value {
        json!({"context": self.context(), "identity_loaded_at_boot": self.identity_loaded_at_boot, "write_failures": self.failures.load(Ordering::Relaxed),
            "last_error": self.last_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()})
    }
}

fn read_identity(path: &Path) -> Option<String> {
    let mut value = String::new();
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            // An unreadable identity is not an absent one: the caller mints a
            // fresh world id next, so the continuity break must be visible.
            eprintln!("telemetry: identity_read_failed: {e}");
            return None;
        },
    };
    match file.take(129).read_to_string(&mut value) {
        Ok(_) => Some(value),
        Err(e) => {
            eprintln!("telemetry: identity_read_failed: {e}");
            None
        },
    }
}

fn identity_path(save: &Path) -> PathBuf {
    let mut name = save.as_os_str().to_os_string();
    name.push(".identity");
    PathBuf::from(name)
}

/// The incarnation the process is currently writing evidence for.
///
/// The panic hook cannot borrow the telemetry state (it may panic while that
/// state is held), so the world id is published here when an identity is
/// loaded or minted and read back with `try_lock`.
static CURRENT_WORLD: Mutex<String> = Mutex::new(String::new());

fn publish_world(world_id: &str) {
    let Ok(mut slot) = CURRENT_WORLD.lock() else {
        return;
    };
    slot.clear();
    slot.push_str(world_id);
}

fn published_world() -> String {
    CURRENT_WORLD
        .try_lock()
        .map_or_else(|_| String::new(), |slot| slot.clone())
}

/// Keep the latest panic location across process restarts without persisting
/// the panic message (which can contain observer input).
///
/// Separate from the journal lock so a panic during telemetry cannot deadlock its own hook.
pub fn install_panic_hook(save: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let context = Context {
            session_id: session_id(),
            operation_id: OPERATION.try_with(Clone::clone).ok(),
            source_id: env!("SOCIETY_SOURCE_ID").into(),
            world_id: published_world(),
        };
        let value = json!({"wall_time": crate::diagnostics::wall_now(), "action": "panic",
            "context": context, "file": info.location().map(std::panic::Location::file),
            "line": info.location().map(std::panic::Location::line),
            "coverage": "latest panic location only; message omitted; no signal/OOM/power-loss handler"});
        if let Err(e) = write_crash(&save, &value) {
            eprintln!("telemetry: crash_write_failed: {e}");
        }
        previous(info);
    }));
}

fn write_crash(save: &Path, value: &Value) -> std::io::Result<()> {
    let mut name = save.as_os_str().to_os_string();
    name.push(".crash.json");
    let path = PathBuf::from(name);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(serde_json::to_string(value)?.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    /// A reset whose identity write fails must not leave the process naming a
    /// world the sidecar does not: the next boot would split one journal across
    /// two ids.
    #[cfg(unix)]
    #[test]
    fn failed_identity_persist_keeps_the_previous_world_id() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("society-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.join("v.json");
        let state = super::Telemetry::open(&save, false);
        let before = state.context().world_id;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        state.reset(&save);
        let after = state.context().world_id;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(after, before, "identity rolled back after a failed write");
        assert!(state.status()["write_failures"].as_u64().unwrap() > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn panic_hook_retains_location_without_message() {
        if let Some(path) = std::env::var_os("SOCIETY_TEST_PANIC_PATH") {
            let save = std::path::PathBuf::from(&path);
            let _ = super::Telemetry::open(&save, false);
            super::install_panic_hook(save);
            panic!("PRIVATE_PANIC_SENTINEL");
        }
        let dir = std::env::temp_dir().join(format!("society-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "telemetry::tests::panic_hook_retains_location_without_message",
            ])
            .env("SOCIETY_TEST_PANIC_PATH", dir.join("v.json"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        let text = std::fs::read_to_string(dir.join("v.json.crash.json")).unwrap();
        assert!(!text.contains("PRIVATE_PANIC_SENTINEL"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value["file"].as_str().unwrap().ends_with("telemetry.rs"));
        assert!(value["line"].as_u64().unwrap() > 0);
        let identity = std::fs::read_to_string(dir.join("v.json.identity")).unwrap();
        assert_eq!(value["context"]["world_id"], identity.trim());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
