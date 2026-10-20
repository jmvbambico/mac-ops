//! One user LaunchAgent per enabled automation, each invoking bin/run-job.
//! NEVER a LaunchDaemon, never root: mole refuses to run as root and no job
//! may handle a password.
//!
//! Everything here stays inside the `gui/<uid>` domain and
//! `~/Library/LaunchAgents`. There is no `sudo` and no `system/` domain
//! anywhere in this module, by design.
use crate::types::Schedule;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Label prefix for the per-job agents this module owns (product name:
/// MacOps). Deliberately NOT the prefix of the pre-existing tier agent
/// `ai.machineops.daily`, which is live and managed by install.sh — nothing
/// here ever touches it.
const LABEL_PREFIX: &str = "ai.macops";

/// launchd starts jobs with a minimal PATH; the plist pins a full one, and
/// bin/run-job pins the same defensively.
const JOB_PATH: &str = "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Set to "1" to make `install`/`uninstall`/`kickstart` render and write
/// nothing to launchctl. Internal escape hatch so the plist writer can be
/// exercised without mutating a live system.
const DRY_RUN_ENV: &str = "MACHINE_OPS_CONSOLE_LAUNCHCTL_DRY_RUN";

fn dry_run() -> bool {
    std::env::var(DRY_RUN_ENV).map(|v| v == "1").unwrap_or(false)
}

/// "30-brew-update.sh" -> "brew-update". The number prefix is run order for
/// the tier dispatcher and is not part of a job's identity.
fn slug_for(job_file: &str) -> String {
    // Tolerate a path being passed where a bare filename is expected.
    let base = job_file.rsplit('/').next().unwrap_or(job_file);
    let stem = base.strip_suffix(".sh").unwrap_or(base);
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.trim_start_matches(['-', '_']);
    let rest = if rest.is_empty() { stem } else { rest };
    let cleaned: String = rest
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_ascii_lowercase();
    if cleaned.is_empty() { "job".to_string() } else { cleaned }
}

/// "30-brew-update.sh" -> "ai.macops.brew-update"
pub fn label_for(job_file: &str) -> String {
    format!("{LABEL_PREFIX}.{}", slug_for(job_file))
}

fn agents_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; cannot locate ~/Library/LaunchAgents"))?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents"))
}

fn plist_path(job_file: &str) -> Result<PathBuf> {
    Ok(agents_dir()?.join(format!("{}.plist", label_for(job_file))))
}

/// The numeric uid, for the `gui/<uid>` domain target. Never 0: a root
/// console would mean a root agent, which this project forbids.
fn uid() -> Result<u32> {
    let out = Command::new("/usr/bin/id")
        .arg("-u")
        .stdin(Stdio::null())
        .output()
        .context("run /usr/bin/id -u")?;
    if !out.status.success() {
        anyhow::bail!("/usr/bin/id -u failed");
    }
    let uid: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .context("parse uid")?;
    if uid == 0 {
        anyhow::bail!(
            "refusing to manage LaunchAgents as root: machine-ops jobs must run as a user \
             (mole refuses to run as root and no job may handle a password)"
        );
    }
    Ok(uid)
}

