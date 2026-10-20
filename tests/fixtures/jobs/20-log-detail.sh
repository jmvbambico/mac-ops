#!/usr/bin/env bash
# tier: daily
# timeout: 30
# Synthetic fixture: a job that HAS detail to report but reports it to the
# run log via log_line, never to stdout. Detail belongs in the log; stdout is
# the alert channel.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh. It never invokes mole, brew, npm or any other
# external tool: it must be safe to run on a contributor's machine.

set -u

TOOL_ROOT="${MOPS_TOOL_ROOT:-${MACHINE_OPS_TOOL_ROOT:?MOPS_TOOL_ROOT must point at the checkout under test}}"
# shellcheck disable=SC1091 # tool root is resolved at runtime, not at parse time
source "$TOOL_ROOT/lib/common.sh"

log_line "[fixture-log-detail] inspected 3 things, all fine; this detail belongs in the log"

exit 0
