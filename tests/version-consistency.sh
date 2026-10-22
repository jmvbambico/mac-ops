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
# The extraction itself lives in tests/lib/version-consistency.py rather than
# in a here-document below. bash 3.2 — the /bin/bash on macOS and on GitHub's
# macOS runners — mis-parses a here-document nested inside a command
# substitution: it loses the heredoc boundary and resumes parsing the heredoc
# body as shell code, so the gate fails to parse on the very shells it is
# meant to guard. A separate file has no such hazard, and the path is resolved
# from TOOL_ROOT so the gate runs from any working directory.
#
# Silent on success (AGENTS.md Principle I): a nominal run prints NOTHING.

set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! EXTRACTED="$(python3 "$TOOL_ROOT/tests/lib/version-consistency.py" "$TOOL_ROOT")"; then
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
