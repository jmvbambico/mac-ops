#!/usr/bin/env bash
# tier: daily
# machine-ops job: mole clean watchdog.
#
# Runs `mole clean` in apply mode. Non-interactivity comes ONLY from an
# absent TTY (stdin is /dev/null on every mole invocation): with no TTY,
# mole prints "Running in non-interactive mode" and proceeds with user-level
# cleanup instead of prompting. mole 1.54.0 has NO --yes flag — do not add one.
#
# Alerts only on non-zero exit or an obvious failure string; otherwise silent.
# Reclaimed space / item counts are recorded to the run log (best-effort
# parse — apply-mode wording may differ from dry-run wording).
#
# MOPS_DRY_RUN=1 switches to `mole clean --dry-run` (the legacy
# MACHINE_OPS_DRY_RUN name is still accepted until the owner's automations in
# ~/projects/machine-ops are migrated; expected 0.5.0).
# Never sudo, never a password.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# lib/common.sh belongs to the TOOL root, which may differ from this job's
# automations root. bin/run and bin/run-job export MOPS_TOOL_ROOT.
MACOPS_LIB="${MOPS_TOOL_ROOT:-${MACHINE_OPS_TOOL_ROOT:-$SCRIPT_DIR/..}}/lib"
# shellcheck disable=SC1091 # path resolved at runtime from the tool root
if ! source "$MACOPS_LIB/common.sh"; then
    printf '20-mole-clean: cannot load lib/common.sh\n' >&2
    exit 1
fi

MOLE=/opt/homebrew/bin/mole
LABEL="mole-clean"
DRY="${MOPS_DRY_RUN:-${MACHINE_OPS_DRY_RUN:-0}}"

if [[ ! -x "$MOLE" ]]; then
    alert "[$LABEL] mole not found at $MOLE"
    log_line "[$LABEL] mole missing at $MOLE dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

if [[ "$DRY" == "1" ]]; then
    CLEAN_OUT="$("$MOLE" clean --dry-run < /dev/null 2>&1)"
    CLEAN_RC=$?
else
    CLEAN_OUT="$("$MOLE" clean < /dev/null 2>&1)"
    CLEAN_RC=$?
fi
CLEAN="$(printf '%s\n' "$CLEAN_OUT" | strip_ansi)"

# Best-effort parse. Dry-run prints a summary line, e.g.
#   "Potential space: 8.50GB | Items: 343 | Categories: 6".
# Prefer the summary line: the body lists per-category item counts that
# must not be mistaken for the total.
RECLAIMED="$(printf '%s\n' "$CLEAN" | grep -o -E -m1 'Potential space: [^|]+|Reclaimed: [^,;]+|Freed: [^,;]+|reclaimed [^,;]+' | head -n1 || true)"
ITEMS="$(printf '%s\n' "$CLEAN" | grep -o -E -m1 'Items: [0-9]+' | head -n1 || true)"
if [[ -z "$ITEMS" ]]; then
    ITEMS="$(printf '%s\n' "$CLEAN" | grep -o -E -m1 '[0-9]+ items' | head -n1 || true)"
fi
# Trim surrounding whitespace only; internal spacing stays readable.
RECLAIMED="$(printf '%s' "$RECLAIMED" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
ITEMS="$(printf '%s' "$ITEMS" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
if [[ -z "$RECLAIMED" ]]; then
    RECLAIMED="unknown"
fi
if [[ -z "$ITEMS" ]]; then
    ITEMS="unknown"
fi

log_line "[$LABEL] reclaimed=${RECLAIMED} items=${ITEMS} rc=${CLEAN_RC} dry_run=${DRY}"

if [[ "$CLEAN_RC" != "0" ]]; then
    alert "[$LABEL] mole clean exited rc=$CLEAN_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$CLEAN" | tail -n 15)"
    log_line "[$LABEL] clean output tail: $(printf '%s\n' "$CLEAN" | tail -n 30 | tr '\n' ';')"
elif printf '%s\n' "$CLEAN" | grep -qi -E '\berror\b|\bfailed\b|\bfailure\b|traceback|unbound variable|permission denied|command not found'; then
    alert "[$LABEL] mole clean output mentions failure (rc=$CLEAN_RC dry_run=$DRY)"
    alert "$(printf '%s\n' "$CLEAN" | grep -i -E '\berror\b|\bfailed\b|\bfailure\b|traceback|unbound variable|permission denied|command not found' | head -n 10)"
    log_line "[$LABEL] clean output tail: $(printf '%s\n' "$CLEAN" | tail -n 30 | tr '\n' ';')"
fi

flush_alerts "$LABEL"
exit 0
