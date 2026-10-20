//! Persistence. This is the only module in the server that touches the
//! filesystem, and it is the only new IO the society has.
//!
//! One file per village, written after every committed turn: close the window,
//! come back later, and the village is where you left it. Two failure modes are
//! designed around:
//!
//! * a half-written world — writes go to a sibling temporary file, are flushed
//!   and synced, then renamed over the target;
//! * a silent reset — a corrupt or version-mismatched save is a loud startup
//!   error naming the path, never a fresh founding.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use society_sim::{Event, EventKind, Place, World};

/// Bumped whenever the serialised shape changes incompatibly. A save carrying
/// any other version is rejected with its path, not misparsed.
pub const SAVE_VERSION: u32 = 2;

/// Overrides where the village file lives, so several villages can sit side by
/// side: `SOCIETY_SAVE_PATH=data/second.json cargo run`.
pub const SAVE_PATH_ENV: &str = "SOCIETY_SAVE_PATH";

/// Default under the project's ignored data directory.
pub const DEFAULT_SAVE_PATH: &str = "data/society.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    world: World,
}

/// Borrowed form for writing: a save must not clone the world it is saving.
#[derive(Serialize)]
struct EnvelopeRef<'a> {
    version: u32,
    world: &'a World,
}

/// Lightweight metadata summary of a saved village on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SaveSummary {
    pub version: u32,
    pub seed: u64,
    pub turn: u64,
    pub day: u64,
    pub living: usize,
    pub season: String,
    pub weather_kind: String,
    pub halted: Option<String>,
}

#[derive(Debug)]
pub enum SaveError {
    /// The file could not be read, written, or removed.
    Io {
        path: PathBuf,
        action: &'static str,
        source: std::io::Error,
    },
    /// The bytes on disk are not a save this build understands.
    Corrupt { path: PathBuf, reason: String },
    /// A different schema version wrote this file.
    Version {
        path: PathBuf,
        found: u64,
        expected: u32,
    },
    /// The world could not be turned into a save that would load back.
    Unwritable { path: PathBuf, reason: String },
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                path,
                action,
                source,
            } => write!(f, "cannot {action} save {}: {source}", path.display()),
            Self::Corrupt { path, reason } => {
                write!(f, "save {} is corrupt: {reason}", path.display())
            },
            Self::Version {
                path,
                found,
                expected,
            } => write!(
                f,
                "save {} has format version {found}, this build reads version {expected}",
                path.display()
            ),
            Self::Unwritable { path, reason } => {
                write!(f, "cannot write save {}: {reason}", path.display())
            },
        }
    }
}

impl std::error::Error for SaveError {}

/// Resolve the village path: the environment variable if set, otherwise the
/// project's ignored data directory.
pub fn save_path() -> PathBuf {
    std::env::var_os(SAVE_PATH_ENV).map_or_else(|| PathBuf::from(DEFAULT_SAVE_PATH), PathBuf::from)
}

