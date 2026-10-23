//! SQLite index over run history. DERIVED DATA ONLY.
//!
//! Design rule that must hold: `logs/machine-ops.log` stays the durable
//! record. Losing or corrupting this database costs queryability, never
//! history — `ingest_log` can rebuild it. Mirrors the fail-safe already in
//! bin/run: a broken state file must never stop maintenance.
use crate::types::{DayStat, NewRun, Run, Status};
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Bumped only when the schema changes shape. The DDL below is all
/// `IF NOT EXISTS`, so `migrate` is safe to run on every open; the version
/// exists to make a future destructive migration detectable.
const SCHEMA_VERSION: i64 = 1;

/// Every column of `runs`, in the order `row_to_run` expects.
const RUN_COLS: &str = "id, job, started_at, duration_s, rc, status, dry_run, summary, stdout";

pub struct Db {
    pub conn: Mutex<Connection>,
}

impl Db {
    /// Open (creating if absent) and apply pragmas:
    /// `journal_mode=WAL` (concurrent readers + one writer — the bash jobs
    /// write here too), `busy_timeout=5000`, `synchronous=NORMAL`,
    /// `foreign_keys=ON`. Then run `migrate`.
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
        }
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        // journal_mode returns a row, so it needs query_row rather than execute.
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .context("cannot set journal_mode=WAL")?;
        debug_assert!(mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory"));
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    /// Idempotent schema creation + a `schema_version` user_version bump.
    /// Tables: `runs` (see types::Run) plus indexes
    /// `runs(job, started_at DESC)` and `runs(status, started_at DESC)`,
    /// and an FTS5 virtual table `runs_fts(summary, stdout)` contentless-
    /// linked to `runs` with triggers to keep it in sync.
    pub fn migrate(&self) -> Result<()> {
        let conn = self.lock();
        // Run unconditionally: the whole schema is `IF NOT EXISTS`, so this
        // also self-heals a database someone dropped a table out of. The
        // derived index must always be rebuildable in place.
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS runs (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                job        TEXT    NOT NULL,
                started_at INTEGER NOT NULL,
                duration_s INTEGER NOT NULL DEFAULT 0,
                rc         INTEGER NOT NULL DEFAULT 0,
                status     TEXT    NOT NULL,
                dry_run    INTEGER NOT NULL DEFAULT 0,
                summary    TEXT    NOT NULL DEFAULT '',
                stdout     TEXT    NOT NULL DEFAULT '',
                -- Identity of a run, and the whole basis of ingest idempotency:
                -- one job cannot start twice in the same second.
                UNIQUE (job, started_at)
            );

            CREATE INDEX IF NOT EXISTS runs_job_started_idx
                ON runs (job, started_at DESC);
            CREATE INDEX IF NOT EXISTS runs_status_started_idx
                ON runs (status, started_at DESC);

            -- External-content FTS5: the text lives in `runs`, the index only
            -- holds terms. Dropping runs_fts costs nothing but search.
            CREATE VIRTUAL TABLE IF NOT EXISTS runs_fts USING fts5(
                summary, stdout, content='runs', content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS runs_fts_ai AFTER INSERT ON runs BEGIN
                INSERT INTO runs_fts(rowid, summary, stdout)
                VALUES (new.id, new.summary, new.stdout);
            END;
            CREATE TRIGGER IF NOT EXISTS runs_fts_ad AFTER DELETE ON runs BEGIN
                INSERT INTO runs_fts(runs_fts, rowid, summary, stdout)
                VALUES ('delete', old.id, old.summary, old.stdout);
            END;
            CREATE TRIGGER IF NOT EXISTS runs_fts_au AFTER UPDATE ON runs BEGIN
                INSERT INTO runs_fts(runs_fts, rowid, summary, stdout)
                VALUES ('delete', old.id, old.summary, old.stdout);
                INSERT INTO runs_fts(rowid, summary, stdout)
                VALUES (new.id, new.summary, new.stdout);
            END;
            "#,
        )
        .context("schema migration failed")?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    pub fn record_run(&self, r: &NewRun) -> Result<i64> {
        let conn = self.lock();
        upsert_run(&conn, r)
    }

    pub fn last_run(&self, job: &str) -> Result<Option<Run>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT {RUN_COLS} FROM runs WHERE job = ?1 ORDER BY started_at DESC, id DESC LIMIT 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([job])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_run(row)?)),
            None => Ok(None),
        }
    }

    pub fn runs_for(&self, job: &str, limit: i64) -> Result<Vec<Run>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT {RUN_COLS} FROM runs WHERE job = ?1 \
             ORDER BY started_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![job, limit.max(0)], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Worst outcome per (group, day) over the last `days` days.
    /// `groups` maps job file -> group name, since grouping lives in
    /// console.json, not in the database.
    pub fn fleet_history(
        &self,
        days: i64,
        groups: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<DayStat>> {
        let days = days.max(1);
        // Whole local calendar days, so the heatmap gets exactly `days`
        // columns rather than a window that cuts today in half.
        let first_day = Local::now().date_naive() - chrono::Duration::days(days - 1);
        let cutoff = day_start_ts(first_day);

        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT job, started_at, status FROM runs WHERE started_at >= ?1")?;
        let rows = stmt.query_map([cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        // (day, group) -> worst status seen.
        let mut worst: BTreeMap<(String, String), Status> = BTreeMap::new();
        for row in rows {
            let (job, started_at, status) = row?;
            let Some(day) = local_day(started_at) else { continue };
            // A job missing from console.json still has history worth
            // showing; mirror list_jobs' own default rather than drop it.
            let group = groups
                .get(&job)
                .cloned()
                .unwrap_or_else(|| "Ungrouped".to_string());
            let st = Status::parse(&status);
            worst
                .entry((day, group))
                .and_modify(|cur| {
                    if severity(st) < severity(*cur) {
                        *cur = st;
                    }
                })
                .or_insert(st);
        }

        Ok(worst
            .into_iter()
            .map(|((day, group), status)| DayStat { day, group, status })
            .collect())
    }

    /// Recent runs, newest first, when there is no search text at all.
    ///
    /// Deliberately reads `runs` rather than FTS: the index only covers
    /// `summary` and `stdout`, and a quiet run has both empty, so no wildcard
    /// expression could ever list one. This is the Logs view's default path.
    fn recent(&self, limit: i64) -> Result<Vec<Run>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT {RUN_COLS} FROM runs ORDER BY started_at DESC, id DESC LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![limit.max(0)], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// FTS5 search across summary + stdout, newest first. An empty or
    /// whitespace-only query lists the most recent runs instead: that is the
    /// Logs view on first load, and it must not render an empty history.
    pub fn search(&self, q: &str, limit: i64) -> Result<Vec<Run>> {
        let Some(match_expr) = fts_query(q) else {
            return self.recent(limit);
        };
        let conn = self.lock();
        // Subquery rather than a join: FTS5's MATCH wants the table name on
        // its left-hand side, and this keeps the ORDER BY on the real table.
        let sql = format!(
            "SELECT {RUN_COLS} FROM runs \
             WHERE id IN (SELECT rowid FROM runs_fts WHERE runs_fts MATCH ?1) \
             ORDER BY started_at DESC, id DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![match_expr, limit.max(0)], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Time-based retention, per job. This is the whole reason for SQLite:
    /// the old 5 MB size-based rotation was global, so one chatty job
    /// destroyed every other job's history.
    pub fn prune(&self, keep_days: i64) -> Result<usize> {
        let keep_days = keep_days.max(0);
        let cutoff = day_start_ts(Local::now().date_naive() - chrono::Duration::days(keep_days));
        let conn = self.lock();
        // Per job, independently: each job keeps its own `keep_days` of
        // history, and its MOST RECENT run is never pruned — a job that runs
        // monthly must not read as "never ran" just because a chatty
        // neighbour triggered retention.
        let n = conn.execute(
            "DELETE FROM runs
              WHERE started_at < ?1
                AND id NOT IN (
                    SELECT id FROM (
                        SELECT id, ROW_NUMBER() OVER (
                            PARTITION BY job ORDER BY started_at DESC, id DESC
                        ) AS rn
                        FROM runs
                    ) WHERE rn = 1
                )",
            [cutoff],
        )?;
        Ok(n)
    }

    /// Rebuild history by parsing logs/machine-ops.log. Must be idempotent
    /// (re-ingesting the same log must not duplicate rows) and must tolerate
    /// malformed lines by skipping them.
    pub fn ingest_log(&self, log: &Path) -> Result<usize> {
        let mut records: Vec<NewRun> = Vec::new();
        // bin/run rotates past ~5 MB keeping one `.1`; read it first so a
        // rebuild recovers the older half of history too. Both files are
        // optional: a missing log means "nothing to index", never an error.
        let rotated = {
            let mut p = log.as_os_str().to_owned();
            p.push(".1");
            std::path::PathBuf::from(p)
        };
        for path in [rotated.as_path(), log] {
            if let Some(text) = read_log(path) {
                parse_log(&text, &mut records);
            }
        }

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut n = 0usize;
        for r in &records {
            // Upsert on (job, started_at): re-ingesting the same log rewrites
            // the same rows instead of growing the table.
            upsert_run(&tx, r)?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Ingest the structured feed `record_run` appends to logs/runs.jsonl.
    /// This is the PRIMARY source: it carries the producer's own status, so
    /// the console never re-derives it from prose. `ingest_log` stays the
    /// rebuild path for history that predates the feed (or a lost feed).
    ///
    /// Tolerant by contract: a missing file is "nothing to index", never an
    /// error; a malformed, half-written or truncated line is skipped and the
    /// remaining lines still land. Idempotent via the same upsert on
    /// (job, started_at) that `ingest_log` uses.
    pub fn ingest_runs_jsonl(&self, runs: &Path) -> Result<usize> {
        let Some(text) = read_log(runs) else {
            return Ok(0);
        };
        let mut records: Vec<NewRun> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<RunRecord>(line) else {
                continue;
            };
            // Without a job key the row cannot be attributed; without it the
            // record is unusable, which is malformed for our purposes.
            if rec.job.trim().is_empty() {
                continue;
            }
            records.push(rec.into_new_run());
        }

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut n = 0usize;
        for r in &records {
            upsert_run(&tx, r)?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panicked writer must not take the console down with it: the
        // database is derived, so a poisoned lock is recoverable by design.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---------------------------------------------------------------------------
// structured feed parsing (logs/runs.jsonl)
// ---------------------------------------------------------------------------

/// One line of the feed, exactly as `record_run` in lib/common.sh emits it:
/// `job`, `started_at`, `duration_s`, `rc`, `status`, `dry_run`, `summary`,
/// `stdout`. Only the two key fields are required so a slightly older or
/// newer producer still yields a row instead of being dropped wholesale.
#[derive(serde::Deserialize)]
struct RunRecord {
    job: String,
    started_at: i64,
    #[serde(default)]
    duration_s: i64,
    #[serde(default)]
    rc: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    stdout: String,
}

impl RunRecord {
    fn into_new_run(self) -> NewRun {
        // The feed carries the producer's own derivation; trust it and only
        // fall back when it is absent or not in the shared vocabulary.
        let rc = i32::try_from(self.rc).unwrap_or(-1);
        let status = self
            .status
            .as_deref()
            .map(Status::parse)
            .filter(|s| *s != Status::None)
            .unwrap_or_else(|| Status::derive(rc, rc == 124, &self.stdout));
        NewRun {
            job: self.job,
            started_at: self.started_at,
            duration_s: self.duration_s,
            rc,
            status,
            dry_run: self.dry_run,
            summary: self.summary,
            stdout: self.stdout,
        }
    }
}

// ---------------------------------------------------------------------------
// row/statement helpers
// ---------------------------------------------------------------------------

fn upsert_run(conn: &Connection, r: &NewRun) -> Result<i64> {
    let id: i64 = conn.query_row(
        "INSERT INTO runs (job, started_at, duration_s, rc, status, dry_run, summary, stdout)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (job, started_at) DO UPDATE SET
             duration_s = excluded.duration_s,
             rc         = excluded.rc,
             status     = excluded.status,
             dry_run    = excluded.dry_run,
             -- The two TEXT columns are the one exception to last-write-wins.
             -- The producer (`record_run` in lib/common.sh) only ever sees
             -- stdout, so a quiet run's feed record legitimately carries
             -- summary='' / stdout='' (Principle I: a nominal run prints
             -- NOTHING). The text log, meanwhile, still holds the job's
             -- `log_line` detail for that same run. Production ingests the
             -- text log first and the feed second, so an unconditional
             -- overwrite let the feed's empty strings CLOBBER the rich
             -- summary the text log had just recovered -- blanking every
             -- quiet run's row in the Logs view. The feed stays authoritative
             -- everywhere it actually carries data: only the empty case
             -- defers to what is already stored.
             --
             -- duration_s, rc, status and dry_run deliberately keep
             -- overwriting unconditionally: they are not text, the feed is
             -- genuinely authoritative for them, and 0 / false are
             -- legitimate values that must be able to win.
             summary    = CASE WHEN excluded.summary <> '' THEN excluded.summary ELSE runs.summary END,
             stdout     = CASE WHEN excluded.stdout  <> '' THEN excluded.stdout  ELSE runs.stdout  END
         RETURNING id",
        rusqlite::params![
            r.job,
            r.started_at,
            r.duration_s,
            r.rc,
            r.status.as_str(),
            r.dry_run as i64,
            r.summary,
            r.stdout,
        ],
        |row| row.get(0),
    )?;
    Ok(id)
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let status: String = row.get(5)?;
    Ok(Run {
        id: row.get(0)?,
        job: row.get(1)?,
        started_at: row.get(2)?,
        duration_s: row.get(3)?,
        rc: row.get(4)?,
        status: Status::parse(&status),
        dry_run: row.get::<_, i64>(6)? != 0,
        summary: row.get(7)?,
        stdout: row.get(8)?,
    })
}

/// Worst (0) to best. `Off`/`None` are not outcomes, so they rank last —
/// a day with a real outcome must never be shown as "unknown".
fn severity(s: Status) -> u8 {
    match s {
        Status::Hang => 0,
        Status::Fail => 1,
        Status::Spoke => 2,
        Status::Quiet => 3,
        Status::Off => 4,
        Status::None => 5,
    }
}

fn day_start_ts(day: NaiveDate) -> i64 {
    day.and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

fn local_day(ts: i64) -> Option<String> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Turn free user text into an FTS5 expression that cannot be a syntax
/// error: every token becomes a quoted string. A UI search box is not a
/// query language, and a stray `"` must not surface as a database error.
fn fts_query(q: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for tok in q.split_whitespace() {
        let cleaned: String = tok
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '=' | ':'))
            .collect();
        if !cleaned.is_empty() {
            parts.push(format!("\"{cleaned}\""));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

// ---------------------------------------------------------------------------
// log parsing
// ---------------------------------------------------------------------------

fn read_log(path: &Path) -> Option<String> {
    // Lossy on purpose: one bad byte must not cost the whole rebuild.
    std::fs::read(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

struct LogLine<'a> {
    ts: i64,
    src: &'a str,
    msg: &'a str,
}

/// `2026-10-20 02:02:14 [brew-update] upgraded=0 held=3 rc=1 dry_run=0`
fn parse_line(line: &str) -> Option<LogLine<'_>> {
    const STAMP: usize = 19; // "YYYY-MM-DD HH:MM:SS"
    if line.len() <= STAMP || !line.is_char_boundary(STAMP) {
        return None;
    }
    let (stamp, rest) = line.split_at(STAMP);
    let ts = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S")
        .ok()
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp())?;
    let rest = rest.strip_prefix(" [")?;
    let (src, msg) = rest.split_once(']')?;
    if src.is_empty() {
        return None;
    }
    Some(LogLine { ts, src, msg: msg.strip_prefix(' ').unwrap_or(msg) })
}

/// A job's own summary line ends in `dry_run=<n>` and carries its own final
/// `rc=` token. Returns (rc, dry_run).
///
/// The FINAL `rc=` token is what counts: `cleanup=failed(rc=1) ... rc=0` is a
/// clean run with a log-only cleanup note, not a failure. An inner `(rc=1)`
/// never starts a token, so it cannot be mistaken for the job's rc.
fn summary_rc(msg: &str) -> Option<(i32, bool)> {
    let last = msg.split_whitespace().next_back()?;
    let dry: i32 = last.strip_prefix("dry_run=")?.parse().ok()?;
    let rc = msg
        .split_whitespace()
        .filter_map(|t| t.strip_prefix("rc="))
        .filter_map(|v| v.parse::<i32>().ok())
        .next_back()?;
    Some((rc, dry != 0))
}

/// `30-brew-update.sh` -> `brew-update` (the label jobs log under).
fn label_of(file: &str) -> &str {
    let base = file.strip_suffix(".sh").unwrap_or(file);
    match base.split_once('-') {
        Some((pre, rest)) if !pre.is_empty() && pre.bytes().all(|b| b.is_ascii_digit()) => rest,
        _ => base,
    }
}

/// Facts gathered for one job within one dispatcher window.
#[derive(Default)]
struct Rec {
    started_at: Option<i64>,
    end: i64,
    /// From the job's own summary line — authoritative.
    rc_own: Option<i32>,
    /// From the dispatcher / wrapper completion line.
    rc_outer: Option<i32>,
    timed_out: bool,
    duration: Option<i64>,
    dry_run: Option<bool>,
    summary: String,
    stdout: Vec<String>,
}

impl Rec {
    fn finish(self, job: String, window_dry_run: Option<bool>) -> Option<NewRun> {
        // Evidence of an actual run, or nothing to record.
        if self.rc_own.is_none()
            && self.rc_outer.is_none()
            && !self.timed_out
            && self.stdout.is_empty()
        {
            return None;
        }
        let rc = self.rc_own.or(self.rc_outer).unwrap_or(0);
        let timed_out = self.timed_out || self.rc_outer == Some(124) || rc == 124;
        let stdout = self.stdout.join("\n");
        let started_at = self.started_at.unwrap_or(self.end);
        let duration = self
            .duration
            .unwrap_or_else(|| (self.end - started_at).max(0));
        let summary = if self.summary.is_empty() {
            stdout.lines().next().unwrap_or("").to_string()
        } else {
            self.summary
        };
        Some(NewRun {
            job,
            started_at,
            duration_s: duration,
            rc,
            status: Status::derive(rc, timed_out, &stdout),
            dry_run: self.dry_run.or(window_dry_run).unwrap_or(false),
            summary,
            stdout,
        })
    }
}

/// Parse a whole log into runs. Malformed lines are skipped, never fatal.
fn parse_log(text: &str, out: &mut Vec<NewRun>) {
    // Pass 1: learn label -> job-file, so a job's own `[brew-update]` lines
    // can be attributed to `30-brew-update.sh` (the key lib.rs queries by).
    let mut label_to_file: BTreeMap<String, String> = BTreeMap::new();
    let mut note = |file: &str| {
        if file.ends_with(".sh") {
            label_to_file.insert(label_of(file).to_string(), file.to_string());
        }
    };
    for line in text.lines() {
        let Some(l) = parse_line(line) else { continue };
        if let Some(file) = l.src.strip_suffix(" stdout").or(l.src.strip_suffix(" stderr")) {
            note(file);
        } else if l.src == "run" {
            if let Some(rest) = l.msg.strip_prefix("job ") {
                note(rest.split_whitespace().next().unwrap_or(""));
            }
        } else if l.src == "run-job" {
            if let Some(file) = l
                .msg
                .split_whitespace()
                .find_map(|t| t.strip_prefix("job="))
            {
                note(file);
            }
        }
    }
    let resolve = |label: &str| -> String {
        label_to_file
            .get(label)
            .cloned()
            .unwrap_or_else(|| label.to_string())
    };

    // Pass 2: walk the log in order, accumulating one Rec per job per window.
    let mut pending: BTreeMap<String, Rec> = BTreeMap::new();
    // Start boundary of the next job to complete: the dispatcher logs
    // `start`, then one line per job completion, so the previous completion
    // is the next job's start. That yields real wall-clock durations.
    let mut cursor: Option<i64> = None;
    let mut window_dry_run: Option<bool> = None;

    macro_rules! flush_all {
        () => {
            for (job, rec) in std::mem::take(&mut pending) {
                if let Some(r) = rec.finish(job, window_dry_run) {
                    out.push(r);
                }
            }
        };
    }

    for line in text.lines() {
        let Some(l) = parse_line(line) else { continue };

        // --- captured job output: the alert channel ---------------------
        if let Some(file) = l.src.strip_suffix(" stdout") {
            let rec = pending.entry(file.to_string()).or_default();
            rec.stdout.push(l.msg.to_string());
            rec.end = rec.end.max(l.ts);
            continue;
        }
        if l.src.strip_suffix(" stderr").is_some() {
            // stderr is log-only detail; a clean run that only wrote stderr
            // stays Quiet, exactly as bin/run-job decides it.
            continue;
        }

        match l.src {
            // --- tier dispatcher (bin/run) ------------------------------
            "run" => {
                if let Some(rest) = l.msg.strip_prefix("start ") {
                    flush_all!();
                    cursor = Some(l.ts);
                    window_dry_run = rest
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix("dry_run="))
                        .and_then(|v| v.parse::<i32>().ok())
                        .map(|v| v != 0);
                } else if l.msg.starts_with("finish ") {
                    flush_all!();
                    cursor = None;
                } else if let Some(rest) = l.msg.strip_prefix("job ") {
                    let mut it = rest.splitn(2, ' ');
                    let file = it.next().unwrap_or("");
                    let tail = it.next().unwrap_or("");
                    if file.is_empty() {
                        continue;
                    }
                    if let Some(start) = tail
                        .strip_prefix("started=")
                        .and_then(|v| v.split_whitespace().next())
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        // The dispatcher logged the job's REAL start epoch.
                        // Prefer it over the cursor: the cursor is the
                        // previous job's completion, which is a second off
                        // when the hand-off crosses a boundary. The cursor
                        // path below stays as the fallback for historical
                        // logs that predate this line (and for a run whose
                        // writer never logged one).
                        let rec = pending.entry(file.to_string()).or_default();
                        rec.started_at = Some(start);
                        continue;
                    }
                    if let Some(rc) = tail.strip_prefix("rc=").and_then(|v| v.trim().parse().ok()) {
                        // Job completion: closes its slice of the window.
                        let rec = pending.entry(file.to_string()).or_default();
                        rec.rc_outer = Some(rc);
                        if rec.started_at.is_none() {
                            rec.started_at = cursor;
                        }
                        rec.end = rec.end.max(l.ts);
                        cursor = Some(l.ts);
                    } else if tail.starts_with("TIMED OUT") {
                        let rec = pending.entry(file.to_string()).or_default();
                        rec.timed_out = true;
                        if rec.started_at.is_none() {
                            rec.started_at = cursor;
                        }
                        rec.end = rec.end.max(l.ts);
                    }
                    // Everything else under `[run] job ...` (tier notes,
                    // stuck-tree dumps, kill bookkeeping) is diagnostics.
                }
            }

            // --- single-job wrapper (bin/run-job) -----------------------
            "run-job" => {
                let file = l
                    .msg
                    .split_whitespace()
                    .find_map(|t| t.strip_prefix("job="))
                    .unwrap_or("")
                    .to_string();
                if file.is_empty() {
                    continue;
                }
                if l.msg.starts_with("start ") {
                    if let Some(rec) = pending.remove(&file) {
                        if let Some(r) = rec.finish(file.clone(), window_dry_run) {
                            out.push(r);
                        }
                    }
                    // Prefer the explicit start epoch (the same `started=`
                    // shape bin/run logs); fall back to the line timestamp
                    // only for logs written before it existed.
                    let started = l
                        .msg
                        .split_whitespace()
                        .find_map(|t| t.strip_prefix("started="))
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(l.ts);
                    let rec = pending.entry(file).or_default();
                    rec.started_at = Some(started);
                    rec.end = l.ts;
                } else if l.msg.starts_with("finish ") {
                    let mut rec = pending.remove(&file).unwrap_or_default();
                    for tok in l.msg.split_whitespace() {
                        if let Some(v) = tok.strip_prefix("rc=") {
                            rec.rc_outer = v.parse().ok();
                        } else if let Some(v) = tok.strip_prefix("timeout=") {
                            // `timeout=<0|1>` here, not a duration.
                            rec.timed_out |= v == "1";
                        } else if let Some(v) = tok.strip_prefix("elapsed=") {
                            rec.duration = v.trim_end_matches('s').parse().ok();
                        }
                    }
                    rec.end = rec.end.max(l.ts);
                    if rec.started_at.is_none() {
                        rec.started_at = Some(l.ts - rec.duration.unwrap_or(0));
                    }
                    if let Some(r) = rec.finish(file, window_dry_run) {
                        out.push(r);
                    }
                }
            }

            // --- a job's own log_line, keyed by its label ---------------
            label if !label.contains(char::is_whitespace) => {
                let Some((rc, dry_run)) = summary_rc(l.msg) else { continue };
                let job = resolve(label);
                let rec = pending.entry(job).or_default();
                rec.rc_own = Some(rc);
                rec.dry_run = Some(dry_run);
                rec.summary = l.msg.to_string();
                if rec.started_at.is_none() {
                    rec.started_at = cursor;
                }
                rec.end = rec.end.max(l.ts);
            }

            // Unknown bracketed source with spaces in it: not ours.
            _ => {}
        }
    }
    flush_all!();
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// A unique scratch directory per test; the database is derived data, so
    /// nothing here needs to survive.
    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!(
                "machine-ops-db-test-{}-{}-{}",
                std::process::id(),
                tag,
                n
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tmp(dir)
        }
        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
        fn db(&self) -> Db {
            Db::open(&self.path("console.db")).unwrap()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn new_run(job: &str, started_at: i64, rc: i32, stdout: &str) -> NewRun {
        NewRun {
            job: job.into(),
            started_at,
            duration_s: 7,
            rc,
            status: Status::derive(rc, false, stdout),
            dry_run: false,
            summary: stdout.lines().next().unwrap_or("").to_string(),
            stdout: stdout.into(),
        }
    }

    fn count(db: &Db) -> i64 {
        db.lock()
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap()
    }

    /// The real log, trimmed to the shapes that matter.
    const SAMPLE_LOG: &str = "\
2026-10-20 02:00:06 [run] start tier=daily dry_run=0
2026-10-20 02:00:23 [mole-optimize] score 94->94 disk 63G->63G applied=3 rc=0 dry_run=0
2026-10-20 02:00:24 [run] job 10-mole-optimize.sh rc=0
2026-10-20 02:01:55 [mole-clean] reclaimed=unknown items=14 items rc=0 dry_run=0
2026-10-20 02:01:56 [run] job 20-mole-clean.sh rc=0
2026-10-20 02:02:02 [brew-update] held (outdated but never auto-upgraded): mole node tor
2026-10-20 02:02:13 [brew-update] brew cleanup failed rc=1 (log-only, not an alert)
2026-10-20 02:02:14 [brew-update] upgraded=4 held=3 cleanup=failed(rc=1) rc=0 dry_run=0
2026-10-20 02:02:14 [run] job 30-brew-update.sh rc=0
2026-10-20 02:02:14 [30-brew-update.sh stdout] == brew-update ==
2026-10-20 02:02:14 [30-brew-update.sh stdout] gitea 28.1.0 -> 28.2.0
2026-10-20 02:02:20 [run] job 90-probe-timeout.sh TIMED OUT after 5s (pgid 47623)
2026-10-20 02:02:22 [run] job 90-probe-timeout.sh rc=124
2026-10-20 02:02:22 [skills-update] skills=9 updated=none rc=0 dry_run=0
2026-10-20 02:02:22 [run] job 40-skills-update.sh rc=0
2026-10-20 02:02:22 [run] finish tier=daily jobs=5 speaking=1
this line is not a log line at all
2026-13-45 99:99:99 [run] start tier=daily dry_run=0
2026-10-20 03:00:00 [bogus
2026-10-21 05:00:00 [run-job] start job=50-solo.sh tier=daily timeout=900s
2026-10-21 05:00:04 [50-solo.sh stderr] noise nobody should see
2026-10-21 05:00:05 [run-job] finish job=50-solo.sh rc=3 timeout=0 tier=daily elapsed=5s
";

    #[test]
    fn migrate_is_idempotent() {
        let t = Tmp::new("migrate");
        let db = t.db(); // open() already migrates once
        for _ in 0..3 {
            db.migrate().unwrap();
        }
        let conn = db.lock();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);

        let objs: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for want in [
            "runs",
            "runs_job_started_idx",
            "runs_status_started_idx",
            "runs_fts",
            "runs_fts_ai",
            "runs_fts_ad",
            "runs_fts_au",
        ] {
            assert!(objs.contains(&want.to_string()), "missing {want} in {objs:?}");
        }
        // Exactly one `runs` table, not three stacked migrations' worth.
        assert_eq!(objs.iter().filter(|o| *o == "runs").count(), 1);
        drop(conn);

        // And a reopen on top of an existing file still works.
        drop(db);
        let db2 = t.db();
        db2.migrate().unwrap();
        assert_eq!(count(&db2), 0);
    }

    #[test]
    fn record_and_last_run_round_trip() {
        let t = Tmp::new("record");
        let db = t.db();
        assert!(db.last_run("10-mole-optimize.sh").unwrap().is_none());

        let r = NewRun {
            job: "10-mole-optimize.sh".into(),
            started_at: 1_700_000_000,
            duration_s: 18,
            rc: 0,
            status: Status::Quiet,
            dry_run: true,
            summary: "score 94->94".into(),
            stdout: String::new(),
        };
        let id = db.record_run(&r).unwrap();
        assert!(id > 0);

        let got = db.last_run("10-mole-optimize.sh").unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.job, r.job);
        assert_eq!(got.started_at, r.started_at);
        assert_eq!(got.duration_s, 18);
        assert_eq!(got.rc, 0);
        assert_eq!(got.status, Status::Quiet);
        assert!(got.dry_run);
        assert_eq!(got.summary, "score 94->94");
        assert_eq!(got.stdout, "");

        // Newer run wins, and a second job does not leak into the first.
        db.record_run(&new_run("10-mole-optimize.sh", 1_700_000_900, 1, "boom"))
            .unwrap();
        db.record_run(&new_run("20-mole-clean.sh", 1_700_001_000, 0, ""))
            .unwrap();
        let got = db.last_run("10-mole-optimize.sh").unwrap().unwrap();
        assert_eq!(got.started_at, 1_700_000_900);
        assert_eq!(got.status, Status::Fail);

        let hist = db.runs_for("10-mole-optimize.sh", 10).unwrap();
        assert_eq!(hist.len(), 2);
        assert!(hist[0].started_at > hist[1].started_at);
        assert_eq!(db.runs_for("10-mole-optimize.sh", 1).unwrap().len(), 1);

        // Same (job, started_at) updates in place rather than duplicating.
        let again = db.record_run(&new_run("20-mole-clean.sh", 1_700_001_000, 2, "")).unwrap();
        assert_eq!(count(&db), 3);
        assert_eq!(db.last_run("20-mole-clean.sh").unwrap().unwrap().rc, 2);
        assert!(again > 0);
    }

    #[test]
    fn ingest_log_is_idempotent_and_skips_malformed() {
        let t = Tmp::new("ingest");
        let log = t.path("machine-ops.log");
        std::fs::write(&log, SAMPLE_LOG).unwrap();
        let db = t.db();

        let n1 = db.ingest_log(&log).unwrap();
        let rows1 = count(&db);
        assert_eq!(n1 as i64, rows1, "every parsed record should be one row");
        assert_eq!(rows1, 6, "5 dispatcher jobs + 1 run-job wrapper run");

        // Re-ingesting the same log must not duplicate anything.
        let n2 = db.ingest_log(&log).unwrap();
        assert_eq!(n2, n1);
        assert_eq!(count(&db), rows1);
        // A third pass, and a pass on a fresh database, agree too.
        db.ingest_log(&log).unwrap();
        assert_eq!(count(&db), rows1);

        // A missing log is "nothing to index", not an error.
        assert_eq!(db.ingest_log(&t.path("nope.log")).unwrap(), 0);

        // Quiet run with a real duration (02:00:06 -> 02:00:24).
        let opt = db.last_run("10-mole-optimize.sh").unwrap().unwrap();
        assert_eq!(opt.status, Status::Quiet);
        assert_eq!(opt.rc, 0);
        assert_eq!(opt.duration_s, 18);
        assert!(!opt.dry_run);
        assert!(opt.summary.contains("score 94->94"));

        // THE rc rule: an inner cleanup=failed(rc=1) on a line that ends
        // rc=0 is log-only detail. The job spoke, so it is Spoke, not Fail.
        let brew = db.last_run("30-brew-update.sh").unwrap().unwrap();
        assert_eq!(brew.rc, 0, "inner (rc=1) must not become the job's rc");
        assert_eq!(brew.status, Status::Spoke);
        assert!(brew.stdout.contains("== brew-update =="));
        assert!(brew.stdout.contains("gitea 28.1.0 -> 28.2.0"));
        assert!(brew.summary.ends_with("dry_run=0"));

        // Timeout -> Hang, via rc=124 and the TIMED OUT line.
        let hang = db.last_run("90-probe-timeout.sh").unwrap().unwrap();
        assert_eq!(hang.status, Status::Hang);
        assert_eq!(hang.rc, 124);

        // bin/run-job wrapper: rc=3 -> Fail, elapsed=5s honoured, and a
        // stderr-only run is NOT treated as having spoken.
        let solo = db.last_run("50-solo.sh").unwrap().unwrap();
        assert_eq!(solo.rc, 3);
        assert_eq!(solo.status, Status::Fail);
        assert_eq!(solo.duration_s, 5);
        assert_eq!(solo.stdout, "");

        // The malformed lines produced no phantom jobs.
        assert!(db.last_run("bogus").unwrap().is_none());
        assert!(db.last_run("this").unwrap().is_none());
    }

    #[test]
    fn ingest_log_reads_the_rotated_sibling() {
        let t = Tmp::new("rotated");
        let log = t.path("machine-ops.log");
        std::fs::write(
            t.path("machine-ops.log.1"),
            "2026-10-01 02:00:00 [run] start tier=daily dry_run=0\n\
             2026-10-01 02:00:10 [mole-clean] reclaimed=1MB rc=0 dry_run=0\n\
             2026-10-01 02:00:11 [run] job 20-mole-clean.sh rc=0\n\
             2026-10-01 02:00:11 [run] finish tier=daily jobs=1 speaking=0\n",
        )
        .unwrap();
        std::fs::write(&log, SAMPLE_LOG).unwrap();
        let db = t.db();
        let n = db.ingest_log(&log).unwrap();
        assert_eq!(n, 7);
        assert_eq!(db.runs_for("20-mole-clean.sh", 10).unwrap().len(), 2);
    }

    /// Regression for the duplicate-run / blank-summary defect. The
    /// dispatcher passes the job's true start epoch to the feed and now logs
    /// it too; the parser must prefer that epoch over the cursor (the
    /// previous job's completion), which is off by a second whenever the
    /// hand-off crosses a boundary and made the two writers key different
    /// rows. Derived from the completion line so it is timezone-independent.
    #[test]
    fn a_logged_dispatcher_start_beats_the_cursor_guess() {
        const JOB: &str = "40-skills-update.sh";
        let t = Tmp::new("start-line");
        let log = t.path("machine-ops.log");
        // Previous job completed at 02:03:07. This job really started one
        // second later (02:03:08) and completed at 02:03:21.
        let completion = "2026-10-21 02:03:21 [run] job 40-skills-update.sh rc=0";
        let end = parse_line(completion).unwrap().ts;
        let started = end - 13;
        std::fs::write(
            &log,
            format!(
                "2026-10-21 02:03:07 [run] start tier=daily dry_run=0\n\
                 2026-10-21 02:03:07 [brew-update] upgraded=9 held=0 rc=0 dry_run=0\n\
                 2026-10-21 02:03:07 [run] job 30-brew-update.sh rc=0\n\
                 2026-10-21 02:03:08 [run] job 40-skills-update.sh started={started}\n\
                 2026-10-21 02:03:20 [skills-update] skills=9 updated=none rc=0 dry_run=0\n\
                 {completion}\n\
                 2026-10-21 02:03:21 [run] finish tier=daily jobs=2 speaking=0\n"
            ),
        )
        .unwrap();
        let db = t.db();
        db.ingest_log(&log).unwrap();

        let runs = db.runs_for(JOB, 10).unwrap();
        assert_eq!(runs.len(), 1, "the start line must not add a second record");
        let run = &runs[0];
        assert_eq!(run.started_at, started, "the logged start must beat the cursor");
        assert_eq!(
            run.duration_s,
            end - started,
            "the window must run from the true start, not the previous completion"
        );
        assert_eq!(run.summary, "skills=9 updated=none rc=0 dry_run=0");
    }

    /// Historical logs predate the start line. They must keep parsing exactly
    /// as before: with no logged start, the cursor (the previous completion)
    /// is still the fallback. `ingest_log_is_idempotent_and_skips_malformed`
    /// also runs SAMPLE_LOG, which has no start lines, unchanged.
    #[test]
    fn a_log_without_start_lines_keeps_the_cursor_fallback() {
        let t = Tmp::new("no-start-line");
        let log = t.path("machine-ops.log");
        std::fs::write(&log, SAMPLE_LOG).unwrap();
        let db = t.db();
        db.ingest_log(&log).unwrap();

        let fallback = parse_line("2026-10-20 02:00:24 [run] job 10-mole-optimize.sh rc=0")
            .unwrap()
            .ts;
        let clean = db.last_run("20-mole-clean.sh").unwrap().unwrap();
        assert_eq!(
            clean.started_at, fallback,
            "with no start line the previous completion is still the start"
        );
        assert_eq!(clean.duration_s, 92, "02:00:24 -> 02:01:56");
    }

    /// With the start line, the feed record and the text-log record for one
    /// run share a key, so the upsert produces ONE row: the parser's summary
    /// fills the feed's blank, and the feed keeps rc/status/duration.
    #[test]
    fn a_dispatcher_start_line_keys_the_feed_and_the_log_to_one_row() {
        const JOB: &str = "40-skills-update.sh";
        let t = Tmp::new("start-line-feed");
        let log = t.path("machine-ops.log");
        let feed = t.path("runs.jsonl");
        let completion = "2026-10-21 02:03:21 [run] job 40-skills-update.sh rc=0";
        let end = parse_line(completion).unwrap().ts;
        let started = end - 13;
        std::fs::write(
            &log,
            format!(
                "2026-10-21 02:03:07 [run] start tier=daily dry_run=0\n\
                 2026-10-21 02:03:07 [brew-update] upgraded=9 held=0 rc=0 dry_run=0\n\
                 2026-10-21 02:03:07 [run] job 30-brew-update.sh rc=0\n\
                 2026-10-21 02:03:08 [run] job 40-skills-update.sh started={started}\n\
                 2026-10-21 02:03:20 [skills-update] skills=9 updated=none rc=0 dry_run=0\n\
                 {completion}\n\
                 2026-10-21 02:03:21 [run] finish tier=daily jobs=2 speaking=0\n"
            ),
        )
        .unwrap();
        // The feed's record for the same run: quiet, so summary/stdout empty.
        std::fs::write(
            &feed,
            format!(
                "{{\"job\": \"{JOB}\", \"started_at\": {started}, \"duration_s\": 12, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}}\n"
            ),
        )
        .unwrap();

        let db = t.db();
        db.ingest_log(&log).unwrap();
        db.ingest_runs_jsonl(&feed).unwrap();

        let rows = db.runs_for(JOB, 10).unwrap();
        assert_eq!(rows.len(), 1, "feed and log must share the logged-start key");
        let row = &rows[0];
        assert_eq!(row.started_at, started);
        assert_eq!(
            row.summary, "skills=9 updated=none rc=0 dry_run=0",
            "the log's summary must fill the feed's blank"
        );
        assert_eq!(row.duration_s, 12, "the feed stays authoritative for duration");
        assert_eq!(row.rc, 0);
        assert_eq!(row.status, Status::Quiet);
    }

    /// End-to-end: a real bin/run sweep, then the production ingest order
    /// (text log, then feed). The logged start key lands both writers on one
    /// row per job, with the quiet run's summary recovered rather than blank.
    #[test]
    fn a_real_sweep_indexes_one_row_per_job() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root");
        let t = Tmp::new("e2e-sweep");
        let root = t.path("root");
        let home = t.path("home");
        std::fs::create_dir_all(root.join("jobs")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        for (file, label, summary) in [
            ("10-alpha.sh", "alpha", "updated=1"),
            ("20-beta.sh", "beta", "skills=2 updated=none"),
        ] {
            std::fs::write(
                root.join("jobs").join(file),
                format!(
                    "#!/usr/bin/env bash\n# tier: daily\n\
                     source \"$MOPS_TOOL_ROOT/lib/common.sh\"\n\
                     sleep 1\n\
                     log_line \"[{label}] {summary} rc=0 dry_run=0\"\n\
                     exit 0\n"
                ),
            )
            .unwrap();
        }

        let out = std::process::Command::new("/bin/bash")
            .arg(repo.join("bin/run"))
            .arg("daily")
            .arg("--dry-run")
            .env("MOPS_TOOL_ROOT", repo)
            .env("MOPS_AUTOMATIONS_ROOT", &root)
            .env("HOME", &home)
            .env("MOPS_DRY_RUN", "1")
            // Never let an inherited feed/log override point at the real root.
            .env_remove("MOPS_RUNS")
            .env_remove("MOPS_LOG")
            .output()
            .expect("run bin/run");
        assert!(
            out.status.success(),
            "bin/run exited {:?}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.is_empty(),
            "a nominal sweep must print nothing (Principle I), got:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );

        let log = root.join("logs/machine-ops.log");
        let feed = root.join("logs/runs.jsonl");
        let db = t.db();
        db.ingest_log(&log).unwrap();
        db.ingest_runs_jsonl(&feed).unwrap();

        for (file, detail) in [("10-alpha.sh", "updated=1"), ("20-beta.sh", "skills=2")] {
            let hist = db.runs_for(file, 10).unwrap();
            assert_eq!(
                hist.len(),
                1,
                "{file} must have exactly one row after both ingests, got {}",
                hist.len()
            );
            assert!(!hist[0].summary.is_empty(), "{file} summary must not be blank");
            assert!(
                hist[0].summary.contains(detail),
                "{file} summary did not come from its log line: {:?}",
                hist[0].summary
            );
            eprintln!(
                "e2e {file}: started={} duration={}s summary={:?}",
                hist[0].started_at, hist[0].duration_s, hist[0].summary
            );
        }
    }

    /// One line per run, in the exact shape `record_run` in lib/common.sh
    /// writes (measured from the producer, not inferred from the text log).
    const SAMPLE_RUNS_JSONL: &str = "\
{\"job\": \"10-mole-optimize.sh\", \"started_at\": 1700000000, \"duration_s\": 18, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}
{\"job\": \"30-brew-update.sh\", \"started_at\": 1700000010, \"duration_s\": 12, \"rc\": 0, \"status\": \"spoke\", \"dry_run\": false, \"summary\": \"upgraded=4\", \"stdout\": \"== brew-update ==\\ngitea 28.1.0 -> 28.2.0 (caf\\u00e9)\"}
{\"job\": \"90-probe-timeout.sh\", \"started_at\": 1700000020, \"duration_s\": 5, \"rc\": 124, \"status\": \"hang\", \"dry_run\": true, \"summary\": \"timed out\", \"stdout\": \"\"}
{\"job\": \"50-solo.sh\", \"started_at\": 1700000030, \"duration_s\": 5, \"rc\": 3, \"status\": \"fail\", \"dry_run\": false, \"summary\": \"exited 3\", \"stdout\": \"\"}
";

    #[test]
    fn ingest_runs_jsonl_reads_the_producer_record() {
        let t = Tmp::new("jsonl");
        let runs = t.path("runs.jsonl");
        std::fs::write(&runs, SAMPLE_RUNS_JSONL).unwrap();
        let db = t.db();

        assert_eq!(db.ingest_runs_jsonl(&runs).unwrap(), 4);
        assert_eq!(count(&db), 4);

        // Quiet: the feed's own status wins, no re-derivation from prose.
        let opt = db.last_run("10-mole-optimize.sh").unwrap().unwrap();
        assert_eq!(opt.started_at, 1_700_000_000);
        assert_eq!(opt.duration_s, 18);
        assert_eq!(opt.rc, 0);
        assert_eq!(opt.status, Status::Quiet);
        assert!(!opt.dry_run);
        assert_eq!(opt.summary, "");
        assert_eq!(opt.stdout, "");

        // Spoke, with embedded newline and non-ASCII text preserved.
        let brew = db.last_run("30-brew-update.sh").unwrap().unwrap();
        assert_eq!(brew.status, Status::Spoke);
        assert_eq!(brew.summary, "upgraded=4");
        assert!(brew.stdout.contains("gitea 28.1.0 -> 28.2.0 (café)"));

        // Hang carries dry_run=true and rc=124 straight through.
        let hang = db.last_run("90-probe-timeout.sh").unwrap().unwrap();
        assert_eq!(hang.status, Status::Hang);
        assert_eq!(hang.rc, 124);
        assert!(hang.dry_run);

        let fail = db.last_run("50-solo.sh").unwrap().unwrap();
        assert_eq!(fail.status, Status::Fail);
        assert_eq!(fail.rc, 3);
    }

    #[test]
    fn ingest_runs_jsonl_skips_malformed_and_truncated_lines() {
        let t = Tmp::new("jsonl-bad");
        let runs = t.path("runs.jsonl");
        std::fs::write(
            &runs,
            "\
{\"job\": \"10-mole-optimize.sh\", \"started_at\": 1700000000, \"duration_s\": 18, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}
{ not json at all
[]
{\"job\": \"  \", \"started_at\": 1700000040, \"rc\": 0, \"status\": \"quiet\"}
{\"job\": \"missing-key.sh\", \"duration_s\": 1}
{\"job\": \"30-brew-update.sh\", \"started_at\": 1700000010, \"duration_s\": 12, \"rc\": 0, \"status\": \"spoke\", \"dry_run\": false, \"summary\": \"upgraded=4\", \"stdout\": \"\"}
{\"job\": \"40-skills-update.sh\", \"started_at\": 1700",
        )
        .unwrap();
        let db = t.db();

        // The two complete records land; every bad/truncated line is skipped.
        assert_eq!(db.ingest_runs_jsonl(&runs).unwrap(), 2);
        assert_eq!(count(&db), 2);
        assert!(db.last_run("10-mole-optimize.sh").unwrap().is_some());
        assert!(db.last_run("30-brew-update.sh").unwrap().is_some());
        // No phantom rows from the truncated tail, the empty job, or the
        // record with no key.
        assert!(db.last_run("40-skills-update.sh").unwrap().is_none());
        assert!(db.last_run("missing-key.sh").unwrap().is_none());
        assert!(db.last_run("").unwrap().is_none());
    }

    #[test]
    fn ingest_runs_jsonl_missing_file_is_a_noop() {
        let t = Tmp::new("jsonl-missing");
        let db = t.db();
        // A missing feed means "nothing to index", never an error.
        assert_eq!(db.ingest_runs_jsonl(&t.path("nope.jsonl")).unwrap(), 0);
        assert_eq!(count(&db), 0);
    }

    #[test]
    fn ingest_runs_jsonl_is_idempotent_and_updates_in_place() {
        let t = Tmp::new("jsonl-again");
        let runs = t.path("runs.jsonl");
        std::fs::write(&runs, SAMPLE_RUNS_JSONL).unwrap();
        let db = t.db();

        let n1 = db.ingest_runs_jsonl(&runs).unwrap();
        assert_eq!(n1, 4);
        assert_eq!(count(&db), 4);
        // Re-ingesting the same file must not duplicate anything.
        assert_eq!(db.ingest_runs_jsonl(&runs).unwrap(), n1);
        assert_eq!(count(&db), 4);

        // A rewritten record with the same (job, started_at) updates the row
        // rather than adding one: the keying matches ingest_log's.
        std::fs::write(
            &runs,
            "{\"job\": \"30-brew-update.sh\", \"started_at\": 1700000010, \"duration_s\": 12, \"rc\": 1, \"status\": \"fail\", \"dry_run\": false, \"summary\": \"boom\", \"stdout\": \"\"}\n",
        )
        .unwrap();
        assert_eq!(db.ingest_runs_jsonl(&runs).unwrap(), 1);
        assert_eq!(count(&db), 4);
        let brew = db.last_run("30-brew-update.sh").unwrap().unwrap();
        assert_eq!(brew.rc, 1);
        assert_eq!(brew.status, Status::Fail);
        assert_eq!(brew.summary, "boom");
    }

    /// Guards the `ON CONFLICT (job, started_at) DO UPDATE` clause that the
    /// feed-is-primary contract rests on. Production ingests the text log
    /// first and the feed second, so the feed only wins because the upsert is
    /// genuinely last-write-wins. If that clause ever became
    /// `DO NOTHING` / `OR IGNORE`, every other test would still pass while
    /// the text log silently started winning — the exact inversion this
    /// branch exists to prevent. Hence both orderings are asserted here:
    /// the precedence must come from call order, not from the upsert.
    #[test]
    fn the_feed_wins_over_the_text_log_on_the_same_key() {
        const JOB: &str = "60-precedence.sh";
        let t = Tmp::new("precedence");
        let log = t.path("machine-ops.log");
        let runs = t.path("runs.jsonl");
        std::fs::write(
            &log,
            "2026-10-21 05:00:00 [run-job] start job=60-precedence.sh tier=daily timeout=900s\n\
             2026-10-21 05:00:04 [precedence] log-side summary rc=3 dry_run=0\n\
             2026-10-21 05:00:05 [run-job] finish job=60-precedence.sh rc=3 timeout=0 tier=daily elapsed=5s\n",
        )
        .unwrap();

        // Production order: text log first, feed second.
        let db = t.db();
        db.ingest_log(&log).unwrap();
        // Prove the crafted line actually parsed before relying on the key,
        // so a silently skipped line cannot make this test vacuous.
        let from_log = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(from_log.rc, 3);
        assert_eq!(from_log.status, Status::Fail);
        assert_eq!(from_log.duration_s, 5);
        assert_eq!(from_log.summary, "log-side summary rc=3 dry_run=0");

        // Same (job, started_at) key, deliberately different values.
        std::fs::write(
            &runs,
            format!(
                "{{\"job\": \"{JOB}\", \"started_at\": {}, \"duration_s\": 42, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"feed-side summary\", \"stdout\": \"\"}}\n",
                from_log.started_at
            ),
        )
        .unwrap();
        db.ingest_runs_jsonl(&runs).unwrap();

        let row = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(row.started_at, from_log.started_at, "both records share the key");
        assert_eq!(row.rc, 0, "feed rc must win");
        assert_eq!(row.status, Status::Quiet, "feed status must win");
        assert_eq!(row.duration_s, 42, "feed duration must win");
        assert_eq!(row.summary, "feed-side summary", "feed summary must win");

        // Reverse order in a fresh database: last write wins in either
        // direction, which is what makes the production ordering (not a
        // hard-coded precedence) the thing that keeps the feed primary.
        let t2 = Tmp::new("precedence-reverse");
        let db2 = t2.db();
        let log2 = t2.path("machine-ops.log");
        let runs2 = t2.path("runs.jsonl");
        std::fs::copy(&log, &log2).unwrap();
        std::fs::copy(&runs, &runs2).unwrap();
        db2.ingest_runs_jsonl(&runs2).unwrap();
        db2.ingest_log(&log2).unwrap();

        let row2 = db2.last_run(JOB).unwrap().unwrap();
        assert_eq!(row2.started_at, from_log.started_at);
        assert_eq!(row2.rc, 3, "with reversed order the text log must win");
        assert_eq!(row2.status, Status::Fail);
        assert_eq!(row2.duration_s, 5);
        assert_eq!(row2.summary, "log-side summary rc=3 dry_run=0");
    }

    /// The data-loss regression. `record_run` in lib/common.sh only ever
    /// sees stdout, so a quiet run's feed record legitimately carries
    /// summary='' / stdout='' (Principle I: a nominal run prints NOTHING).
    /// The text log still holds that run's `log_line` detail. Production
    /// ingests the text log first and the feed second, so an unconditional
    /// overwrite let the feed's empty strings blank the Logs view.
    #[test]
    fn an_empty_feed_record_does_not_blank_the_log_detail() {
        const JOB: &str = "60-quiet.sh";
        let t = Tmp::new("empty-feed");
        let log = t.path("machine-ops.log");
        let runs = t.path("runs.jsonl");
        std::fs::write(
            &log,
            "2026-10-21 02:00:22 [run-job] start job=60-quiet.sh tier=daily timeout=900s\n\
             2026-10-21 02:00:30 [quiet] reclaimed=unknown items=13 items rc=0 dry_run=0\n\
             2026-10-21 02:00:31 [run-job] finish job=60-quiet.sh rc=0 timeout=0 tier=daily elapsed=9s\n",
        )
        .unwrap();

        let db = t.db();
        db.ingest_log(&log).unwrap();
        let from_log = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(
            from_log.summary, "reclaimed=unknown items=13 items rc=0 dry_run=0",
            "the crafted log line must really parse, or this test is vacuous"
        );

        // The feed record for the same run: a quiet run, so no stdout.
        std::fs::write(
            &runs,
            format!(
                "{{\"job\": \"{JOB}\", \"started_at\": {}, \"duration_s\": 9, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}}\n",
                from_log.started_at
            ),
        )
        .unwrap();
        db.ingest_runs_jsonl(&runs).unwrap();

        let row = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(row.started_at, from_log.started_at, "both records share the key");
        assert_eq!(
            row.summary, "reclaimed=unknown items=13 items rc=0 dry_run=0",
            "an empty feed summary must not blank the text log's detail"
        );
        assert_eq!(row.stdout, "");
    }

    /// The flip side: the feed stays authoritative wherever it actually
    /// carries data. The empty case is the ONLY exception.
    #[test]
    fn a_feed_record_with_data_still_overwrites_the_log_detail() {
        const JOB: &str = "60-feed-wins.sh";
        let t = Tmp::new("feed-wins");
        let log = t.path("machine-ops.log");
        let runs = t.path("runs.jsonl");
        std::fs::write(
            &log,
            "2026-10-21 03:00:00 [run-job] start job=60-feed-wins.sh tier=daily timeout=900s\n\
             2026-10-21 03:00:04 [feed-wins] log-side summary rc=0 dry_run=0\n\
             2026-10-21 03:00:05 [run-job] finish job=60-feed-wins.sh rc=0 timeout=0 tier=daily elapsed=5s\n",
        )
        .unwrap();

        let db = t.db();
        db.ingest_log(&log).unwrap();
        let from_log = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(from_log.summary, "log-side summary rc=0 dry_run=0");

        std::fs::write(
            &runs,
            format!(
                "{{\"job\": \"{JOB}\", \"started_at\": {}, \"duration_s\": 42, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"feed-side summary\", \"stdout\": \"feed stdout\"}}\n",
                from_log.started_at
            ),
        )
        .unwrap();
        db.ingest_runs_jsonl(&runs).unwrap();

        let row = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(row.summary, "feed-side summary", "feed summary must win");
        assert_eq!(row.stdout, "feed stdout", "feed stdout must win");
    }

    /// The guard is scoped to the two TEXT columns. duration_s, rc, status
    /// and dry_run keep overwriting unconditionally — 0 and false are
    /// legitimate values that must be able to win.
    #[test]
    fn the_empty_feed_guard_does_not_leak_to_the_non_text_columns() {
        const JOB: &str = "60-guard.sh";
        let t = Tmp::new("guard-scope");
        let log = t.path("machine-ops.log");
        let runs = t.path("runs.jsonl");
        std::fs::write(
            &log,
            "2026-10-21 04:00:00 [run-job] start job=60-guard.sh tier=daily timeout=900s\n\
             2026-10-21 04:00:04 [guard] log-side summary rc=3 dry_run=1\n\
             2026-10-21 04:00:05 [run-job] finish job=60-guard.sh rc=3 timeout=0 tier=daily elapsed=5s\n",
        )
        .unwrap();

        let db = t.db();
        db.ingest_log(&log).unwrap();
        let from_log = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(from_log.rc, 3);
        assert_eq!(from_log.status, Status::Fail);
        assert_eq!(from_log.duration_s, 5);
        assert!(from_log.dry_run, "the log says dry_run=1");

        // A clean, non-dry feed record: rc=0 and dry_run=false must be able
        // to overwrite the stored rc=3 / dry_run=true.
        std::fs::write(
            &runs,
            format!(
                "{{\"job\": \"{JOB}\", \"started_at\": {}, \"duration_s\": 42, \"rc\": 0, \"status\": \"quiet\", \"dry_run\": false, \"summary\": \"\", \"stdout\": \"\"}}\n",
                from_log.started_at
            ),
        )
        .unwrap();
        db.ingest_runs_jsonl(&runs).unwrap();

        let row = db.last_run(JOB).unwrap().unwrap();
        assert_eq!(row.rc, 0, "feed rc=0 must overwrite the stored rc=3");
        assert_eq!(row.status, Status::Quiet, "feed status must win");
        assert_eq!(row.duration_s, 42, "feed duration must win");
        assert!(!row.dry_run, "feed dry_run=false must overwrite the stored true");
        // The two TEXT columns are the only ones that defer to what is
        // already stored — see the two tests above.
    }

    #[test]
    fn prune_is_per_job_and_keeps_the_latest() {
        let t = Tmp::new("prune");
        let db = t.db();
        let now = Local::now().timestamp();
        let day = 86_400;

        // A chatty job with lots of recent history...
        for i in 0..20 {
            db.record_run(&new_run("30-brew-update.sh", now - i * 3600, 0, ""))
                .unwrap();
        }
        // ...plus old entries for it,
        for i in 1..5 {
            db.record_run(&new_run("30-brew-update.sh", now - 30 * day - i, 0, ""))
                .unwrap();
        }
        // ...and a quiet monthly job whose only run is ancient.
        db.record_run(&new_run("99-monthly.sh", now - 60 * day, 0, ""))
            .unwrap();
        assert_eq!(count(&db), 25);

        let removed = db.prune(7).unwrap();
        assert_eq!(removed, 4, "only the chatty job's stale rows go");
        assert_eq!(count(&db), 21);

        // The chatty job keeps its own week; the quiet job keeps its last
        // run no matter how old — the old global rotation is exactly what
        // this must not reproduce.
        assert_eq!(db.runs_for("30-brew-update.sh", 100).unwrap().len(), 20);
        let monthly = db.last_run("99-monthly.sh").unwrap().unwrap();
        assert_eq!(monthly.started_at, now - 60 * day);

        // Idempotent: a second prune has nothing left to do.
        assert_eq!(db.prune(7).unwrap(), 0);
        // Even keep_days=0 cannot erase a job's last run.
        db.prune(0).unwrap();
        assert!(db.last_run("99-monthly.sh").unwrap().is_some());
        assert!(db.last_run("30-brew-update.sh").unwrap().is_some());
    }

    /// Regression: the Logs view opens with an empty search box, and the
    /// quiet runs it must show carry `summary=''` and `stdout=''` — text FTS
    /// cannot match, so a wildcard expression would still hide most of them.
    /// An empty/whitespace-only query must list recent rows from `runs`.
    #[test]
    fn an_empty_query_lists_recent_runs_including_silent_ones() {
        let t = Tmp::new("recent");
        let db = t.db();
        db.record_run(&new_run("10-mole-optimize.sh", 1_000, 0, "")).unwrap();
        db.record_run(&new_run("20-mole-clean.sh", 2_000, 0, "")).unwrap();
        db.record_run(&new_run("30-brew-update.sh", 3_000, 0, "spoke up"))
            .unwrap();

        let got = db.search("", 50).unwrap();
        assert_eq!(got.len(), 3, "an empty query must not hide the run history");
        assert_eq!(got[0].started_at, 3_000, "newest first");
        assert_eq!(got[1].started_at, 2_000);
        assert_eq!(got[2].started_at, 1_000);
        // The silent runs are present even though FTS cannot see them.
        assert_eq!(got[1].summary, "");
        assert_eq!(got[1].stdout, "");
        assert_eq!(got[0].summary, "spoke up");

        // Whitespace-only is the same "no query" case.
        assert_eq!(db.search("   ", 50).unwrap().len(), 3);
        // `limit` is honoured, still newest first.
        let two = db.search(" \t ", 2).unwrap();
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].started_at, 3_000);
        assert_eq!(two[1].started_at, 2_000);

        // Same tiebreak as the FTS path: started_at DESC, id DESC.
        db.record_run(&new_run("40-a.sh", 4_000, 0, "")).unwrap();
        db.record_run(&new_run("40-b.sh", 4_000, 0, "")).unwrap();
        let tied = db.search("", 2).unwrap();
        assert_eq!(tied[0].job, "40-b.sh");
        assert_eq!(tied[1].job, "40-a.sh");

        // A real query still filters exactly as before.
        assert_eq!(db.search("spoke", 50).unwrap().len(), 1);
        assert_eq!(db.search("spoke", 50).unwrap()[0].job, "30-brew-update.sh");
    }

    #[test]
    fn fts5_search_finds_summary_and_stdout() {
        let t = Tmp::new("search");
        let db = t.db();
        db.record_run(&new_run("30-brew-update.sh", 1_000, 0, "upgrading gitea to 28.2.0"))
            .unwrap();
        db.record_run(&new_run("10-mole-optimize.sh", 2_000, 0, "health_score dropped"))
            .unwrap();
        db.record_run(&new_run("40-skills-update.sh", 3_000, 0, "gitea mentioned again"))
            .unwrap();

        let hits = db.search("gitea", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].started_at, 3_000, "newest first");
        assert_eq!(hits[1].job, "30-brew-update.sh");

        // Summary column is indexed too, and limit is honoured.
        assert_eq!(db.search("health_score", 10).unwrap().len(), 1);
        assert_eq!(db.search("gitea", 1).unwrap().len(), 1);
        assert!(db.search("nothingmatchesthis", 10).unwrap().is_empty());

        // Hostile input must come back empty, never as a syntax error.
        for q in ["", "   ", "\"", "AND OR *", "a\"b) NEAR("] {
            db.search(q, 10).expect("malformed query must not error");
        }

        // The index tracks updates and deletes through the triggers.
        db.record_run(&new_run("30-brew-update.sh", 1_000, 0, "now about certifi"))
            .unwrap();
        assert_eq!(db.search("gitea", 10).unwrap().len(), 1);
        assert_eq!(db.search("certifi", 10).unwrap().len(), 1);
        db.lock().execute("DELETE FROM runs", []).unwrap();
        assert!(db.search("certifi", 10).unwrap().is_empty());
    }

    #[test]
    fn fleet_history_takes_the_worst_outcome_per_group_and_day() {
        let t = Tmp::new("fleet");
        let db = t.db();
        let noon = Local::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .earliest()
            .unwrap()
            .timestamp();
        let yesterday = noon - 86_400;

        let mut groups = BTreeMap::new();
        groups.insert("10-mole-optimize.sh".to_string(), "Disk".to_string());
        groups.insert("20-mole-clean.sh".to_string(), "Disk".to_string());
        groups.insert("30-brew-update.sh".to_string(), "Packages".to_string());

        // Today, group Disk: quiet + spoke + fail -> Fail wins.
        db.record_run(&new_run("10-mole-optimize.sh", noon, 0, "")).unwrap();
        db.record_run(&new_run("20-mole-clean.sh", noon + 60, 0, "spoke up")).unwrap();
        db.record_run(&new_run("20-mole-clean.sh", noon + 120, 1, "")).unwrap();
        // Today, group Packages: spoke only.
        db.record_run(&new_run("30-brew-update.sh", noon + 30, 0, "said something")).unwrap();
        // Yesterday, group Disk: a hang outranks everything.
        db.record_run(&new_run("10-mole-optimize.sh", yesterday, 0, "")).unwrap();
        db.record_run(&NewRun {
            status: Status::Hang,
            rc: 124,
            ..new_run("20-mole-clean.sh", yesterday + 60, 124, "")
        })
        .unwrap();
        // A job nobody grouped still shows up, as Ungrouped.
        db.record_run(&new_run("99-orphan.sh", noon + 5, 1, "")).unwrap();
        // Outside the window: must not appear at all.
        db.record_run(&new_run("30-brew-update.sh", noon - 40 * 86_400, 124, "")).unwrap();

        let today_s = Local::now().format("%Y-%m-%d").to_string();
        let yday_s = Local
            .timestamp_opt(yesterday, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%d")
            .to_string();

        let stats = db.fleet_history(2, &groups).unwrap();
        let get = |day: &str, group: &str| -> Option<Status> {
            stats
                .iter()
                .find(|s| s.day == day && s.group == group)
                .map(|s| s.status)
        };
        assert_eq!(get(&today_s, "Disk"), Some(Status::Fail));
        assert_eq!(get(&today_s, "Packages"), Some(Status::Spoke));
        assert_eq!(get(&today_s, "Ungrouped"), Some(Status::Fail));
        assert_eq!(get(&yday_s, "Disk"), Some(Status::Hang), "Hang beats Quiet");
        assert_eq!(get(&yday_s, "Packages"), None, "no runs, no cell");
        assert_eq!(stats.len(), 4);

        // The 40-day-old Hang is outside a 2-day window but inside a 60-day
        // one — the window really is applied.
        let wide = db.fleet_history(60, &groups).unwrap();
        assert!(wide.len() > stats.len());
        assert!(wide.iter().any(|s| s.group == "Packages" && s.status == Status::Hang));

        // Severity order is worst -> best, exactly as the heatmap needs.
        assert!(severity(Status::Hang) < severity(Status::Fail));
        assert!(severity(Status::Fail) < severity(Status::Spoke));
        assert!(severity(Status::Spoke) < severity(Status::Quiet));
    }

    #[test]
    fn summary_rc_takes_the_final_token() {
        // The rule that keeps a log-only cleanup note from paging anyone.
        assert_eq!(
            summary_rc("upgraded=4 held=3 cleanup=failed(rc=1) rc=0 dry_run=0"),
            Some((0, false))
        );
        assert_eq!(summary_rc("upgraded=0 held=3 rc=1 dry_run=0"), Some((1, false)));
        assert_eq!(summary_rc("score 94->94 applied=3 rc=0 dry_run=1"), Some((0, true)));
        // Not a summary line: no trailing dry_run= token.
        assert_eq!(summary_rc("brew cleanup failed rc=1 (log-only, not an alert)"), None);
        assert_eq!(summary_rc("held (outdated but never auto-upgraded): mole node"), None);
        assert_eq!(summary_rc(""), None);
        // Label derivation for attributing a job's own lines to its file.
        assert_eq!(label_of("30-brew-update.sh"), "brew-update");
        assert_eq!(label_of("10-mole-optimize.sh"), "mole-optimize");
        assert_eq!(label_of("probe.sh"), "probe");
    }
}

