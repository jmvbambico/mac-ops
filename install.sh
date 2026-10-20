#!/usr/bin/env bash
#
# install.sh — install (or refresh) the machine-ops user LaunchAgent.
#
# Copies launchagents/ai.machineops.daily.plist into
# ~/Library/LaunchAgents/ with the repo's absolute paths substituted,
# then unloads any previous copy and bootstraps the new one.
#
# Idempotent: safe to re-run after pulling updates or moving the repo
# (re-run after a move so the plist picks up the new absolute path).
#
# This installs a *user* LaunchAgent, never a system LaunchDaemon, and
# never uses sudo: mole refuses to run as root, so the job must run as you.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LABEL="ai.machineops.daily"
SRC="$REPO_ROOT/launchagents/$LABEL.plist"
DEST="$HOME/Library/LaunchAgents/$LABEL.plist"

if [[ ! -f "$SRC" ]]; then
    printf 'install.sh: template missing: %s\n' "$SRC" >&2
    exit 1
fi

mkdir -p "$HOME/Library/LaunchAgents" "$REPO_ROOT/logs"

# Render absolute paths into the installed copy (a symlink could not do this).
# Escape a path for the sed replacement (delimiter |): backslashes FIRST,
# or the escapes added for & and | would be doubled.
sed_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/&/\\&/g' -e 's/|/\\|/g'
}
esc_repo="$(sed_escape "$REPO_ROOT")"
esc_home="$(sed_escape "$HOME")"
sed -e "s|__REPO_ROOT__|$esc_repo|g" -e "s|__HOME__|$esc_home|g" "$SRC" > "$DEST"
chmod 644 "$DEST"

UID_NUM="$(id -u)"
# Unload any previous copy; bootout fails when nothing is loaded — ignore it.
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID_NUM" "$DEST"

printf 'Installed %s\n' "$DEST"
printf '\nVerify:\n'
printf '  launchctl print gui/%s/%s | head -30\n' "$UID_NUM" "$LABEL"
printf '  tail -f %s/logs/machine-ops.log\n' "$REPO_ROOT"
printf '\nTrigger now without waiting for 02:00:\n'
printf '  launchctl kickstart -k gui/%s/%s\n' "$UID_NUM" "$LABEL"
printf '\nUninstall:\n'
printf '  launchctl bootout gui/%s/%s; rm %s\n' "$UID_NUM" "$LABEL" "$DEST"
