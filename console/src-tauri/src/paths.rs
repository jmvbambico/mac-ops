//! Root resolution. There are TWO independent roots, and conflating them is a
//! bug (AGENTS.md Principle II):
//!
//!   * the TOOL root — holds `bin/` and `lib/`, the installed MacOps code;
//!   * the AUTOMATIONS root — holds `jobs/`, `logs/` and `state/`, the user's
//!     data. Never write user data into the tool's directory.
//!
//! They may legitimately be the SAME directory (a git checkout used in place,
//! with `jobs/` alongside `bin/`), so every helper takes an explicit root
//! rather than assuming one. Callers must pass the automations root for
//! `jobs_dir`/`logs_dir`/`state_file`/`run_log`/`db_file`, and the tool root
//! for `run_job_path`.
use anyhow::{bail, Context, Result};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// A directory is the tool root when it contains the two things the runner
/// itself needs. Note that `jobs/` is deliberately NOT part of this test: the
/// automations root is a separate thing and may not contain `jobs/` yet.
fn looks_like_tool_root(p: &Path) -> bool {
    p.join("bin/run").is_file() && p.join("lib/common.sh").is_file()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Read `primary`, falling back to `legacy` when primary is unset or empty.
///
/// The new `MOPS_*` name wins when both are set. The legacy `MACHINE_OPS_*`
/// names exist only for compatibility until 0.4.0: the owner's own live
/// automations live in a SEPARATE repository this rename cannot reach, and
/// there are no third-party users. Mirrors `mops_env_value` in lib/common.sh.
pub(crate) fn env_alias(primary: &str, legacy: &str) -> Option<std::ffi::OsString> {
    for key in [primary, legacy] {
        if let Some(value) = std::env::var_os(key) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// The user's automations root. Resolution order, first hit wins:
///
///   1. `$MOPS_AUTOMATIONS_ROOT` (legacy: `$MACHINE_OPS_AUTOMATIONS_ROOT`)
///   2. `automations_root` in `~/.config/mops/config.json`
///   3. `$HOME/.mops`
///
/// A missing config file is normal and silent. A MALFORMED config falls
/// through to the default rather than aborting: a job that quietly never runs
/// is the worst failure mode here (Principle IV, fail safe toward running).
#[allow(dead_code)] // part of the resolver API; the app needs source() too
pub fn automations_root() -> Result<PathBuf> {
    automations_root_with_source().map(|(root, _source)| root)
}

/// Like [`automations_root`], but also reports WHERE the answer came from:
/// exactly one of `"env"`, `"config"` or `"default"`. The console surfaces
/// this so the split is visible in the GUI.
pub fn automations_root_with_source() -> Result<(PathBuf, &'static str)> {
    let primary = std::env::var_os("MOPS_AUTOMATIONS_ROOT");
    let legacy = std::env::var_os("MACHINE_OPS_AUTOMATIONS_ROOT");
    let config = config_path().ok();
    let home = home_dir();
    resolve_automations_root_alias(primary, legacy, config.as_deref(), home.as_deref())
}

/// Read-side alias step, split out so tests can prove precedence without
/// touching the process environment.
fn resolve_automations_root_alias(
    env_primary: Option<std::ffi::OsString>,
    env_legacy: Option<std::ffi::OsString>,
    config_file: Option<&Path>,
    home: Option<&Path>,
) -> Result<(PathBuf, &'static str)> {
    let env = env_primary
        .filter(|value| !value.is_empty())
        .or_else(|| env_legacy.filter(|value| !value.is_empty()));
    resolve_automations_root(env, config_file, home)
}

/// The pure resolution step, split out so tests never have to mutate `HOME`.
fn resolve_automations_root(
    env: Option<std::ffi::OsString>,
    config_file: Option<&Path>,
    home: Option<&Path>,
) -> Result<(PathBuf, &'static str)> {
    if let Some(value) = env {
        if !value.is_empty() {
            return Ok((PathBuf::from(value), "env"));
        }
    }
    if let Some(path) = config_file {
        if let Some(root) = read_config_root(path) {
            return Ok((root, "config"));
        }
    }
    if let Some(home) = home {
        return Ok((home.join(".mops"), "default"));
    }
    bail!("cannot resolve the automations root: HOME is not set and no override is configured")
}

#[derive(serde::Deserialize)]
struct ConfigFile {
    #[serde(default)]
    automations_root: Option<String>,
}

/// Python's `str.strip()` — the reference trim used by lib/common.sh — treats
/// the four ASCII separator controls (U+001C..U+001F) as whitespace, while
/// Rust's `char::is_whitespace` (Unicode White_Space) does not. Both readers
/// MUST store exactly the same path, so mirror Python's set here.
fn is_python_whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{1f}')
}

fn trim_like_bash_config(s: &str) -> &str {
    s.trim_matches(is_python_whitespace)
}

/// Read `automations_root` from the config file. Returns None for a missing
/// file, unreadable content, invalid JSON, or a non-string/empty value — all of
/// which fall through to the default root instead of failing the run.
///
/// The value is trimmed exactly as lib/common.sh's Python reader trims it:
/// the stored path must be byte-identical on both sides, or the GUI and the
/// 02:00 run read different directories (a parser that disagrees with
/// bin/run). Rust's serde typing also rejects non-string values and non-object
/// top levels, matching Bash's `isinstance` guards. Neither side expands `~`
/// or resolves a relative path: both pass the string through unchanged.
fn read_config_root(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cfg: ConfigFile = serde_json::from_str(&raw).ok()?;
    let raw_value = cfg.automations_root?;
    let value = trim_like_bash_config(&raw_value);
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// `~/.config/mops/config.json` — the one place a user configures the
/// automations root without setting an environment variable.
pub fn config_path() -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| {
        anyhow::anyhow!("HOME is not set; cannot locate ~/.config/mops/config.json")
    })?;
    Ok(home.join(".config/mops/config.json"))
}

/// The filesystem identity of an existing path: device + inode. Unlike a
/// canonical path string this is unaffected by case spelling (APFS is
/// case-insensitive but case-preserving), symlinks, or hard links — every
/// spelling that reaches the same directory yields the same pair.
fn file_identity(path: &Path) -> std::io::Result<(u64, u64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// Resolve `requested` to the deepest prefix that exists on disk, following
/// symlinks and `..` exactly as `create_dir_all` will. Returns the canonical
/// directory and whether `requested` itself was that directory (no missing
/// tail left to create).
///
/// Anything that exists but cannot be resolved — a dangling symlink, a symlink
/// loop, a permission error — fails closed: the guard cannot prove the write
/// stays out of the tool tree, so it must not allow it.
fn resolve_deepest(requested: &Path) -> Result<(PathBuf, bool)> {
    let mut candidate = requested.to_path_buf();
    let mut is_requested = true;
    loop {
        match candidate.canonicalize() {
            Ok(resolved) => return Ok((resolved, is_requested)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Distinguish "missing" from "exists as a dangling symlink":
                // the latter is present on disk and must not be walked past.
                match std::fs::symlink_metadata(&candidate) {
                    Ok(_) => {
                        return Err(err)
                            .with_context(|| format!("resolve {}", candidate.display()));
                    }
                    Err(meta_err) if meta_err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(meta_err) => {
                        return Err(meta_err)
                            .with_context(|| format!("inspect {}", candidate.display()));
                    }
                }
            }
            Err(err) => {
                return Err(err).with_context(|| format!("resolve {}", candidate.display()));
            }
        }
        match candidate.parent() {
            Some(parent) => {
                is_requested = false;
                candidate = parent.to_path_buf();
            }
            None => {
                bail!(
                    "cannot resolve any existing ancestor of {}",
                    requested.display()
                );
            }
        }
    }
}

/// True when `requested` is the tool root itself or lies strictly inside it.
///
/// The check runs on the RESOLVED path, not on lexical ancestors: `create_dir_all`
/// follows symlinks, so validating the unresolved spelling lets a symlink from
/// outside into a tool subdirectory escape the guard. `resolve_deepest` returns
/// the canonical existing ancestor; anything the caller then creates lives
/// below it, so only that directory and its real parents matter.
///
/// An existing directory equal to the tool root is the documented in-place
/// checkout and is allowed; a strict ancestor equal to it (or the root with a
/// missing tail) means the target lands inside the tool tree and is rejected.
/// Identity comparison keeps this immune to case spelling, symlinks and hard
/// links. TOCTOU between this check and `ensure_dirs` is accepted: this is a
/// single-user settings write, not a privilege boundary.
fn targets_tool_root(requested: &Path, tool_root: &Path) -> Result<bool> {
    let tool = file_identity(tool_root)
        .with_context(|| format!("inspect the tool root {}", tool_root.display()))?;
    let (resolved, is_requested) = resolve_deepest(requested)?;

    if file_identity(&resolved).with_context(|| format!("inspect {}", resolved.display()))? == tool
    {
        // The tool root itself is allowed; a not-yet-existing path below it is
        // not (it would be created inside the tool tree).
        return Ok(!is_requested);
    }

    let mut ancestor = resolved.parent();
    while let Some(dir) = ancestor {
        if file_identity(dir).with_context(|| format!("inspect {}", dir.display()))? == tool {
            return Ok(true);
        }
        ancestor = dir.parent();
    }
    Ok(false)
}

/// Read the config as a generic object so an unrelated key written by the user
/// (or a future version) survives a save. A missing or corrupt file becomes an
/// empty object: corrupt JSON has no readable keys to preserve, and refusing to
/// save would leave the user unable to repair it from the GUI. Principle IV
/// still applies on the read side — a corrupt file falls through to `~/.mops`.
fn read_config_object(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Write via a temp file in the SAME directory and rename: an interrupted write
/// can leave a stray temp file, never a truncated `config.json`.
///
/// The temp file is created with `create_new` (never truncating or following a
/// symlink at the chosen name) and the parent directory is fsynced after the
/// rename so the new directory entry is durable too — without that, a power
/// loss can lose a config the command reported as saved. That matters here
/// because this file tells the 02:00 run where the user's automations live.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.json");

    // A predictable name is a symlink target for an attacker; `create_new`
    // fails rather than following one. The pid + nanosecond + attempt
    // components make a collision implausible, and AlreadyExists retries.
    let (tmp, mut file) = {
        let mut created = None;
        let mut last_collision = None;
        for attempt in 0..16u32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let candidate = dir.join(format!(
                ".{name}.{}.{nanos}.{attempt}.tmp",
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    created = Some((candidate, file));
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some(err);
                    continue;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("create {}", candidate.display()));
                }
            }
        }
        created.ok_or_else(|| {
            let detail = last_collision
                .map(|err| format!(": {err}"))
                .unwrap_or_default();
            anyhow::anyhow!(
                "temp-file creation collided repeatedly next to {}{detail}",
                path.display()
            )
        })?
    };

    let result = (|| -> Result<()> {
        // Open the directory before the rename: if this fails, the save has not
        // happened yet. The handle is still valid after the rename.
        let dir_file = std::fs::File::open(dir)
            .with_context(|| format!("open directory {}", dir.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("flush {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
        // The rename has already replaced the file. A failed directory sync
        // must not look like "the save failed" — the new value is live, only
        // its durability is unconfirmed — so say exactly that.
        dir_file.sync_all().with_context(|| {
            format!(
                "the config was saved to {} but its directory could not be fsynced, so durability is not confirmed",
                path.display()
            )
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Persist `raw` as the configured automations root, then return the value
/// that was stored (trimmed exactly like both readers).
///
/// Validation, in order:
///   * the value is non-empty after the reader-compatible trim;
///   * it is absolute (neither reader expands `~` or a relative path);
///   * an existing path must be a directory — a missing one is created with
///     `jobs/`, `logs/` and `state/` by `ensure_dirs`.
///
/// The tool-root comparison uses filesystem identity (device + inode), not
/// canonical path strings: APFS is case-insensitive but case-preserving, and a
/// symlink can reach the same tree under a different spelling. An in-place git
/// checkout, where the automations root IS the tool root, is a documented
/// supported layout, so it is accepted. A directory strictly INSIDE the tool
/// root is rejected: that would scatter the user's data through the installed
/// code, which is exactly what Principle II forbids and is not the supported
/// in-place case.
pub fn set_config_root(config_file: &Path, tool_root: &Path, raw: &str) -> Result<PathBuf> {
    let trimmed = trim_like_bash_config(raw);
    if trimmed.is_empty() {
        bail!("the automations root cannot be empty");
    }

    let requested = Path::new(trimmed);
    if !requested.is_absolute() {
        bail!("the automations root must be an absolute path (got {trimmed:?})");
    }
    if requested.exists() && !requested.is_dir() {
        bail!("{} exists but is not a directory", requested.display());
    }

    if targets_tool_root(requested, tool_root)? {
        bail!(
            "{} is inside the MacOps tool directory ({}); choose a directory outside the installed code",
            requested.display(),
            tool_root.display()
        );
    }

    // Creating the user's root is legitimate; creating it here also exercises
    // the same helper the app uses at startup, so a fresh root is complete.
    ensure_dirs(requested)?;

    let mut config = read_config_object(config_file);
    config.insert(
        "automations_root".to_string(),
        serde_json::Value::String(trimmed.to_string()),
    );
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(config))
        .context("serialise the mops config")?
        + "\n";
    write_atomic(config_file, &body)?;

    Ok(PathBuf::from(trimmed))
}

/// The installed MacOps tool. Resolution order, first hit wins:
///
///   1. `$MOPS_TOOL_ROOT` (legacy: `$MACHINE_OPS_TOOL_ROOT`), when it
///      actually looks like a tool root;
///   2. upward from the running executable (dev builds live under
///      `console/src-tauri/target/...`) and then from the current directory.
///
/// There is deliberately no absolute user-path fallback: this repo is public.
pub fn tool_root() -> Result<PathBuf> {
    if let Some(value) = env_alias("MOPS_TOOL_ROOT", "MACHINE_OPS_TOOL_ROOT") {
        let candidate = PathBuf::from(value);
        if looks_like_tool_root(&candidate) {
            return Ok(candidate);
        }
    }
    let mut seeds: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            seeds.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        seeds.push(cwd);
    }
    for seed in seeds {
        let mut cur = seed.as_path();
        loop {
            if looks_like_tool_root(cur) {
                return Ok(cur.to_path_buf());
            }
            match cur.parent() {
                Some(parent) => cur = parent,
                None => break,
            }
        }
    }
    bail!("could not locate the MacOps tool root (set MOPS_TOOL_ROOT)")
}

// ---------------------------------------------------------------------------
// Automations-root joins. Callers pass `automations_root()`.
// ---------------------------------------------------------------------------

pub fn jobs_dir(automations_root: &Path) -> PathBuf {
    automations_root.join("jobs")
}
pub fn logs_dir(automations_root: &Path) -> PathBuf {
    automations_root.join("logs")
}
pub fn state_file(automations_root: &Path) -> PathBuf {
    automations_root.join("state/console.json")
}
pub fn run_log(automations_root: &Path) -> PathBuf {
    automations_root.join("logs/machine-ops.log")
}
/// The SQLite index. Derived data only — never the source of truth.
pub fn db_file(automations_root: &Path) -> PathBuf {
    automations_root.join("state/console.db")
}

// ---------------------------------------------------------------------------
// Tool-root joins. Callers pass `tool_root()`.
// ---------------------------------------------------------------------------

pub fn run_job_path(tool_root: &Path) -> PathBuf {
    tool_root.join("bin/run-job")
}

/// Create `jobs/`, `logs/` and `state/` under the AUTOMATIONS root only.
///
/// This must never create or write anything under the tool root: the tool is
/// installed code, the automations root is the user's data. A fresh
/// `~/.mops` is legitimate and is created here rather than rejected.
pub fn ensure_dirs(automations_root: &Path) -> Result<()> {
    for sub in ["jobs", "logs", "state"] {
        let dir = automations_root.join(sub);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// `current_dir`/env are process-global; this serializes the few tests
    /// that use the environment. No test ever touches the real `HOME`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "machine-ops-paths-test-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_tool_root(root: &Path) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("bin/run"), "#!/usr/bin/env bash\n").unwrap();
        std::fs::write(root.join("lib/common.sh"), "#!/usr/bin/env bash\n").unwrap();
    }

    fn write_config(home: &Path, body: &str) -> PathBuf {
        let cfg = home.join(".config/mops/config.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, body).unwrap();
        cfg
    }

    /// Restores an env var on drop, so a failed assertion cannot leak state.
    struct EnvVar {
        key: &'static str,
        prev: Option<OsString>,
    }
    impl EnvVar {
        fn set(key: &'static str, value: &Path) -> EnvVar {
            let prev = std::env::var_os(key);
            // SAFETY: every user of this holds ENV_LOCK, and no other module
            // reads either of these two variables.
            unsafe { std::env::set_var(key, value) };
            EnvVar { key, prev }
        }

        fn set_str(key: &'static str, value: &str) -> EnvVar {
            let prev = std::env::var_os(key);
            // SAFETY: as above; the caller holds ENV_LOCK.
            unsafe { std::env::set_var(key, value) };
            EnvVar { key, prev }
        }
    }
    impl Drop for EnvVar {
        fn drop(&mut self) {
            // SAFETY: as above; the guard is still held by the caller.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn env_beats_config_beats_default() {
        let base = Tmp::new("precedence");
        let env_root = base.path().join("env-root");
        let config_root = base.path().join("config-root");
        let home = base.path().join("home");
        let cfg = write_config(&home, r#"{"automations_root": "CONFIG_ROOT"}"#
            .replace("CONFIG_ROOT", &config_root.to_string_lossy())
            .as_str());

        let (root, source) = resolve_automations_root(
            Some(env_root.clone().into_os_string()),
            Some(&cfg),
            Some(&home),
        )
        .unwrap();
        assert_eq!(root, env_root);
        assert_eq!(source, "env");

        let (root, source) =
            resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, config_root);
        assert_eq!(source, "config");

        let (root, source) = resolve_automations_root(None, None, Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"));
        assert_eq!(source, "default");
    }

    /// The read-side alias: the new name wins, the legacy name still works,
    /// and neither means the normal config/default chain. If the legacy read
    /// ever disappears, the owner's live jobs/config silently move.
    #[test]
    fn automations_root_alias_prefers_new_then_legacy_then_default() {
        let base = Tmp::new("alias");
        let new_root = base.path().join("new-root");
        let legacy_root = base.path().join("legacy-root");
        let home = base.path().join("home");
        let cfg = write_config(
            &home,
            r#"{"automations_root": "CONFIG_ROOT"}"#
                .replace("CONFIG_ROOT", &base.path().join("config-root").to_string_lossy())
                .as_str(),
        );

        // Both set: MOPS_* wins.
        let (root, source) = resolve_automations_root_alias(
            Some(new_root.clone().into_os_string()),
            Some(legacy_root.clone().into_os_string()),
            Some(&cfg),
            Some(&home),
        )
        .unwrap();
        assert_eq!(root, new_root);
        assert_eq!(source, "env");

        // Legacy alone still resolves.
        let (root, source) = resolve_automations_root_alias(
            None,
            Some(legacy_root.clone().into_os_string()),
            Some(&cfg),
            Some(&home),
        )
        .unwrap();
        assert_eq!(root, legacy_root);
        assert_eq!(source, "env");

        // An empty new name is not an override: legacy wins over config.
        let (root, source) = resolve_automations_root_alias(
            Some(OsString::new()),
            Some(legacy_root.clone().into_os_string()),
            Some(&cfg),
            Some(&home),
        )
        .unwrap();
        assert_eq!(root, legacy_root);
        assert_eq!(source, "env");

        // Neither set: config, then default.
        let (root, source) =
            resolve_automations_root_alias(None, None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, base.path().join("config-root"));
        assert_eq!(source, "config");
        let (root, source) =
            resolve_automations_root_alias(None, None, None, Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"));
        assert_eq!(source, "default");
    }

    #[test]
    fn tool_root_alias_prefers_new_then_legacy() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = Tmp::new("tool-alias");
        let new_root = base.path().join("tool-new");
        let legacy_root = base.path().join("tool-legacy");
        make_tool_root(&new_root);
        make_tool_root(&legacy_root);

        // Both valid: the new name wins.
        let _new = EnvVar::set("MOPS_TOOL_ROOT", &new_root);
        let _legacy = EnvVar::set("MACHINE_OPS_TOOL_ROOT", &legacy_root);
        assert_eq!(tool_root().unwrap(), new_root);

        // Legacy alone still resolves.
        let _new_empty = EnvVar::set_str("MOPS_TOOL_ROOT", "");
        assert_eq!(tool_root().unwrap(), legacy_root);
    }

    #[test]
    fn blank_env_override_is_ignored() {
        let base = Tmp::new("blank-env");
        let home = base.path().join("home");
        let (root, source) =
            resolve_automations_root(Some(OsString::new()), None, Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"));
        assert_eq!(source, "default");
    }

    #[test]
    fn malformed_config_falls_through_to_the_default() {
        let base = Tmp::new("malformed");
        let home = base.path().join("home");
        for junk in ["{ not json", "", "[]", "null", "{\"automations_root\": 42}"] {
            let cfg = write_config(&home, junk);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(
                root,
                home.join(".mops"),
                "config {junk:?} must fall through, not abort"
            );
            assert_eq!(source, "default");
        }
    }

    #[test]
    fn missing_or_empty_config_root_falls_through() {
        let base = Tmp::new("empty-config");
        let home = base.path().join("home");
        for body in ["{}", "{\"automations_root\": null}", "{\"automations_root\": \"\"}"] {
            let cfg = write_config(&home, body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, home.join(".mops"));
            assert_eq!(source, "default");
        }
        // Missing file is the normal case: silent default.
        let missing = home.join(".config/mops/nope.json");
        let (root, _) =
            resolve_automations_root(None, Some(&missing), Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"));
    }

    #[test]
    fn config_tolerates_unknown_fields() {
        let base = Tmp::new("extra-fields");
        let home = base.path().join("home");
        let wanted = base.path().join("auto");
        let cfg = write_config(
            &home,
            &format!(
                "{{\"version\": 9, \"automations_root\": \"{}\", \"other\": true}}",
                wanted.display()
            ),
        );
        let (root, source) =
            resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, wanted);
        assert_eq!(source, "config");
    }

    /// The blocking defect: Rust stored the untrimmed value while
    /// lib/common.sh's Python reader stored `value.strip()`. A trailing space
    /// is a different directory on macOS, so the GUI and the 02:00 run would
    /// disagree. These cases assert the trimmed path is what gets stored.
    #[test]
    fn config_value_is_trimmed_exactly_like_bin_run() {
        let base = Tmp::new("trim");
        let home = base.path().join("home");
        let cases = [
            (r#"{"automations_root": "  /Volumes/MacOps  "}"#, "/Volumes/MacOps"),
            (r#"{"automations_root": "\t/Volumes/My Disk\n"}"#, "/Volumes/My Disk"),
            (r#"{"automations_root": "\r\n/Volumes/First\r\n"}"#, "/Volumes/First"),
            // Python's str.strip() also treats U+001C..U+001F as whitespace;
            // Rust's char::is_whitespace alone does not.
            (r#"{"automations_root": "\u001c/Volumes/Separated\u001f"}"#, "/Volumes/Separated"),
        ];
        for (body, want) in cases {
            let cfg = write_config(&home, body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, PathBuf::from(want), "config {body:?}");
            assert_eq!(source, "config");
        }
    }

    #[test]
    fn whitespace_only_config_value_falls_through() {
        let base = Tmp::new("ws-only");
        let home = base.path().join("home");
        for body in [
            r#"{"automations_root": "   "}"#,
            r#"{"automations_root": "\t\n\r"}"#,
            r#"{"automations_root": "\u001c\u001d\u001e\u001f"}"#,
        ] {
            let cfg = write_config(&home, body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, home.join(".mops"), "config {body:?}");
            assert_eq!(source, "default");
        }
    }

    /// Bash guards with `isinstance(value, str)`; serde's typed Option<String>
    /// rejects every non-string, so both fall through to the default.
    #[test]
    fn non_string_config_value_falls_through() {
        let base = Tmp::new("non-string");
        let home = base.path().join("home");
        for body in [
            r#"{"automations_root": 42}"#,
            r#"{"automations_root": true}"#,
            r#"{"automations_root": null}"#,
            r#"{"automations_root": []}"#,
            r#"{"automations_root": ["/Volumes/X"]}"#,
            r#"{"automations_root": {}}"#,
        ] {
            let cfg = write_config(&home, body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, home.join(".mops"), "config {body:?}");
            assert_eq!(source, "default");
        }
    }

    /// Bash guards with `isinstance(data, dict)`; serde's struct deserializer
    /// rejects a non-object top level, so both fall through to the default.
    #[test]
    fn non_object_config_falls_through() {
        let base = Tmp::new("non-object");
        let home = base.path().join("home");
        for body in ["[]", "[1, 2]", "\"a string\"", "42", "true", "null"] {
            let cfg = write_config(&home, body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, home.join(".mops"), "config {body:?}");
            assert_eq!(source, "default");
        }
    }

    /// Neither reader expands `~` or resolves a relative path; both pass the
    /// string through unchanged. Consistency, not capability.
    #[test]
    fn relative_and_tilde_paths_are_passed_through_by_both_readers() {
        let base = Tmp::new("relative");
        let home = base.path().join("home");
        for want in ["relative/path", "~/macops"] {
            let body = format!(r#"{{"automations_root": "{want}"}}"#);
            let cfg = write_config(&home, &body);
            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, PathBuf::from(want));
            assert_eq!(source, "config");
            assert_ne!(root, home.join("macops"), "neither side expands ~");
        }
    }

    /// Python's json rejects invalid UTF-8 and a leading BOM; Rust's
    /// read_to_string/serde_json must also fall through rather than abort.
    #[test]
    fn invalid_utf8_and_bom_config_fall_through() {
        let base = Tmp::new("encoding");
        let home = base.path().join("home");
        let cfg = home.join(".config/mops/config.json");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();

        std::fs::write(&cfg, [0xff, 0xfe, b'{', b'}']).unwrap();
        let (root, source) =
            resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"), "invalid UTF-8 must fall through");
        assert_eq!(source, "default");

        std::fs::write(
            &cfg,
            "\u{feff}{\"automations_root\": \"/Volumes/Bom\"}",
        )
        .unwrap();
        let (root, source) =
            resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, home.join(".mops"), "a BOM must fall through like Python");
        assert_eq!(source, "default");
    }

    #[test]
    fn ensure_dirs_writes_only_under_the_automations_root() {
        let base = Tmp::new("ensure");
        let tool = base.path().join("tool");
        let auto = base.path().join("auto");
        make_tool_root(&tool);
        std::fs::create_dir_all(&auto).unwrap();
        let tool_before = list_tree(&tool);

        ensure_dirs(&auto).unwrap();

        assert!(auto.join("jobs").is_dir());
        assert!(auto.join("logs").is_dir());
        assert!(auto.join("state").is_dir());
        // THE most important assertion in the split: the tool root is untouched.
        assert_eq!(
            list_tree(&tool),
            tool_before,
            "ensure_dirs must never create anything under the tool root"
        );
        for forbidden in ["jobs", "logs", "state"] {
            assert!(!tool.join(forbidden).exists(), "tool root got {forbidden}/");
        }
    }

    #[test]
    fn tool_root_and_automations_root_resolve_independently() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = Tmp::new("independent");
        let tool = base.path().join("tool");
        let auto = base.path().join("auto");
        make_tool_root(&tool);
        std::fs::create_dir_all(&auto).unwrap();

        let _tool_env = EnvVar::set("MACHINE_OPS_TOOL_ROOT", &tool);
        let _auto_env = EnvVar::set("MACHINE_OPS_AUTOMATIONS_ROOT", &auto);

        let resolved_tool = tool_root().unwrap();
        let (resolved_auto, source) = automations_root_with_source().unwrap();
        assert_eq!(resolved_tool, tool);
        assert_eq!(resolved_auto, auto);
        assert_eq!(source, "env");
        assert_ne!(resolved_tool, resolved_auto);
    }

    #[test]
    fn the_same_directory_can_be_both_roots() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = Tmp::new("same-dir");
        let shared = base.path().join("checkout");
        make_tool_root(&shared);
        std::fs::create_dir_all(shared.join("jobs")).unwrap();
        std::fs::write(
            shared.join("jobs/10-example.sh"),
            "#!/usr/bin/env bash\n# tier: daily\n",
        )
        .unwrap();

        let _tool_env = EnvVar::set("MACHINE_OPS_TOOL_ROOT", &shared);
        let _auto_env = EnvVar::set("MACHINE_OPS_AUTOMATIONS_ROOT", &shared);

        assert_eq!(tool_root().unwrap(), shared);
        assert_eq!(automations_root().unwrap(), shared);

        ensure_dirs(&automations_root().unwrap()).unwrap();
        assert!(shared.join("bin/run").is_file(), "tool files must survive");
        assert!(shared.join("lib/common.sh").is_file());
        assert!(shared.join("jobs/10-example.sh").is_file());
        assert!(shared.join("logs").is_dir());
        assert!(shared.join("state").is_dir());
    }

    #[test]
    fn public_automations_root_honours_the_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = Tmp::new("public-env");
        let auto = base.path().join("somewhere with spaces");
        std::fs::create_dir_all(&auto).unwrap();

        let _auto_env = EnvVar::set("MACHINE_OPS_AUTOMATIONS_ROOT", &auto);
        let (root, source) = automations_root_with_source().unwrap();
        assert_eq!(root, auto);
        assert_eq!(source, "env");
        // It must not require jobs/ to already exist: a fresh root is valid.
        assert!(!root.join("jobs").exists());
    }

    #[test]
    fn tool_root_ignores_an_env_value_that_is_not_a_tool_root() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = Tmp::new("bad-tool-env");
        let bogus = base.path().join("not-a-tool");
        std::fs::create_dir_all(&bogus).unwrap();
        let _tool_env = EnvVar::set("MACHINE_OPS_TOOL_ROOT", &bogus);
        // It falls through to the executable/current-dir search. In this
        // worktree that finds the real repo root, so just assert it is not
        // the bogus value (and that a tool root was found).
        let found = tool_root().unwrap();
        assert_ne!(found, bogus);
        assert!(looks_like_tool_root(&found));
    }

    /// The write-side validator: empty/whitespace, relative and
    /// file-not-directory are rejected before anything is created or saved.
    #[test]
    fn set_config_root_rejects_empty_relative_and_non_directory() {
        let base = Tmp::new("set-invalid");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");
        let file = base.path().join("not-a-dir");
        std::fs::write(&file, "x").unwrap();

        let bad = [
            "",
            "   ",
            "\t\n\r",
            "\u{1c}\u{1d}\u{1e}\u{1f}",
            "relative/path",
            "~/macops",
            file.to_str().unwrap(),
        ];
        for value in bad {
            let err = set_config_root(&cfg, &tool, value)
                .expect_err(&format!("{value:?} must be rejected"))
                .to_string();
            assert!(!err.is_empty(), "{value:?} must explain the rejection");
        }

        // Validation happens before any filesystem work: no config directory,
        // no config file, nothing partially written.
        assert!(!cfg.exists(), "a rejected value must not write config.json");
        assert!(!cfg.parent().unwrap().exists(), "a rejected value must not create ~/.config/mops");
    }

    #[test]
    fn set_config_root_creates_a_fresh_root_with_jobs_logs_state() {
        let base = Tmp::new("set-fresh");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        let auto = base.path().join("fresh root");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");
        assert!(!auto.exists());

        let stored = set_config_root(&cfg, &tool, auto.to_str().unwrap()).unwrap();
        assert_eq!(stored, auto);
        for sub in ["jobs", "logs", "state"] {
            assert!(auto.join(sub).is_dir(), "ensure_dirs must create {sub}/");
        }
        assert!(cfg.is_file(), "the missing ~/.config/mops directory is created");
        let (root, source) = resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, auto);
        assert_eq!(source, "config");
    }

    /// Read-modify-write: unrelated keys written by the user (or a newer
    /// version) must survive. Clobbering them would silently lose settings.
    #[test]
    fn set_config_root_preserves_unrelated_config_keys() {
        let base = Tmp::new("set-preserve");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        let auto = base.path().join("auto");
        make_tool_root(&tool);
        let cfg = write_config(
            &home,
            r#"{"version": 9, "theme": "dark", "nested": {"keep": true}, "automations_root": "/old"}"#,
        );

        set_config_root(&cfg, &tool, auto.to_str().unwrap()).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(saved["version"], 9);
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["nested"]["keep"], true);
        assert_eq!(saved["automations_root"], auto.to_str().unwrap());
    }

    /// The stored string must be byte-identical for both readers. We trim with
    /// the exact reader function, so a second read (which trims again) is a
    /// no-op; the raw on-disk value already has no surrounding whitespace.
    #[test]
    fn set_config_root_value_round_trips_through_the_reader() {
        let base = Tmp::new("set-roundtrip");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        let auto = base.path().join("Volumes with spaces");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");
        let path = auto.to_str().unwrap();

        // Every form Python's str.strip() removes, including U+001C..U+001F
        // that Rust's char::is_whitespace alone does not.
        for wrapped in [
            format!("  {path}  "),
            format!("\t{path}\n"),
            format!("\r\n{path}\r\n"),
            format!("\u{1c}{path}\u{1f}"),
        ] {
            let stored = set_config_root(&cfg, &tool, &wrapped).unwrap();
            assert_eq!(stored, auto);

            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
            let written = saved["automations_root"].as_str().unwrap();
            assert_eq!(written, path, "the trim must happen BEFORE writing");
            assert_eq!(
                trim_like_bash_config(written),
                written,
                "the stored value must be trim-stable for both readers"
            );

            let (root, source) =
                resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
            assert_eq!(root, auto);
            assert_eq!(source, "config");
        }
    }

    /// A git checkout used in place is documented as supported: jobs/ beside
    /// bin/. Pointing the automations root at the tool root must work.
    #[test]
    fn set_config_root_allows_the_tool_root_itself() {
        let base = Tmp::new("set-same-dir");
        let home = base.path().join("home");
        let shared = base.path().join("checkout");
        make_tool_root(&shared);
        let cfg = home.join(".config/mops/config.json");

        set_config_root(&cfg, &shared, shared.to_str().unwrap()).unwrap();

        for sub in ["jobs", "logs", "state"] {
            assert!(shared.join(sub).is_dir(), "in-place checkout gets {sub}/");
        }
        assert!(shared.join("bin/run").is_file(), "tool files must survive");
        assert!(shared.join("lib/common.sh").is_file());
        let (root, source) = resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, shared);
        assert_eq!(source, "config");
    }

    /// A directory strictly inside the tool root is NOT the supported in-place
    /// layout; it scatters user data through installed code (Principle II).
    /// The rejection happens before anything is created.
    #[test]
    fn set_config_root_rejects_a_directory_inside_the_tool_root() {
        let base = Tmp::new("set-inside-tool");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");
        let inside = tool.join("user-data");

        let err = set_config_root(&cfg, &tool, inside.to_str().unwrap())
            .expect_err("a descendant of the tool root must be rejected")
            .to_string();
        assert!(err.contains("inside the MacOps tool directory"), "{err}");
        assert!(!inside.exists(), "nothing may be created under the tool root");
        assert!(!cfg.exists());
    }

    /// Regression: macOS APFS is case-insensitive but case-preserving, so
    /// `<tool>` and `<TOOL>` are the SAME directory, while `realpath` keeps the
    /// spelling it was given and `Path::starts_with` compares case-sensitively.
    /// The old string guard let this through and wrote jobs/logs/state into the
    /// installed tool root.
    ///
    /// On a case-sensitive volume the differently-cased path is genuinely a
    /// different directory, so there is nothing to assert; skip rather than
    /// pass for the wrong reason.
    #[test]
    fn set_config_root_rejects_inside_the_tool_root_through_a_different_case() {
        let base = Tmp::new("set-case-tool");
        let home = base.path().join("home");
        let tool = base.path().join("mops");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");

        let differently_cased = base.path().join("MOPS");
        if file_identity(&differently_cased).ok() != file_identity(&tool).ok() {
            eprintln!(
                "skipping: {} is case-sensitive, so MOPS is a different directory",
                base.path().display()
            );
            return;
        }

        let inside = differently_cased.join("user-data");
        let err = set_config_root(&cfg, &tool, inside.to_str().unwrap())
            .expect_err("a differently-cased spelling of the tool root must be rejected")
            .to_string();
        assert!(err.contains("inside the MacOps tool directory"), "{err}");
        assert!(
            !tool.join("user-data").exists(),
            "nothing may be created under the tool root"
        );
        assert!(!cfg.exists());
    }

    /// A symlink reaches the same inode under an unrelated string prefix;
    /// identity catches that even though the spelling is completely different.
    #[test]
    fn set_config_root_rejects_inside_the_tool_root_through_a_symlink() {
        let base = Tmp::new("set-symlink-tool");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        let link = base.path().join("tool-link");
        std::os::unix::fs::symlink(&tool, &link).unwrap();
        let cfg = home.join(".config/mops/config.json");

        let inside = link.join("user-data");
        let err = set_config_root(&cfg, &tool, inside.to_str().unwrap())
            .expect_err("a symlinked spelling of the tool root must be rejected")
            .to_string();
        assert!(err.contains("inside the MacOps tool directory"), "{err}");
        assert!(
            !tool.join("user-data").exists(),
            "nothing may be created under the tool root"
        );
        assert!(!cfg.exists());
    }

    /// Regression: a symlink from OUTSIDE the tool tree into a tool
    /// SUBDIRECTORY. The old walk compared lexical ancestors; the alias
    /// resolves to `<tool>/jobs`, whose identity is not the tool root, so the
    /// walk escaped to `/` and `ensure_dirs` wrote inside the installed tool.
    #[test]
    fn set_config_root_rejects_a_symlink_to_a_tool_subdirectory() {
        let base = Tmp::new("set-symlink-child");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        std::fs::create_dir_all(tool.join("jobs")).unwrap();

        let outside = base.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let alias = outside.join("alias");
        std::os::unix::fs::symlink(tool.join("jobs"), &alias).unwrap();
        let cfg = home.join(".config/mops/config.json");

        let requested = alias.join("new-root");
        let err = set_config_root(&cfg, &tool, requested.to_str().unwrap())
            .expect_err("a symlink into a tool subdirectory must be rejected")
            .to_string();
        assert!(err.contains("inside the MacOps tool directory"), "{err}");
        assert!(
            !tool.join("jobs/new-root").exists(),
            "nothing may be created under the tool root"
        );
        assert!(!cfg.exists());
    }

    /// Reaching the tool root itself through a symlink, with no tail to create,
    /// is still the documented in-place checkout and must be allowed.
    #[test]
    fn set_config_root_allows_the_tool_root_reached_through_a_symlink() {
        let base = Tmp::new("set-symlink-root-ok");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        let link = base.path().join("tool-link");
        std::os::unix::fs::symlink(&tool, &link).unwrap();
        let cfg = home.join(".config/mops/config.json");

        set_config_root(&cfg, &tool, link.to_str().unwrap()).unwrap();

        for sub in ["jobs", "logs", "state"] {
            assert!(tool.join(sub).is_dir(), "in-place checkout gets {sub}/");
        }
        assert!(tool.join("bin/run").is_file(), "tool files must survive");
        assert!(cfg.is_file(), "the setting is saved");
    }

    /// A dangling symlink exists on disk but cannot be resolved. The guard
    /// cannot know where `create_dir_all` would follow it, so it must fail
    /// closed rather than assume the path is outside the tool tree.
    #[test]
    fn set_config_root_rejects_an_unresolvable_symlink_in_the_path() {
        let base = Tmp::new("set-symlink-dangling");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);

        let outside = base.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let dangling = outside.join("dangling");
        std::os::unix::fs::symlink(tool.join("missing"), &dangling).unwrap();
        let cfg = home.join(".config/mops/config.json");

        let requested = dangling.join("new-root");
        let err = set_config_root(&cfg, &tool, requested.to_str().unwrap())
            .expect_err("an unresolvable symlink must fail closed")
            .to_string();
        assert!(err.contains("resolve"), "{err}");
        assert!(!tool.join("missing").exists());
        assert!(!cfg.exists());
    }

    /// `..` must not be usable to hide the tool root. Here the final target is
    /// outside the tool, but creating it would first create a directory inside
    /// the tool; reject before anything exists.
    #[test]
    fn set_config_root_rejects_a_dotdot_path_through_the_tool_root() {
        let base = Tmp::new("set-dotdot-tool");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        make_tool_root(&tool);
        let cfg = home.join(".config/mops/config.json");

        let requested = tool.join("missing/../../outside");
        let err = set_config_root(&cfg, &tool, requested.to_str().unwrap())
            .expect_err("a .. traversal through the tool root must be rejected")
            .to_string();
        assert!(err.contains("inside the MacOps tool directory"), "{err}");
        assert!(
            !tool.join("missing").exists(),
            "nothing may be created under the tool root"
        );
        assert!(!base.path().join("outside").exists());
        assert!(!cfg.exists());
    }

    /// A corrupt config has no readable keys to preserve; saving must still
    /// repair it into valid JSON with the new value.
    #[test]
    fn set_config_root_repairs_a_corrupt_config() {
        let base = Tmp::new("set-corrupt");
        let home = base.path().join("home");
        let tool = base.path().join("tool");
        let auto = base.path().join("auto");
        make_tool_root(&tool);
        let cfg = write_config(&home, "{ not json");

        set_config_root(&cfg, &tool, auto.to_str().unwrap()).unwrap();

        let (root, source) = resolve_automations_root(None, Some(&cfg), Some(&home)).unwrap();
        assert_eq!(root, auto);
        assert_eq!(source, "config");
    }

    fn list_tree(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path.clone());
                    }
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }
}
