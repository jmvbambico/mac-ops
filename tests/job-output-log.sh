#!/usr/bin/env bash
#
# tests/job-output-log.sh — the per-job output log contract.
#
# Covers the runner half of "each automation writes its own
# logs/job-<slug>.out.log":
#
#   1. log_output appends to $MOPS_JOB_OUT, strips ANSI escapes, and writes
#      NOTHING to stdout; with MOPS_JOB_OUT unset it is a silent no-op.
#   2. MOPS_JOB_OUT_CAP cuts the output and leaves a marker in the file.
#   3. bin/run-job truncates the file at run start, so a second run does not
#      append to the first run's output, and derives the path from the
#      FILENAME (a trailing .sh stripped) via the shared helper.
#   4. bin/run's sweep gives each job its own file (no cross-job leak) and a
#      second sweep truncates rather than appends.
#
# Hermetic: HOME and MOPS_AUTOMATIONS_ROOT point into a mktemp -d that a trap
# removes, so the real ~/.mops is never read or written. Run standalone or via
# tests/silent-contract.sh (the gate).

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mops-job-out.XXXXXX")" || {
    printf 'job-output-log: cannot create temp dir\n' >&2
    exit 1
}
cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT

HOME_DIR="$WORK/home"
ROOT="$WORK/root"
mkdir -p "$HOME_DIR" "$ROOT"

fail() {
    printf 'job-output-log: FAIL: %s\n' "$*" >&2
    exit 1
}

# run_probe <probe> <stdout> <stderr> <env...> — run a probe with a private
# HOME/automations root and the caller-supplied environment.
PROBE_RC=0
run_probe() {
    local probe="$1" out="$2" err="$3"
    shift 3
    PROBE_RC=0
    (
        env "$@" HOME="$HOME_DIR" MOPS_AUTOMATIONS_ROOT="$ROOT" \
            bash "$probe" "$TOOL_ROOT"
    ) >"$out" 2>"$err" || PROBE_RC=$?
}

# ---------------------------------------------------------------------------
# 1a. log_output writes de-coloured text to the file, silently.
# ---------------------------------------------------------------------------
PLAIN_PROBE="$WORK/plain.sh"
cat > "$PLAIN_PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
text="$(printf '\033[31mred\033[0m and \033[1mbold\033[0m')"
log_output "$text"
exit 0
PROBE

OUT_FILE="$WORK/plain.out.log"
OUT="$WORK/plain.out"
ERR="$WORK/plain.err"
run_probe "$PLAIN_PROBE" "$OUT" "$ERR" "MOPS_JOB_OUT=$OUT_FILE"
(( PROBE_RC == 0 )) || {
    cat "$ERR" >&2
    fail "log_output aborted its caller (exit $PROBE_RC)"
}
if [[ -s "$OUT" ]]; then
    fail "log_output wrote to stdout (Principle I)"
fi
[[ -f "$OUT_FILE" ]] || fail "log_output did not create MOPS_JOB_OUT ($OUT_FILE)"
grep -q -F 'red and bold' "$OUT_FILE" || fail "log_output did not write the text"
if grep -q "$(printf '\033')" "$OUT_FILE"; then
    fail "log_output left ANSI escapes in the file"
fi

# Appending: a second call adds to the first, never replaces it.
APPEND_PROBE="$WORK/append.sh"
cat > "$APPEND_PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
log_output "first"
log_output "second"
exit 0
PROBE
APPEND_FILE="$WORK/append.out.log"
run_probe "$APPEND_PROBE" "$WORK/append.out" "$WORK/append.err" "MOPS_JOB_OUT=$APPEND_FILE"
(( PROBE_RC == 0 )) || fail "repeated log_output calls aborted the caller"
[[ "$(cat "$APPEND_FILE")" == $'first\nsecond' ]] \
    || fail "log_output did not append newline-separated lines across calls"

# Every call ends with exactly one newline: text already ending in one must
# not gain a second, so the file stays one line per call.
NL_PROBE="$WORK/newline.sh"
cat > "$NL_PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
log_output "first"
log_output "second"
third="third"
third="$third"$'\n'
log_output "$third"
exit 0
PROBE
NL_FILE="$WORK/newline.out.log"
printf 'first\nsecond\nthird\n' > "$WORK/newline.expected"
run_probe "$NL_PROBE" "$WORK/newline.out" "$WORK/newline.err" "MOPS_JOB_OUT=$NL_FILE"
(( PROBE_RC == 0 )) || fail "log_output with a trailing-newline text aborted the caller"
cmp -s "$NL_FILE" "$WORK/newline.expected" \
    || fail "log_output did not separate calls with exactly one newline"

