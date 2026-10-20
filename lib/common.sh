#!/usr/bin/env bash
# lib/common.sh — shared watchdog helpers for machine-ops jobs.
#
# Source this file; do not execute it:
#   source "$SCRIPT_DIR/../lib/common.sh"
#
# Contract: SILENT ON SUCCESS. A nominal run prints nothing to stdout.
# Jobs queue text via alert() and emit it once via flush_alerts <label>.
# Anything a job prints to stdout is treated by bin/run as an alert worth
# surfacing, so speak only on change, regression, or error. Diagnostic
# detail belongs in the run log via log_line().
#
# No dependency on `timeout`: macOS ships neither `timeout` nor (without
# coreutils) `gtimeout`. Per-job timeouts live in bin/run, which runs each
# job in its own process group and TERM/KILLs the group on timeout (a
# child-only kill once orphaned a wedged grandchild). Prefer the real
# `gtimeout` only when `have gtimeout` says it exists.

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    printf 'lib/common.sh: source this file, do not execute it\n' >&2
    exit 1
fi

if [[ -n "${_COMMON_SH_LOADED:-}" ]]; then
    return 0
fi
_COMMON_SH_LOADED=1

# Default log location when the dispatcher has not exported one (e.g. a job
# run by hand for debugging). Always absolute, anchored at this file.
if [[ -z "${MACHINE_OPS_LOG:-}" ]]; then
    _COMMON_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    MACHINE_OPS_LOG="$_COMMON_DIR/../logs/machine-ops.log"
    export MACHINE_OPS_LOG
    unset _COMMON_DIR
fi

# ts — current local time, log-friendly.
ts() {
    date "+%Y-%m-%d %H:%M:%S"
}

# log_line <msg> — append one timestamped line to the run log.
# Never fails the caller: logging must not break a watchdog.
log_line() {
    printf '%s %s\n' "$(ts)" "$*" >> "${MACHINE_OPS_LOG:?}" 2>/dev/null || true
}

# Alert queue. Buffer in a variable so a job decides ONCE, at the end,
# whether it has anything worth surfacing.
_ALERT_BUF=""

# alert <msg> — queue a line (or lines) for stdout.
alert() {
    if [[ -z "$_ALERT_BUF" ]]; then
        _ALERT_BUF="$*"
    else
        _ALERT_BUF="${_ALERT_BUF}
$*"
    fi
}

# alerts_pending — true when the queue is non-empty.
alerts_pending() {
    [[ -n "$_ALERT_BUF" ]]
}

# flush_alerts <job-label> — print queued alerts under a label header,
# or print nothing when the queue is empty. Always drains the queue.
flush_alerts() {
    local label="${1:-job}"
    if [[ -n "$_ALERT_BUF" ]]; then
        printf '%s\n' "== ${label} =="
        printf '%s\n' "$_ALERT_BUF"
    fi
    _ALERT_BUF=""
}

# have <cmd> — true when cmd resolves via PATH.
have() {
    command -v "$1" >/dev/null 2>&1
}

# strip_ansi — stdin to stdout with ANSI escape sequences removed.
# Covers full CSI (SGR colours plus cursor/edit sequences with ; : ? < = >
# parameters), OSC sequences such as hyperlinks (terminated by BEL or
# ESC \), and any stray ESC left by a truncated sequence. Mole colourizes
# its output; the log should stay plain text.
strip_ansi() {
    local esc bel
    esc="$(printf '\033')"
    bel="$(printf '\007')"
    sed -E \
        -e "s/${esc}\\[[0-9:;<=>?]*[ -/]*[@-~]//g" \
        -e "s/${esc}\\][^${bel}${esc}]*(${bel}|${esc}\\\\)//g" \
        -e "s/${esc}//g"
}

# ---------------------------------------------------------------------------
# Structured run records for the machine-ops console.
#
# Jobs emit ONE JSON line per run to logs/runs.jsonl; the console ingests
# those into its SQLite index. Deliberately NOT a direct sqlite3 write:
#   - no schema knowledge in bash, so the console can migrate its tables
#     without silently breaking the jobs (silent breakage is the failure mode
#     this project exists to prevent);
#   - no database lock can ever block or fail a 02:00 maintenance run;
#   - /usr/bin/python3 is already a dependency (bin/run parses console.json
#     with it), so this adds none.
# The text run log stays the durable record — runs.jsonl is an index feed,
# and the console can always rebuild from logs/machine-ops.log instead.
#
# Captured stdout is capped: the alert channel is short by contract, and the
# full text is already in the run log and the per-job launchd log.
MACHINE_OPS_RUN_STDOUT_CAP="${MACHINE_OPS_RUN_STDOUT_CAP:-8192}"

# record_run <job> <started_epoch> <duration_s> <rc> <timed_out> <dry_run> <stdout_file>
# Best-effort: never fails the caller, never prints to stdout (that channel
# belongs to alerts alone).
record_run() {
    [[ -n "${MACHINE_OPS_LOG:-}" ]] || return 0
    local runs_file="${MACHINE_OPS_RUNS:-$(dirname "$MACHINE_OPS_LOG")/runs.jsonl}"
    /usr/bin/python3 - "$@" "$MACHINE_OPS_RUN_STDOUT_CAP" >> "$runs_file" 2>/dev/null <<'PY' || true
import json, sys

job, started, dur, rc, timed_out, dry_run, stdout_path, cap = sys.argv[1:9]
try:
    with open(stdout_path, encoding="utf-8", errors="replace") as fh:
        out = fh.read(int(cap))
except (OSError, ValueError):
    out = ""

rc_i = int(rc) if rc.lstrip("-").isdigit() else -1
# Same derivation as bin/run and the console: a timeout outranks a non-zero
# exit, and ANY stdout means the job spoke. Silence with rc=0 is success.
if timed_out == "1" or rc_i == 124:
    status = "hang"
elif rc_i != 0:
    status = "fail"
elif out.strip():
    status = "spoke"
else:
    status = "quiet"

print(json.dumps({
    "job": job,
    "started_at": int(started) if started.isdigit() else 0,
    "duration_s": int(dur) if dur.isdigit() else 0,
    "rc": rc_i,
    "status": status,
    "dry_run": dry_run == "1",
    # A job that fails or hangs SILENTLY still needs a readable summary, or
    # the console shows a blank row for the most important runs.
    "summary": (out.strip().splitlines()[-1] if out.strip()
                else "timed out" if status == "hang"
                else "exited {}".format(rc_i) if status == "fail"
                else ""),
    "stdout": out,
}, ensure_ascii=False))
PY
}
