//! Desired state: `state/console.json`. Deliberately NOT in SQLite — it is
//! small, read by bin/run at 02:00 with system python3, and is the one file
//! a human may want to inspect or hand-edit when something is wrong.
//!
//! Compatibility is load-bearing: bin/run already reads this file and skips
//! jobs whose entry says `"enabled": false`. A missing file, a missing entry,
//! or an unparsable file all mean ENABLED (fail safe toward running).
use crate::types::Schedule;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Fail-safe default for a missing or unparsable `enabled` field: a job the
/// state file cannot describe must still run.
fn default_enabled() -> bool {
    true
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobState {
    /// `#[serde(default)]` is deliberate: an entry that omits (or garbles)
    /// `enabled` is read as enabled, exactly like bin/run, which only ever
    /// skips on a literal `"enabled": false`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub schedule: Schedule,
    /// Display name and grouping are console-only concerns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Default for JobState {
    fn default() -> Self {
        JobState {
            enabled: true,
            schedule: Schedule::default(),
            title: None,
            group: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub jobs: BTreeMap<String, JobState>,
}

impl Default for ConsoleState {
    fn default() -> Self {
        ConsoleState { version: 1, jobs: BTreeMap::new() }
    }
}

/// The one place this module names the file; `paths` owns the layout.
fn state_path(root: &Path) -> PathBuf {
    crate::paths::state_file(root)
}

/// Never fails: an unreadable or malformed file yields the default (which
/// means "everything enabled"), matching bin/run's fail-safe.
pub fn load(root: &Path) -> ConsoleState {
    let path = state_path(root);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ConsoleState::default();
    };
    // Strict parse first. It tolerates missing per-entry fields (serde
    // defaults above) and ignores unknown ones, so forward- and
    // backward-compatible files survive.
    if let Ok(st) = serde_json::from_str::<ConsoleState>(&raw) {
        return st;
    }
    // Lenient salvage: one bad entry must not discard the rest of the file.
    // Anything we cannot understand is simply left out, which means enabled.
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(map)) => {
            let version = map
                .get("version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            let mut jobs = BTreeMap::new();
            if let Some(serde_json::Value::Object(entries)) = map.get("jobs") {
                for (name, entry) in entries {
                    if let Ok(js) = serde_json::from_value::<JobState>(entry.clone()) {
                        jobs.insert(name.clone(), js);
                    } else if entry.get("enabled") == Some(&serde_json::Value::Bool(false)) {
                        // Preserve the one fact bin/run acts on, even when the
                        // rest of the entry is garbage.
                        jobs.insert(name.clone(), JobState { enabled: false, ..Default::default() });
                    }
                }
            }
            ConsoleState { version, jobs }
        }
        _ => ConsoleState::default(),
    }
}

/// Atomic write: serialize to `state/console.json.tmp`, fsync, then rename.
/// A half-written state file read by bin/run at 02:00 would be a silent
/// maintenance outage.
pub fn save(root: &Path, st: &ConsoleState) -> Result<()> {
    let path = state_path(root);
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

    let mut body = serde_json::to_string_pretty(st).context("serialize console state")?;
    body.push('\n');

    let tmp = path.with_extension("json.tmp");
    {
        let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(body.as_bytes()).with_context(|| format!("write {}", tmp.display()))?;
        f.flush().with_context(|| format!("flush {}", tmp.display()))?;
        // fsync BEFORE the rename: a rename that lands before the data does
        // leaves an empty file where bin/run expects state.
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    // Best-effort directory fsync so the rename itself is durable.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Schedule;

    /// Per-test scratch root under the system temp dir (no external crates).
    fn tmp_root(tag: &str) -> PathBuf {
        let uniq = format!(
            "machine-ops-state-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let p = std::env::temp_dir().join(uniq);
        std::fs::create_dir_all(p.join("state")).unwrap();
        p
    }

    #[test]
    fn round_trip_preserves_entries() {
        let root = tmp_root("rt");
        let mut st = ConsoleState::default();
        st.jobs.insert(
            "30-brew-update.sh".into(),
            JobState {
                enabled: false,
                schedule: Schedule::Daily { times: vec!["02:30".into()] },
                title: Some("Brew update".into()),
                group: Some("Packages".into()),
            },
        );
        st.jobs.insert(
            "10-mole-optimize.sh".into(),
            JobState {
                enabled: true,
                schedule: Schedule::Weekly {
                    days: vec!["sun".into()],
                    times: vec!["03:00".into()],
                },
                title: None,
                group: None,
            },
        );
        save(&root, &st).unwrap();

        let back = load(&root);
        assert_eq!(back.version, 1);
        assert_eq!(back.jobs.len(), 2);
        let brew = &back.jobs["30-brew-update.sh"];
        assert!(!brew.enabled);
        assert_eq!(brew.schedule, Schedule::Daily { times: vec!["02:30".into()] });
        assert_eq!(brew.title.as_deref(), Some("Brew update"));
        assert_eq!(brew.group.as_deref(), Some("Packages"));
        assert!(back.jobs["10-mole-optimize.sh"].enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_stays_compatible_with_bin_run() {
        // bin/run only acts on a literal `"enabled": false` under "jobs".
        let root = tmp_root("compat");
        let mut st = ConsoleState::default();
        st.jobs.insert(
            "40-skills-update.sh".into(),
            JobState { enabled: false, ..Default::default() },
        );
        save(&root, &st).unwrap();
        let raw = std::fs::read_to_string(root.join("state/console.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["jobs"]["40-skills-update.sh"]["enabled"], false);
        assert_eq!(v["jobs"]["40-skills-update.sh"]["schedule"]["mode"], "daily");
        // Omitted options must not appear as nulls.
        assert!(v["jobs"]["40-skills-update.sh"].get("title").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_file_means_everything_enabled() {
        let root = tmp_root("missing");
        let st = load(&root);
        assert!(st.jobs.is_empty(), "no entry means enabled");
        assert_eq!(st.version, 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn malformed_file_means_everything_enabled() {
        let root = tmp_root("malformed");
        for junk in [
            "{ not json at all",
            "",
            "[]",
            "null",
            "\0\u{1}binary garbage",
        ] {
            std::fs::write(root.join("state/console.json"), junk).unwrap();
            let st = load(&root);
            assert!(
                st.jobs.is_empty(),
                "malformed state ({junk:?}) must yield the default (all enabled)"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn partial_entries_default_to_enabled() {
        let root = tmp_root("partial");
        std::fs::write(
            root.join("state/console.json"),
            r#"{"jobs":{"10-a.sh":{},"20-b.sh":{"enabled":false},"30-c.sh":{"schedule":{"mode":"manual"}}}}"#,
        )
        .unwrap();
        let st = load(&root);
        assert!(st.jobs["10-a.sh"].enabled);
        assert!(!st.jobs["20-b.sh"].enabled);
        assert!(st.jobs["30-c.sh"].enabled);
        assert_eq!(st.jobs["30-c.sh"].schedule, Schedule::Manual);
        assert_eq!(st.jobs["10-a.sh"].schedule, Schedule::default());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn one_bad_entry_does_not_lose_the_rest() {
        let root = tmp_root("salvage");
        std::fs::write(
            root.join("state/console.json"),
            r#"{"version":1,"jobs":{
                 "10-a.sh":{"enabled":true,"schedule":{"mode":"daily","times":["02:00"]}},
                 "20-bad.sh":{"enabled":false,"schedule":{"mode":"nonsense"}},
                 "30-c.sh":"not even an object"
               }}"#,
        )
        .unwrap();
        let st = load(&root);
        assert!(st.jobs["10-a.sh"].enabled);
        // The disabling fact survives even though the schedule did not.
        assert!(!st.jobs["20-bad.sh"].enabled);
        // Unreadable entries are dropped, which means enabled.
        assert!(!st.jobs.contains_key("30-c.sh"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp() {
        let root = tmp_root("atomic");
        // Pre-existing good content must survive a later save intact.
        let mut st = ConsoleState::default();
        st.jobs.insert("10-a.sh".into(), JobState::default());
        save(&root, &st).unwrap();
        st.jobs.insert("20-b.sh".into(), JobState { enabled: false, ..Default::default() });
        save(&root, &st).unwrap();

        assert!(!root.join("state/console.json.tmp").exists(), "tmp file must be renamed away");
        let raw = std::fs::read_to_string(root.join("state/console.json")).unwrap();
        // Whatever a reader sees is always a complete JSON document.
        let _: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(raw.ends_with('\n'));
        assert_eq!(load(&root).jobs.len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_creates_the_state_dir() {
        let root = tmp_root("mkdir");
        std::fs::remove_dir_all(root.join("state")).unwrap();
        save(&root, &ConsoleState::default()).unwrap();
        assert!(root.join("state/console.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }
}
