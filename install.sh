#!/usr/bin/env bash
#
# install.sh — install (or refresh) the machine-ops user LaunchAgent.
#
# Copies launchagents/ai.machineops.daily.plist into
# ~/Library/LaunchAgents/ with the tool's absolute path and the user's
# automations root substituted, then unloads any previous copy and bootstraps
# the new one.
#
# The two roots are independent (AGENTS.md Principle II): the tool lives in
# this checkout, while jobs/, logs/ and state/ belong to the automations root.
# The automations root is resolved with the same precedence bin/run uses:
#   1. $MOPS_AUTOMATIONS_ROOT (legacy: $MACHINE_OPS_AUTOMATIONS_ROOT)
#   2. automations_root in ~/.config/mops/config.json
#   3. $HOME/.mops
# and is pinned into the plist so launchd and bin/run agree on it.
#
# Idempotent: safe to re-run after pulling updates or moving the repo
# (re-run after a move so the plist picks up the new absolute path, and after
# changing the configured automations root).
#
# This installs a *user* LaunchAgent, never a system LaunchDaemon, and
# never uses sudo: mole refuses to run as root, so the job must run as you.

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LABEL="ai.machineops.daily"
SRC="$TOOL_ROOT/launchagents/$LABEL.plist"
DEST="$HOME/Library/LaunchAgents/$LABEL.plist"

# shellcheck disable=SC1091 # path resolved at runtime from the script location
if ! source "$TOOL_ROOT/lib/common.sh"; then
    printf 'install.sh: cannot load lib/common.sh\n' >&2
    exit 1
fi

if ! AUTOMATIONS_ROOT="$(machine_ops_automations_root)"; then
    printf 'install.sh: cannot resolve the automations root\n' >&2
    exit 1
fi
export MOPS_AUTOMATIONS_ROOT="$AUTOMATIONS_ROOT"
# Legacy alias, same value until 0.4.0 (the owner's separate automations repo
# still reads MACHINE_OPS_*).
export MACHINE_OPS_AUTOMATIONS_ROOT="$AUTOMATIONS_ROOT"

if [[ ! -f "$SRC" ]]; then
    printf 'install.sh: template missing: %s\n' "$SRC" >&2
    exit 1
fi

mkdir -p "$HOME/Library/LaunchAgents" \
    "$AUTOMATIONS_ROOT/jobs" "$AUTOMATIONS_ROOT/logs" "$AUTOMATIONS_ROOT/state"

# Render absolute paths into the installed copy (a symlink could not do this).
# Escape a path for the sed replacement (delimiter |): backslashes FIRST,
# or the escapes added for & and | would be doubled.
sed_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/&/\\&/g' -e 's/|/\\|/g'
}
esc_tool="$(sed_escape "$TOOL_ROOT")"
esc_auto="$(sed_escape "$AUTOMATIONS_ROOT")"
esc_home="$(sed_escape "$HOME")"
sed -e "s|__REPO_ROOT__|$esc_tool|g" \
    -e "s|__AUTOMATIONS_ROOT__|$esc_auto|g" \
    -e "s|__HOME__|$esc_home|g" "$SRC" > "$DEST"
chmod 644 "$DEST"

UID_NUM="$(id -u)"
# Unload any previous copy; bootout fails when nothing is loaded — ignore it.
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID_NUM" "$DEST"

printf 'Installed %s\n' "$DEST"
printf 'Automations root: %s\n' "$AUTOMATIONS_ROOT"
printf '\nVerify:\n'
printf '  launchctl print gui/%s/%s | head -30\n' "$UID_NUM" "$LABEL"
printf '  tail -f %s/logs/machine-ops.log\n' "$AUTOMATIONS_ROOT"
printf '\nTrigger now without waiting for 02:00:\n'
printf '  launchctl kickstart -k gui/%s/%s\n' "$UID_NUM" "$LABEL"
printf '\nUninstall:\n'
printf '  launchctl bootout gui/%s/%s; rm %s\n' "$UID_NUM" "$LABEL" "$DEST"
