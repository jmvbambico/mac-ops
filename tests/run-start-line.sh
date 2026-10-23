#!/usr/bin/env bash
#
# tests/run-start-line.sh — the runner logs a job's REAL start epoch.
#
# The console's text-log parser reconstructs runs from machine-ops.log. Both
# writers now emit `started=<epoch>` on the job's start line:
#
#   [run] job <file> started=<epoch>          (bin/run dispatcher)
#   [run-job] start job=<file> … started=<epoch>  (bin/run-job wrapper)
#
# This test proves the dispatcher logs it, with an epoch that really is the
# run's start (inside the wall-clock window of the sweep), and that the
# existing `[run] job <file> rc=<n>` completion line is still emitted exactly
# as before. stdout must stay empty (Principle I). Hermetic: HOME and
# MOPS_AUTOMATIONS_ROOT point into a mktemp -d a trap removes, so the real
# ~/.mops is never read or written.
#
# Run standalone, or via tests/silent-contract.sh (the gate).

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mops-start-line.XXXXXX")" || {
    printf 'run-start-line: cannot create temp dir\n' >&2
    exit 1
}
cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT

HOME_DIR="$WORK/home"
ROOT="$WORK/root"
mkdir -p "$HOME_DIR" "$ROOT/jobs"

fail() {
    printf 'run-start-line: FAIL: %s\n' "$*" >&2
    exit 1
}

for spec in "10-alpha:alpha" "20-beta:beta"; do
    name="${spec%%:*}"
    label="${spec##*:}"
    cat > "$ROOT/jobs/$name.sh" <<JOB
#!/usr/bin/env bash
# tier: daily
source "\$MOPS_TOOL_ROOT/lib/common.sh"
log_line "[$label] inspected=1 rc=0 dry_run=0"
exit 0
JOB
done

# run_in_window <runner-args...> — run a runner, capturing stdout/stderr and
# the wall-clock window around the call, so a logged epoch can be checked
# against the time the run actually occupied.
RUN_OUT="$WORK/run.out"
RUN_ERR="$WORK/run.err"
RUN_RC=0
WINDOW_BEFORE=0
WINDOW_AFTER=0
run_in_window() {
    RUN_RC=0
    WINDOW_BEFORE="$(date +%s)"
    (
        env -u MOPS_LOG -u MOPS_RUNS \
            MOPS_TOOL_ROOT="$TOOL_ROOT" \
            MOPS_AUTOMATIONS_ROOT="$ROOT" \
            MOPS_DRY_RUN=1 \
            HOME="$HOME_DIR" \
            bash "$@"
    ) >"$RUN_OUT" 2>"$RUN_ERR" || RUN_RC=$?
    WINDOW_AFTER="$(date +%s)"
}

assert_epoch_in_window() {
    local epoch="$1" what="$2"
    [[ "$epoch" =~ ^[0-9]+$ ]] || fail "$what: start epoch is not an integer: $epoch"
    (( epoch >= WINDOW_BEFORE && epoch <= WINDOW_AFTER )) \
        || fail "$what: start epoch $epoch is outside the run window [$WINDOW_BEFORE, $WINDOW_AFTER]"
}

LOG_FILE="$ROOT/logs/machine-ops.log"

# ---------------------------------------------------------------------------
# 1. The dispatcher: one start line per job, with the real epoch, plus the
#    existing completion line unchanged.
# ---------------------------------------------------------------------------
run_in_window "$TOOL_ROOT/bin/run" daily --dry-run
(( RUN_RC == 0 )) || {
    cat "$RUN_ERR" >&2
    fail "bin/run exited $RUN_RC"
}
if [[ -s "$RUN_OUT" ]]; then
    printf 'run-start-line: stdout was NOT empty (%s bytes):\n' \
        "$(wc -c <"$RUN_OUT" | tr -d '[:space:]')" >&2
    cat "$RUN_OUT" >&2
    fail "a nominal sweep printed to stdout (Principle I)"
fi
[[ -f "$LOG_FILE" ]] || fail "the sweep wrote no run log"

for job in 10-alpha.sh 20-beta.sh; do
    starts="$(grep -c -E "^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \[run\] job ${job//./\\.} started=[0-9]+$" "$LOG_FILE" || true)"
    (( starts == 1 )) || fail "$job: expected exactly one start line, found $starts"

    epoch="$(sed -n -E "s/^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \[run\] job ${job//./\\.} started=([0-9]+)$/\1/p" "$LOG_FILE")"
    assert_epoch_in_window "$epoch" "bin/run $job"

    # The completion message predates this change and must still be there.
    completions="$(grep -c -F "[run] job $job rc=0" "$LOG_FILE" || true)"
    (( completions == 1 )) || fail "$job: existing '[run] job ... rc=0' line changed (found $completions)"
done

# The start line must not be mistaken for a diagnostic: its tail is exactly
# one `started=<epoch>` token and it carries no rc= or TIMED OUT text.
if grep -E "\[run\] job .* started=[0-9]+ " "$LOG_FILE" >/dev/null 2>&1; then
    fail "a dispatcher start line carries more than the started token"
fi

# ---------------------------------------------------------------------------
# 2. bin/run-job: the same `started=` shape, same real-epoch guarantee.
# ---------------------------------------------------------------------------
JOB_FILE="$ROOT/jobs/10-alpha.sh"
run_in_window "$TOOL_ROOT/bin/run-job" "$JOB_FILE"
(( RUN_RC == 0 )) || {
    cat "$RUN_ERR" >&2
    fail "bin/run-job exited $RUN_RC"
}
if [[ -s "$RUN_OUT" ]]; then
    fail "bin/run-job's nominal run printed to stdout (Principle I)"
fi

line="$(sed -n -E '/\[run-job\] start job=10-alpha\.sh /p' "$LOG_FILE" | tail -n 1)"
[[ -n "$line" ]] || fail "bin/run-job logged no start line for 10-alpha.sh"
epoch="$(printf '%s\n' "$line" | sed -n -E 's/.* started=([0-9]+)$/\1/p')"
[[ -n "$epoch" ]] || fail "bin/run-job's start line has no trailing started=<epoch>: $line"
assert_epoch_in_window "$epoch" "bin/run-job 10-alpha.sh"
# The wrapper's finish line is untouched.
grep -q -F "[run-job] finish job=10-alpha.sh rc=0 timeout=0" "$LOG_FILE" \
    || fail "bin/run-job's finish line changed"

printf 'run-start-line: ok — bin/run and bin/run-job log the real start epoch as started=<epoch>; completion lines unchanged; stdout silent.\n'
