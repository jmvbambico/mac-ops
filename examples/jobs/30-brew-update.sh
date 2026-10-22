#!/usr/bin/env bash
# tier: daily
# timeout: 3600
# machine-ops job: brew update/upgrade watchdog.
#
# Refreshes the Homebrew index (`brew update`), then upgrades outdated
# formulae EXCEPT the HOLD list below, then `brew cleanup`. The 1h timeout
# is generous on purpose: 33 outdated formulae can compile from source, and
# a mid-upgrade process-group kill is worse than a slow run — only a genuine
# hang should trip it.
#
# HOLD list: these formulae are never upgraded unattended.
# - `node` is the runtime for the user's global npm packages — the agent
#   fleet (opencode, cline, kilocode, …) — and an unattended node major
#   upgrade can break the whole fleet at 02:00.
# - `mole` is the tool these very jobs depend on: mole 1.54.0's exact
#   behaviour (health_score JSON keys, non-interactive-when-no-TTY clean)
#   is load-bearing, and an unattended upgrade could invalidate it silently.
# - `tor` fronts the user's LLM gateway (the whole agent fleet routes
#   through it); an unattended tor upgrade could cut the fleet off.
# Held-but-outdated packages are log-only, never an alert: they would
# otherwise alert every single night forever.
#
# Casks are report-only (never upgraded unattended): a cask upgrade can quit
# a running app and some need authentication.
#
# Offline (laptop with no network at 02:00) skips SILENTLY: a nightly false
# alarm for being offline would poison the alert channel.
#
# Alerts on `brew update`, `brew outdated --formula`, or `brew upgrade`
# failure (with the last ~10 lines of the failing output). An outdated
# query that fails must alert LOUD: silently skipping every upgrade while
# looking successful is the failure this project exists to prevent — so a
# non-zero outdated rc skips the upgrade AND the cleanup. (Zero outdated
# with rc=0 is the legitimate empty case: silent, cleanup still allowed.)
# `brew cleanup` failure is log-only — cleanup failing is not worth waking
# anyone. A failed `brew outdated --cask` is log-only too (report-only
# data) but is recorded as unknown, never as 0.
# A one-line summary always goes to the run log.
#
# MOPS_DRY_RUN=1: dry run mutates nothing — no `brew update`, no
# `brew upgrade`, no `brew cleanup`. The outdated lists are read from the
# existing index (which is NOT refreshed) and the summary logs `planned=`
# instead of `upgraded=`.
# Never sudo, never a password. stdin is /dev/null on every brew invocation.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# lib/common.sh belongs to the TOOL root, which may differ from this job's
# automations root. bin/run and bin/run-job export MOPS_TOOL_ROOT.
MACOPS_LIB="${MOPS_TOOL_ROOT:-$SCRIPT_DIR/..}/lib"
# shellcheck disable=SC1091 # path resolved at runtime from the tool root
if ! source "$MACOPS_LIB/common.sh"; then
    printf '30-brew-update: cannot load lib/common.sh\n' >&2
    exit 1
fi

# Test seam: point MOPS_BREW at a stub to exercise apply paths without
# touching the live Homebrew installation.
BREW="${MOPS_BREW:-/opt/homebrew/bin/brew}"
LABEL="brew-update"
DRY="${MOPS_DRY_RUN:-0}"

# Formulae never upgraded unattended. One entry per line, each with its why.
HOLD_FORMULAE=(
    # node: runtime for the global npm agent fleet (opencode, cline,
    # kilocode, … — 13 globals). An unattended node major upgrade can break
    # the whole fleet at once; upgrade it by hand instead.
    node
    # mole: these very jobs depend on mole 1.54.0's exact behaviour
    # (health_score JSON keys, non-interactive-when-no-TTY clean). An
    # unattended upgrade could invalidate all of it silently.
    mole
    # tor: fronts the user's LLM gateway, which the whole agent fleet
    # routes through. An unattended tor upgrade could cut the fleet off.
    tor
)

# is_online — cheap bounded reachability probe for the brew index host
# (DNS + TCP 443, not a full brew call). ping to a bare IP is only the
# fallback when nc is absent. Bare command names (never absolute paths) so
# a PATH stub can fake either for tests; MOPS_FORCE_OFFLINE=1 forces the
# offline path.
is_online() {
    if [[ "${MOPS_FORCE_OFFLINE:-0}" == "1" ]]; then
        return 1
    fi
    if have nc; then
        nc -z -G 3 -w 3 formulae.brew.sh 443 < /dev/null >/dev/null 2>&1
        return $?
    fi
    if ! have ping; then
        return 1
    fi
    ping -c 1 -t 2 1.1.1.1 < /dev/null >/dev/null 2>&1
}

