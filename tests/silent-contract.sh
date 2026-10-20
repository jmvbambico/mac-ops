#!/usr/bin/env bash
#
# tests/silent-contract.sh — AGENTS.md Principle I, mechanically enforced.
#
# "A nominal run prints NOTHING to stdout. stdout IS the alert channel; the
# exit code is not." This gate runs the REAL bin/run against a SYNTHETIC
# automations root whose jobs are known-silent, and asserts stdout is empty
# byte for byte. stderr is NOT the alert channel: it is captured separately
# and never asserted on (a job writing to stderr is not a contract
# violation).
#
# Hermetic by construction:
#   - the fixture jobs never invoke mole, brew, npm or any network tool;
#   - HOME is redirected to a temp dir, so the real ~/.mops and the real
#     ~/.config/mops/config.json are never consulted;
#   - MACHINE_OPS_AUTOMATIONS_ROOT points the runner at the synthetic root,
#     so the real logs/ and state/ are never read or written;
#   - everything lives under a mktemp -d that a trap removes on exit, on
#     failure as well as on success.
#
# A silence test that cannot fail is worthless, so a negative control runs
# the same harness against a job that deliberately prints to stdout and
# requires the violation to be DETECTED, in its own temp root so it cannot
# pollute the positive assertion.
#
# Root contract (pinned):
#   MACHINE_OPS_TOOL_ROOT         -> holds bin/, lib/    (the checkout)
#   MACHINE_OPS_AUTOMATIONS_ROOT  -> holds jobs/, logs/, state/

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="$TOOL_ROOT/tests/fixtures/jobs"
RUNNER="$TOOL_ROOT/bin/run"

POSITIVE_JOBS=(
    10-silent-trivial.sh
    20-log-detail.sh
    30-alert-conditional.sh
    40-dry-run.sh
)
NEGATIVE_JOB=99-noisy.sh

