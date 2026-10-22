#!/usr/bin/env python3
"""Extract the five version surfaces for tests/version-consistency.sh.

This lives in its own file rather than a here-document inside the calling
shell script because bash 3.2 (the /bin/bash on macOS, and on GitHub's macOS
runners) mis-parses a here-document nested inside a command substitution: it
loses the heredoc boundary and resumes parsing the heredoc body as shell code.
The extraction logic is unchanged by that move.

The rationale for the Cargo.lock handling — why cargo resolves package
identity instead of the file being text-parsed — is documented in the header
of tests/version-consistency.sh.

Usage: version-consistency.py <tool-root>

Prints one "<path>\t<version>" line per surface to stdout. On any failure it
writes a diagnostic to stderr and exits non-zero.
"""

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
