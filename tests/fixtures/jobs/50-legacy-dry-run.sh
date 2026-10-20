#!/usr/bin/env bash
# tier: daily
# timeout: 30
# THE HAZARD FIXTURE. This job reads ONLY the legacy
# MACHINE_OPS_DRY_RUN name, exactly like the owner's live automations in the
# separate repository this rename cannot reach. Under `bin/run --dry-run` it
# MUST observe "1": if the runner ever stops exporting the legacy name, this
# job falls through `:-0` and would run in APPLY MODE. tests/silent-contract.sh
# asserts the value it records here.
#
# It never invokes mole, brew, npm or any other external tool: writing the
# observed flag into the synthetic automations root is its only effect.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh and must be safe on a contributor's machine.

set -u

DRY="${MACHINE_OPS_DRY_RUN:-0}"
ROOT="${MOPS_AUTOMATIONS_ROOT:-${MACHINE_OPS_AUTOMATIONS_ROOT:-.}}"

mkdir -p "$ROOT/state" 2>/dev/null || true
printf '%s\n' "$DRY" > "$ROOT/state/legacy-dry-run-value" 2>/dev/null || true
if [[ "$DRY" != "1" ]]; then
    printf 'APPLY\n' > "$ROOT/state/legacy-dry-run-apply" 2>/dev/null || true
fi

exit 0