# ---------------------------------------------------------------------------
# 1b. With MOPS_JOB_OUT unset, log_output is a silent no-op and creates
#     nothing.
# ---------------------------------------------------------------------------
NOOP_PROBE="$WORK/noop.sh"
cat > "$NOOP_PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
log_output "this must go nowhere"
printf 'SURVIVED\n' >&2
exit 0
PROBE
NOOP_SENTINEL="$WORK/should-not-exist.out.log"
NOOP_OUT="$WORK/noop.out"
NOOP_ERR="$WORK/noop.err"
run_probe "$NOOP_PROBE" "$NOOP_OUT" "$NOOP_ERR" -u MOPS_JOB_OUT
(( PROBE_RC == 0 )) || fail "log_output with no MOPS_JOB_OUT aborted the caller (exit $PROBE_RC)"
if [[ -s "$NOOP_OUT" ]]; then
    fail "log_output with no MOPS_JOB_OUT wrote to stdout"
fi
grep -q -F 'SURVIVED' "$NOOP_ERR" || fail "the calling shell did not survive log_output"
if [[ -e "$NOOP_SENTINEL" ]]; then
    fail "log_output created a file with no MOPS_JOB_OUT"
fi

# ---------------------------------------------------------------------------
# 2. The cap cuts the output and marks the cut inside the file.
# ---------------------------------------------------------------------------
CAP_PROBE="$WORK/cap.sh"
cat > "$CAP_PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
big="$(printf '%500s' '' | tr ' ' 'x')"
log_output "$big"
exit 0
PROBE
CAP_FILE="$WORK/cap.out.log"
run_probe "$CAP_PROBE" "$WORK/cap.out" "$WORK/cap.err" \
    "MOPS_JOB_OUT=$CAP_FILE" "MOPS_JOB_OUT_CAP=100"
(( PROBE_RC == 0 )) || fail "log_output under a cap aborted the caller"
CAP_BYTES="$(wc -c < "$CAP_FILE" | tr -d '[:space:]')"
(( CAP_BYTES <= 100 )) || fail "log_output exceeded the cap ($CAP_BYTES > 100 bytes)"
grep -q -F '[truncated at 100 bytes]' "$CAP_FILE" \
    || fail "a capped log did not say it was truncated"
X_COUNT="$(tr -cd 'x' < "$CAP_FILE" | wc -c | tr -d '[:space:]')"
(( X_COUNT < 500 )) || fail "a capped log kept the whole payload"

# A capped file must not grow on a later call.
CAP_SIZE_BEFORE="$CAP_BYTES"
run_probe "$CAP_PROBE" "$WORK/cap2.out" "$WORK/cap2.err" \
    "MOPS_JOB_OUT=$CAP_FILE" "MOPS_JOB_OUT_CAP=100"
CAP_SIZE_AFTER="$(wc -c < "$CAP_FILE" | tr -d '[:space:]')"
(( CAP_SIZE_AFTER == CAP_SIZE_BEFORE )) || fail "log_output kept growing past the cap"

# ---------------------------------------------------------------------------
# 3. bin/run-job truncates at run start and derives the path from the
#    filename (trailing .sh stripped).
# ---------------------------------------------------------------------------
JOB_DIR="$ROOT/jobs"
mkdir -p "$JOB_DIR"
JOB="$JOB_DIR/10-out.sh"
cat > "$JOB" <<'JOB'
#!/usr/bin/env bash
# tier: daily
source "$MOPS_TOOL_ROOT/lib/common.sh"
log_output "marker=${MOPS_TEST_MARKER:-none}"
exit 0
JOB
chmod +x "$JOB"

EXPECTED="$ROOT/logs/job-10-out.out.log"

# The shell helper is the one path rule bin/run-job calls; the file it
# actually created must match what the helper says for the same filename.
# shellcheck disable=SC2016 # $1/$2/$3 are the child shell's positional args
HELPER_PATH="$(env HOME="$HOME_DIR" MOPS_AUTOMATIONS_ROOT="$ROOT" \
    bash -c 'source "$1/lib/common.sh"; mops_job_out_path "$2" "$3"' \
    _ "$TOOL_ROOT" "$ROOT/logs" "10-out.sh")"
[[ "$HELPER_PATH" == "$EXPECTED" ]] \
    || fail "mops_job_out_path and bin/run-job disagree: $HELPER_PATH vs $EXPECTED"

JOB_OUT="$WORK/job.out"
JOB_ERR="$WORK/job.err"
RUN_JOB_RC=0
run_job() {
    local marker="$1"
    RUN_JOB_RC=0
    env MOPS_TOOL_ROOT="$TOOL_ROOT" \
        MOPS_AUTOMATIONS_ROOT="$ROOT" \
        HOME="$HOME_DIR" \
        MOPS_TEST_MARKER="$marker" \
        bash "$TOOL_ROOT/bin/run-job" "$JOB" >"$JOB_OUT" 2>"$JOB_ERR" \
        || RUN_JOB_RC=$?
}

