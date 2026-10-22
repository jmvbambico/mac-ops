#!/usr/bin/env bash
#
# tests/log-line-no-config.sh — regression for lib/common.sh's log_line.
#
# A job can source common.sh with HOME unset and no MOPS_LOG configured. The
# old "${MOPS_LOG:?}" aborted a non-interactive shell during word expansion,
# before `|| true` or a redirection could help — killing the job at its first
# log_line. log_line must instead be a silent no-op: caller survives, stdout
# stays empty.
#
# Run standalone, or via tests/silent-contract.sh (the gate).

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mops-logline.XXXXXX")" || {
    printf 'log-line-no-config: cannot create temp dir\n' >&2
    exit 1
}
cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT

# The probe sources common.sh with no resolvable log and then reports that it
# survived on STDERR, so any stdout would be a contract violation we can see.
PROBE="$WORK/probe.sh"
cat > "$PROBE" <<'PROBE'
set -u
source "$1/lib/common.sh"
log_line "this must not kill the caller"
printf 'SURVIVED\n' >&2
exit 0
PROBE

OUT="$WORK/out"
ERR="$WORK/err"
RC=0
(
    env -u HOME -u MOPS_LOG \
        bash "$PROBE" "$TOOL_ROOT"
) >"$OUT" 2>"$ERR" || RC=$?

if (( RC != 0 )); then
    printf 'log-line-no-config: FAIL: subshell exited %d — log_line aborted its caller\n' "$RC" >&2
    cat "$ERR" >&2
    exit 1
fi
if [[ -s "$OUT" ]]; then
    printf 'log-line-no-config: FAIL: log_line or its resolution wrote to stdout:\n' >&2
    cat "$OUT" >&2
    exit 1
fi
if ! grep -q 'SURVIVED' "$ERR"; then
    printf 'log-line-no-config: FAIL: the calling shell did not survive log_line\n' >&2
    cat "$ERR" >&2
    exit 1
fi

# The legacy name is no longer honoured: with HOME unset, no MOPS_LOG, and
# only MACHINE_OPS_LOG set, common.sh must not adopt that path. The probe stays
# a silent no-op and the legacy log file is never created.
LEGACY_LOG="$WORK/legacy.log"
RC=0
(
    env -u HOME -u MOPS_LOG MACHINE_OPS_LOG="$LEGACY_LOG" \
        bash "$PROBE" "$TOOL_ROOT"
) >"$OUT" 2>"$ERR" || RC=$?
if (( RC != 0 )); then
    printf 'log-line-no-config: FAIL: subshell exited %d with a legacy log name set\n' "$RC" >&2
    cat "$ERR" >&2
    exit 1
fi
if [[ -s "$OUT" ]]; then
    printf 'log-line-no-config: FAIL: a legacy log name wrote to stdout:\n' >&2
    cat "$OUT" >&2
    exit 1
fi
if [[ -e "$LEGACY_LOG" ]]; then
    printf 'log-line-no-config: FAIL: MACHINE_OPS_LOG was still honoured (%s created)\n' "$LEGACY_LOG" >&2
    exit 1
fi

printf 'log-line-no-config: ok — log_line is a silent no-op with no log configured, and the legacy log name is ignored.\n'
