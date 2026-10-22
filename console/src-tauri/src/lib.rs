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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

/// Merge what is on disk (jobs/*.sh) with desired state (console.json) and
/// last-run facts (SQLite). Disk is authoritative about which automations
/// EXIST; console.json only decorates them.
#[tauri::command]
fn list_jobs(app: tauri::State<'_, App>) -> R<Vec<Job>> {
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

fn prettify(label: &str) -> String {
    let mut s = label.replace('-', " ");
    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
    s
}

#[tauri::command]
fn set_enabled(app: tauri::State<'_, App>, file: String, enabled: bool) -> R<()> {
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
}

#[tauri::command]
fn set_schedule(app: tauri::State<'_, App>, file: String, schedule: Schedule) -> R<()> {
    let mut st = state::load(&app.automations_root);
    let entry = st.jobs.entry(file.clone()).or_insert_with(|| state::JobState {
        enabled: true, schedule: Schedule::default(), title: None, group: None,
    });
    entry.schedule = schedule.clone();
    let enabled = entry.enabled;
    state::save(&app.automations_root, &st).map_err(e)?;
    if enabled { launchd::install(&app.tool_root, &app.automations_root, &file, &schedule).map_err(e)? }
    Ok(())
}

#[tauri::command]
fn set_timeout(app: tauri::State<'_, App>, file: String, secs: u32) -> R<()> {
    jobs::set_timeout(&paths::jobs_dir(&app.automations_root).join(&file), secs).map_err(e)
}

#[tauri::command]
fn set_meta(app: tauri::State<'_, App>, file: String, title: Option<String>, group: Option<String>) -> R<()> {
    let mut st = state::load(&app.automations_root);
    let entry = st.jobs.entry(file).or_insert_with(|| state::JobState {
        enabled: true, schedule: Schedule::default(), title: None, group: None,
    });
    if title.is_some() { entry.title = title; }
    if group.is_some() { entry.group = group; }
    state::save(&app.automations_root, &st).map_err(e)
}

#[tauri::command]
fn create_job(app: tauri::State<'_, App>, spec: NewJob) -> R<String> {
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
}

#[tauri::command]
fn delete_job(app: tauri::State<'_, App>, file: String, keep_script: bool) -> R<()> {
    launchd::uninstall(&file).map_err(e)?;
    let mut st = state::load(&app.automations_root);
    st.jobs.remove(&file);
    state::save(&app.automations_root, &st).map_err(e)?;
    jobs::delete(&app.automations_root, &file, keep_script).map_err(e)
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
        let done = Arc::new(AtomicBool::new(false));
        let poller = stream::spawn(app.clone(), file.clone(), tail, done.clone());

        let started = chrono::Utc::now().timestamp();
        let result = jobs::run(&tool_root, &automations_root, &file, dry);

        // Stop and join BEFORE returning so the final lines (including a
        // trailing partial line) are emitted while the UI still listens.
        done.store(true, Ordering::SeqCst);
        let _ = poller.join();

        let out = result.map_err(e)?;
        let _ = state.db.record_run(&types::NewRun {
            job: file,
            started_at: started,
            duration_s: out.duration_s,
            rc: out.rc,
            status: out.status,
            dry_run: dry,
            summary: out.stdout.lines().next().unwrap_or("").to_string(),
            stdout: out.stdout.clone(),
        });
        Ok(out)
    })
    .await
    .map_err(|err| format!("run task failed: {err}"))?
}

#[tauri::command]
fn runs_for(app: tauri::State<'_, App>, file: String, limit: i64) -> R<Vec<Run>> {
    app.db.runs_for(&file, limit).map_err(e)
}

#[tauri::command]
fn fleet_history(app: tauri::State<'_, App>, days: i64) -> R<Vec<DayStat>> {
    let st = state::load(&app.automations_root);
    let groups: BTreeMap<String, String> = st.jobs.iter()
        .map(|(f, j)| (f.clone(), j.group.clone().unwrap_or_else(|| "Ungrouped".into())))
        .collect();
    app.db.fleet_history(days, &groups).map_err(e)
}

#[tauri::command]
fn search_logs(app: tauri::State<'_, App>, q: String, limit: i64) -> R<Vec<Run>> {
    app.db.search(&q, limit).map_err(e)
}

/// Concurrency per minute-of-day across all enabled automations, for the
/// load histogram and the stagger assistant.
#[tauri::command]
fn load_profile(app: tauri::State<'_, App>) -> R<Vec<u32>> {
    let jobs_list = list_jobs(app)?;
    let mut mins = vec![0u32; 1440];
    for j in jobs_list.iter().filter(|j| j.enabled) {
        let dur_min = j.last.as_ref().map(|r| (r.duration_s / 60).max(1)).unwrap_or(1) as usize;
        for s in j.schedule.starts() {
            for i in 0..dur_min { mins[(s as usize + i) % 1440] += 1; }
        }
    }
    Ok(mins)
}

#[tauri::command]
fn validate_job(app: tauri::State<'_, App>, file: String) -> R<Vec<String>> {
    jobs::validate(&paths::jobs_dir(&app.automations_root).join(&file)).map_err(e)
}

#[tauri::command]
fn prune_runs(app: tauri::State<'_, App>, keep_days: i64) -> R<usize> {
    app.db.prune(keep_days).map_err(e)
}

#[tauri::command]
fn reindex(app: tauri::State<'_, App>) -> R<usize> {
    // Rebuild from both: the text log recovers history that predates the
    // structured feed, then the feed lands last so it wins wherever both
    // describe the same run. The count is records read, not distinct rows.
    let log = app.db.ingest_log(&paths::run_log(&app.automations_root)).map_err(e)?;
    let runs = app
        .db
        .ingest_runs_jsonl(&paths::runs_jsonl(&app.automations_root))
        .map_err(e)?;
    Ok(log + runs)
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
fn set_automations_root(app: tauri::State<'_, App>, path: String) -> R<ResolvedPaths> {
    let config = paths::config_path().map_err(e)?;
    paths::set_config_root(&config, &app.tool_root, &path).map_err(e)?;

    // Re-resolve for the answer: with $MOPS_AUTOMATIONS_ROOT set the effective
    // root is still the environment value, and `source` must say so.
    let (root, source) = paths::automations_root_with_source().map_err(e)?;
    Ok(ResolvedPaths {
        tool_root: app.tool_root.to_string_lossy().into_owned(),
        automations_root: root.to_string_lossy().into_owned(),
        config_path: config.to_string_lossy().into_owned(),
        source: source.to_string(),
    })
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
            resolved_paths, set_automations_root
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
