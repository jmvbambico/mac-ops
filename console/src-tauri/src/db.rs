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

    /// FTS5 search across summary + stdout, newest first.
    pub fn search(&self, q: &str, limit: i64) -> Result<Vec<Run>> {
        let Some(match_expr) = fts_query(q) else {
            return Ok(Vec::new());
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panicked writer must not take the console down with it: the
        // database is derived, so a poisoned lock is recoverable by design.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
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
             summary    = excluded.summary,
             stdout     = excluded.stdout
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
                    let rec = pending.entry(file).or_default();
                    rec.started_at = Some(l.ts);
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

