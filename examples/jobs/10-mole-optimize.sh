#!/usr/bin/env bash
# tier: daily
# machine-ops job: mole optimize watchdog.
#
# Runs `mole optimize` (apply mode, unattended-safe) and alerts only on:
#   - optimize exit code != 0
#   - health_score dropping by SCORE_DROP_MIN or more in one run, confirmed
#     by a recheck after a settle interval (optimize transiently depresses
#     the score it measures, so an instant after-reading is not trusted)
#   - the confirmed (post-settle) score ending below SCORE_FLOOR
#   - free disk on / below 15 GB
#   - health_score_msg mentioning a restart
# Otherwise silent. A one-line summary always goes to the run log.
#
# MACHINE_OPS_DRY_RUN=1 switches to `mole optimize --dry-run`.
# Never sudo, never a password: mole requests privilege itself per-operation
# and degrades cleanly. stdin is /dev/null on every mole invocation.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091 # path resolved at runtime from the script location
if ! source "$SCRIPT_DIR/../lib/common.sh"; then
    printf '10-mole-optimize: cannot load lib/common.sh\n' >&2
    exit 1
fi

MOLE=/opt/homebrew/bin/mole
LABEL="mole-optimize"
DRY="${MACHINE_OPS_DRY_RUN:-0}"
DISK_MIN_GB=15
# The score is a noisy sample: six back-to-back reads on an idle machine gave
# 84 84 84 84 84 83, so "any drop alerts" cried wolf. 5 sits clearly outside
# the observed ±2 noise band.
SCORE_DROP_MIN=5
# A per-run delta alone misses slow drift (one lost point a week stays silent
# forever), so an absolute floor catches it. Bands are narrow here (84 reads
# "Good", 86 "Excellent"), which is also why a raw label change is not used
# as an alert trigger.
# Accepted limitation: a machine declining a few points per run stays silent
# until it crosses the floor — no persistent baseline by design, since stored
# state would need its own noise policy.
SCORE_FLOOR=70
# Confirm apparent drops before alerting: optimize perturbs the very metric
# it is measured by (service restarts, cache rebuilds, load spikes), so the
# after-reading is taken at peak disturbance.
# That disturbance is the relevant noise source for EVERY threshold here, not
# the ±2 wobble of back-to-back idle reads: we measured a 7-point transient
# (91 -> 84, recovered to 92), so a floor crossing can be disturbance too.
# Hence one settle + one recheck whenever EITHER rule trips, and both rules
# are evaluated against the recheck value. A sustained sub-floor machine is
# still worth alerting about — the recheck only filters transients.
SCORE_SETTLE_SEC=30

if [[ ! -x "$MOLE" ]]; then
    alert "[$LABEL] mole not found at $MOLE"
    log_line "[$LABEL] mole missing at $MOLE dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

# mole_health_score <raw-status-stdout> — prints "<score>|<msg>".
# `mole status` emits JSON on stdout by default. Score is numeric or
# "unknown" when the output is not JSON or the key is missing/wrong type.
# Ints stay ints; floats and clean numeric strings keep full precision so a
# fractional drop (90.9 -> 90.1) is never hidden by truncation. Booleans
# and anything else fall back to "unknown".
mole_health_score() {
    printf '%s' "$1" | /usr/bin/python3 -c '
import json, re, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("unknown|unknown")
    sys.exit(0)
score = d.get("health_score", "unknown")
msg = d.get("health_score_msg", "")

def parse(v):
    if isinstance(v, bool):
        return None
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        return v
    if isinstance(v, str):
        s = v.strip()
        if re.fullmatch(r"[+-]?(\d+(\.\d+)?|\.\d+)", s):
            return int(s) if "." not in s else float(s)
        return None
    return None

score = parse(score)
if score is None:
    print("unknown|%s" % str(msg))
else:
    print("%s|%s" % (score, str(msg)))
' 2>/dev/null || printf 'unknown|unknown'
}

