//! Shared types across the console backend. Owned by the integration layer:
//! every module codes against these, none of them redefine them.
use serde::{Deserialize, Serialize};

/// Outcome vocabulary. `Quiet` is SUCCESS — the job ran and said nothing.
/// This is the whole point of the silent-on-success contract: never render
/// "no output" as "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Quiet,
    Spoke,
    Fail,
    Hang,
    Off,
    None,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Quiet => "quiet",
            Status::Spoke => "spoke",
            Status::Fail => "fail",
            Status::Hang => "hang",
            Status::Off => "off",
            Status::None => "none",
        }
    }
    pub fn parse(s: &str) -> Status {
        match s {
            "quiet" => Status::Quiet,
            "spoke" => Status::Spoke,
            "fail" => Status::Fail,
            "hang" => Status::Hang,
            "off" => Status::Off,
            _ => Status::None,
        }
    }
    /// Derive the outcome the way bin/run-job does: a timeout is rc=124,
    /// any stdout at all is an alert, a clean rc=0 with no stdout is quiet.
    pub fn derive(rc: i32, timed_out: bool, stdout: &str) -> Status {
        if timed_out || rc == 124 {
            Status::Hang
        } else if rc != 0 {
            Status::Fail
        } else if !stdout.trim().is_empty() {
            Status::Spoke
        } else {
            Status::Quiet
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Schedule {
    Daily { times: Vec<String> },
    Weekly { days: Vec<String>, times: Vec<String> },
    Interval { every_hours: u32 },
    Manual,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule::Daily { times: vec!["02:00".into()] }
    }
}

impl Schedule {
    /// Minute-of-day start times this schedule produces in a 24h window.
    /// Used by the load profile and the stagger assistant.
    pub fn starts(&self) -> Vec<u32> {
        match self {
            Schedule::Manual => vec![],
            Schedule::Interval { every_hours } => {
                let step = (*every_hours).max(1);
                (0..24).step_by(step as usize).map(|h| h * 60).collect()
            }
            Schedule::Daily { times } | Schedule::Weekly { times, .. } => {
                times.iter().filter_map(|t| parse_hhmm(t)).collect()
            }
        }
    }
    pub fn tier(&self) -> &'static str {
        match self {
            Schedule::Weekly { .. } => "weekly",
            _ => "daily",
        }
    }
}

/// "HH:MM" -> minute of day. Rejects anything out of range.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    if h > 23 || m > 59 { return None; }
    Some(h * 60 + m)
}

/// One automation, as the UI sees it: script metadata + desired state + last run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub file: String,   // "30-brew-update.sh"
    pub label: String,  // "brew-update"
    pub title: String,
    pub group: String,
    pub kind: String,   // mole | pkg | py | sh
    pub tier: String,   // daily | weekly  (line 2 of the script)
    pub timeout: u32,   // line 3 of the script, seconds
    pub enabled: bool,
    pub schedule: Schedule,
    pub last: Option<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub job: String,
    pub started_at: i64, // unix epoch seconds
    pub duration_s: i64,
    pub rc: i32,
    pub status: Status,
    pub dry_run: bool,
    pub summary: String,
    pub stdout: String,
}

/// A run being recorded (no id yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRun {
    pub job: String,
    pub started_at: i64,
    pub duration_s: i64,
    pub rc: i32,
    pub status: Status,
    pub dry_run: bool,
    pub summary: String,
    pub stdout: String,
}

/// One cell of the fleet heatmap: worst outcome for a group on a day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStat {
    pub day: String, // YYYY-MM-DD
    pub group: String,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewJob {
    pub title: String,
    pub group: String,
    pub kind: String, // mole | pkg | py | sh
    pub schedule: Schedule,
    pub timeout: u32,
    /// Python template: path to the .py file, relative to the repo root.
    pub script_path: Option<String>,
    /// Package template: brew | npm | pipx | cargo | rustup | gem
    pub manager: Option<String>,
    /// Mole template: optimize | clean
    pub subcommand: Option<String>,
}

/// Result of running a job through bin/run-job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub status: Status,
    pub rc: i32,
    pub duration_s: i64,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}
