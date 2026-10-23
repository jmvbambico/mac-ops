//! machine-ops console — Tauri command surface.
//!
//! Integration layer. Owns the module wiring and the commands the frontend
//! calls; the modules themselves are implemented independently.
mod db;
mod jobs;
mod launchd;
mod paths;
mod state;
mod stream;
mod types;

use db::Db;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tauri::Manager;
use types::{DayStat, Job, NewJob, Run, RunOutcome, Schedule};

pub struct App {
    pub tool_root: PathBuf,
    pub automations_root: PathBuf,
    /// Where the automations root came from: "env", "config" or "default".
    pub automations_source: &'static str,
    pub db: Db,
}

/// Commands return `Result<T, String>`: Tauri needs a serializable error,
/// and the UI shows the message rather than dying.
type R<T> = Result<T, String>;
fn e<E: std::fmt::Display>(err: E) -> String { err.to_string() }

/// Run a blocking task off Tauri's main thread and flatten the join error.
///
/// Tauri runs SYNCHRONOUS commands on the main thread — the webview's event
/// loop — so file I/O, a child process or an SQLite query freezes the whole
/// window for its duration. Splitting the spawn from `with_app` keeps a seam
/// the tests can drive without a Tauri runtime: a panic in the task must come
/// back as the error string the UI shows, never as an unwind through the
/// command.
async fn off_main<T, F>(task: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce() -> R<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|err| format!("task failed: {err}"))?
}

/// Synchronous commands used to run one at a time on the main thread. State
/// updates are read-modify-write on one JSON file, and `state::save` reuses a
/// single `.tmp` path, so allowing those bodies to run concurrently would
/// lose updates or interleave writes into the same temp file. This keeps
/// their order unchanged now that they share the blocking pool.
static COMMAND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Convert a command body into the `spawn_blocking` shape `run_job` uses.
///
/// An async command cannot hold a borrowed `State<'_, App>` across an await,
/// so the command takes the owned `AppHandle` and resolves `State<App>` inside
/// the blocking closure, where a synchronous borrow is safe.
async fn with_app<T, F>(app: tauri::AppHandle, task: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce(&App) -> R<T> + Send + 'static,
{
    off_main(move || {
        // Recover from a poisoned lock: one panicking command must not stop
        // every later command from running.
        let _guard = COMMAND_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        task(app.state::<App>().inner())
    })
    .await
}

/// Merge what is on disk (jobs/*.sh) with desired state (console.json) and
/// last-run facts (SQLite). Disk is authoritative about which automations
/// EXIST; console.json only decorates them.
///
/// The body is split out so `load_profile` can reuse it synchronously inside
/// its own blocking task.
fn collect_jobs(app: &App) -> R<Vec<Job>> {
    let metas = jobs::scan(&app.automations_root).map_err(e)?;
    let st = state::load(&app.automations_root);
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let js = st.jobs.get(&m.file);
        let last = app.db.last_run(&m.file).map_err(e)?;
        out.push(Job {
            title: js.and_then(|j| j.title.clone()).unwrap_or_else(|| prettify(&m.label)),
            group: js.and_then(|j| j.group.clone()).unwrap_or_else(|| "Ungrouped".into()),
            schedule: js.map(|j| j.schedule.clone()).unwrap_or_default(),
            enabled: js.map(|j| j.enabled).unwrap_or(true), // fail safe: enabled
            tier: m.tier.clone().unwrap_or_else(|| "unknown".into()),
            timeout: m.timeout,
            kind: m.kind.clone(),
            label: m.label.clone(),
            file: m.file.clone(),
            last,
        });
    }
    Ok(out)
}

#[tauri::command]
async fn list_jobs(app: tauri::AppHandle) -> R<Vec<Job>> {
    with_app(app, collect_jobs).await
}

fn prettify(label: &str) -> String {
    let mut s = label.replace('-', " ");
    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
    s
}

#[tauri::command]
async fn set_enabled(app: tauri::AppHandle, file: String, enabled: bool) -> R<()> {
    with_app(app, move |app| {
        let mut st = state::load(&app.automations_root);
        let entry = st.jobs.entry(file.clone()).or_insert_with(|| state::JobState {
            enabled: true, schedule: Schedule::default(), title: None, group: None,
        });
        entry.enabled = enabled;
        let sched = entry.schedule.clone();
        state::save(&app.automations_root, &st).map_err(e)?;
        if enabled {
            launchd::install(&app.tool_root, &app.automations_root, &file, &sched).map_err(e)
        } else {
            launchd::uninstall(&file).map_err(e)
        }
    })
    .await
}

