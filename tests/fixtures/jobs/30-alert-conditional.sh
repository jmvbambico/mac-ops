#!/usr/bin/env bash
# tier: daily
# timeout: 30
# Synthetic fixture: a job that queues an alert but whose condition does NOT
# fire, so flush_alerts emits nothing. Queuing an alert is not speaking; only
# the flush is, and an empty queue must stay silent.
#
# The condition is the presence of a marker file in the automations root's
# state/ directory, which the test never creates — so the alert never fires.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh. It never invokes mole, brew, npm or any other
# external tool: it must be safe to run on a contributor's machine.

set -u

TOOL_ROOT="${MOPS_TOOL_ROOT:?MOPS_TOOL_ROOT must point at the checkout under test}"
# shellcheck disable=SC1091 # tool root is resolved at runtime, not at parse time
source "$TOOL_ROOT/lib/common.sh"

AUTOMATIONS_ROOT="${MOPS_AUTOMATIONS_ROOT:-.}"
MARKER="$AUTOMATIONS_ROOT/state/fixture-alert-marker"

if [[ -f "$MARKER" ]]; then
    alert "[fixture-alert-conditional] marker present at $MARKER"
fi
log_line "[fixture-alert-conditional] checked marker $MARKER (absent: nothing to alert)"

flush_alerts "fixture-alert-conditional"

exit 0
