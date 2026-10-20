#!/usr/bin/env bash
# tier: daily
# timeout: 30
# Synthetic fixture: a job that has nothing to do and must therefore say
# nothing. It is the baseline case for the silence contract.
#
# This file is copied into a synthetic automations root by
# tests/silent-contract.sh. It never invokes mole, brew, npm or any other
# external tool: it must be safe to run on a contributor's machine.

set -u

exit 0