#[tauri::command]
async fn set_schedule(app: tauri::AppHandle, file: String, schedule: Schedule) -> R<()> {
    with_app(app, move |app| {
        let mut st = state::load(&app.automations_root);
        let entry = st.jobs.entry(file.clone()).or_insert_with(|| state::JobState {
            enabled: true, schedule: Schedule::default(), title: None, group: None,
        });
        entry.schedule = schedule.clone();
        let enabled = entry.enabled;
        state::save(&app.automations_root, &st).map_err(e)?;
        if enabled { launchd::install(&app.tool_root, &app.automations_root, &file, &schedule).map_err(e)? }
        Ok(())
    })
    .await
}

#[tauri::command]
async fn set_timeout(app: tauri::AppHandle, file: String, secs: u32) -> R<()> {
    with_app(app, move |app| {
        jobs::set_timeout(&paths::jobs_dir(&app.automations_root).join(&file), secs).map_err(e)
    })
    .await
}

#[tauri::command]
async fn set_meta(app: tauri::AppHandle, file: String, title: Option<String>, group: Option<String>) -> R<()> {
    with_app(app, move |app| {
        let mut st = state::load(&app.automations_root);
        let entry = st.jobs.entry(file).or_insert_with(|| state::JobState {
            enabled: true, schedule: Schedule::default(), title: None, group: None,
        });
        if title.is_some() { entry.title = title; }
        if group.is_some() { entry.group = group; }
        state::save(&app.automations_root, &st).map_err(e)
    })
    .await
}

#[tauri::command]
async fn create_job(app: tauri::AppHandle, spec: NewJob) -> R<String> {
    with_app(app, move |app| {
        let file = jobs::create(&app.automations_root, &spec).map_err(e)?;
        let diags = jobs::validate(&paths::jobs_dir(&app.automations_root).join(&file)).map_err(e)?;
        if !diags.is_empty() {
            let _ = jobs::delete(&app.automations_root, &file, false);
            return Err(format!("generated script failed validation:\n{}", diags.join("\n")));
        }
        let mut st = state::load(&app.automations_root);
        st.jobs.insert(file.clone(), state::JobState {
            enabled: false, // scheduled only after a clean dry run
            schedule: spec.schedule.clone(),
            title: Some(spec.title.clone()),
            group: Some(spec.group.clone()),
        });
        state::save(&app.automations_root, &st).map_err(e)?;
        Ok(file)
    })
    .await
}

#[tauri::command]
async fn delete_job(app: tauri::AppHandle, file: String, keep_script: bool) -> R<()> {
    with_app(app, move |app| {
        launchd::uninstall(&file).map_err(e)?;
        let mut st = state::load(&app.automations_root);
        st.jobs.remove(&file);
        state::save(&app.automations_root, &st).map_err(e)?;
        jobs::delete(&app.automations_root, &file, keep_script).map_err(e)
    })
    .await
}

/// Record a finished console-triggered run from what the SHELL wrote.
///
/// `bin/run-job` is the single clock and the single writer: it stamps
/// `run_started_epoch` once, logs it as `started=<epoch>`, and passes that
/// same value to `record_run` for `logs/runs.jsonl`. The console must not
/// stamp a second start time of its own — two clocks can land a second apart
/// and split one run into two rows, one of them blank.
///
/// Ingest order is load-bearing, exactly as in `reindex`: the text log first
/// (its job summary fills a quiet run's empty feed summary), then the feed
/// (authoritative for duration/rc/status/dry_run). Reversing it would let an
/// empty feed summary land first and then let the parser overwrite metrics.
///
/// Best-effort, like the feed write itself: a database hiccup must never turn
/// a completed run into a reported failure.
fn record_finished_run(db: &Db, automations_root: &std::path::Path) {
    let _ = db.ingest_log(&paths::run_log(automations_root));
    let _ = db.ingest_runs_jsonl(&paths::runs_jsonl(automations_root));
}