disk_free_gb() {
    local v
    v="$(df -g / 2>/dev/null | awk 'NR==2{print $4}')"
    v="$(printf '%s' "$v" | tr -d '[:space:]')"
    case "$v" in
        ''|*[!0-9]*) printf 'unknown' ;;
        *) printf '%s' "$v" ;;
    esac
}

# is_number <v> — true for ints and floats ("82", "90.9", "+3", ".5").
is_number() {
    [[ "$1" =~ ^[+-]?([0-9]+(\.[0-9]+)?|\.[0-9]+)$ ]]
}

RAW_BEFORE="$("$MOLE" status < /dev/null 2>/dev/null)" || RAW_BEFORE=""
PAIR_BEFORE="$(mole_health_score "$RAW_BEFORE")"
SCORE_BEFORE="${PAIR_BEFORE%%|*}"
MSG_BEFORE="${PAIR_BEFORE#*|}"
DISK_BEFORE="$(disk_free_gb)"

if [[ "$DRY" == "1" ]]; then
    OPT_OUT="$("$MOLE" optimize --dry-run < /dev/null 2>&1)"
    OPT_RC=$?
else
    OPT_OUT="$("$MOLE" optimize < /dev/null 2>&1)"
    OPT_RC=$?
fi
OPT_CLEAN="$(printf '%s\n' "$OPT_OUT" | strip_ansi)"

# Count of applied optimizations, best-effort. Prefer mole's own summary
# line when it prints one (e.g. "Would apply 3 optimizations"); otherwise
# fall back to counting action lines ("  → ..."), ignoring no-op lines that
# merely report something was already optimal. When neither yields a number,
# log "unknown" rather than a misleading 0.
APPLIED="$(printf '%s\n' "$OPT_CLEAN" | grep -o -i -E -m1 'appl[a-z]* +[0-9]+ +optimization' | grep -o -E -m1 '[0-9]+' | head -n1 || true)"
if [[ -z "$APPLIED" ]]; then
    APPLIED="$(printf '%s\n' "$OPT_CLEAN" | awk '/^ +→ / && tolower($0) !~ /already/ {n++} END {print n+0}')"
    if [[ "$APPLIED" == "0" ]]; then
        APPLIED="unknown"
    fi
fi

RAW_AFTER="$("$MOLE" status < /dev/null 2>/dev/null)" || RAW_AFTER=""
PAIR_AFTER="$(mole_health_score "$RAW_AFTER")"
SCORE_AFTER="${PAIR_AFTER%%|*}"
MSG_AFTER="${PAIR_AFTER#*|}"
DISK_AFTER="$(disk_free_gb)"

# In dry-run nothing is applied, so the count is what *would* be done.
COUNT_LABEL="applied"
if [[ "$DRY" == "1" ]]; then
    COUNT_LABEL="planned"
fi
log_line "[$LABEL] score ${SCORE_BEFORE}->${SCORE_AFTER} disk ${DISK_BEFORE}G->${DISK_AFTER}G ${COUNT_LABEL}=${APPLIED} rc=${OPT_RC} dry_run=${DRY}"

if [[ "$OPT_RC" != "0" ]]; then
    alert "[$LABEL] mole optimize exited rc=$OPT_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$OPT_CLEAN" | tail -n 15)"
    log_line "[$LABEL] optimize output tail: $(printf '%s\n' "$OPT_CLEAN" | tail -n 30 | tr '\n' ';')"
fi

if ! is_number "$SCORE_BEFORE" || ! is_number "$SCORE_AFTER"; then
    alert "[$LABEL] could not parse health_score (before=${SCORE_BEFORE} after=${SCORE_AFTER})"