fn domain_target(job_file: &str) -> Result<String> {
    Ok(format!("gui/{}/{}", uid()?, label_for(job_file)))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// launchd Weekday: 0 (and 7) are Sunday. Accepts names, three-letter
/// abbreviations, and bare numbers.
fn weekday_num(day: &str) -> Option<u32> {
    let d = day.trim().to_ascii_lowercase();
    if let Ok(n) = d.parse::<u32>() {
        return if n <= 7 { Some(n % 7) } else { None };
    }
    let n = match d.as_str() {
        "sun" | "sunday" => 0,
        "mon" | "monday" => 1,
        "tue" | "tues" | "tuesday" => 2,
        "wed" | "weds" | "wednesday" => 3,
        "thu" | "thur" | "thurs" | "thursday" => 4,
        "fri" | "friday" => 5,
        "sat" | "saturday" => 6,
        _ => return None,
    };
    Some(n)
}

/// Valid "HH:MM" entries as (hour, minute). Falls back to 02:00 when nothing
/// parses: an enabled job with an unusable time list must still get a
/// schedule rather than silently never running.
fn times_or_default(times: &[String]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = times
        .iter()
        .filter_map(|t| crate::types::parse_hhmm(t).map(|m| (m / 60, m % 60)))
        .collect();
    out.dedup();
    if out.is_empty() { vec![(2, 0)] } else { out }
}

fn cal_dict(indent: &str, weekday: Option<u32>, hour: u32, minute: u32) -> String {
    let mut s = format!("{indent}<dict>\n");
    if let Some(w) = weekday {
        s.push_str(&format!("{indent}    <key>Weekday</key>\n{indent}    <integer>{w}</integer>\n"));
    }
    s.push_str(&format!("{indent}    <key>Hour</key>\n{indent}    <integer>{hour}</integer>\n"));
    s.push_str(&format!("{indent}    <key>Minute</key>\n{indent}    <integer>{minute}</integer>\n"));
    s.push_str(&format!("{indent}</dict>\n"));
    s
}

/// The start key (or nothing at all, for Manual).
fn start_key(sched: &Schedule) -> String {
    match sched {
        // Manual is never scheduled by launchd: the agent exists only so the
        // console can kickstart it on demand. NO start key, deliberately.
        Schedule::Manual => String::new(),
        Schedule::Interval { every_hours } => {
            let secs = (*every_hours).max(1) as u64 * 3600;
            format!("    <key>StartInterval</key>\n    <integer>{secs}</integer>\n")
        }
        Schedule::Daily { times } => {
            let ts = times_or_default(times);
            let mut s = String::from("    <key>StartCalendarInterval</key>\n");
            if ts.len() == 1 {
                // A single time is a plain dict, like the tier agent's plist.
                s.push_str(&cal_dict("    ", None, ts[0].0, ts[0].1));
            } else {
                s.push_str("    <array>\n");
                for (h, m) in ts {
                    s.push_str(&cal_dict("        ", None, h, m));
                }
                s.push_str("    </array>\n");
            }
            s
        }
        Schedule::Weekly { days, times } => {
            let ts = times_or_default(times);
            let mut wds: Vec<u32> = days.iter().filter_map(|d| weekday_num(d)).collect();
            wds.dedup();
            let mut s = String::from("    <key>StartCalendarInterval</key>\n    <array>\n");
            if wds.is_empty() {
                // No parsable day: schedule every day rather than never.
                for (h, m) in &ts {
                    s.push_str(&cal_dict("        ", None, *h, *m));
                }
            } else {
                for w in wds {
                    for (h, m) in &ts {
                        s.push_str(&cal_dict("        ", Some(w), *h, *m));
                    }
                }
            }
            s.push_str("    </array>\n");
            s
        }
    }
}

/// Render the plist. Must set: ProgramArguments
/// ["/bin/bash", "<root>/bin/run-job", "<root>/jobs/<file>"],
/// WorkingDirectory, RunAtLoad=false, no KeepAlive, an explicit PATH
/// including /opt/homebrew/bin (launchd's PATH is minimal), HOME, and
/// StandardOut/ErrorPath at logs/job-<slug>.out.log / .err.log.
///
/// Schedule mapping: Daily -> StartCalendarInterval dict (or an array when
/// there are several times); Weekly -> array with Weekday; Interval ->
/// StartInterval seconds; Manual -> no start key at all (never scheduled).
pub fn plist_xml(root: &Path, job_file: &str, sched: &Schedule) -> Result<String> {
    let root_s = root
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo root is not valid UTF-8: {}", root.display()))?;
    // The plist names <root>/jobs/<file>, so the filename must be a plain
    // basename: no traversal, no absolute path, nothing to escape jobs/.
    let base = job_file.rsplit('/').next().unwrap_or(job_file);
    if base.is_empty() || job_file.contains("..") || job_file.starts_with('/') {
        anyhow::bail!("suspicious job filename: {job_file}");
    }
    let slug = slug_for(base);
    let label = label_for(base);
    let home = std::env::var("HOME").unwrap_or_default();

    let root_x = xml_escape(root_s);
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{root_x}/bin/run-job</string>
        <string>{root_x}/jobs/{file}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{root_x}</string>
{start}    <key>RunAtLoad</key>
    <false/>
    <key>StandardOutPath</key>
    <string>{root_x}/logs/job-{slug}.out.log</string>
    <key>StandardErrorPath</key>
    <string>{root_x}/logs/job-{slug}.err.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path}</string>
        <key>HOME</key>
        <string>{home}</string>
    </dict>
</dict>
</plist>
"#,
        label = xml_escape(&label),
        file = xml_escape(base),
        start = start_key(sched),
        slug = xml_escape(&slug),
        path = JOB_PATH,
        home = xml_escape(&home),
    );
    Ok(xml)
}