/// Load a village. An absent file is not an error: it means first run, and the
/// caller founds a fresh world from the seed.
///
/// # Errors
///
/// Returns a `SaveError` if the file cannot be read, parsed, or migrated.
pub fn load(path: &Path) -> Result<Option<World>, SaveError> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SaveError::Io {
                path: path.to_path_buf(),
                action: "read",
                source,
            })
        },
    };

    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| SaveError::Corrupt {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    let Some(version) = value.get("version").and_then(serde_json::Value::as_u64) else {
        return Err(SaveError::Corrupt {
            path: path.to_path_buf(),
            reason: "no format version".into(),
        });
    };
    if version != u64::from(SAVE_VERSION) {
        return Err(SaveError::Version {
            path: path.to_path_buf(),
            found: version,
            expected: SAVE_VERSION,
        });
    }
    let world = value.get("world").ok_or_else(|| SaveError::Corrupt {
        path: path.to_path_buf(),
        reason: "no world in save".into(),
    })?;
    let world = World::deserialize(world).map_err(|e| SaveError::Corrupt {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    Ok(Some(world))
}

/// Inspect a village save file without replacing the active simulation state.
///
/// # Errors
///
/// Returns a `SaveError` under the same conditions as [`load`].
pub fn inspect(path: &Path) -> Result<Option<SaveSummary>, SaveError> {
    let Some(w) = load(path)? else {
        return Ok(None);
    };
    Ok(Some(SaveSummary {
        version: SAVE_VERSION,
        seed: w.seed,
        turn: w.turn,
        day: w.day,
        living: w.living().count(),
        season: w.season().id().to_owned(),
        weather_kind: w.weather_kind.id().to_owned(),
        halted: w.halted,
    }))
}

/// Check that every floating-point invariant in `World` is finite before disk
/// serialization.
///
/// # Errors
///
/// Returns a `SaveError` if any floating-point field is not finite.
pub fn validate_world(world: &World, path: &Path) -> Result<(), SaveError> {
    if !world.weather.is_finite() || !world.village_morale.is_finite() {
        return Err(SaveError::Unwritable {
            path: path.to_path_buf(),
            reason: "non-finite world weather or village_morale".into(),
        });
    }
    for a in &world.agents {
        if !a.standing.is_finite() || a.trust.iter().any(|(_, v)| !v.is_finite()) {
            return Err(SaveError::Unwritable {
                path: path.to_path_buf(),
                reason: format!("non-finite standing or trust on agent {}", a.name),
            });
        }
    }
    Ok(())
}

/// Save a village atomically. The bytes are serialised and verified to load
/// back before the target is touched, so a save that could not be read is
/// never written.
///
/// # Errors
///
/// Returns a `SaveError` if validation, serialisation, verification or the write fails.
pub fn save(world: &World, path: &Path) -> Result<(), SaveError> {
    validate_world(world, path)?;
    let envelope = EnvelopeRef {
        version: SAVE_VERSION,
        world,
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|e| SaveError::Unwritable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    // serde_json writes non-finite floats as null, which would fail to load
    // back as an f32. Verify before the old save is replaced. This parse is the
    // price of "a save you cannot load is never written"; it allocates one
    // world per save and nothing more.
    if let Err(e) = serde_json::from_slice::<Envelope>(&bytes) {
        return Err(SaveError::Unwritable {
            path: path.to_path_buf(),
            reason: format!("would not load back: {e}"),
        });
    }

    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|source| SaveError::Io {
            path: dir.to_path_buf(),
            action: "create",
            source,
        })?;
    }

    let tmp = temp_path(path);
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(source) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(SaveError::Io {
            path: path.to_path_buf(),
            action: "write",
            source,
        });
    }
    fs::rename(&tmp, path).map_err(|source| {
        let _ = fs::remove_file(&tmp);
        SaveError::Io {
            path: path.to_path_buf(),
            action: "replace",
            source,
        }
    })
}

/// Remove the save. Returns whether a file was actually cleared, so `/reset`
/// and `--reset` can report what they did.
///
/// # Errors
///
/// Returns a `SaveError` if the file exists but cannot be removed.
pub fn clear(path: &Path) -> Result<bool, SaveError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SaveError::Io {
            path: path.to_path_buf(),
            action: "remove",
            source,
        }),
    }
}

/// Record that a discontinuity happened, so the observers of the *new* world
/// can see it did not simply begin.
pub fn mark_reset(world: &mut World, cause: &str) {
    world.push_event(Event {
        turn: world.turn,
        actor: "observer".into(),
        place: Place::Granary,
        kind: EventKind::Divine,
        action: "reset".into(),
        text: cause.to_owned(),
        social: None,
        intensity: None,
        grounded: None,
        divine: true,
    });
}