# valid_pkg_name <name> — conservative allowlist for outdated-list entries.
# Names are letters, digits, and @ . + - _ , never starting with `-`, so a
# corrupt or hostile entry can never be passed to brew as an option.
# Anything else is dropped (log-only) before it reaches an argv.
valid_pkg_name() {
    [[ "$1" =~ ^[A-Za-z0-9@.+_][A-Za-z0-9@.+_-]*$ ]]
}

# is_held <name> — true when name is exactly a HOLD_FORMULAE entry.
is_held() {
    local needle="$1" h
    for h in "${HOLD_FORMULAE[@]}"; do
        if [[ "$h" == "$needle" ]]; then
            return 0
        fi
    done
    return 1
}

if [[ ! -x "$BREW" ]]; then
    alert "[$LABEL] brew not found at $BREW"
    log_line "[$LABEL] brew missing at $BREW dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

if ! is_online; then
    log_line "[$LABEL] skipped: offline (no route to formulae.brew.sh, staying silent) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

# Dry run refreshes nothing: `brew update` rewrites Homebrew's local
# repo/API metadata, so it is not read-only and has no place in a preview.
# Work from whatever index state already exists.
if [[ "$DRY" == "1" ]]; then
    log_line "[$LABEL] dry run: index not refreshed (brew update skipped)"
else
    UPDATE_OUT="$("$BREW" update < /dev/null 2>&1)"
    UPDATE_RC=$?
    UPDATE_CLEAN="$(printf '%s\n' "$UPDATE_OUT" | strip_ansi)"

    if [[ "$UPDATE_RC" != "0" ]]; then
        alert "[$LABEL] brew update exited rc=$UPDATE_RC (dry_run=$DRY)"
        alert "$(printf '%s\n' "$UPDATE_CLEAN" | tail -n 10)"
        log_line "[$LABEL] brew update failed rc=$UPDATE_RC dry_run=$DRY"
        log_line "[$LABEL] upgraded=0 held=unknown casks_outdated=unknown cleanup=skipped rc=$UPDATE_RC dry_run=$DRY"
        log_line "[$LABEL] update output tail: $(printf '%s\n' "$UPDATE_CLEAN" | tail -n 30 | tr '\n' ';')"
        flush_alerts "$LABEL"
        exit 0
    fi
fi

# A failed outdated query is a HARD failure, not an empty list: with no
# trustworthy list there is nothing safe to upgrade, so the upgrade AND the
# cleanup are both skipped and the run alerts. (An empty list with rc=0 is
# the legitimate "nothing outdated" case and stays on the silent path.)
OUTDATED_RAW="$("$BREW" outdated --formula --quiet < /dev/null 2>&1)"
OUTDATED_RC=$?
if [[ "$OUTDATED_RC" != "0" ]]; then
    OUTDATED_CLEAN="$(printf '%s\n' "$OUTDATED_RAW" | strip_ansi)"
    alert "[$LABEL] brew outdated --formula exited rc=$OUTDATED_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$OUTDATED_CLEAN" | tail -n 10)"
    log_line "[$LABEL] outdated query failed rc=$OUTDATED_RC; upgrade and cleanup skipped (dry_run=$DRY)"
    if [[ "$DRY" == "1" ]]; then
        log_line "[$LABEL] planned=0 held=unknown casks_outdated=unknown cleanup=skipped rc=$OUTDATED_RC dry_run=$DRY"
    else
        log_line "[$LABEL] upgraded=0 held=unknown casks_outdated=unknown cleanup=skipped rc=$OUTDATED_RC dry_run=$DRY"
    fi
    log_line "[$LABEL] outdated output tail: $(printf '%s\n' "$OUTDATED_CLEAN" | tail -n 30 | tr '\n' ';')"
    flush_alerts "$LABEL"
    exit 0
fi

OUTDATED_LIST=()
while IFS= read -r line || [[ -n "$line" ]]; do
    pkg="$(printf '%s' "$line" | tr -d '[:space:]')"
    if [[ -z "$pkg" ]]; then
        continue
    fi
    if valid_pkg_name "$pkg"; then
        OUTDATED_LIST+=("$pkg")
    else
        log_line "[$LABEL] dropped invalid package name from outdated list: $pkg (log-only, never passed to brew)"
    fi
done <<< "$OUTDATED_RAW"

