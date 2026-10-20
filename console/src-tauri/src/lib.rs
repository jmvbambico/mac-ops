//! machine-ops console — Tauri command surface.
//!
//! Integration layer. Owns the module wiring and the commands the frontend
//! calls; the modules themselves are implemented independently.
mod db;
mod jobs;
mod launchd;
mod paths;
mod state;
mod types;

use db::Db;
use std::collections::BTreeMap;
use std::path::PathBuf;
use types::{DayStat, Job, NewJob, Run, RunOutcome, Schedule};

pub struct App {
    pub root: PathBuf,
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
    let metas = jobs::scan(&app.root).map_err(e)?;
    let st = state::load(&app.root);
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
    let mut st = state::load(&app.root);
    let entry = st.jobs.entry(file.clone()).or_insert_with(|| state::JobState {
        enabled: true, schedule: Schedule::default(), title: None, group: None,
    });
    entry.enabled = enabled;
    let sched = entry.schedule.clone();
    state::save(&app.root, &st).map_err(e)?;
    if enabled {
        launchd::install(&app.root, &file, &sched).map_err(e)
    } else {
        launchd::uninstall(&file).map_err(e)
    }
}

#[tauri::command]
fn set_schedule(app: tauri::State<'_, App>, file: String, schedule: Schedule) -> R<()> {
    let mut st = state::load(&app.root);
    let entry = st.jobs.entry(file.clone()).or_insert_with(|| state::JobState {
        enabled: true, schedule: Schedule::default(), title: None, group: None,
    });
    entry.schedule = schedule.clone();
    let enabled = entry.enabled;
    state::save(&app.root, &st).map_err(e)?;
    if enabled { launchd::install(&app.root, &file, &schedule).map_err(e)? }
    Ok(())
}

#[tauri::command]
fn set_timeout(app: tauri::State<'_, App>, file: String, secs: u32) -> R<()> {
    jobs::set_timeout(&paths::jobs_dir(&app.root).join(&file), secs).map_err(e)
}

#[tauri::command]
fn set_meta(app: tauri::State<'_, App>, file: String, title: Option<String>, group: Option<String>) -> R<()> {
    let mut st = state::load(&app.root);
    let entry = st.jobs.entry(file).or_insert_with(|| state::JobState {
        enabled: true, schedule: Schedule::default(), title: None, group: None,
    });
    if title.is_some() { entry.title = title; }
    if group.is_some() { entry.group = group; }
    state::save(&app.root, &st).map_err(e)
}

#[tauri::command]
fn create_job(app: tauri::State<'_, App>, spec: NewJob) -> R<String> {
    let file = jobs::create(&app.root, &spec).map_err(e)?;
    let diags = jobs::validate(&paths::jobs_dir(&app.root).join(&file)).map_err(e)?;
    if !diags.is_empty() {
        let _ = jobs::delete(&app.root, &file, false);
        return Err(format!("generated script failed validation:\n{}", diags.join("\n")));
    }
    let mut st = state::load(&app.root);
    st.jobs.insert(file.clone(), state::JobState {
        enabled: false, // scheduled only after a clean dry run
        schedule: spec.schedule.clone(),
        title: Some(spec.title.clone()),
        group: Some(spec.group.clone()),
    });
    state::save(&app.root, &st).map_err(e)?;
    Ok(file)
}

#[tauri::command]
fn delete_job(app: tauri::State<'_, App>, file: String, keep_script: bool) -> R<()> {
    launchd::uninstall(&file).map_err(e)?;
    let mut st = state::load(&app.root);
    st.jobs.remove(&file);
    state::save(&app.root, &st).map_err(e)?;
    jobs::delete(&app.root, &file, keep_script).map_err(e)
}

/// Run now, in-process (not via launchd) so the UI can stream the outcome.
/// The run is recorded best-effort: a database failure must never be
/// reported as a job failure.
#[tauri::command]
fn run_job(app: tauri::State<'_, App>, file: String, dry: bool) -> R<RunOutcome> {
    let started = chrono::Utc::now().timestamp();
    let out = jobs::run(&app.root, &file, dry).map_err(e)?;
    let _ = app.db.record_run(&types::NewRun {
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
}

#[tauri::command]
fn runs_for(app: tauri::State<'_, App>, file: String, limit: i64) -> R<Vec<Run>> {
    app.db.runs_for(&file, limit).map_err(e)
}

#[tauri::command]
fn fleet_history(app: tauri::State<'_, App>, days: i64) -> R<Vec<DayStat>> {
    let st = state::load(&app.root);
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
    jobs::validate(&paths::jobs_dir(&app.root).join(&file)).map_err(e)
}

#[tauri::command]
fn prune_runs(app: tauri::State<'_, App>, keep_days: i64) -> R<usize> {
    app.db.prune(keep_days).map_err(e)
}

#[tauri::command]
fn reindex(app: tauri::State<'_, App>) -> R<usize> {
    app.db.ingest_log(&paths::run_log(&app.root)).map_err(e)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let root = paths::repo_root().expect("machine-ops repo not found");
    paths::ensure_dirs(&root).expect("cannot create state/ and logs/");
    let db = Db::open(&paths::db_file(&root)).expect("cannot open console.db");
    // Best-effort backfill so a fresh database is not an empty console.
    let _ = db.ingest_log(&paths::run_log(&root));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(App { root, db })
        .invoke_handler(tauri::generate_handler![
            list_jobs, set_enabled, set_schedule, set_timeout, set_meta,
            create_job, delete_job, run_job, runs_for, fleet_history,
            search_logs, load_profile, validate_job, prune_runs, reindex
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