fail() {
    printf 'silent-contract: FAIL: %s\n' "$*" >&2
    exit 1
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mops-silent-contract.XXXXXX")" || fail "cannot create temp dir"
cleanup() {
    rm -rf "$WORK"
}
trap cleanup EXIT

# A private HOME: the runner must not read the real ~/.mops or the real
# ~/.config/mops/config.json. Nothing in the runner or the fixtures needs
# HOME, so redirecting it costs nothing and removes a real-machine dependency.
HOME_DIR="$WORK/home"
mkdir -p "$HOME_DIR"

# build_root <dest-root> <fixture...> — a synthetic automations root. The
# runner creates logs/ and state/ itself; only jobs/ is ours to populate.
build_root() {
    local dest="$1"
    shift
    mkdir -p "$dest/jobs"
    local fixture
    for fixture in "$@"; do
        cp "$FIXTURE_DIR/$fixture" "$dest/jobs/$fixture"
        chmod +x "$dest/jobs/$fixture"
    done
}

# run_runner <root> <stdout-file> <stderr-file> — run the real dispatcher and
# capture the two channels SEPARATELY. Only stdout is the alert channel.
RUNNER_RC=0
run_runner() {
    local root="$1" out="$2" err="$3"
    RUNNER_RC=0
    (
        env MACHINE_OPS_TOOL_ROOT="$TOOL_ROOT" \
            MACHINE_OPS_AUTOMATIONS_ROOT="$root" \
            MACHINE_OPS_DRY_RUN=1 \
            HOME="$HOME_DIR" \
            "$RUNNER" daily --dry-run
    ) >"$out" 2>"$err" || RUNNER_RC=$?
}

# The tool root must never gain the user's logs/ or state/ (AGENTS.md
# Principle II: the tool and the user's automations are separate things).
# The snapshot is taken before the first run and compared after every run.
TOOL_LOGS_PRE=0
TOOL_STATE_PRE=0
if [[ -e "$TOOL_ROOT/logs" ]]; then TOOL_LOGS_PRE=1; fi
if [[ -e "$TOOL_ROOT/state" ]]; then TOOL_STATE_PRE=1; fi

# assert_tool_root_clean <label> — fail if the runner created logs/ or
# state/ inside the tool root during the run.
assert_tool_root_clean() {
    local label="$1" now_logs=0 now_state=0
    if [[ -e "$TOOL_ROOT/logs" ]]; then now_logs=1; fi
    if [[ -e "$TOOL_ROOT/state" ]]; then now_state=1; fi
    if (( now_logs != TOOL_LOGS_PRE || now_state != TOOL_STATE_PRE )); then
        fail "$label: the runner wrote logs/ or state/ into the tool root ($TOOL_ROOT)"
    fi
}

# ---------------------------------------------------------------------------
# 1. Fixture sanity: every positive fixture must be silent ON ITS OWN.
#    This is what makes a later runner-level failure attributable to the
#    runner rather than to a sloppy fixture.
# ---------------------------------------------------------------------------
POS_ROOT="$WORK/positive"
build_root "$POS_ROOT" "${POSITIVE_JOBS[@]}"

for job in "${POSITIVE_JOBS[@]}"; do
    f_out="$WORK/fixture-$job.out"
    f_err="$WORK/fixture-$job.err"
    f_rc=0
    (
        env MACHINE_OPS_TOOL_ROOT="$TOOL_ROOT" \
            MACHINE_OPS_AUTOMATIONS_ROOT="$POS_ROOT" \
            MACHINE_OPS_LOG="$WORK/fixture-selftest.log" \
            MACHINE_OPS_DRY_RUN=1 \
            HOME="$HOME_DIR" \
            bash "$FIXTURE_DIR/$job"
    ) >"$f_out" 2>"$f_err" || f_rc=$?
    if (( f_rc != 0 )); then
        printf 'silent-contract: fixture %s exited %s\nstderr:\n' "$job" "$f_rc" >&2
        cat "$f_err" >&2
        fail "fixture $job is not runnable"
    fi
    if [[ -s "$f_out" ]]; then
        printf 'silent-contract: fixture %s wrote to stdout:\n' "$job" >&2
        cat "$f_out" >&2
        fail "fixture $job is not silent on its own"
    fi
done

# ---------------------------------------------------------------------------
# 2. The contract: bin/run daily --dry-run against the synthetic root must
#    print NOTHING to stdout.
# ---------------------------------------------------------------------------
POS_OUT="$WORK/positive.out"
POS_ERR="$WORK/positive.err"
run_runner "$POS_ROOT" "$POS_OUT" "$POS_ERR"

assert_tool_root_clean "positive case"

if (( RUNNER_RC != 0 )); then
    printf 'silent-contract: bin/run exited %s\nstderr:\n' "$RUNNER_RC" >&2
    cat "$POS_ERR" >&2
    if grep -q -F 'jobs directory missing' "$POS_ERR"; then
        printf 'hint: bin/run did not honour MACHINE_OPS_AUTOMATIONS_ROOT (%s).\n' "$POS_ROOT" >&2
    fi
    fail "bin/run did not complete against the synthetic automations root"
fi

if [[ -s "$POS_OUT" ]]; then
    printf 'silent-contract: stdout was NOT empty (%s bytes).\n' "$(wc -c <"$POS_OUT" | tr -d ' ')" >&2
    printf 'The alert channel must stay silent on a nominal run. Unexpected stdout:\n' >&2
    cat "$POS_OUT" >&2
    fail "nominal run printed to stdout"
fi

# The run log lives in the automations root (never in the tool root) and
# records every job, so it proves the synthetic fixtures are what actually
# ran — otherwise a runner that silently ignored the root could pass this
# gate by doing nothing at all.
POS_LOG="$POS_ROOT/logs/machine-ops.log"
if [[ ! -f "$POS_LOG" ]]; then
    fail "no run log at $POS_LOG — the automations root owns logs/"
fi
for job in "${POSITIVE_JOBS[@]}"; do
    if ! grep -q -F -- "$job" "$POS_LOG"; then
        fail "the run log never mentions $job — the runner did not run the synthetic fixtures"
    fi
done

# ---------------------------------------------------------------------------
# 3. Negative control: the same harness against a job that prints to stdout
#    MUST report non-silence. Its own temp root, so it cannot pollute the
#    positive assertion above.
# ---------------------------------------------------------------------------
NEG_ROOT="$WORK/negative"
build_root "$NEG_ROOT" "$NEGATIVE_JOB"

NEG_OUT="$WORK/negative.out"
NEG_ERR="$WORK/negative.err"
run_runner "$NEG_ROOT" "$NEG_OUT" "$NEG_ERR"

assert_tool_root_clean "negative case"

if [[ ! -s "$NEG_OUT" ]]; then
    printf 'silent-contract: negative control NOT detected.\n' >&2
    printf 'The runner produced no stdout for a job that printed to stdout — this harness cannot fail, so it is worthless.\n' >&2
    fail "negative control did not detect a stdout violation"
fi

printf 'silent-contract: ok — nominal run printed nothing to stdout; negative control detected (%s bytes surfaced).\n' \
    "$(wc -c <"$NEG_OUT" | tr -d ' ')"
