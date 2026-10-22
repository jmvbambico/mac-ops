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

# ---------------------------------------------------------------------------
# Automations root resolution (AGENTS.md Principle II).
#
# The tool root (this file's directory) holds bin/ and lib/. The AUTOMATIONS
# root holds the user's jobs/, logs/ and state/. They may be the same
# directory (a checkout used in place) but must never be assumed to be.
#
# Precedence, first hit wins:
#   1. $MOPS_AUTOMATIONS_ROOT
#   2. automations_root in ~/.config/mops/config.json
#   3. $HOME/.mops  (the default)
#
# A missing config is normal. A malformed config falls through to the default
# instead of aborting: a job that quietly never runs is the worst failure
# mode here (Principle IV).
#
# Prints the root on stdout (callers capture it). Diagnostics go to stderr;
# nothing here may ever write to a nominal run's stdout.
# ---------------------------------------------------------------------------

# mops_env_value <name> — print the value of a NON-EMPTY environment variable.
# Prints nothing and returns 1 when it is unset or empty.
mops_env_value() {
    local name="$1"
    if [[ -n "${!name:-}" ]]; then
        printf '%s\n' "${!name}"
        return 0
    fi
    return 1
}

machine_ops_config_path() {
    [[ -n "${HOME:-}" ]] || return 1
    printf '%s\n' "$HOME/.config/mops/config.json"
}

machine_ops_automations_root() {
    local _root=""
    if _root="$(mops_env_value MOPS_AUTOMATIONS_ROOT)"; then
        printf '%s\n' "$_root"
        return 0
    fi

    local config_file="" config_root=""
    config_file="$(machine_ops_config_path 2>/dev/null || true)"
    if [[ -n "$config_file" && -f "$config_file" ]]; then
        # Same python3 dependency already used by record_run/bin/run; no jq.
        config_root="$(/usr/bin/python3 - "$config_file" <<'PY' 2>/dev/null
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        data = json.load(fh)
    value = data.get("automations_root") if isinstance(data, dict) else None
    if isinstance(value, str) and value.strip():
        print(value.strip())
except Exception:
    pass
PY
)" || config_root=""
    fi

    if [[ -n "$config_root" ]]; then
        printf '%s\n' "$config_root"
        return 0
    fi
    if [[ -n "${HOME:-}" ]]; then
        printf '%s\n' "$HOME/.mops"
        return 0
    fi
    printf 'lib/common.sh: cannot resolve the automations root (HOME is not set)\n' >&2
    return 1
}

# The run log. The dispatcher exports it; a job run by hand may have set it
# directly.
if _COMMON_LOG="$(mops_env_value MOPS_LOG)"; then
    MOPS_LOG="$_COMMON_LOG"
    export MOPS_LOG
elif _COMMON_AUTOMATIONS_ROOT="$(machine_ops_automations_root)"; then
    # Default log location when the dispatcher has not exported one (e.g. a
    # job run by hand for debugging). It belongs to the AUTOMATIONS root,
    # never to the tool directory.
    _COMMON_LOG="$_COMMON_AUTOMATIONS_ROOT/logs/machine-ops.log"
    MOPS_LOG="$_COMMON_LOG"
    export MOPS_LOG
fi
unset _COMMON_AUTOMATIONS_ROOT 2>/dev/null || true
unset _COMMON_LOG 2>/dev/null || true

# ts — current local time, log-friendly.
ts() {
    date "+%Y-%m-%d %H:%M:%S"
}

# log_line <msg> — append one timestamped line to the run log.
# Never fails the caller: logging must not break a watchdog.
#
# The guard must come BEFORE the path is expanded. "${MOPS_LOG:?}" would abort
# a non-interactive shell during word expansion — before `|| true` or a
# redirection could help — when a job runs with HOME unset and no log path
# configured. With no log, logging is a silent no-op (never stdout: Principle I).
log_line() {
    [[ -n "${MOPS_LOG:-}" ]] || return 0
    printf '%s %s\n' "$(ts)" "$*" >> "$MOPS_LOG" 2>/dev/null || true
}

# Cap for the per-job output log. This is scratch detail, not the durable
# record (logs/machine-ops.log is), so a pathological job must not be able to
# fill the disk. The cap is checked across calls: however many times a job
# calls log_output, the FILE never grows past it.
MOPS_JOB_OUT_CAP="${MOPS_JOB_OUT_CAP:-262144}"

# log_output <text> — append text to the per-job output log.
#
# bin/run-job exports MOPS_JOB_OUT (logs/job-<slug>.out.log) and truncates it
# at run start. A job run outside bin/run-job has none, and this is then a
# SILENT no-op — same guard-before-expansion discipline as log_line above.
#
# APPENDS, so a job may call it more than once in a run. ANSI escapes are
# stripped (mole colourizes its output; the console renders this into an
# escaped <pre>). Never writes to stdout (Principle I: stdout is the alert
# channel) and never fails the caller. Past MOPS_JOB_OUT_CAP the write stops
# and a "[truncated at N bytes]" marker is left in the file, because a
# silently truncated log is a quiet lie.
log_output() {
    [[ -n "${MOPS_JOB_OUT:-}" ]] || return 0
    local cap="${MOPS_JOB_OUT_CAP:-262144}"
    [[ "$cap" =~ ^[0-9]+$ ]] || cap=262144
    (( cap > 0 )) || return 0

    local text text_bytes cur room marker budget
    text="$(printf '%s' "$*" | strip_ansi 2>/dev/null || true)"
    [[ -n "$text" ]] || return 0

    cur="$(wc -c < "$MOPS_JOB_OUT" 2>/dev/null | tr -d '[:space:]' || true)"
    [[ "$cur" =~ ^[0-9]+$ ]] || cur=0
    (( cur < cap )) || return 0

    room=$(( cap - cur ))
    text_bytes="$(printf '%s' "$text" | wc -c | tr -d '[:space:]' || true)"
    [[ "$text_bytes" =~ ^[0-9]+$ ]] || text_bytes=0
    if (( text_bytes <= room )); then
        printf '%s' "$text" >> "$MOPS_JOB_OUT" 2>/dev/null || true
        return 0
    fi

    # Reserve room for the marker so the file lands on the cap when both fit.
    # A tiny remaining room still gets the full marker: it may extend a few
    # bytes past the cap, but a silent cut would be worse.
    marker=$'\n'"[truncated at ${cap} bytes]"$'\n'
    budget=$(( room - ${#marker} ))
    (( budget > 0 )) || budget=0
    printf '%s' "$text" | head -c "$budget" >> "$MOPS_JOB_OUT" 2>/dev/null || true
    printf '%s' "$marker" >> "$MOPS_JOB_OUT" 2>/dev/null || true
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
MOPS_RUN_STDOUT_CAP="${MOPS_RUN_STDOUT_CAP:-8192}"

# record_run <job> <started_epoch> <duration_s> <rc> <timed_out> <dry_run> <stdout_file>
# Best-effort: never fails the caller, never prints to stdout (that channel
# belongs to alerts alone).
record_run() {
    [[ -n "${MOPS_LOG:-}" ]] || return 0
    local runs_file="${MOPS_RUNS:-$(dirname "$MOPS_LOG")/runs.jsonl}"
    /usr/bin/python3 - "$@" "$MOPS_RUN_STDOUT_CAP" >> "$runs_file" 2>/dev/null <<'PY' || true
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
