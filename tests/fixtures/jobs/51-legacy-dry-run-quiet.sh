#!/usr/bin/env bash
# tier: daily
# timeout: 30
# A second legacy-only job. It does nothing except read the old
# MACHINE_OPS_DRY_RUN name. Its purpose is to prove the migration note is
# logged AT MOST ONCE per run, not once per job: two legacy jobs must produce
# exactly one note. It must stay silent.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh and never invokes an external tool.

set -u

DRY="${MACHINE_OPS_DRY_RUN:-0}"
: "$DRY"

exit 0