/// Run now, in-process (not via launchd) so the UI can stream the outcome.
/// The run is recorded best-effort: a database failure must never be
/// reported as a job failure.
///
/// Async on purpose. Tauri runs SYNCHRONOUS commands on the main thread, and
/// `jobs::run` blocks until the child exits, so a 105-second job froze the
/// whole webview for 105 seconds. The blocking work moves to
/// `spawn_blocking`, and the command itself holds no borrowed `State` across
/// an await: it takes the owned `AppHandle` and resolves `State<App>` inside
/// the blocking closure, where a borrow is safe.
///
/// While the job runs, a poller tails `logs/machine-ops.log` from the offset
/// recorded BEFORE the child starts and emits line batches on `run-log`. The
/// log is the durable record (Principle VI): the child's stdout is silent on
/// success, so streaming stdout would show nothing.
#[tauri::command]
async fn run_job(app: tauri::AppHandle, file: String, dry: bool) -> R<RunOutcome> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<App>();
        let tool_root = state.tool_root.clone();
        let automations_root = state.automations_root.clone();

        // Offset first, then start the poller, then the child: that order is
        // what makes "new" mean this run. An overlapping scheduled run may
        // interleave its lines; that is accepted by design.
        let tail = stream::LogTail::at_end(&paths::run_log(&automations_root));
        let mut poller = stream::spawn(app.clone(), file.clone(), tail);

        let result = jobs::run(&tool_root, &automations_root, &file, dry);

        // Stop and join BEFORE recording the outcome so the final lines
        // (including a trailing partial line) are emitted while the UI still
        // listens. If `jobs::run` panics, the guard's `Drop` does this for us
        // on the unwind path, so the poller can never outlive the run.
        poller.stop();

        let out = result.map_err(e)?;
        // No direct record_run here: bin/run-job already wrote the feed from
        // the ONE shell clock, and both ingests below derive the row from it.
        record_finished_run(&state.db, &automations_root);
        Ok(out)
    })
    .await
    .map_err(|err| format!("run task failed: {err}"))?
}

#[tauri::command]
async fn runs_for(app: tauri::AppHandle, file: String, limit: i64) -> R<Vec<Run>> {
    with_app(app, move |app| app.db.runs_for(&file, limit).map_err(e)).await
}

#[tauri::command]
async fn fleet_history(app: tauri::AppHandle, days: i64) -> R<Vec<DayStat>> {
    with_app(app, move |app| {
        let st = state::load(&app.automations_root);
        let groups: BTreeMap<String, String> = st.jobs.iter()
            .map(|(f, j)| (f.clone(), j.group.clone().unwrap_or_else(|| "Ungrouped".into())))
            .collect();
        app.db.fleet_history(days, &groups).map_err(e)
    })
    .await
}

#[tauri::command]
async fn search_logs(app: tauri::AppHandle, q: String, limit: i64) -> R<Vec<Run>> {
    with_app(app, move |app| app.db.search(&q, limit).map_err(e)).await
}

/// Concurrency per minute-of-day across all enabled automations, for the
/// load histogram and the stagger assistant.
#[tauri::command]
async fn load_profile(app: tauri::AppHandle) -> R<Vec<u32>> {
    with_app(app, |app| {
        let jobs_list = collect_jobs(app)?;
        let mut mins = vec![0u32; 1440];
        for j in jobs_list.iter().filter(|j| j.enabled) {
            let dur_min = j.last.as_ref().map(|r| (r.duration_s / 60).max(1)).unwrap_or(1) as usize;
            for s in j.schedule.starts() {
                for i in 0..dur_min { mins[(s as usize + i) % 1440] += 1; }
            }
        }
        Ok(mins)
    })
    .await
}

#[tauri::command]
async fn validate_job(app: tauri::AppHandle, file: String) -> R<Vec<String>> {
    with_app(app, move |app| {
        jobs::validate(&paths::jobs_dir(&app.automations_root).join(&file)).map_err(e)
    })
    .await
}

/// Largest captured-output payload returned to the UI.
///
/// The runner's `log_output` caps the file at 256 KiB, so a normal file is
/// never cut here. This is a guard against a hand-edited, older or foreign
/// file — not an expected path — and it is reported to the reader rather than
/// applied silently.
pub const MAX_JOB_OUTPUT_BYTES: usize = 256 * 1024;

