#!/usr/bin/env bash
# tier: daily
# timeout: 30
# Synthetic fixture: a job that honours MACHINE_OPS_DRY_RUN=1 by previewing
# what it would do instead of doing it. In preview mode it must still print
# nothing — a dry run is still a nominal run.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh. It never invokes mole, brew, npm or any other
# external tool: it must be safe to run on a contributor's machine.

set -u

TOOL_ROOT="${MACHINE_OPS_TOOL_ROOT:?MACHINE_OPS_TOOL_ROOT must point at the checkout under test}"
# shellcheck disable=SC1091 # tool root is resolved at runtime, not at parse time
source "$TOOL_ROOT/lib/common.sh"

DRY="${MACHINE_OPS_DRY_RUN:-0}"

if [[ "$DRY" == "1" ]]; then
    log_line "[fixture-dry-run] dry run: would prune 2 stale caches, did nothing"
else
    log_line "[fixture-dry-run] applying: pruned 2 stale caches"
fi

exit 0