else
    # Trip flags from the instant after-reading. Either rule tripping leads
    # to ONE shared settle + recheck; both rules are then evaluated against
    # the recheck value, since optimize's own disturbance (measured at 7
    # points) can cross either threshold transiently.
    # awk (not bash (( ))) so fractional scores compare without truncation.
    drop_trip=0
    floor_trip=0
    if awk -v before="$SCORE_BEFORE" -v after="$SCORE_AFTER" -v min="$SCORE_DROP_MIN" \
        'BEGIN { exit !((before - after) >= min) }' 2>/dev/null; then
        drop_trip=1
    fi
    if awk -v after="$SCORE_AFTER" -v floor="$SCORE_FLOOR" \
        'BEGIN { exit !(after < floor) }' 2>/dev/null; then
        floor_trip=1
    fi
    if (( drop_trip == 1 || floor_trip == 1 )); then
        # The sleep runs solely on this rare path — nominal runs never pay it.
        sleep "$SCORE_SETTLE_SEC"
        RAW_RECHECK="$("$MOLE" status < /dev/null 2>/dev/null)" || RAW_RECHECK=""
        PAIR_RECHECK="$(mole_health_score "$RAW_RECHECK")"
        SCORE_RECHECK="${PAIR_RECHECK%%|*}"
        if ! is_number "$SCORE_RECHECK"; then
            # Fail open: a missing recheck must not swallow a real regression.
            TRIPPED=""
            if (( drop_trip == 1 )); then
                TRIPPED="drop>=${SCORE_DROP_MIN}"
            fi
            if (( floor_trip == 1 )); then
                if [[ -n "$TRIPPED" ]]; then
                    TRIPPED="${TRIPPED} + floor<${SCORE_FLOOR}"
                else
                    TRIPPED="floor<${SCORE_FLOOR}"
                fi
            fi
            alert "[$LABEL] health_score ${SCORE_BEFORE} -> ${SCORE_AFTER} tripped (${TRIPPED}) but recheck unavailable (dry_run=$DRY)"
            log_line "[$LABEL] score ${SCORE_BEFORE}->${SCORE_AFTER} recheck=${SCORE_RECHECK} (no recheck, alerting)"
        else
            CONFIRMED=""
            if (( drop_trip == 1 )) && awk -v before="$SCORE_BEFORE" -v after="$SCORE_RECHECK" -v min="$SCORE_DROP_MIN" \
                'BEGIN { exit !((before - after) >= min) }' 2>/dev/null; then
                alert "[$LABEL] health_score dropped ${SCORE_BEFORE} -> ${SCORE_AFTER} (recheck ${SCORE_RECHECK}, drop >= ${SCORE_DROP_MIN}, dry_run=$DRY)"
                CONFIRMED="drop"
            fi
            if (( floor_trip == 1 )) && awk -v after="$SCORE_RECHECK" -v floor="$SCORE_FLOOR" \
                'BEGIN { exit !(after < floor) }' 2>/dev/null; then
                alert "[$LABEL] health_score below floor: recheck ${SCORE_RECHECK} < ${SCORE_FLOOR} (was ${SCORE_BEFORE} -> ${SCORE_AFTER}, dry_run=$DRY)"
                if [[ -n "$CONFIRMED" ]]; then
                    CONFIRMED="${CONFIRMED}+floor"
                else
                    CONFIRMED="floor"
                fi
            fi
            if [[ -n "$CONFIRMED" ]]; then
                log_line "[$LABEL] score ${SCORE_BEFORE}->${SCORE_AFTER} recheck=${SCORE_RECHECK} (confirmed ${CONFIRMED})"
            else
                log_line "[$LABEL] score ${SCORE_BEFORE}->${SCORE_AFTER} recheck=${SCORE_RECHECK} (transient, no alert)"
            fi
        fi
    fi
fi

# DISK_AFTER is digit-only-or-unknown by construction (see disk_free_gb),
# so bash arithmetic here can never see a float.
if is_number "$DISK_AFTER" && (( DISK_AFTER < DISK_MIN_GB )); then
    alert "[$LABEL] free disk low: ${DISK_AFTER}G on / (min ${DISK_MIN_GB}G)"
fi

RESTART_MSG=""
if printf '%s' "$MSG_AFTER" | grep -qi 'restart'; then
    RESTART_MSG="$MSG_AFTER"
elif printf '%s' "$MSG_BEFORE" | grep -qi 'restart'; then
    RESTART_MSG="$MSG_BEFORE"
fi
if [[ -n "$RESTART_MSG" ]]; then
    alert "[$LABEL] health message asks for restart: ${RESTART_MSG}"
fi

flush_alerts "$LABEL"
exit 0