/// Read `logs/job-<slug>.out.log` (written by `log_output` in lib/common.sh).
///
/// A missing file is `Ok("")`: a job that never opted into `log_output` is the
/// ordinary case, and the modal simply falls back to the streamed run log. A
/// file larger than the cap yields its TAIL — the newest output — behind a
/// visible truncation notice, because the beginning is the part a reader can
/// most afford to lose.
fn read_job_output_file(path: &std::path::Path) -> R<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(err) => return Err(format!("read {}: {err}", path.display())),
    };
    let len = file
        .metadata()
        .map_err(|err| format!("stat {}: {err}", path.display()))?
        .len();

    let truncated = len > MAX_JOB_OUTPUT_BYTES as u64;
    if truncated {
        file.seek(SeekFrom::End(-(MAX_JOB_OUTPUT_BYTES as i64)))
            .map_err(|err| format!("seek {}: {err}", path.display()))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    // Commands may emit bytes that are not valid UTF-8; lossy beats failing
    // the whole view over one stray byte.
    let mut text = String::from_utf8_lossy(&bytes).into_owned();

    if truncated {
        // Drop the partial first line the byte cut left behind, then say what
        // was dropped. Silent truncation would read as a job that stopped.
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
        text.insert_str(
            0,
            &format!(
                "… output truncated — showing the last {} KiB of {} KiB; the full file is on disk\n",
                MAX_JOB_OUTPUT_BYTES / 1024,
                (len + 1023) / 1024
            ),
        );
    }
    Ok(text)
}

/// Captured per-job output for the run modal, fetched AFTER the run finishes:
/// the job captures command output first and calls `log_output` near the end.
#[tauri::command]
async fn read_job_output(app: tauri::AppHandle, file: String) -> R<String> {
    with_app(app, move |app| {
        read_job_output_file(&paths::job_output_log(&app.automations_root, &file))
    })
    .await
}

#[tauri::command]
async fn prune_runs(app: tauri::AppHandle, keep_days: i64) -> R<usize> {
    with_app(app, move |app| app.db.prune(keep_days).map_err(e)).await
}

#[tauri::command]
async fn reindex(app: tauri::AppHandle) -> R<usize> {
    with_app(app, |app| {
        // Rebuild from both: the text log recovers history that predates the
        // structured feed, then the feed lands last so it wins wherever both
        // describe the same run. The count is records read, not distinct rows.
        let log = app.db.ingest_log(&paths::run_log(&app.automations_root)).map_err(e)?;
        let runs = app
            .db
            .ingest_runs_jsonl(&paths::runs_jsonl(&app.automations_root))
            .map_err(e)?;
        Ok(log + runs)
    })
    .await
}

/// The resolved split, so the GUI can show where the tool lives versus where
/// the user's jobs/logs/state live. `source` is exactly one of `"env"`,
/// `"config"` or `"default"`.
#[derive(serde::Serialize)]
pub struct ResolvedPaths {
    pub tool_root: String,
    pub automations_root: String,
    pub config_path: String,
    pub source: String,
}

/// Left synchronous on purpose: it touches no file, spawns nothing and runs
/// no query — only field reads plus a `$HOME` lookup to build the config path
/// string, so it cannot block the event loop.
#[tauri::command]
fn resolved_paths(app: tauri::State<'_, App>) -> R<ResolvedPaths> {
    Ok(ResolvedPaths {
        tool_root: app.tool_root.to_string_lossy().into_owned(),
        automations_root: app.automations_root.to_string_lossy().into_owned(),
        config_path: paths::config_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        source: app.automations_source.to_string(),
    })
}

