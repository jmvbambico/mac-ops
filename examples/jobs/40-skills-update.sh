#!/usr/bin/env bash
# tier: daily
# timeout: 600
# machine-ops job: skills CLI update watchdog.
#
# Runs `npx --yes skills update -g -y` (global scope; -y skips the scope
# prompt) and diffs ~/.agents/.skill-lock.json before/after on each skill's
# skillFolderHash: routine hash changes are log-only and stay silent, but a
# skill DISAPPEARING from the lockfile is data loss and alerts loudly — as
# does a non-zero exit. What matters is the TRANSITION, not just the end
# state: a VALID baseline followed by a missing/unparseable lockfile after
# the update alerts (the update destroyed state), even when the update
# itself exited 0. Baseline unreadable/absent followed by post-state
# unreadable stays log-only — the CLI may simply not be set up on some
# machine. A transiently empty post-state (valid JSON, zero skills, non-zero
# baseline) alerts as "lockfile is empty (was N skills)", never as N
# individually enumerated disappearances. Post-update reads retry with a
# short pause to tolerate an atomic or in-progress rewrite, and enumerated
# name lists in alerts are capped so one bad read can never flood the
# alert channel.
#
# Offline skips SILENTLY, same contract as the brew job: a nightly false
# alarm for having no network would poison the alert channel.
#
# MOPS_DRY_RUN=1: the update never runs; the lockfile is read and the
# current skill count is logged with a `planned` label.
# A one-line summary always goes to the run log.
# Never sudo, never a password. stdin is /dev/null on the update invocation.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# lib/common.sh belongs to the TOOL root, which may differ from this job's
# automations root. bin/run and bin/run-job export MOPS_TOOL_ROOT.
MACOPS_LIB="${MOPS_TOOL_ROOT:-${MACHINE_OPS_TOOL_ROOT:-$SCRIPT_DIR/..}}/lib"
# shellcheck disable=SC1091 # path resolved at runtime from the tool root
if ! source "$MACOPS_LIB/common.sh"; then
    printf '40-skills-update: cannot load lib/common.sh\n' >&2
    exit 1
fi

LABEL="skills-update"
DRY="${MOPS_DRY_RUN:-${MACHINE_OPS_DRY_RUN:-0}}"
# Test seams: MOPS_SKILL_LOCK points at fixture lockfiles and MOPS_NPX at a
# stub, so apply paths are provable without touching the live skills
# installation (legacy MACHINE_OPS_* names accepted until the owner's
# automations in ~/projects/machine-ops are migrated; expected 0.5.0).
LOCKFILE="${MOPS_SKILL_LOCK:-${MACHINE_OPS_SKILL_LOCK:-$HOME/.agents/.skill-lock.json}}"
NPX="${MOPS_NPX:-${MACHINE_OPS_NPX:-/opt/homebrew/bin/npx}}"

# is_online — cheap bounded reachability probe for the npm registry
# (DNS + TCP 443, never the update itself). Same shape as the brew job's
# probe; MOPS_FORCE_OFFLINE=1 (legacy MACHINE_OPS_FORCE_OFFLINE accepted)
# forces the offline path for tests.
is_online() {
    if [[ "${MOPS_FORCE_OFFLINE:-${MACHINE_OPS_FORCE_OFFLINE:-0}}" == "1" ]]; then
        return 1
    fi
    if have nc; then
        nc -z -G 3 -w 3 registry.npmjs.org 443 < /dev/null >/dev/null 2>&1
        return $?
    fi
    if ! have ping; then
        return 1
    fi
    ping -c 1 -t 2 1.1.1.1 < /dev/null >/dev/null 2>&1
}

