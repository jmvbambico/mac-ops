//! Live tail of the run log for an on-demand run.
//!
//! The console streams `logs/machine-ops.log` — the durable record (AGENTS.md
//! Principle VI) — rather than the child's stdout. A nominal job prints
//! nothing on stdout, so stdout would show an empty box for exactly the runs
//! the user wants to watch; the job's `log_line` detail is already in the log
//! while it works.
//!
//! The reader is byte-offset based: record the log length before spawning the
//! job, then emit whatever is appended past that offset. It deliberately does
//! NOT filter by job label: labels (`[mole-clean]`) do not match job filenames
//! (`20-mole-clean.sh`), and mapping between them is a trap. A scheduled run
//! overlapping an on-demand run interleaves its lines into the stream; that is
//! acceptable (arguably useful), and no locking is attempted.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// Channel the frontend subscribes to. Keep in sync with `api.ts`.
pub const RUN_LOG_EVENT: &str = "run-log";

/// One incremental batch of complete log lines. `job` pins the batch to the
/// run that produced it so a stale listener cannot render another job's lines.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunLogBatch {
    pub job: String,
    pub lines: Vec<String>,
}

/// A byte offset into the run log. `read_new` returns only complete lines
/// appended since the previous call; a partially written final line is held
/// back until its newline arrives, so a line is never delivered twice.
pub struct LogTail {
    path: PathBuf,
    offset: u64,
}

impl LogTail {
    /// Offset at the current end of the log (0 when it does not exist yet).
    /// Call this BEFORE spawning the job so only that run's lines are new.
    pub fn at_end(path: &Path) -> LogTail {
        let offset = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        LogTail {
            path: path.to_path_buf(),
            offset,
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Complete lines appended since the last read. A missing file, an empty
    /// file, a read error or a partial trailing line all yield `Ok` with no
    /// lines: the stream is best-effort and must never fail the run.
    pub fn read_new(&mut self) -> Vec<String> {
        self.read(false)
    }

    /// The trailing partial line, if any, for the final batch after the job
    /// has exited. Without this a log whose last line lacked a newline would
    /// silently lose that line.
    pub fn flush_partial(&mut self) -> Vec<String> {
        self.read(true)
    }

    fn read(&mut self, include_partial: bool) -> Vec<String> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return Vec::new();
        };

        // `bin/run-job` rotates machine-ops.log to `.1`. If the file is now
        // shorter than our offset it was replaced, and the old offset points
        // past the new file's end: restart from the beginning rather than
        // seeking into nothing and losing the stream for the rest of the run.
        if len < self.offset {
            self.offset = 0;
        }
        if len == self.offset {
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }

        let mut buf = Vec::new();
        if file.take(len - self.offset).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let mut consumed = 0usize;
        let mut start = 0usize;
        for (i, byte) in buf.iter().enumerate() {
            if *byte == b'\n' {
                let mut line = &buf[start..i];
                if line.ends_with(b"\r") {
                    line = &line[..line.len() - 1];
                }
                lines.push(String::from_utf8_lossy(line).into_owned());
                start = i + 1;
                consumed = start;
            }
        }
        if include_partial && start < buf.len() {
            lines.push(String::from_utf8_lossy(&buf[start..]).into_owned());
            consumed = buf.len();
        }

        self.offset += consumed as u64;
        lines
    }
}

fn emit(app: &AppHandle, job: &str, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    // A closed window / vanished listener is not an error: the run and its
    // durable log record are unaffected.
    let _ = app.emit(
        RUN_LOG_EVENT,
        RunLogBatch {
            job: job.to_string(),
            lines,
        },
    );
}

/// Poll `tail` until `done` is set, emitting batches as lines appear. The
/// final partial line is flushed after the stop flag so nothing written just
/// before exit is dropped.
pub fn spawn(
    app: AppHandle,
    job: String,
    mut tail: LogTail,
    done: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        emit(&app, &job, tail.read_new());
        if done.load(Ordering::SeqCst) {
            emit(&app, &job, tail.flush_partial());
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let base = std::env::temp_dir().join(format!(
                "machine-ops-stream-test-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&base).unwrap();
            Tmp(base)
        }
        fn log(&self) -> PathBuf {
            self.0.join("machine-ops.log")
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn returns_only_bytes_appended_after_the_offset() {
        let tmp = Tmp::new("offset");
        append(&tmp.log(), "before one\nbefore two\n");
        let mut tail = LogTail::at_end(&tmp.log());

        append(&tmp.log(), "new one\nnew two\n");
        assert_eq!(tail.read_new(), vec!["new one", "new two"]);
        // Nothing new: a second poll must not repeat the same lines.
        assert!(tail.read_new().is_empty());
    }

    #[test]
    fn a_missing_log_is_empty_not_an_error() {
        let tmp = Tmp::new("missing");
        let mut tail = LogTail::at_end(&tmp.log());
        assert_eq!(tail.offset(), 0);
        assert!(tail.read_new().is_empty());

        // The log appearing later streams from its beginning.
        append(&tmp.log(), "created\n");
        assert_eq!(tail.read_new(), vec!["created"]);
    }

    #[test]
    fn a_partial_final_line_is_held_until_complete() {
        let tmp = Tmp::new("partial");
        append(&tmp.log(), "complete\n");
        let mut tail = LogTail::at_end(&tmp.log());

        append(&tmp.log(), "half a li");
        assert!(
            tail.read_new().is_empty(),
            "a partial line must not surface before its newline"
        );

        append(&tmp.log(), "ne\n");
        assert_eq!(tail.read_new(), vec!["half a line"]);
    }

    #[test]
    fn flush_partial_delivers_a_line_without_a_newline() {
        let tmp = Tmp::new("flush");
        append(&tmp.log(), "first\n");
        let mut tail = LogTail::at_end(&tmp.log());
        append(&tmp.log(), "last line no newline");
        assert!(tail.read_new().is_empty());
        assert_eq!(tail.flush_partial(), vec!["last line no newline"]);
        assert!(tail.flush_partial().is_empty(), "flush must not repeat");
    }

    #[test]
    fn does_not_block_on_an_empty_file() {
        let tmp = Tmp::new("empty");
        std::fs::write(tmp.log(), b"").unwrap();
        let mut tail = LogTail::at_end(&tmp.log());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            tx.send(tail.read_new()).unwrap();
        });
        let got = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("read_new must return on an empty file");
        assert!(got.is_empty());
    }

    #[test]
    fn a_shrunk_log_restarts_from_the_beginning() {
        let tmp = Tmp::new("rotate");
        append(&tmp.log(), "one\ntwo\nthree\n");
        let mut tail = LogTail::at_end(&tmp.log());
        assert!(tail.read_new().is_empty());

        // bin/run-job moved the file aside and a fresh log started shorter
        // than the recorded offset. Read the new file from its start.
        std::fs::write(tmp.log(), "fresh\n").unwrap();
        assert_eq!(tail.read_new(), vec!["fresh"]);
        assert_eq!(tail.offset(), "fresh\n".len() as u64);
    }
}
