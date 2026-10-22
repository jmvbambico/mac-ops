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
#   console/src-tauri/Cargo.lock        (this crate's own resolved entry)
#   console/package-lock.json           (the root package entry)
#
# A grep for the first "version" in a lock file would find a dependency
# instead. package-lock.json is read structurally, via the root package entry
# ("packages[""]"), never a dependency.
#
# Cargo.lock is NOT parsed here at all. Its text gives a workspace member and a
# path dependency identical shape (both are source-less [[package]] blocks), so
# no pattern over it can prove which block is THIS crate. cargo can: it knows
# the workspace graph. `cargo metadata --no-deps` yields the package's canonical
# path-qualified ID, and `cargo pkgid --locked` resolves that ID to the version
# recorded in Cargo.lock. A same-named crates.io or path dependency therefore
# cannot be selected by mistake — cargo resolves by package identity, not name.
#
# This depends on one Cargo output format: the Package ID Spec
# `path+file:///abs/dir#name@version` that `cargo metadata` emits in
# `packages[].id` and `cargo pkgid` prints. Per `cargo help pkgid` they share a
# single specifier format, so the two steps agree by construction. Cargo 1.77.0
# stabilized that format (see the Cargo CHANGELOG); older cargo printed the
# legacy `name 0.4.0 (path+file:///abs/dir)`, which has no `@`. There
# `rsplit('@', 1)[0]` yields the whole string, the `'#' not in spec` check
# fires, and the gate fails — it stops loudly rather than selecting the wrong
# package. That is the intended fail-safe direction, though it would block CI
# on such a toolchain. Known-good: cargo 1.99.1, against which this gate was
# developed and verified; CI installs the toolchain in the workflow's
# `Install Rust toolchain` step.
#
# Consequence: this gate needs the Rust toolchain on PATH (CI installs it and
# the rust-check gate already runs cargo). If cargo is missing the gate FAILS
# loudly rather than quietly checking four surfaces while claiming five.
#
# python3 is used for reliable JSON extraction; it is part of the base system
# on macOS and on the GitHub runners.
#
# Silent on success (AGENTS.md Principle I): a nominal run prints NOTHING.

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! EXTRACTED="$(python3 - "$TOOL_ROOT" <<'PY'
import json
import os
import re
import shutil
import subprocess
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


def run_cargo(args, cwd):
    cargo = shutil.which('cargo')
    if not cargo:
        fail('cargo is not on PATH; cannot check console/src-tauri/Cargo.lock '
             '(the Rust toolchain is required)')
    try:
        return subprocess.run(
            [cargo] + args,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        fail('cannot run cargo %s: %s' % (args[0], exc))


def cargo_lock_version(root, package_name):
    # Cargo.lock is authoritative here only if cargo reads it. `cargo metadata
    # --no-deps --locked` needs the manifest but does NOT consult the lock (it
    # reports the manifest version), so it is used solely to obtain the
    # package's canonical, path-qualified ID. `cargo pkgid --locked` takes that
    # ID and returns the version recorded in Cargo.lock — the resolved one.
    # Both run --offline: neither needs the registry index, and the gate must
    # never require network access.
    src_tauri = os.path.join(root, 'console/src-tauri')
    meta = run_cargo(
        ['metadata', '--no-deps', '--format-version', '1', '--locked', '--offline'],
        src_tauri,
    )
    if meta.returncode != 0:
        detail = (meta.stderr or '').strip() or '(no stderr)'
        fail('cargo metadata failed, so Cargo.toml and '
             'console/src-tauri/Cargo.lock disagree or are unreadable: %s' % detail)
    try:
        data = json.loads(meta.stdout)
    except ValueError as exc:
        fail('cargo metadata produced invalid JSON: %s' % exc)

    # With --no-deps, packages[] holds only this workspace's own package(s).
    matches = [p for p in data.get('packages', [])
               if p.get('name') == package_name and isinstance(p.get('id'), str)]
    if not matches:
        fail('cargo metadata reports no workspace package named %s' % package_name)
    if len(matches) > 1:
        fail('cargo metadata reports %d workspace packages named %s; '
             'cannot pick the resolved version' % (len(matches), package_name))
    # The ID looks like `path+file:///abs/dir#name@version`. Drop the trailing
    # version so pkgid resolves it from Cargo.lock instead of the manifest.
    spec = matches[0]['id'].rsplit('@', 1)[0]
    if '#' not in spec:
        fail('cargo metadata returned an unexpected package ID: %s'
             % matches[0]['id'])

    pkg = run_cargo(['pkgid', '--locked', '--offline', spec], src_tauri)
    if pkg.returncode != 0:
        detail = (pkg.stderr or '').strip() or '(no stderr)'
        fail('cargo pkgid failed, so Cargo.lock has no entry for %s: %s'
             % (package_name, detail))
    resolved = (pkg.stdout or '').strip()
    # `path+file:///abs/dir#name@version`
    if '#' not in resolved or '@' not in resolved.rsplit('#', 1)[1]:
        fail('cargo pkgid returned an unexpected package ID: %s' % resolved)
    return resolved.rsplit('@', 1)[1]


root = sys.argv[1]
package_json = os.path.join(root, 'console/package.json')
tauri_conf = os.path.join(root, 'console/src-tauri/tauri.conf.json')
cargo_toml = os.path.join(root, 'console/src-tauri/Cargo.toml')
npm_lock = os.path.join(root, 'console/package-lock.json')

crate = cargo_package_name(cargo_toml)
surfaces = [
    ('console/package.json', json_version(package_json)),
    ('console/src-tauri/tauri.conf.json', json_version(tauri_conf)),
    ('console/src-tauri/Cargo.toml', cargo_package_version(cargo_toml)),
    ('console/src-tauri/Cargo.lock', cargo_lock_version(root, crate)),
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