# read_skill_lock <file> — prints "name<TAB>skillFolderHash" per skill,
# name-sorted. Non-zero when the file is missing or unparseable; the caller
# treats that as log-only, never an alert.
read_skill_lock() {
    /usr/bin/python3 - "$1" <<'PYEOF' 2>/dev/null
import json, sys
try:
    with open(sys.argv[1]) as f:
        d = json.load(f)
    skills = d.get("skills", {})
    if not isinstance(skills, dict):
        raise ValueError("skills is not an object")
    for name in sorted(skills):
        meta = skills[name]
        if not isinstance(meta, dict):
            meta = {}
        print("%s\t%s" % (name, meta.get("skillFolderHash", "")))
except Exception:
    sys.exit(1)
PYEOF
}

# count_skills <snapshot> — number of non-blank lines in a lock snapshot.
count_skills() {
    local snap="$1" n
    if [[ -z "$snap" ]]; then
        printf '0'
        return 0
    fi
    n="$(printf '%s\n' "$snap" | grep -c '[^[:space:]]' || true)"
    printf '%s' "$n"
}

if ! is_online; then
    log_line "[$LABEL] skipped: offline (no route to registry.npmjs.org, staying silent) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

BEFORE="$(read_skill_lock "$LOCKFILE")"
BEFORE_RC=$?
BASELINE_OK=1
BASELINE_N="unknown"
if [[ "$BEFORE_RC" != "0" ]]; then
    BASELINE_OK=0
    BEFORE=""
    log_line "[$LABEL] lockfile unreadable before update: $LOCKFILE (log-only; continuing)"
else
    BASELINE_N="$(count_skills "$BEFORE")"
fi

if [[ "$DRY" == "1" ]]; then
    if [[ "$BASELINE_OK" == "1" ]]; then
        N="$(count_skills "$BEFORE")"
    else
        N="unknown"
    fi
    log_line "[$LABEL] skills=${N} updated=none added=none removed=none planned=dry-run rc=0 dry_run=1"
    flush_alerts "$LABEL"
    exit 0
fi

SCRATCH="$(mktemp -d 2>/dev/null)" || {
    alert "[$LABEL] cannot create temp dir for lockfile diff"
    log_line "[$LABEL] mktemp failed (alerting; update not attempted)"
    flush_alerts "$LABEL"
    exit 0
}
trap 'rm -rf "$SCRATCH"' EXIT

printf '%s\n' "$BEFORE" > "$SCRATCH/before.txt"

UPDATE_OUT="$("$NPX" --yes skills update -g -y < /dev/null 2>&1)"
UPDATE_RC=$?
UPDATE_CLEAN="$(printf '%s\n' "$UPDATE_OUT" | strip_ansi)"

# Stable post-update read: retry a couple of times with a short pause so an
# atomic or in-progress lockfile rewrite is mistaken for neither data loss
# nor a mass disappearance. An empty snapshot against a non-empty baseline
# is retried for the same reason.
AFTER=""
AFTER_RC=1
for attempt in 1 2 3; do
    if AFTER="$(read_skill_lock "$LOCKFILE")"; then
        AFTER_RC=0
        if [[ "$BASELINE_OK" == "1" && "$BASELINE_N" != "0" && -z "$AFTER" && "$attempt" != "3" ]]; then
            sleep 2
            continue
        fi
        break
    else
        AFTER_RC=$?
        AFTER=""
        if [[ "$attempt" != "3" ]]; then
            sleep 2
        fi
    fi
done
if [[ "$AFTER_RC" != "0" ]]; then
    log_line "[$LABEL] lockfile unreadable after update (3 attempts): $LOCKFILE"
fi
printf '%s\n' "$AFTER" > "$SCRATCH/after.txt"