/// Run launchctl, returning (success, combined stderr). Never inherits stdin.
fn launchctl(args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new("/bin/launchctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run launchctl {}", args.join(" ")))?;
    let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    Ok((out.status.success(), msg))
}

/// "nothing to bootout" is the normal case on a first install, and the only
/// failure `install`/`uninstall` may ignore.
fn is_not_loaded(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("no such process")
        || m.contains("not loaded")
        || m.contains("could not find specified service")
        || m.contains("no such file or directory")
}

/// Write the plist to ~/Library/LaunchAgents and bootstrap it. Idempotent:
/// bootout any previous copy first, ignoring "not loaded" errors.
pub fn install(root: &Path, job_file: &str, sched: &Schedule) -> Result<()> {
    let xml = plist_xml(root, job_file, sched)?;
    let dest = plist_path(job_file)?;
    let dir = dest.parent().expect("plist path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    // launchd refuses to start a job whose StandardOutPath directory is
    // missing, so make sure logs/ exists before bootstrapping.
    std::fs::create_dir_all(root.join("logs"))
        .with_context(|| format!("create {}", root.join("logs").display()))?;

    // Write the plist atomically too: launchd may read it the instant we
    // bootstrap, and a truncated plist is a job that never runs.
    let tmp = dest.with_extension("plist.tmp");
    std::fs::write(&tmp, xml.as_bytes()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
    }

    if dry_run() {
        return Ok(());
    }
    let target = domain_target(job_file)?;
    // Unload any previous copy; failing because nothing was loaded is fine.
    let (ok, msg) = launchctl(&["bootout", &target])?;
    if !ok && !is_not_loaded(&msg) {
        anyhow::bail!("launchctl bootout {target} failed: {msg}");
    }
    let dest_s = dest.to_string_lossy().to_string();
    let (ok, msg) = launchctl(&["bootstrap", &format!("gui/{}", uid()?), &dest_s])?;
    if !ok {
        anyhow::bail!("launchctl bootstrap {dest_s} failed: {msg}");
    }
    Ok(())
}

/// bootout + remove the plist. Must not error when nothing is loaded.
pub fn uninstall(job_file: &str) -> Result<()> {
    if !dry_run() {
        let target = domain_target(job_file)?;
        let (ok, msg) = launchctl(&["bootout", &target])?;
        if !ok && !is_not_loaded(&msg) {
            anyhow::bail!("launchctl bootout {target} failed: {msg}");
        }
    }
    let dest = plist_path(job_file)?;
    match std::fs::remove_file(&dest) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove {}", dest.display())),
    }
}

/// `launchctl kickstart -k gui/<uid>/<label>` — run now, via launchd.
#[allow(dead_code)] // part of the module's contract; the UI wires it up later
pub fn kickstart(job_file: &str) -> Result<()> {
    if dry_run() {
        return Ok(());
    }
    let target = domain_target(job_file)?;
    let (ok, msg) = launchctl(&["kickstart", "-k", &target])?;
    if !ok {
        anyhow::bail!("launchctl kickstart -k {target} failed: {msg}");
    }
    Ok(())
}

