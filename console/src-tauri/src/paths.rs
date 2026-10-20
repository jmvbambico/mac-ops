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

/// The user's automations root. Resolution order, first hit wins:
///
///   1. `$MACHINE_OPS_AUTOMATIONS_ROOT`
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
    let env = std::env::var_os("MACHINE_OPS_AUTOMATIONS_ROOT");
    let config = config_path().ok();
    let home = home_dir();
    resolve_automations_root(env, config.as_deref(), home.as_deref())
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

/// Read `automations_root` from the config file. Returns None for a missing
/// file, unreadable content, invalid JSON, or a non-string/empty value — all of
/// which fall through to the default root instead of failing the run.
fn read_config_root(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(path).ok()?;
    let cfg: ConfigFile = serde_json::from_str(&raw).ok()?;
    let value = cfg.automations_root?;
    if value.trim().is_empty() {
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

/// The installed MacOps tool. Resolution order, first hit wins:
///
///   1. `$MACHINE_OPS_TOOL_ROOT`, when it actually looks like a tool root;
///   2. upward from the running executable (dev builds live under
///      `console/src-tauri/target/...`) and then from the current directory.
///
/// There is deliberately no absolute user-path fallback: this repo is public.
pub fn tool_root() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("MACHINE_OPS_TOOL_ROOT") {
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
    bail!("could not locate the MacOps tool root (set MACHINE_OPS_TOOL_ROOT)")
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
