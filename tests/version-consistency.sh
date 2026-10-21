#!/usr/bin/env bash
#
# tests/version-consistency.sh — the five version surfaces must agree.
#
# The release version is recorded in five places, and nothing else checks that
# they stay in step:
#
#   console/package.json
#   console/src-tauri/tauri.conf.json
#   console/src-tauri/Cargo.toml
#   console/src-tauri/Cargo.lock        (this crate's own entry)
#   console/package-lock.json           (the root package entry)
#
# A grep for the first "version" in a lock file would find a dependency
# instead, so both lock files are read structurally: Cargo.lock by locating the
# [[package]] block whose name matches the crate, package-lock.json by reading
# the root package entry ("packages[""]"), never a dependency. python3 is used
# for reliable JSON and block-aware TOML extraction; it is part of the base
# system on macOS and on the GitHub runners.
#
# Silent on success (AGENTS.md Principle I): a nominal run prints NOTHING.

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! EXTRACTED="$(python3 - "$TOOL_ROOT" <<'PY'
import json
import os
import re
import sys


def fail(message):
    sys.stderr.write('version-consistency: %s\n' % message)
    raise SystemExit(1)


def json_version(path):
    try:
        with open(path, encoding='utf-8') as fh:
            data = json.load(fh)
    except (OSError, ValueError) as exc:
        fail('cannot read %s: %s' % (path, exc))
    version = data.get('version')
    if not isinstance(version, str) or not version:
        fail('%s has no top-level version string' % path)
    return version


def npm_root_version(path):
    try:
        with open(path, encoding='utf-8') as fh:
            data = json.load(fh)
    except (OSError, ValueError) as exc:
        fail('cannot read %s: %s' % (path, exc))
    # The root package entry, not a dependency's entry.
    root = data.get('packages', {}).get('')
    if isinstance(root, dict) and isinstance(root.get('version'), str):
        return root['version']
    # Lockfile v1 has no packages map; the top level is the root package.
    if isinstance(data.get('version'), str):
        return data['version']
    fail('%s has no root package version entry' % path)


def cargo_package_name(path):
    section = None
    try:
        with open(path, encoding='utf-8') as fh:
            for raw in fh:
                line = raw.strip()
                if line.startswith('[') and line.endswith(']'):
                    section = line[1:-1].strip()
                    continue
                if section == 'package':
                    match = re.match(r'name\s*=\s*"([^"]*)"', line)
                    if match:
                        return match.group(1)
    except OSError as exc:
        fail('cannot read %s: %s' % (path, exc))
    fail('%s has no [package] name' % path)


def cargo_package_version(path):
    # Handles only a literal `version = "..."` under [package]. A manifest
    # using `version.workspace = true` makes this FAIL rather than silently
    # return a wrong value, which is the fail-safe direction.
    section = None
    try:
        with open(path, encoding='utf-8') as fh:
            for raw in fh:
                line = raw.strip()
                if line.startswith('[') and line.endswith(']'):
                    section = line[1:-1].strip()
                    continue
                if section == 'package':
                    match = re.match(r'version\s*=\s*"([^"]*)"', line)
                    if match:
                        return match.group(1)
    except OSError as exc:
        fail('cannot read %s: %s' % (path, exc))
    fail('%s has no [package] version' % path)


def cargo_lock_version(path, package_name):
    try:
        with open(path, encoding='utf-8') as fh:
            text = fh.read()
    except OSError as exc:
        fail('cannot read %s: %s' % (path, exc))
    # Inspect the [[package]] block for THIS crate AND this kind of package.
    # The crate name alone is not enough: `console` is also a crates.io crate,
    # so a dependency could introduce a second block with the same name. A
    # workspace member / path package has NO `source` key, while every registry
    # dependency has `source = "registry+..."`; that is the discriminator. We
    # must NOT select by the Cargo.toml version — detecting a stale lock is one
    # of the disagreements this gate exists to catch.
    matches = []
    for block in text.split('[[package]]')[1:]:
        # A block's keys end at the next section header; the final block runs
        # to EOF otherwise, so truncate before reading any key.
        block = block.split('[[', 1)[0]
        name = re.search(r'^name\s*=\s*"([^"]*)"', block, re.MULTILINE)
        if not name or name.group(1) != package_name:
            continue
        if re.search(r'^source\s*=', block, re.MULTILINE):
            continue
        version = re.search(r'^version\s*=\s*"([^"]*)"', block, re.MULTILINE)
        if not version:
            fail('%s has a source-less [[package]] for %s without a version'
                 % (path, package_name))
        matches.append(version.group(1))
    if not matches:
        fail('%s has no source-less [[package]] entry for %s'
             % (path, package_name))
    if len(matches) > 1:
        fail('%s has %d source-less [[package]] entries for %s: %s'
             % (path, len(matches), package_name, ', '.join(matches)))
    return matches[0]


root = sys.argv[1]
package_json = os.path.join(root, 'console/package.json')
tauri_conf = os.path.join(root, 'console/src-tauri/tauri.conf.json')
cargo_toml = os.path.join(root, 'console/src-tauri/Cargo.toml')
cargo_lock = os.path.join(root, 'console/src-tauri/Cargo.lock')
npm_lock = os.path.join(root, 'console/package-lock.json')

crate = cargo_package_name(cargo_toml)
surfaces = [
    ('console/package.json', json_version(package_json)),
    ('console/src-tauri/tauri.conf.json', json_version(tauri_conf)),
    ('console/src-tauri/Cargo.toml', cargo_package_version(cargo_toml)),
    ('console/src-tauri/Cargo.lock', cargo_lock_version(cargo_lock, crate)),
    ('console/package-lock.json', npm_root_version(npm_lock)),
]

for path, version in surfaces:
    print('%s\t%s' % (path, version))
PY
)"; then
    printf 'version-consistency: FAIL: could not extract a version from every surface\n' >&2
    exit 1
fi

paths=()
versions=()
while IFS=$'\t' read -r path version; do
    if [[ -n "$path" ]]; then
        paths+=("$path")
        versions+=("$version")
    fi
done <<< "$EXTRACTED"

if (( ${#versions[@]} != 5 )); then
    printf 'version-consistency: FAIL: expected 5 version surfaces, extracted %d\n' \
        "${#versions[@]}" >&2
    exit 1
fi

mismatch=0
for (( i = 1; i < ${#versions[@]}; i++ )); do
    if [[ "${versions[$i]}" != "${versions[0]}" ]]; then
        mismatch=1
    fi
done

if (( mismatch )); then
    printf 'version-consistency: FAIL: version surfaces disagree:\n' >&2
    for i in "${!versions[@]}"; do
        printf '  %s: %s\n' "${paths[$i]}" "${versions[$i]}" >&2
    done
    exit 1
fi

# Silent on success (AGENTS.md Principle I).
exit 0
