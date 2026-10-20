//! Repo-root resolution. Every other module takes `&Path` to the repo root,
//! so this is the only place that knows how the app finds machine-ops.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A directory is the machine-ops root when it has both `jobs/` and
/// `lib/common.sh` — the two things every job depends on.
fn looks_like_root(p: &Path) -> bool {
    p.join("jobs").is_dir() && p.join("lib/common.sh").is_file()
}

/// Resolution order: explicit override, then upward from the executable
/// (dev builds live in console/src-tauri/target/...), then the current dir,
/// then the conventional location.
pub fn repo_root() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("MACHINE_OPS_ROOT") {
        let p = PathBuf::from(p);
        if looks_like_root(&p) { return Ok(p); }
    }
    let mut seeds: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() { seeds.push(exe); }
    if let Ok(cwd) = std::env::current_dir() { seeds.push(cwd); }
    for seed in seeds {
        let mut cur = seed.as_path();
        while let Some(parent) = cur.parent() {
            if looks_like_root(parent) { return Ok(parent.to_path_buf()); }
            cur = parent;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("projects/machine-ops");
        if looks_like_root(&p) { return Ok(p); }
    }
    anyhow::bail!("could not locate the machine-ops repo (set MACHINE_OPS_ROOT)")
}

pub fn jobs_dir(root: &Path) -> PathBuf { root.join("jobs") }
pub fn logs_dir(root: &Path) -> PathBuf { root.join("logs") }
pub fn state_file(root: &Path) -> PathBuf { root.join("state/console.json") }
pub fn run_log(root: &Path) -> PathBuf { root.join("logs/machine-ops.log") }
/// The SQLite index. Derived data only — never the source of truth.
pub fn db_file(root: &Path) -> PathBuf { root.join("state/console.db") }

pub fn ensure_dirs(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root.join("state")).context("create state/")?;
    std::fs::create_dir_all(logs_dir(root)).context("create logs/")?;
    Ok(())
}
