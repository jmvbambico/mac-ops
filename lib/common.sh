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
#   1. $MOPS_AUTOMATIONS_ROOT (legacy: $MACHINE_OPS_AUTOMATIONS_ROOT)
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

# mops_env_value <primary> <legacy> — print the value of the first NON-EMPTY
# variable of the two names, primary first. Prints nothing and returns 1 when
# neither is set. This is the READ side of the MOPS_* rename: the new name
# always wins when both are set.
#
# The legacy MACHINE_OPS_* names exist only for compatibility and will be
# removed in 0.4.0. They are honoured because the owner's own live automations
# live in a SEPARATE repository that this rename cannot reach; there are no
# third-party users (the repo has no stars, forks or clones). When those
# automations are migrated, the legacy reads and exports can go.
mops_env_value() {
    local primary="$1" legacy="$2"
    if [[ -n "${!primary:-}" ]]; then
        printf '%s\n' "${!primary}"
        return 0
    fi
    if [[ -n "${!legacy:-}" ]]; then
        printf '%s\n' "${!legacy}"
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
    if _root="$(mops_env_value MOPS_AUTOMATIONS_ROOT MACHINE_OPS_AUTOMATIONS_ROOT)"; then
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

# The run log. The dispatcher exports both names with the same value; a job
# run by hand may have set only one. Resolve the alias once and mirror it so
# every child process sees both, whichever name it reads.
if _COMMON_LOG="$(mops_env_value MOPS_LOG MACHINE_OPS_LOG)"; then
    MOPS_LOG="$_COMMON_LOG"
    MACHINE_OPS_LOG="$_COMMON_LOG"
    export MOPS_LOG MACHINE_OPS_LOG
elif _COMMON_AUTOMATIONS_ROOT="$(machine_ops_automations_root)"; then
    # Default log location when the dispatcher has not exported one (e.g. a
    # job run by hand for debugging). It belongs to the AUTOMATIONS root,
    # never to the tool directory.
    _COMMON_LOG="$_COMMON_AUTOMATIONS_ROOT/logs/machine-ops.log"
    MOPS_LOG="$_COMMON_LOG"
    MACHINE_OPS_LOG="$_COMMON_LOG"
    export MOPS_LOG MACHINE_OPS_LOG
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

# warn_legacy_dry_run <job-file> <context> — log ONE migration note when a job
# still reads the legacy MACHINE_OPS_DRY_RUN and not MOPS_DRY_RUN. Returns 0
# when it logged, 1 otherwise (no note is a normal, silent outcome).
#
# The note goes to the RUN LOG only, never to stdout: stdout is the alert
# channel and a nominal run must stay silent (Principle I). Because both
# names are exported with the same value, the job still works correctly at
# runtime; this is a migration record, not an alert. Callers that run many
# jobs must gate this on their own once-per-run flag.
warn_legacy_dry_run() {
    local file="$1" context="${2:-job}"
    [[ -f "$file" ]] || return 1
    # Match an actual variable EXPANSION, not a prose mention: the `$` sigil is
    # required, so `${MACHINE_OPS_DRY_RUN:-0}` and `$MACHINE_OPS_DRY_RUN` count
    # while a bare comment mention does not. The "already migrated" check uses
    # the same shape; MOPS_DRY_RUN cannot satisfy it via the legacy name
    # because the sigil must sit immediately before MOPS (not after MACHINE_).
    if grep -Eq '\$[{]?MACHINE_OPS_DRY_RUN([^A-Za-z0-9_]|$)' "$file" 2>/dev/null \
        && ! grep -Eq '\$[{]?MOPS_DRY_RUN([^A-Za-z0-9_]|$)' "$file" 2>/dev/null; then
        log_line "${context} reads deprecated MACHINE_OPS_DRY_RUN; migrate to MOPS_DRY_RUN (legacy alias kept until 0.4.0)"
        return 0
    fi
    return 1
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
# Read-side alias, same precedence as the rest: MOPS_* wins, legacy falls back.
MOPS_RUN_STDOUT_CAP="${MOPS_RUN_STDOUT_CAP:-${MACHINE_OPS_RUN_STDOUT_CAP:-8192}}"

# record_run <job> <started_epoch> <duration_s> <rc> <timed_out> <dry_run> <stdout_file>
# Best-effort: never fails the caller, never prints to stdout (that channel
# belongs to alerts alone).
record_run() {
    [[ -n "${MOPS_LOG:-}" ]] || return 0
    local runs_file="${MOPS_RUNS:-${MACHINE_OPS_RUNS:-$(dirname "$MOPS_LOG")/runs.jsonl}}"
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