#[allow(dead_code)] // part of the module's contract; the UI wires it up later
pub fn is_loaded(job_file: &str) -> bool {
    if dry_run() {
        return false;
    }
    let Ok(target) = domain_target(job_file) else { return false };
    matches!(launchctl(&["print", &target]), Ok((true, _)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/someone/projects/machine-ops";

    fn xml(sched: &Schedule) -> String {
        plist_xml(Path::new(ROOT), "30-brew-update.sh", sched).unwrap()
    }

    #[test]
    fn label_for_maps_filenames_to_labels() {
        assert_eq!(label_for("30-brew-update.sh"), "ai.macops.brew-update");
        assert_eq!(label_for("10-mole-optimize.sh"), "ai.macops.mole-optimize");
        assert_eq!(label_for("40-skills-update.sh"), "ai.macops.skills-update");
        // No number prefix, no .sh, a path, odd characters, upper case.
        assert_eq!(label_for("backup.sh"), "ai.macops.backup");
        assert_eq!(label_for("jobs/30-brew-update.sh"), "ai.macops.brew-update");
        assert_eq!(label_for("50-My Thing.sh"), "ai.macops.my-thing");
        assert_eq!(label_for("99.sh"), "ai.macops.99");
        // The live tier agent's label is install.sh's, never ours: even a
        // job file named "daily.sh" cannot collide with ai.machineops.daily.
        assert_eq!(label_for("daily.sh"), "ai.macops.daily");
    }

    #[test]
    fn common_keys_are_present_and_safe() {
        let x = xml(&Schedule::default());
        assert!(x.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(x.contains("<key>Label</key>\n    <string>ai.macops.brew-update</string>"));
        assert!(x.contains("<string>/bin/bash</string>"));
        assert!(x.contains(&format!("<string>{ROOT}/bin/run-job</string>")));
        assert!(x.contains(&format!("<string>{ROOT}/jobs/30-brew-update.sh</string>")));
        assert!(x.contains(&format!("<key>WorkingDirectory</key>\n    <string>{ROOT}</string>")));
        assert!(x.contains("<key>RunAtLoad</key>\n    <false/>"));
        assert!(!x.contains("KeepAlive"), "a periodic job is not a daemon");
        assert!(x.contains("/opt/homebrew/bin"), "launchd's PATH is minimal");
        assert!(x.contains("<key>PATH</key>"));
        assert!(x.contains("<key>HOME</key>"));
        assert!(x.contains(&format!(
            "<key>StandardOutPath</key>\n    <string>{ROOT}/logs/job-brew-update.out.log</string>"
        )));
        assert!(x.contains(&format!(
            "<key>StandardErrorPath</key>\n    <string>{ROOT}/logs/job-brew-update.err.log</string>"
        )));
        // Never a daemon, never root.
        assert!(!x.to_lowercase().contains("username"));
        assert!(!x.to_lowercase().contains("sudo"));
    }

    #[test]
    fn daily_single_time_is_a_dict() {
        let x = xml(&Schedule::Daily { times: vec!["02:30".into()] });
        assert!(x.contains("<key>StartCalendarInterval</key>\n    <dict>"));
        assert!(x.contains("<key>Hour</key>\n        <integer>2</integer>"));
        assert!(x.contains("<key>Minute</key>\n        <integer>30</integer>"));
        assert!(!x.contains("<key>StartInterval</key>"));
        assert!(!x.contains("<key>Weekday</key>"));
    }

    #[test]
    fn daily_several_times_is_an_array() {
        let x = xml(&Schedule::Daily { times: vec!["02:00".into(), "14:05".into()] });
        assert!(x.contains("<key>StartCalendarInterval</key>\n    <array>"));
        assert_eq!(x.matches("<key>Hour</key>").count(), 2);
        assert!(x.contains("<integer>14</integer>"));
        assert!(x.contains("<key>Minute</key>\n            <integer>5</integer>"));
        assert!(!x.contains("<key>Weekday</key>"));
    }

    #[test]
    fn daily_with_no_usable_time_falls_back_to_0200() {
        let x = xml(&Schedule::Daily { times: vec!["nope".into(), "25:99".into()] });
        assert!(x.contains("<key>StartCalendarInterval</key>\n    <dict>"));
        assert!(x.contains("<key>Hour</key>\n        <integer>2</integer>"));
        assert!(x.contains("<key>Minute</key>\n        <integer>0</integer>"));
    }

    #[test]
    fn weekly_is_an_array_of_dicts_with_weekday() {
        let x = xml(&Schedule::Weekly {
            days: vec!["sun".into(), "Wednesday".into()],
            times: vec!["03:15".into()],
        });
        assert!(x.contains("<key>StartCalendarInterval</key>\n    <array>"));
        assert_eq!(x.matches("<key>Weekday</key>").count(), 2);
        assert!(x.contains("<key>Weekday</key>\n            <integer>0</integer>"));
        assert!(x.contains("<key>Weekday</key>\n            <integer>3</integer>"));
        assert_eq!(x.matches("<key>Minute</key>").count(), 2);
        assert!(!x.contains("<key>StartInterval</key>"));
    }

    #[test]
    fn weekly_crosses_days_with_times() {
        let x = xml(&Schedule::Weekly {
            days: vec!["mon".into(), "fri".into()],
            times: vec!["01:00".into(), "13:00".into()],
        });
        assert_eq!(x.matches("<key>Weekday</key>").count(), 4);
        assert_eq!(x.matches("<key>Hour</key>").count(), 4);
    }

    #[test]
    fn weekday_parsing_accepts_names_and_numbers() {
        assert_eq!(weekday_num("Sunday"), Some(0));
        assert_eq!(weekday_num("sat"), Some(6));
        assert_eq!(weekday_num("0"), Some(0));
        assert_eq!(weekday_num("7"), Some(0)); // launchd treats 7 as Sunday
        assert_eq!(weekday_num("3"), Some(3));
        assert_eq!(weekday_num("8"), None);
        assert_eq!(weekday_num("someday"), None);
    }

    #[test]
    fn interval_is_seconds() {
        let x = xml(&Schedule::Interval { every_hours: 6 });
        assert!(x.contains("<key>StartInterval</key>\n    <integer>21600</integer>"));
        assert!(!x.contains("<key>StartCalendarInterval</key>"));
        // Zero hours would mean a spin loop; clamp to one hour.
        let x0 = xml(&Schedule::Interval { every_hours: 0 });
        assert!(x0.contains("<key>StartInterval</key>\n    <integer>3600</integer>"));
    }

    #[test]
    fn manual_has_no_start_key_at_all() {
        let x = xml(&Schedule::Manual);
        assert!(!x.contains("StartCalendarInterval"));
        assert!(!x.contains("StartInterval"));
        assert!(!x.contains("<key>Start"));
        assert!(!x.contains("KeepAlive"));
        // Still a complete, loadable agent — just never scheduled.
        assert!(x.contains("<key>Label</key>"));
        assert!(x.contains("<key>RunAtLoad</key>\n    <false/>"));
    }

    #[test]
    fn every_variant_renders_wellformed_xml() {
        for s in [
            Schedule::Daily { times: vec!["02:00".into()] },
            Schedule::Daily { times: vec!["02:00".into(), "22:00".into()] },
            Schedule::Weekly { days: vec!["sun".into()], times: vec!["04:00".into()] },
            Schedule::Interval { every_hours: 4 },
            Schedule::Manual,
        ] {
            let x = xml(&s);
            assert_eq!(x.matches("<dict>").count(), x.matches("</dict>").count());
            assert_eq!(x.matches("<array>").count(), x.matches("</array>").count());
            assert!(x.trim_end().ends_with("</plist>"));
            // Only the top-level plist dict plus EnvironmentVariables when
            // there is no calendar entry.
            if matches!(s, Schedule::Manual | Schedule::Interval { .. }) {
                assert_eq!(x.matches("<dict>").count(), 2);
            }
        }
    }

    #[test]
    fn paths_and_names_are_xml_escaped() {
        let x = plist_xml(
            Path::new("/Users/a&b/machine-ops"),
            "30-a<b>.sh",
            &Schedule::Manual,
        )
        .unwrap();
        assert!(x.contains("/Users/a&amp;b/machine-ops/bin/run-job"));
        assert!(!x.contains("/Users/a&b/"));
        assert!(!x.contains("a<b>"));
    }

    #[test]
    fn traversal_filenames_are_rejected() {
        assert!(plist_xml(Path::new(ROOT), "../../evil.sh", &Schedule::Manual).is_err());
    }

    #[test]
    fn plist_lives_in_user_launchagents_only() {
        // Never /Library/LaunchDaemons, never /Library/LaunchAgents.
        let p = plist_path("30-brew-update.sh").unwrap();
        let s = p.to_string_lossy();
        assert!(s.ends_with("/Library/LaunchAgents/ai.macops.brew-update.plist"));
        // Must never resolve to the live tier agent's plist.
        assert!(!s.contains("ai.machineops.daily"));
        assert!(!s.contains("LaunchDaemons"));
        assert!(s.starts_with(&std::env::var("HOME").unwrap()));
    }

    /// `plutil -lint` is the same parser launchd uses. Skips silently when
    /// plutil is unavailable; never touches launchctl.
    #[test]
    fn plutil_accepts_every_variant() {
        if !Path::new("/usr/bin/plutil").exists() {
            return;
        }
        for (i, s) in [
            Schedule::Daily { times: vec!["02:00".into()] },
            Schedule::Daily { times: vec!["02:00".into(), "22:30".into()] },
            Schedule::Weekly { days: vec!["sun".into(), "wed".into()], times: vec!["04:00".into()] },
            Schedule::Interval { every_hours: 4 },
            Schedule::Manual,
        ]
        .iter()
        .enumerate()
        {
            let p = std::env::temp_dir().join(format!(
                "machine-ops-plist-{}-{i}.plist",
                std::process::id()
            ));
            std::fs::write(&p, xml(s)).unwrap();
            let out = Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&p)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let ok = out.status.success();
            std::fs::remove_file(&p).ok();
            assert!(ok, "plutil rejected variant {i}: {}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn not_loaded_errors_are_recognized() {
        assert!(is_not_loaded("Boot-out failed: 3: No such process"));
        assert!(is_not_loaded("Could not find specified service"));
        assert!(!is_not_loaded("Bootstrap failed: 5: Input/output error"));
    }
}