# cap_list <csv> — cap an enumerated name list at 5 so one bad read can
# never flood the alert channel with dozens of names.
cap_list() {
    local list="$1" first5 commas count
    commas="${list//[^,]/}"
    count=$(( ${#commas} + 1 ))
    if (( count > 5 )); then
        first5="$(printf '%s' "$list" | cut -d, -f1-5)"
        printf '%s,+(%d more)' "$first5" "$((count - 5))"
    else
        printf '%s' "$list"
    fi
}

UPDATED="unknown"
ADDED="unknown"
REMOVED="unknown"
if [[ "$BASELINE_OK" == "1" && "$AFTER_RC" == "0" ]]; then
    if [[ -z "$AFTER" && "$BASELINE_N" != "0" ]]; then
        # Still empty after retries: worth alerting, but as one empty
        # lockfile — never as N individually enumerated disappearances.
        UPDATED="none"
        ADDED="none"
        REMOVED="empty-was-${BASELINE_N}"
        alert "[$LABEL] lockfile is empty after update (was ${BASELINE_N} skills); not enumerating removals (possible data loss)"
    else
        DIFF="$(/usr/bin/python3 - "$SCRATCH/before.txt" "$SCRATCH/after.txt" <<'PYEOF' 2>/dev/null
import sys
def load(p):
    d = {}
    with open(p) as f:
        for line in f:
            line = line.rstrip("\n")
            if not line.strip():
                continue
            name, _, h = line.partition("\t")
            d[name.strip()] = h.strip()
    return d
before = load(sys.argv[1])
after = load(sys.argv[2])
updated = sorted(n for n in before if n in after and before[n] != after[n])
added = sorted(n for n in after if n not in before)
removed = sorted(n for n in before if n not in after)
def fmt(xs):
    return ",".join(xs) if xs else "none"
print("updated:%s" % fmt(updated))
print("added:%s" % fmt(added))
print("removed:%s" % fmt(removed))
PYEOF
)"
        DIFF_RC=$?
        if [[ "$DIFF_RC" == "0" && -n "$DIFF" ]]; then
            UPDATED="$(printf '%s\n' "$DIFF" | sed -n 's/^updated://p')"
            ADDED="$(printf '%s\n' "$DIFF" | sed -n 's/^added://p')"
            REMOVED="$(printf '%s\n' "$DIFF" | sed -n 's/^removed://p')"
            if [[ -z "$UPDATED" || -z "$ADDED" || -z "$REMOVED" ]]; then
                UPDATED="unknown"
                ADDED="unknown"
                REMOVED="unknown"
                log_line "[$LABEL] lockfile diff parse failed (log-only)"
            elif [[ "$REMOVED" != "none" && "$REMOVED" != "unknown" ]]; then
                alert "[$LABEL] skill(s) disappeared from lockfile: $(cap_list "$REMOVED") (possible data loss)"
            fi
        else
            log_line "[$LABEL] lockfile diff failed rc=$DIFF_RC (log-only)"
        fi
    fi
elif [[ "$BASELINE_OK" == "1" ]]; then
    # Valid baseline, post-state lost: the update destroyed readable state,
    # even if it exited 0. That transition is the data-loss signal.
    alert "[$LABEL] lockfile became unreadable after update (baseline had ${BASELINE_N} skills): $LOCKFILE (possible data loss)"
else
    log_line "[$LABEL] no baseline diff available (log-only)"
fi

if [[ "$BASELINE_OK" == "1" && "$AFTER_RC" == "0" ]]; then
    N="$(count_skills "$AFTER")"
elif [[ "$BASELINE_OK" == "1" ]]; then
    N="$BASELINE_N"
else
    N="unknown"
fi

log_line "[$LABEL] skills=${N} updated=${UPDATED} added=${ADDED} removed=${REMOVED} rc=${UPDATE_RC} dry_run=${DRY}"

if [[ "$UPDATE_RC" != "0" ]]; then
    alert "[$LABEL] skills update exited rc=$UPDATE_RC"
    alert "$(printf '%s\n' "$UPDATE_CLEAN" | tail -n 10)"
    log_line "[$LABEL] update output tail: $(printf '%s\n' "$UPDATE_CLEAN" | tail -n 30 | tr '\n' ';')"
fi

flush_alerts "$LABEL"
exit 0
