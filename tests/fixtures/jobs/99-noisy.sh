#!/usr/bin/env bash
# tier: daily
# timeout: 30
# NEGATIVE CONTROL. This job deliberately prints to stdout, which violates
# the silence contract (AGENTS.md Principle I: stdout IS the alert channel).
#
# It is never copied into the positive automations root. It lives in its own
# synthetic root, and tests/silent-contract.sh requires the runner to
# surface its stdout — a silence test that cannot fail is worthless.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh. It never invokes mole, brew, npm or any other
# external tool: it must be safe to run on a contributor's machine.

set -u

printf '%s\n' "fixture-noisy: this line deliberately violates the silence contract"

exit 0