/// Load the village at `path`, founding a new one at `seed` only when there is
/// no save. `--seed` is deliberately ignored when a village exists: restarting
/// must never replace a living settlement.
///
/// # Errors
///
/// Returns a `SaveError` if an existing save cannot be loaded.
pub fn load_or_found(path: &Path, seed: u64) -> Result<(World, bool), SaveError> {
    Ok(load(path)?.map_or_else(|| (World::founding(seed), false), |world| (world, true)))
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use society_sim::{Agent, Memory, Promise};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Temporary directory that removes itself.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "society-persist-{}-{}-{n}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A world that differs from the founding in every field the observer can
    /// see: turn, day, weather, halt, trust, promises, scars, events, lineage.
    fn lived_in_world(seed: u64) -> World {
        let mut w = World::founding(seed);
        w.turn = 31;
        w.day = 2;
        w.weather = 1.37;
        w.morale_bias = 0.17;
        w.halted = Some("the last villager died".into());
        w.agent_mut("Ada").unwrap().food = 3;
        w.agent_mut("Ada").unwrap().hunger = 0;
        w.agent_mut("Ada")
            .unwrap()
            .adopt_explicit_goal("raise the granary roof");
        w.agent_mut("Ada").unwrap().set_trust("Bo", 0.35);
        w.agent_mut("Bo").unwrap().promise = Some(Promise {
            to: "Ada".into(),
            text: "half my grain".into(),
            made_turn: 20,
            kept: None,
        });
        w.agent_mut("Bo")
            .unwrap()
            .remember(Memory::scar(9, "You watched Cor starve."));
        let names: Vec<String> = w.agents.iter().map(|a| a.name.clone()).collect();
        let mut child = Agent::new(
            "Ash",
            "cautious; anxious",
            Place::Hut,
            2,
            1,
            27,
            1,
            vec!["Ada".into(), "Bo".into()],
            &names,
        );
        child.set_trust("Ada", 3.25);
        w.agents.push(child);
        w.place_state_mut(Place::Granary).food = 11;
        w.place_state_mut(Place::Market).owner = Some("Sable".into());
        w.push_event(Event {
            turn: 30,
            actor: "Tam".into(),
            place: Place::Woods,
            kind: EventKind::Birth,
            action: "birth".into(),
            text: "Ash was born to Ada and Bo.".into(),
            social: None,
            intensity: Some(3.5),
            grounded: None,
            divine: false,
        });
        w
    }

    fn json(w: &World) -> serde_json::Value {
        serde_json::to_value(w).unwrap()
    }

    #[test]
    fn round_trip_preserves_every_observable_field() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        let world = lived_in_world(7);
        save(&world, &path).unwrap();

        let loaded = load(&path).unwrap().expect("a village");
        assert_eq!(json(&loaded), json(&world));
        assert_eq!(loaded.halted.as_deref(), Some("the last villager died"));
        assert_eq!(loaded.agent("Ash").unwrap().parents, vec!["Ada", "Bo"]);
        assert_eq!(loaded.agent("Ash").unwrap().generation, 1);
    }

    #[test]
    fn a_loaded_village_continues_like_the_uninterrupted_one() {
        use society_sim::{apply, Intent, SocialAct};

        let script = |w: &mut World| {
            let intent = Intent::parse("go:market", Place::Granary).unwrap();
            apply(w, "Ada", &intent, "", SocialAct::None);
            let intent = Intent::parse("speak", Place::Market).unwrap();
            apply(w, "Nell", &intent, "the grain is short", SocialAct::Gossip);
            w.settle();
        };

        let dir = TempDir::new();
        let path = dir.join("village.json");
        let mut uninterrupted = World::founding(41);
        // Start just before a day boundary so the settlement below consumes
        // the RNG for weather: equal outcomes then prove the RNG state itself
        // survived, not just the visible fields.
        uninterrupted.turn = 10;
        save(&uninterrupted, &path).unwrap();
        let mut resumed = load(&path).unwrap().unwrap();

        for _ in 0..3 {
            script(&mut uninterrupted);
            script(&mut resumed);
        }
        assert_eq!(resumed.turn, 13, "the script crosses a day boundary");
        assert!(resumed.day > 0);
        assert_eq!(json(&resumed), json(&uninterrupted));
    }

    #[test]
    fn absent_save_yields_a_fresh_founding_at_the_requested_seed() {
        let dir = TempDir::new();
        let path = dir.join("nothing.json");
        assert!(load(&path).unwrap().is_none());
        let (world, resumed) = load_or_found(&path, 99).unwrap();
        assert!(!resumed);
        assert_eq!(world.seed, 99);
        assert_eq!(world.turn, 0);
        assert_eq!(world.living().count(), 9);
    }

    #[test]
    fn an_existing_save_wins_over_the_seed() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        let mut world = World::founding(5);
        world.turn = 17;
        save(&world, &path).unwrap();

        let (loaded, resumed) = load_or_found(&path, 1234).unwrap();
        assert!(resumed);
        assert_eq!(loaded.seed, 5);
        assert_eq!(loaded.turn, 17);
    }

    #[test]
    fn corrupt_save_fails_loudly_and_names_the_path() {
        let dir = TempDir::new();
        let path = dir.join("broken.json");
        fs::write(&path, b"{not a world").unwrap();
        let err = load(&path).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert!(matches!(err, SaveError::Corrupt { .. }));
        // No fallback was written.
        assert_eq!(fs::read(&path).unwrap(), b"{not a world");
    }

    #[test]
    fn wrong_version_fails_loudly_and_names_the_path() {
        let dir = TempDir::new();
        let path = dir.join("future.json");
        fs::write(&path, br#"{"version": 99, "world": {}}"#).unwrap();
        let err = load(&path).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert!(text.contains("99"), "{text}");
        match err {
            SaveError::Version {
                found, expected, ..
            } => {
                assert_eq!(found, 99);
                assert_eq!(expected, SAVE_VERSION);
            },
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn an_unwritable_world_never_replaces_a_good_save() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        let good = lived_in_world(11);
        save(&good, &path).unwrap();
        let before = fs::read(&path).unwrap();

        // NaN cannot survive the JSON round trip: saved as null it would fail
        // to load as an f32, so the save is refused before the target moves.
        let mut broken = good.clone();
        broken.weather = f32::NAN;
        let err = save(&broken, &path).unwrap_err();
        assert!(matches!(err, SaveError::Unwritable { .. }), "{err}");

        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(json(&load(&path).unwrap().unwrap()), json(&good));
        assert!(
            !temp_path(&path).exists(),
            "no partial temp file left behind"
        );
    }

    #[test]
    fn save_creates_the_data_directory() {
        let dir = TempDir::new();
        let path = dir.join("nested/data/village.json");
        save(&World::founding(3), &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn clear_removes_the_save_and_reports_what_it_did() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        assert!(!clear(&path).unwrap(), "nothing to remove yet");
        save(&World::founding(13), &path).unwrap();
        assert!(clear(&path).unwrap());
        assert!(load(&path).unwrap().is_none());
        assert!(!clear(&path).unwrap());
    }

    #[test]
    fn a_reset_is_visible_in_the_new_world() {
        let mut w = World::founding(17);
        let before = w.events.len();
        mark_reset(&mut w, "the observer discarded the village");
        let e = w.events.last().unwrap();
        assert_eq!(e.kind, EventKind::Divine);
        assert!(e.divine);
        assert_eq!(w.events.len(), before + 1);
    }

    #[test]
    fn a_halted_village_reloads_halted() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        let mut w = World::founding(19);
        w.halt("the last villager died");
        save(&w, &path).unwrap();
        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.halted.as_deref(), Some("the last villager died"));
    }

    #[test]
    fn inspect_summarizes_saved_village_metadata() {
        let dir = TempDir::new();
        let path = dir.join("village.json");
        assert!(inspect(&path).unwrap().is_none());

        let w = lived_in_world(23);
        save(&w, &path).unwrap();
        let summary = inspect(&path).unwrap().expect("save summary");
        assert_eq!(summary.seed, 23);
        assert_eq!(summary.turn, 31);
        assert_eq!(summary.day, 2);
        assert_eq!(summary.halted.as_deref(), Some("the last villager died"));
    }
}
