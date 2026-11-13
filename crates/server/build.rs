use std::hash::{Hash, Hasher};
use std::path::Path;

/// Watch every existing path beneath `path`, so an edit to an existing file is
/// caught by its own entry rather than by the directory's timestamp, which does
/// not move when a file's contents change. The directory entries stay watched so
/// an added or removed file also reruns the script.
fn watch_tree(path: &Path, hash: &mut std::collections::hash_map::DefaultHasher) {
    println!("cargo:rerun-if-changed={}", path.display());
    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let child = entry.path();
            println!("cargo:rerun-if-changed={}", child.display());
            if child.is_dir() {
                pending.push(child);
            } else {
                entries.push(child);
            }
        }
    }
    entries.sort();
    for entry in entries {
        entry.to_string_lossy().hash(hash);
        std::fs::read(&entry).expect("source bytes").hash(hash);
    }
}

fn source(path: &Path, hash: &mut std::collections::hash_map::DefaultHasher) {
    if path.is_dir() {
        watch_tree(path, hash);
    } else {
        println!("cargo:rerun-if-changed={}", path.display());
        path.to_string_lossy().hash(hash);
        std::fs::read(path).expect("source bytes").hash(hash);
    }
}

fn git_path(flag: &str) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--path-format=absolute")
        .arg(flag)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!path.is_empty()).then(|| std::path::PathBuf::from(path))
}

/// Watch what a commit moves. `HEAD` names the checkout's branch, and the
/// `refs` tree holds every ref a commit can rewrite, loose or packed. A linked
/// worktree keeps its own `HEAD` in the git dir while branch refs stay in the
/// common dir, so both are watched. Paths that do not exist are skipped: cargo
/// treats an absent watch path as permanently dirty, and a fresh repository has
/// no `packed-refs` until git packs refs.
fn watch_head() {
    let git_dir = git_path("--absolute-git-dir");
    let common = git_path("--git-common-dir").or_else(|| git_dir.clone());
    let mut watched: Vec<std::path::PathBuf> = Vec::new();
    for dir in [git_dir.as_deref(), common.as_deref()]
        .into_iter()
        .flatten()
    {
        for name in ["HEAD", "refs", "packed-refs"] {
            let path = dir.join(name);
            // Skipping absent paths matters: cargo treats a watch on a missing
            // file as permanently dirty, and fresh repositories have no
            // `packed-refs` until git packs refs.
            if path.exists() {
                watched.push(path);
            }
        }
    }
    watched.sort();
    watched.dedup();
    for path in watched {
        if path.is_dir() {
            let mut ignored = std::collections::hash_map::DefaultHasher::new();
            watch_tree(&path, &mut ignored);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn main() {
    watch_head();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for p in [
        "../../Cargo.toml",
        "../../Cargo.lock",
        "../sim/src",
        "../sim/Cargo.toml",
        "src",
        "Cargo.toml",
        "build.rs",
        "../desktop/src",
        "../desktop/Cargo.toml",
        "../../.cargo/config.toml",
    ] {
        source(Path::new(p), &mut hash);
    }
    if let Ok(compiler) = std::process::Command::new("rustc").arg("-vV").output() {
        compiler.stdout.hash(&mut hash);
    }
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        );
    println!(
        "cargo:rustc-env=SOCIETY_SOURCE_ID={commit}:{:016x}",
        hash.finish()
    );
}