run_job first
(( RUN_JOB_RC == 0 )) || {
    cat "$JOB_ERR" >&2
    fail "bin/run-job exited $RUN_JOB_RC on the first run"
}
if [[ -s "$JOB_OUT" ]]; then
    fail "bin/run-job's nominal run wrote to stdout"
fi
[[ -f "$EXPECTED" ]] || fail "no per-job log at the path rule: $EXPECTED"
grep -q -F 'marker=first' "$EXPECTED" || fail "the per-job log lacks the first run's output"

run_job second
(( RUN_JOB_RC == 0 )) || {
    cat "$JOB_ERR" >&2
    fail "bin/run-job exited $RUN_JOB_RC on the second run"
}
if [[ -s "$JOB_OUT" ]]; then
    fail "bin/run-job's second nominal run wrote to stdout"
fi
grep -q -F 'marker=second' "$EXPECTED" || fail "the per-job log lacks the second run's output"
if grep -q -F 'marker=first' "$EXPECTED"; then
    fail "bin/run-job did not truncate the per-job log at run start"
fi

# ---------------------------------------------------------------------------
# 4. The scheduled sweep (bin/run) gives EACH job its own file, and a second
#    sweep truncates rather than appends.
# ---------------------------------------------------------------------------
SWEEP_ROOT="$WORK/sweep"
mkdir -p "$SWEEP_ROOT/jobs"
for spec in "10-alpha:alpha" "20-beta:beta"; do
    name="${spec%%:*}"
    word="${spec##*:}"
    cat > "$SWEEP_ROOT/jobs/$name.sh" <<JOB
#!/usr/bin/env bash
# tier: daily
source "\$MOPS_TOOL_ROOT/lib/common.sh"
log_output "$word=\${MOPS_TEST_MARKER:-none}"
exit 0
JOB
    chmod +x "$SWEEP_ROOT/jobs/$name.sh"
done

SWEEP_OUT="$WORK/sweep.out"
SWEEP_ERR="$WORK/sweep.err"
SWEEP_RC=0
run_sweep() {
    local marker="$1"
    SWEEP_RC=0
    env MOPS_TOOL_ROOT="$TOOL_ROOT" \
        MOPS_AUTOMATIONS_ROOT="$SWEEP_ROOT" \
        HOME="$HOME_DIR" \
        MOPS_TEST_MARKER="$marker" \
        bash "$TOOL_ROOT/bin/run" daily --dry-run \
        >"$SWEEP_OUT" 2>"$SWEEP_ERR" || SWEEP_RC=$?
}

ALPHA_OUT="$SWEEP_ROOT/logs/job-10-alpha.out.log"
BETA_OUT="$SWEEP_ROOT/logs/job-20-beta.out.log"

run_sweep one
(( SWEEP_RC == 0 )) || {
    cat "$SWEEP_ERR" >&2
    fail "bin/run exited $SWEEP_RC on the first sweep"
}
if [[ -s "$SWEEP_OUT" ]]; then
    fail "bin/run's nominal sweep wrote to stdout"
fi
[[ -f "$ALPHA_OUT" ]] || fail "the sweep wrote no per-job log for 10-alpha"
[[ -f "$BETA_OUT" ]] || fail "the sweep wrote no per-job log for 20-beta"
grep -q -F 'alpha=one' "$ALPHA_OUT" || fail "10-alpha's log lacks its own output"
grep -q -F 'beta=one' "$BETA_OUT" || fail "20-beta's log lacks its own output"
if grep -q -F 'beta=' "$ALPHA_OUT" || grep -q -F 'alpha=' "$BETA_OUT"; then
    fail "the sweep leaked one job's output into another job's file"
fi

run_sweep two
(( SWEEP_RC == 0 )) || {
    cat "$SWEEP_ERR" >&2
    fail "bin/run exited $SWEEP_RC on the second sweep"
}
if [[ -s "$SWEEP_OUT" ]]; then
    fail "bin/run's second nominal sweep wrote to stdout"
fi
grep -q -F 'alpha=two' "$ALPHA_OUT" || fail "the second sweep did not write 10-alpha's output"
grep -q -F 'beta=two' "$BETA_OUT" || fail "the second sweep did not write 20-beta's output"
if grep -q -F '=one' "$ALPHA_OUT" || grep -q -F '=one' "$BETA_OUT"; then
    fail "the second sweep appended to the first sweep's output instead of truncating"
fi

printf 'job-output-log: ok — log_output is silent, newline-separated and capped; run-job and run both truncate job-<slug>.out.log per run.\n'
