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
//!
//! Known limitation: if the log is rotated (replaced) and the new file grows
//! past the old byte offset within a single 120 ms poll, the bytes written in
//! that window are skipped. Narrow, and fixing it properly needs inode/device
//! identity tracking; the durable log still holds everything.

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

    /// The current byte offset. Only the tests read it.
    #[cfg(test)]
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

/// Owns the poller thread: its stop flag and its join handle.
///
/// The guard exists because a leaked poller is a permanent defect. If the
/// job runner panics, unwinding skips any explicit stop-and-join, and the
/// thread would then poll every 120 ms for the life of the process, holding an
/// `AppHandle` and emitting events nobody is listening for. Owning the thread
/// in a guard means the stop happens on the normal path AND on unwind.
///
/// `Drop` sets the flag BEFORE joining: the thread checks the flag after its
/// next poll and flushes the final partial batch on its way out, so the join
/// must not race ahead of that flush.
pub struct StreamGuard {
    done: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl StreamGuard {
    /// Stop the thread and wait for it to finish. Idempotent: a second call
    /// does nothing, and `Drop` calls it again.
    pub fn stop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // `join` returns Err when the thread panicked. Swallow it: a
            // panic inside `Drop` during an unwind aborts the process.
            let _ = handle.join();
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Poll `tail` until the stop flag is set, emitting batches as lines appear.
/// The final partial line is flushed after the stop flag so nothing written
/// just before exit is dropped.
pub fn spawn(app: AppHandle, job: String, tail: LogTail) -> StreamGuard {
    spawn_with(tail, move |lines| emit(&app, &job, lines))
}

/// Spawn a poller with an explicit sink. `spawn` is the production seam (it
/// emits to the frontend); this one exists so the thread's lifetime can be
/// tested without a Tauri runtime.
pub fn spawn_with<F>(mut tail: LogTail, mut sink: F) -> StreamGuard
where
    F: FnMut(Vec<String>) + Send + 'static,
{
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let handle = std::thread::spawn(move || loop {
        sink(tail.read_new());
        if flag.load(Ordering::SeqCst) {
            sink(tail.flush_partial());
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    });
    StreamGuard {
        done,
        handle: Some(handle),
    }
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

    #[test]
    fn dropping_the_guard_stops_the_thread() {
        let tmp = Tmp::new("guard");
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = polls.clone();

        {
            let mut guard = spawn_with(LogTail::at_end(&tmp.log()), move |lines| {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = lines;
            });
            std::thread::sleep(Duration::from_millis(300));
            let before = polls.load(std::sync::atomic::Ordering::SeqCst);
            assert!(before > 0, "the poller must have run before the drop");
            guard.stop();
        }

        let after_stop = polls.load(std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        let later = polls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            after_stop, later,
            "the poller must not keep polling after the guard is dropped"
        );
    }

    #[test]
    fn the_guard_stops_the_thread_while_unwinding() {
        let tmp = Tmp::new("unwind");
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = polls.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Silence the default panic output: this panic is the point of the
            // test, not a failure.
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let _guard = spawn_with(LogTail::at_end(&tmp.log()), move |lines| {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = lines;
            });
            std::panic::set_hook(previous);
            panic!("the job runner panicked");
        }));
        assert!(result.is_err(), "the closure must have unwound");

        let after = polls.load(std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            after,
            polls.load(std::sync::atomic::Ordering::SeqCst),
            "a panic in the run must not leak the poller thread"
        );
    }

    #[test]
    fn the_final_partial_line_is_flushed_when_the_guard_stops() {
        let tmp = Tmp::new("guardflush");
        append(&tmp.log(), "first\n");
        let tail = LogTail::at_end(&tmp.log());
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = got.clone();

        let mut guard = spawn_with(tail, move |lines| {
            if !lines.is_empty() {
                sink.lock().unwrap().extend(lines);
            }
        });
        append(&tmp.log(), "last line no newline");
        std::thread::sleep(Duration::from_millis(200));
        guard.stop();

        let seen = got.lock().unwrap().clone();
        assert!(
            seen.iter().any(|l| l == "last line no newline"),
            "the trailing partial line must be flushed before the join: {seen:?}"
        );
    }
}
