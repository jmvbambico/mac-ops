#!/usr/bin/env bash
# tier: daily
# timeout: 30
# COMMENT-ONLY fixture: MACHINE_OPS_DRY_RUN is named here in prose, but this
# job never expands either dry-run variable. The migration note must not fire:
# it is for jobs that actually READ the legacy flag, not for comments that
# mention it. tests/silent-contract.sh asserts zero notes for this root.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh and never invokes an external tool.

set -u

exit 0