/// Persist a user-chosen automations root.
///
/// The write changes the CONFIG FILE only. `App` keeps the root it resolved at
/// startup — the SQLite handle, cached jobs and launchd plists are all derived
/// from it — so the returned paths describe what the NEXT launch will use, not
/// a live switch. The UI must say "restart to apply"; a half-switched console
/// with a stale database would be worse than asking for a restart.
#[tauri::command]
async fn set_automations_root(app: tauri::AppHandle, path: String) -> R<ResolvedPaths> {
    with_app(app, move |app| {
        let config = paths::config_path().map_err(e)?;
        paths::set_config_root(&config, &app.tool_root, &path).map_err(e)?;

        // Re-resolve for the answer: with $MOPS_AUTOMATIONS_ROOT set the
        // effective root is still the environment value, and `source` must
        // say so.
        let (root, source) = paths::automations_root_with_source().map_err(e)?;
        Ok(ResolvedPaths {
            tool_root: app.tool_root.to_string_lossy().into_owned(),
            automations_root: root.to_string_lossy().into_owned(),
            config_path: config.to_string_lossy().into_owned(),
            source: source.to_string(),
        })
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let tool_root = paths::tool_root()
        .expect("MacOps tool root not found (set MOPS_TOOL_ROOT)");
    let (automations_root, automations_source) = paths::automations_root_with_source()
        .expect("cannot resolve the MacOps automations root");
    paths::ensure_dirs(&automations_root)
        .expect("cannot create jobs/, logs/ and state/ under the automations root");
    let db = Db::open(&paths::db_file(&automations_root)).expect("cannot open console.db");
    // Best-effort backfill so a fresh database is not an empty console. The
    // text log restores older history, then the structured feed lands last:
    // it is the primary source wherever both describe the same run.
    let _ = db.ingest_log(&paths::run_log(&automations_root));
    let _ = db.ingest_runs_jsonl(&paths::runs_jsonl(&automations_root));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(App { tool_root, automations_root, automations_source, db })
        .invoke_handler(tauri::generate_handler![
            list_jobs, set_enabled, set_schedule, set_timeout, set_meta,
            create_job, delete_job, run_job, runs_for, fleet_history,
            search_logs, load_profile, validate_job, prune_runs, reindex,
            resolved_paths, set_automations_root, read_job_output
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "machine-ops-lib-test-{}-{}-{}",
                std::process::id(),
                tag,
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("jobs")).unwrap();
            Tmp(dir)
        }
        fn app(&self) -> App {
            App {
                tool_root: self.0.clone(),
                automations_root: self.0.clone(),
                automations_source: "env",
                db: Db::open(&self.0.join("state/console.db")).unwrap(),
            }
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `with_app` cannot be driven without a live `AppHandle`, so the seam it
    /// uses is tested directly: a converted command's work must still yield
    /// its ordinary value.
    #[test]
    fn off_main_returns_the_task_value() {
        let got = tauri::async_runtime::block_on(off_main(|| Ok::<u32, String>(41 + 1)));
        assert_eq!(got, Ok(42));
    }

    /// A panic in blocking work must surface as the `task failed: …` string
    /// the UI can show — never unwind through the command.
    #[test]
    fn off_main_turns_a_panicking_task_into_an_error_string() {
        let err = tauri::async_runtime::block_on(off_main(|| -> R<()> { panic!("boom") }))
            .unwrap_err();
        assert!(err.starts_with("task failed: "), "{err}");
        assert!(err.contains("boom"), "{err}");
    }

    /// The synchronous core `list_jobs` now runs on the blocking thread still
    /// merges disk, state and SQLite exactly as the command used to.
    #[test]
    fn collect_jobs_keeps_the_command_surface() {
        let tmp = Tmp::new("collect-jobs");
        std::fs::write(
            tmp.0.join("jobs/10-test.sh"),
            "#!/bin/bash\n# tier: daily\n# timeout: 120\necho hi\n",
        )
        .unwrap();
        let app = tmp.app();

        let jobs = collect_jobs(&app).unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.file, "10-test.sh");
        assert_eq!(job.label, "10-test");
        assert_eq!(job.title, "10 test");
        assert_eq!(job.group, "Ungrouped");
        assert_eq!(job.tier, "daily");
        assert_eq!(job.timeout, 120);
        assert_eq!(job.kind, "sh");
        assert!(job.enabled, "missing state must fail safe toward enabled");
        assert!(job.last.is_none());
    }

    /// A job that has not opted into `log_output` has no file. That is the
    /// normal case for the modal, so it must read as "nothing captured" rather
    /// than an error the UI has to explain.
    #[test]
    fn reading_a_missing_job_output_is_empty_not_an_error() {
        let tmp = Tmp::new("output-missing");
        let path = paths::job_output_log(&tmp.0, "20-mole-clean.sh");
        assert_eq!(read_job_output_file(&path).unwrap(), "");
    }

    /// `log_output` caps the runner's file at 256 KiB, but the console must not
    /// assume that. A larger file is cut to its newest bytes and the cut is
    /// NAMED to the reader — a silent cut would look like a job that stopped.
    #[test]
    fn large_job_output_is_truncated_visibly() {
        let tmp = Tmp::new("output-large");
        std::fs::create_dir_all(tmp.0.join("logs")).unwrap();
        let path = paths::job_output_log(&tmp.0, "20-mole-clean.sh");
        let line = "0123456789abcdef\n";
        let body = line.repeat(MAX_JOB_OUTPUT_BYTES / line.len() + 10);
        std::fs::write(&path, &body).unwrap();

        let got = read_job_output_file(&path).unwrap();
        assert!(got.starts_with("… output truncated"), "{:?}", &got[..60.min(got.len())]);
        assert!(got.contains("the full file is on disk"), "{got}");
        assert!(got.len() < body.len(), "the cap must actually shrink the payload");
        assert!(got.ends_with(line), "the newest output must survive");
        assert!(
            !got.contains("0123456789abcdef\n… output truncated"),
            "nothing may be prepended into the tail itself"
        );
    }

    /// A console-triggered run must key on the SHELL's clock, not a second
    /// one read in Rust. `bin/run-job` logs `started=<epoch>` and writes the
    /// feed with that same epoch; the console only ingests. Under the old
    /// code a direct `record_run` stamped another `Utc::now()` and could land
    /// a second earlier, leaving two rows for one run (one blank).
    #[test]
    fn a_console_run_records_one_row_from_the_shell_clock() {
        let tmp = Tmp::new("console-run");
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root");
        std::fs::write(
            tmp.0.join("jobs/20-console.sh"),
            "#!/usr/bin/env bash\n# tier: daily\n\
             source \"$MOPS_TOOL_ROOT/lib/common.sh\"\n\
             log_line \"[console] skills=4 updated=none rc=0 dry_run=0\"\n\
             exit 0\n",
        )
        .unwrap();

        // The console's path: jobs::run shells out to bin/run-job, which is
        // the single writer of both the text log and runs.jsonl.
        let out = jobs::run(repo, &tmp.0, "20-console.sh", false).unwrap();
        assert!(out.stdout.is_empty(), "a nominal run is silent");
        assert_eq!(out.rc, 0);

        let app = tmp.app();
        record_finished_run(&app.db, &tmp.0);

        let log = std::fs::read_to_string(tmp.0.join("logs/machine-ops.log")).unwrap();
        let shell_start: i64 = log
            .lines()
            .find(|l| l.contains("[run-job] start job=20-console.sh"))
            .and_then(|l| {
                l.split_whitespace()
                    .find_map(|t| t.strip_prefix("started="))
            })
            .and_then(|v| v.parse().ok())
            .expect("bin/run-job must log its real start epoch");

        let rows = app.db.runs_for("20-console.sh", 10).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the shell is the only writer; a second clock must not add a row"
        );
        assert_eq!(
            rows[0].started_at, shell_start,
            "the row's identity comes from the shell's logged start"
        );
        assert_eq!(rows[0].summary, "skills=4 updated=none rc=0 dry_run=0");
        assert_eq!(rows[0].rc, 0);
        assert_eq!(rows[0].status, crate::types::Status::Quiet);
        eprintln!(
            "console e2e: started={} (shell {shell_start}) rc={} duration={}s summary={:?}",
            rows[0].started_at, rows[0].rc, rows[0].duration_s, rows[0].summary
        );
    }

    /// Pins the post-run ingest ORDER: text log first (recover the summary),
    /// feed second (authoritative metrics). This is reindex's contract; the
    /// on-demand path must not diverge from it.
    #[test]
    fn the_post_run_ingests_the_log_before_the_feed() {
        let tmp = Tmp::new("post-run-order");
        let started = 1_700_000_000i64;
        std::fs::create_dir_all(tmp.0.join("logs")).unwrap();
        std::fs::write(
            tmp.0.join("logs/machine-ops.log"),
            format!(
                "2026-10-21 05:00:00 [run-job] start job=20-order.sh tier=daily timeout=900s started={started}\n\
                 2026-10-21 05:00:04 [order] skills=4 rc=0 dry_run=0\n\
                 2026-10-21 05:00:05 [run-job] finish job=20-order.sh rc=0 timeout=0 tier=daily elapsed=5s\n"
            ),
        )
        .unwrap();
        std::fs::write(
            tmp.0.join("logs/runs.jsonl"),
            format!(
                "{{\"job\": \"20-order.sh\", \"started_at\": {started}, \"duration_s\": 42, \"rc\": 1, \"status\": \"fail\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}}\n"
            ),
        )
        .unwrap();

        let app = tmp.app();
        record_finished_run(&app.db, &tmp.0);

        let rows = app.db.runs_for("20-order.sh", 10).unwrap();
        assert_eq!(rows.len(), 1, "same key, so one row");
        assert_eq!(
            rows[0].summary, "skills=4 rc=0 dry_run=0",
            "the text log's summary must fill the feed's blank"
        );
        assert_eq!(rows[0].duration_s, 42, "the feed's duration must win");
        assert_eq!(rows[0].rc, 1, "the feed's rc must win");
        assert_eq!(rows[0].status, crate::types::Status::Fail);
    }
}