# Subtract the HOLD list: held names are upgraded never, passed never.
# The upgrade below always names packages explicitly — never bare
# `brew upgrade`, which would take everything including the held ones.
TO_UPGRADE=()
HELD_OUTDATED=()
if (( ${#OUTDATED_LIST[@]} > 0 )); then
    for pkg in "${OUTDATED_LIST[@]}"; do
        if is_held "$pkg"; then
            HELD_OUTDATED+=("$pkg")
        else
            TO_UPGRADE+=("$pkg")
        fi
    done
fi
if (( ${#HELD_OUTDATED[@]} > 0 )); then
    log_line "[$LABEL] held (outdated but never auto-upgraded): ${HELD_OUTDATED[*]}"
fi

UPGRADE_RC=0
UPGRADED_N=0
PLANNED_N=0
if (( ${#TO_UPGRADE[@]} == 0 )); then
    log_line "[$LABEL] nothing to upgrade (outdated=${#OUTDATED_LIST[@]} all held or none)"
elif [[ "$DRY" == "1" ]]; then
    # Dry run issues no upgrade at all — not even --dry-run: the plan is
    # computed from the outdated list above, nothing more.
    PLANNED_N=${#TO_UPGRADE[@]}
    log_line "[$LABEL] dry run: would upgrade ${PLANNED_N}: ${TO_UPGRADE[*]} (no brew upgrade issued)"
else
    # `--` ends option parsing (verified against `brew upgrade --help`):
    # belt and braces behind the name validation above.
    log_line "[$LABEL] upgrade argv: brew upgrade --formula -- ${TO_UPGRADE[*]}"
    UP_OUT="$("$BREW" upgrade --formula -- "${TO_UPGRADE[@]}" < /dev/null 2>&1)"
    UPGRADE_RC=$?
    if [[ "$UPGRADE_RC" == "0" ]]; then
        UPGRADED_N=${#TO_UPGRADE[@]}
    fi
    UP_CLEAN="$(printf '%s\n' "$UP_OUT" | strip_ansi)"
    if [[ "$UPGRADE_RC" != "0" ]]; then
        alert "[$LABEL] brew upgrade exited rc=$UPGRADE_RC (dry_run=$DRY)"
        alert "$(printf '%s\n' "$UP_CLEAN" | tail -n 10)"
        log_line "[$LABEL] upgrade output tail: $(printf '%s\n' "$UP_CLEAN" | tail -n 30 | tr '\n' ';')"
    fi
fi

# Casks are report-only: upgrading one unattended can quit a running app or
# stall on an auth prompt, so only the count is logged.
# Note: this machine carries 2 stale Docker cask receipts whose app is
# already gone, so the count below can over-report.
CASK_RAW="$("$BREW" outdated --cask --quiet < /dev/null 2>/dev/null)"
CASK_RC=$?
CASKS="unknown"
if [[ "$CASK_RC" == "0" ]]; then
    CASKS="$(printf '%s\n' "$CASK_RAW" | grep -c '[^[:space:]]' || true)"
else
    log_line "[$LABEL] brew outdated --cask failed rc=$CASK_RC (log-only)"
fi

# Cleanup runs only on the success path in apply mode: after a failed
# upgrade the operator looks first and reclaims disk second. Dry-run skips
# it entirely. Failure is log-only, never an alert.
CLEANUP="skipped"
if [[ "$DRY" != "1" && "$UPGRADE_RC" == "0" ]]; then
    CLEAN_OUT="$("$BREW" cleanup < /dev/null 2>&1)"
    CLEAN_RC=$?
    CLEAN_CLEAN="$(printf '%s\n' "$CLEAN_OUT" | strip_ansi)"
    if [[ "$CLEAN_RC" == "0" ]]; then
        FREED="$(printf '%s\n' "$CLEAN_CLEAN" | grep -o -E -i -m1 'freed approximately [^.]+|freed [^,;.]+' | head -n1 || true)"
        FREED="$(printf '%s' "$FREED" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
        if [[ -z "$FREED" ]]; then
            FREED="done"
        fi
        CLEANUP="$FREED"
    else
        CLEANUP="failed(rc=$CLEAN_RC)"
        log_line "[$LABEL] brew cleanup failed rc=$CLEAN_RC (log-only, not an alert)"
        log_line "[$LABEL] cleanup output tail: $(printf '%s\n' "$CLEAN_CLEAN" | tail -n 30 | tr '\n' ';')"
    fi
fi

if [[ "$DRY" == "1" ]]; then
    log_line "[$LABEL] planned=${PLANNED_N} held=${#HELD_OUTDATED[@]} casks_outdated=${CASKS} cleanup=${CLEANUP} rc=${UPGRADE_RC} dry_run=${DRY}"
else
    log_line "[$LABEL] upgraded=${UPGRADED_N} held=${#HELD_OUTDATED[@]} casks_outdated=${CASKS} cleanup=${CLEANUP} rc=${UPGRADE_RC} dry_run=${DRY}"
fi

flush_alerts "$LABEL"
exit 0
