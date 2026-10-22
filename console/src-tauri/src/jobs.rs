//! Job scripts on disk: discovery, metadata, templates, validation.
//!
//! The metadata contract is already established by bin/run and must be
//! parsed with the SAME anchored rules, or the console and the dispatcher
//! will disagree about what runs:
//!   line 2: `# tier: <name>`     (missing/malformed ALERTS in bin/run)
//!   line 3: `# timeout: <secs>`  (optional; missing means the 900s default,
//!                                 accepted range 1..=86400)
//! Anchored full-line match only — `echo "# tier: daily"` must NOT validate.
use crate::types::{NewJob, RunOutcome, Status};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct JobMeta {
    pub file: String,
    pub label: String,
    pub tier: Option<String>, // None = missing/malformed, surfaced loudly
    pub timeout: u32,         // resolved, default 900
    pub kind: String,         // inferred: mole | pkg | py | sh
}

pub const DEFAULT_TIMEOUT: u32 = 900;

/// The kinds `create` can generate, and the kinds `read_meta` can infer.
pub const KINDS: [&str; 4] = ["mole", "pkg", "py", "sh"];

// ---------------------------------------------------------------------------
// Anchored metadata parsing — must stay byte-compatible with bin/run.
//
// bin/run matches, with bash's own [[:space:]] class:
//   ^[[:space:]]*#[[:space:]]*tier:[[:space:]]*([a-z]+)[[:space:]]*$
//   ^[[:space:]]*#[[:space:]]*timeout:[[:space:]]*([0-9]+)[[:space:]]*$
// There is no regex crate here (and none may be added), so the same grammar
// is spelled out with plain char logic. Anchoring is the whole point:
// `echo "# tier: daily"` is executable code, not metadata, and must not
// validate — otherwise a job the dispatcher skips would look scheduled.
// ---------------------------------------------------------------------------

/// bash's `[[:space:]]` in the C locale, minus the newline (lines are
/// already split): space, tab, CR, vertical tab, form feed.
fn is_sp(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\u{b}' | '\u{c}')
}

/// `^[[:space:]]*#[[:space:]]*<key>:[[:space:]]*(VALUE)[[:space:]]*$`
/// Returns VALUE with surrounding space stripped, or None when the line is
/// not a whole-line metadata comment for `key`.
fn meta_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start_matches(is_sp);
    let rest = rest.strip_prefix('#')?;
    let rest = rest.trim_start_matches(is_sp);
    let rest = rest.strip_prefix(key)?; // case-sensitive, like the regex
    let rest = rest.strip_prefix(':')?;
    let value = rest.trim_start_matches(is_sp).trim_end_matches(is_sp);
    if value.is_empty() { None } else { Some(value) }
}

/// Line 2's tier, or None when missing/malformed (bin/run ALERTS on that;
/// the console renders it as "unknown" rather than inventing a tier).
pub fn parse_tier_line(line: &str) -> Option<String> {
    let v = meta_value(line, "tier")?;
    if v.chars().all(|c| c.is_ascii_lowercase()) {
        Some(v.to_string())
    } else {
        None
    }
}

/// Line 3's timeout override, or None when missing/malformed/out of range —
/// all of which mean "use the default" (never an alert), exactly as in
/// bin/run. The digit-length guard mirrors bin/run's own: it strips leading
/// zeros first, so `# timeout: 0900` is 900, while anything that needs more
/// than five digits is out of range before it is ever parsed.
pub fn parse_timeout_line(line: &str) -> Option<u32> {
    let v = meta_value(line, "timeout")?;
    if !v.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let stripped = v.trim_start_matches('0');
    let stripped = if stripped.is_empty() { "0" } else { stripped };
    if stripped.len() > 5 {
        return None;
    }
    let n: u32 = stripped.parse().ok()?;
    if (1..=86400).contains(&n) { Some(n) } else { None }
}

/// Optional `# kind: <name>` marker written by `create`, so a generated job
/// round-trips to the template it came from instead of being re-guessed.
/// Anchored like the rest, so `echo "# kind: pkg"` is not metadata.
fn parse_kind_marker(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(v) = meta_value(line, "kind") {
            if KINDS.contains(&v) {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Best-effort kind inference for hand-written jobs. Comments are dropped
/// first: `jobs/30-brew-update.sh` *talks about* mole in its HOLD-list
/// rationale, and a comment must never decide the kind.
fn infer_kind(content: &str) -> String {
    let code: String = content
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let low = code.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| low.contains(n));

    if code.contains("MOLE=") || code.contains("$MOLE") || has(&["mole optimize", "mole clean", "bin/mole"]) {
        "mole".into()
    } else if has(&["python3 -m venv", "/venv", "venv_dir", "pip install", ".py\"", ".py "]) {
        "py".into()
    } else if code.contains("BREW=")
        || code.contains("PKG_BIN")
        || has(&["brew ", "npm ", "npx ", "pipx ", "cargo ", "rustup ", "gem "])
    {
        "pkg".into()
    } else {
        "sh".into()
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// `root` is the AUTOMATIONS root, never the tool root: job scripts are the
/// user's data and live outside the installed tool.
fn jobs_dir(root: &Path) -> PathBuf {
    root.join("jobs")
}

/// Every jobs/*.sh in filename sort order.
pub fn scan(root: &Path) -> Result<Vec<JobMeta>> {
    let dir = jobs_dir(root);
    let mut files: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?;
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("sh") {
            continue;
        }
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(true)
        {
            continue;
        }
        files.push(path);
    }
    // Filename sort order IS run order for bin/run; keep the console honest.
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        out.push(read_meta(&f)?);
    }
    Ok(out)
}

pub fn read_meta(path: &Path) -> Result<JobMeta> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let content = String::from_utf8_lossy(&bytes).to_string();
    let file = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let label = file.strip_suffix(".sh").unwrap_or(&file).to_string();

    let mut lines = content.lines();
    let _line1 = lines.next();
    let tier = lines.next().and_then(parse_tier_line);
    let timeout = lines
        .next()
        .and_then(parse_timeout_line)
        .unwrap_or(DEFAULT_TIMEOUT);
    let kind = parse_kind_marker(&content).unwrap_or_else(|| infer_kind(&content));

    Ok(JobMeta { file, label, tier, timeout, kind })
}

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

/// Lowercase ASCII slug: alphanumerics kept, every other run collapsed to a
/// single dash, no leading/trailing dash.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else if c.is_alphabetic() || c.is_numeric() {
            // Non-ASCII letters have no safe filename spelling here.
            pending_dash = true;
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The two-digit prefixes already in use. Only a leading `NN-` counts.
fn used_numbers(dir: &Path) -> Vec<u32> {
    let mut used = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let name = match name.to_str() {
                Some(n) => n,
                None => continue,
            };
            let b = name.as_bytes();
            if b.len() >= 3 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b'-' {
                if let Ok(n) = name[0..2].parse::<u32>() {
                    used.push(n);
                }
            }
        }
    }
    used.sort_unstable();
    used.dedup();
    used
}

/// Pick a free `NN` prefix. Decades first (10, 20, 30 … — the convention in
/// `jobs/`, which leaves room to insert between jobs later), then any free
/// slot, so the number keeps meaning "run order".
fn next_number(dir: &Path) -> Result<u32> {
    let used = used_numbers(dir);
    for n in (10..=90).step_by(10) {
        if !used.contains(&n) {
            return Ok(n);
        }
    }
    for n in 1..=99u32 {
        if !used.contains(&n) {
            return Ok(n);
        }
    }
    bail!("no free NN- prefix left in {} (00-99 all taken)", dir.display())
}

/// Create jobs/<NN>-<slug>.sh from a template. The number prefix sets run
/// order and must not collide with an existing file. The generated script
/// MUST follow the house contract: `set -u`, source lib/common.sh, define
/// LABEL, honor MOPS_DRY_RUN=1, redirect stdin from /dev/null on any
/// command that could prompt, log_line the summary, alert ONLY on
/// change/regression/error, end with flush_alerts "$LABEL" and exit 0.
/// Never sudo. Returns the new filename.
pub fn create(root: &Path, spec: &NewJob) -> Result<String> {
    let dir = jobs_dir(root);
    if !dir.is_dir() {
        bail!("jobs directory missing: {}", dir.display());
    }
    if !KINDS.contains(&spec.kind.as_str()) {
        bail!("unknown job kind: {} (expected one of {:?})", spec.kind, KINDS);
    }
    let slug = {
        let s = slugify(&spec.title);
        if s.is_empty() { slugify(&spec.kind) } else { s }
    };
    let tier = spec.schedule.tier();
    let timeout = if (1..=86400).contains(&spec.timeout) {
        spec.timeout
    } else {
        DEFAULT_TIMEOUT
    };

    // Number first, then make sure the whole filename is free: a slug can
    // collide with an existing job carrying a different prefix.
    let mut number = next_number(&dir)?;
    let mut file = format!("{number:02}-{slug}.sh");
    let mut guard = 0;
    while dir.join(&file).exists() {
        guard += 1;
        if guard > 100 {
            bail!("could not find a free filename for slug {slug}");
        }
        let mut used = used_numbers(&dir);
        used.push(number);
        number = (1..=99u32)
            .find(|n| !used.contains(n))
            .ok_or_else(|| anyhow::anyhow!("no free NN- prefix left"))?;
        file = format!("{number:02}-{slug}.sh");
    }

    let label = file.strip_suffix(".sh").unwrap_or(&file).to_string();
    let body = render(spec, &file, &label, tier, timeout)?;
    let path = dir.join(&file);
    std::fs::write(&path, body.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    make_executable(&path)?;
    Ok(file)
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("chmod +x {}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Templates
//
// Every generated script is a watchdog first: silent on a nominal run,
// speaking only on change, regression, or error. The shapes here follow
// jobs/30-brew-update.sh and jobs/10-mole-optimize.sh deliberately — a new
// job should read like the ones already in the repo.
// ---------------------------------------------------------------------------

/// Shebang + the two anchored metadata lines + the kind marker + doc block.
/// `# tier:` is line 2 and `# timeout:` is line 3, exactly where bin/run
/// looks; nothing may be inserted above them.
fn header(file: &str, tier: &str, timeout: u32, kind: &str, title: &str, doc: &[&str]) -> String {
    let mut s = String::new();
    s.push_str("#!/usr/bin/env bash\n");
    s.push_str(&format!("# tier: {tier}\n"));
    s.push_str(&format!("# timeout: {timeout}\n"));
    s.push_str(&format!("# kind: {kind}\n"));
    s.push_str(&format!("# machine-ops job: {title}.\n"));
    s.push_str("#\n");
    for line in doc {
        if line.is_empty() {
            s.push_str("#\n");
        } else {
            s.push_str(&format!("# {line}\n"));
        }
    }
    s.push_str("#\n");
    s.push_str("# Silent on success: a nominal run prints NOTHING. Detail goes to the run\n");
    s.push_str("# log via log_line; alert only on change, regression, or error.\n");
    s.push_str("# MOPS_DRY_RUN=1 previews and mutates nothing.\n");
    s.push_str("# Never sudo, never a password. stdin is /dev/null on every command that\n");
    s.push_str("# could prompt.\n");
    s.push_str("\nset -u\n\n");
    s.push_str("SCRIPT_DIR=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)\"\n");
    s.push_str("# lib/common.sh belongs to the TOOL root, which may differ from this job's\n");
    s.push_str("# automations root. bin/run and bin/run-job export MOPS_TOOL_ROOT;\n");
    s.push_str("# the fallback keeps a checkout used in place (same root) working.\n");
    s.push_str("MACOPS_LIB=\"${MOPS_TOOL_ROOT:-$SCRIPT_DIR/..}/lib\"\n");
    s.push_str("# shellcheck disable=SC1091 # path resolved at runtime from the tool root\n");
    s.push_str("if ! source \"$MACOPS_LIB/common.sh\"; then\n");
    s.push_str(&format!(
        "    printf '{}: cannot load lib/common.sh\\n' >&2\n",
        file.strip_suffix(".sh").unwrap_or(file)
    ));
    s.push_str("    exit 1\n");
    s.push_str("fi\n\n");
    s
}

/// The reachability probe shared by every network-touching template.
/// Offline SKIPS SILENTLY: at 02:00 a laptop may simply have no network,
/// and a nightly false alarm for being offline would poison the alert
/// channel. MOPS_FORCE_OFFLINE=1 is the test seam.
fn online_probe() -> &'static str {
    r#"# is_online — cheap bounded reachability probe (DNS + TCP 443), never the
# real operation. Offline skips SILENTLY: a nightly false alarm for having
# no network would poison the alert channel.
# MOPS_FORCE_OFFLINE=1 forces the offline path.
is_online() {
    if [[ "${MOPS_FORCE_OFFLINE:-0}" == "1" ]]; then
        return 1
    fi
    if have nc; then
        nc -z -G 3 -w 3 "$PROBE_HOST" 443 < /dev/null >/dev/null 2>&1
        return $?
    fi
    if ! have ping; then
        return 1
    fi
    ping -c 1 -t 2 1.1.1.1 < /dev/null >/dev/null 2>&1
}

"#
}

fn render(spec: &NewJob, file: &str, label: &str, tier: &str, timeout: u32) -> Result<String> {
    match spec.kind.as_str() {
        "mole" => render_mole(spec, file, label, tier, timeout),
        "pkg" => render_pkg(spec, file, label, tier, timeout),
        "py" => render_py(spec, file, label, tier, timeout),
        "sh" => Ok(render_sh(spec, file, label, tier, timeout)),
        other => bail!("unknown job kind: {other}"),
    }
}

fn render_mole(spec: &NewJob, file: &str, label: &str, tier: &str, timeout: u32) -> Result<String> {
    let sub = spec.subcommand.as_deref().unwrap_or("optimize");
    if sub != "optimize" && sub != "clean" {
        bail!("mole template: subcommand must be optimize or clean (got {sub})");
    }
    let doc: Vec<String> = vec![
        format!("Runs `mole {sub}` unattended and alerts only when it exits non-zero."),
        String::new(),
        "Mole refuses to run as root and requests privilege itself per operation,".into(),
        "so this never uses sudo. Mole prompts when stdin is a TTY and proceeds".into(),
        "automatically otherwise, which is why every invocation reads /dev/null.".into(),
        "Captured output is stripped of ANSI colour before it reaches the log.".into(),
        String::new(),
        format!("MOPS_DRY_RUN=1 runs `mole {sub} --dry-run` instead (preview only)."),
    ];
    let doc_refs: Vec<&str> = doc.iter().map(|s| s.as_str()).collect();
    let mut s = header(
        file,
        tier,
        timeout,
        "mole",
        &format!("mole {sub} watchdog"),
        &doc_refs,
    );
    s.push_str(&format!("LABEL=\"{label}\"\n"));
    s.push_str("DRY=\"${MOPS_DRY_RUN:-0}\"\n");
    s.push_str("# Test seam: point MOPS_MOLE at a stub to exercise this job\n");
    s.push_str("# without touching the real installation.\n");
    s.push_str("MOLE=\"${MOPS_MOLE:-/opt/homebrew/bin/mole}\"\n\n");
    s.push_str(
        r#"if [[ ! -x "$MOLE" ]]; then
    alert "[$LABEL] mole not found at $MOLE"
    log_line "[$LABEL] mole missing at $MOLE dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

"#,
    );
    s.push_str(&format!(
        r#"if [[ "$DRY" == "1" ]]; then
    OUT="$("$MOLE" {sub} --dry-run < /dev/null 2>&1)"
    RC=$?
else
    OUT="$("$MOLE" {sub} < /dev/null 2>&1)"
    RC=$?
fi
CLEAN="$(printf '%s\n' "$OUT" | strip_ansi)"
ACTIONS="$(printf '%s\n' "$CLEAN" | grep -c -E '^ +(->|=>|.) ' || true)"

log_line "[$LABEL] mole {sub} rc=$RC actions=$ACTIONS dry_run=$DRY"

if [[ "$RC" != "0" ]]; then
    alert "[$LABEL] mole {sub} exited rc=$RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$CLEAN" | tail -n 15)"
    log_line "[$LABEL] output tail: $(printf '%s\n' "$CLEAN" | tail -n 30 | tr '\n' ';')"
fi

# TODO: add the change/regression checks that make this job worth running —
# see jobs/10-mole-optimize.sh for the health-score drop/floor pattern
# (threshold + settle + recheck, so a transient never cries wolf).

flush_alerts "$LABEL"
exit 0
"#
    ));
    Ok(s)
}

struct PkgSpec {
    bin: &'static str,
    host: &'static str,
    /// Named-upgrade flow (query an outdated list, upgrade those names) vs
    /// upgrade-all flow (the manager has no trustworthy machine-readable
    /// outdated list, or signals "updates exist" with a non-zero exit).
    named: bool,
    refresh: &'static str,
    list: &'static str,
    upgrade: &'static str,
    note: &'static str,
}

fn pkg_spec(manager: &str) -> Result<PkgSpec> {
    Ok(match manager {
        "brew" => PkgSpec {
            bin: "/opt/homebrew/bin/brew",
            host: "formulae.brew.sh",
            named: true,
            refresh: "    \"$PKG_BIN\" update < /dev/null 2>&1\n",
            list: "    \"$PKG_BIN\" outdated --formula --quiet < /dev/null 2>&1\n",
            upgrade: "    # Always explicit names, never bare `brew upgrade`: that would also\n    # take anything deliberately held back. `--` ends option parsing.\n    \"$PKG_BIN\" upgrade --formula -- \"$@\" < /dev/null 2>&1\n",
            note: "Casks are deliberately untouched: a cask upgrade can quit a running app or stall on an authentication prompt.",
        },
        "npm" => PkgSpec {
            bin: "/opt/homebrew/bin/npm",
            host: "registry.npmjs.org",
            named: false,
            refresh: "    # npm has no separate index refresh step.\n    return 0\n",
            list: "    \"$PKG_BIN\" --location=global outdated < /dev/null 2>&1\n    # npm exits non-zero merely because updates EXIST, so its rc is not a\n    # failure signal here; the preview is informational only.\n    return 0\n",
            upgrade: "    \"$PKG_BIN\" --location=global update < /dev/null 2>&1\n",
            note: "Global npm packages can BE your tooling: an unattended breaking change there may be worse than staleness. Consider pinning, or narrowing this to named packages.",
        },
        "pipx" => PkgSpec {
            bin: "/opt/homebrew/bin/pipx",
            host: "pypi.org",
            named: false,
            refresh: "    # pipx has no separate index refresh step.\n    return 0\n",
            list: "    \"$PKG_BIN\" list --short < /dev/null 2>&1\n",
            upgrade: "    \"$PKG_BIN\" upgrade-all < /dev/null 2>&1\n",
            note: "",
        },
        "cargo" => PkgSpec {
            bin: "$HOME/.cargo/bin/cargo",
            host: "static.crates.io",
            named: false,
            refresh: "    # cargo refreshes its index as part of the install itself.\n    return 0\n",
            list: "    \"$PKG_BIN\" install --list < /dev/null 2>&1\n",
            upgrade: "    if ! have cargo-install-update; then\n        log_line \"[$LABEL] cargo-install-update absent; nothing upgraded (log-only)\"\n        return 0\n    fi\n    \"$PKG_BIN\" install-update -a < /dev/null 2>&1\n",
            note: "Upgrades need the cargo-install-update subcommand (cargo install cargo-update); without it this job stays a no-op and says so in the log.",
        },
        "rustup" => PkgSpec {
            bin: "$HOME/.cargo/bin/rustup",
            host: "static.rust-lang.org",
            named: false,
            refresh: "    # rustup fetches its manifests as part of the update itself.\n    return 0\n",
            list: "    \"$PKG_BIN\" check < /dev/null 2>&1\n",
            upgrade: "    \"$PKG_BIN\" update < /dev/null 2>&1\n",
            note: "",
        },
        "gem" => PkgSpec {
            bin: "/usr/bin/gem",
            host: "rubygems.org",
            named: false,
            refresh: "    # gem resolves against the remote index on demand.\n    return 0\n",
            list: "    \"$PKG_BIN\" outdated < /dev/null 2>&1\n",
            upgrade: "    # --user-install keeps this out of the system gem dir: no sudo, ever.\n    \"$PKG_BIN\" update --user-install < /dev/null 2>&1\n",
            note: "--user-install is deliberate: updating the system gems would need root, and nothing here may run under sudo.",
        },
        other => bail!(
            "pkg template: unknown manager {other} (expected brew|npm|pipx|cargo|rustup|gem)"
        ),
    })
}

fn render_pkg(spec: &NewJob, file: &str, label: &str, tier: &str, timeout: u32) -> Result<String> {
    let manager = spec
        .manager
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("pkg template: spec.manager is required"))?;
    let p = pkg_spec(manager)?;

    let mut doc: Vec<String> = vec![
        format!("Keeps {manager} packages current and alerts only on failure."),
        String::new(),
        format!(
            "Offline skips SILENTLY (probe: {}): at 02:00 a laptop may have no",
            p.host
        ),
        "network, and a nightly false alarm for that would poison the alert".into(),
        "channel.".into(),
        String::new(),
        "A failed outdated/preview query is a HARD failure, not an empty list:".into(),
        "silently upgrading nothing while looking successful is the failure this".into(),
        "project exists to prevent.".into(),
    ];
    if !p.note.is_empty() {
        doc.push(String::new());
        for line in wrap(p.note, 70) {
            doc.push(line);
        }
    }
    doc.push(String::new());
    doc.push("MOPS_DRY_RUN=1 refreshes nothing and upgrades nothing; it only".into());
    doc.push("reads the current state and logs what it WOULD do.".into());
    let doc_refs: Vec<&str> = doc.iter().map(|s| s.as_str()).collect();

    let mut s = header(
        file,
        tier,
        timeout,
        "pkg",
        &format!("{manager} package-update watchdog"),
        &doc_refs,
    );
    s.push_str(&format!("LABEL=\"{label}\"\n"));
    s.push_str("DRY=\"${MOPS_DRY_RUN:-0}\"\n");
    s.push_str("# Test seam: point MOPS_PKG_BIN at a stub to exercise the apply\n");
    s.push_str("# paths without touching the real installation.\n");
    s.push_str(&format!(
        "PKG_BIN=\"${{MOPS_PKG_BIN:-{}}}\"\n",
        p.bin
    ));
    s.push_str(&format!("PROBE_HOST=\"{}\"\n\n", p.host));
    s.push_str(online_probe());

    if p.named {
        s.push_str(
            r#"# valid_pkg_name <name> — conservative allowlist for list entries, so a
# corrupt or hostile entry can never reach an argv as an option. Anything
# else is dropped (log-only) before it is used.
valid_pkg_name() {
    [[ "$1" =~ ^[A-Za-z0-9@.+_][A-Za-z0-9@.+_-]*$ ]]
}

"#,
        );
    }

    s.push_str("# pkg_refresh — refresh the package index (mutating; never in dry run).\npkg_refresh() {\n");
    s.push_str(p.refresh);
    s.push_str("}\n\n");
    s.push_str("# pkg_list — one item per line: what is outdated (or installed, when the\n# manager offers no outdated query).\npkg_list() {\n");
    s.push_str(p.list);
    s.push_str("}\n\n");
    if p.named {
        s.push_str("# pkg_upgrade <names...> — upgrade exactly these (mutating).\npkg_upgrade() {\n");
    } else {
        s.push_str("# pkg_upgrade — upgrade everything this manager considers outdated\n# (mutating). Narrow it to named packages if staleness beats surprise.\npkg_upgrade() {\n");
    }
    s.push_str(p.upgrade);
    s.push_str("}\n\n");

    s.push_str(
        r#"if ! have "$PKG_BIN" && [[ ! -x "$PKG_BIN" ]]; then
    alert "[$LABEL] package manager not found: $PKG_BIN"
    log_line "[$LABEL] manager missing ($PKG_BIN) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

if ! is_online; then
    log_line "[$LABEL] skipped: offline (no route to $PROBE_HOST, staying silent) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

# A dry run refreshes nothing: an index refresh rewrites local metadata, so
# it is not read-only and has no place in a preview.
if [[ "$DRY" == "1" ]]; then
    log_line "[$LABEL] dry run: index not refreshed (nothing mutated)"
else
    REFRESH_OUT="$(pkg_refresh)"
    REFRESH_RC=$?
    if [[ "$REFRESH_RC" != "0" ]]; then
        alert "[$LABEL] index refresh exited rc=$REFRESH_RC (dry_run=$DRY)"
        alert "$(printf '%s\n' "$REFRESH_OUT" | strip_ansi | tail -n 10)"
        log_line "[$LABEL] upgraded=0 refresh=failed rc=$REFRESH_RC dry_run=$DRY"
        flush_alerts "$LABEL"
        exit 0
    fi
fi

LIST_RAW="$(pkg_list)"
LIST_RC=$?
LIST_CLEAN="$(printf '%s\n' "$LIST_RAW" | strip_ansi)"
if [[ "$LIST_RC" != "0" ]]; then
    alert "[$LABEL] outdated query exited rc=$LIST_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$LIST_CLEAN" | tail -n 10)"
    log_line "[$LABEL] upgraded=0 list=failed rc=$LIST_RC dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

"#,
    );

    if p.named {
        s.push_str(
            r#"ITEMS=()
while IFS= read -r line || [[ -n "$line" ]]; do
    pkg="$(printf '%s' "$line" | tr -d '[:space:]')"
    if [[ -z "$pkg" ]]; then
        continue
    fi
    if valid_pkg_name "$pkg"; then
        ITEMS+=("$pkg")
    else
        log_line "[$LABEL] dropped invalid package name: $pkg (log-only, never passed through)"
    fi
done <<< "$LIST_CLEAN"

if (( ${#ITEMS[@]} == 0 )); then
    log_line "[$LABEL] nothing outdated rc=0 dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

if [[ "$DRY" == "1" ]]; then
    log_line "[$LABEL] planned=${#ITEMS[@]} (${ITEMS[*]}) no upgrade issued dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

log_line "[$LABEL] upgrade argv names: ${ITEMS[*]}"
UP_OUT="$(pkg_upgrade "${ITEMS[@]}")"
UP_RC=$?
UP_CLEAN="$(printf '%s\n' "$UP_OUT" | strip_ansi)"
if [[ "$UP_RC" != "0" ]]; then
    alert "[$LABEL] upgrade exited rc=$UP_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$UP_CLEAN" | tail -n 10)"
    log_line "[$LABEL] upgraded=0 attempted=${#ITEMS[@]} rc=$UP_RC dry_run=$DRY"
    log_line "[$LABEL] upgrade output tail: $(printf '%s\n' "$UP_CLEAN" | tail -n 30 | tr '\n' ';')"
else
    log_line "[$LABEL] upgraded=${#ITEMS[@]} (${ITEMS[*]}) rc=0 dry_run=$DRY"
fi

flush_alerts "$LABEL"
exit 0
"#,
        );
    } else {
        s.push_str(
            r#"ITEM_COUNT="$(printf '%s\n' "$LIST_CLEAN" | grep -c '[^[:space:]]' || true)"

if [[ "$DRY" == "1" ]]; then
    log_line "[$LABEL] dry run: planned (no upgrade issued) items=$ITEM_COUNT dry_run=$DRY"
    log_line "[$LABEL] preview tail: $(printf '%s\n' "$LIST_CLEAN" | tail -n 20 | tr '\n' ';')"
    flush_alerts "$LABEL"
    exit 0
fi

UP_OUT="$(pkg_upgrade)"
UP_RC=$?
UP_CLEAN="$(printf '%s\n' "$UP_OUT" | strip_ansi)"
log_line "[$LABEL] upgrade rc=$UP_RC items_before=$ITEM_COUNT dry_run=$DRY"
if [[ "$UP_RC" != "0" ]]; then
    alert "[$LABEL] upgrade exited rc=$UP_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$UP_CLEAN" | tail -n 10)"
    log_line "[$LABEL] upgrade output tail: $(printf '%s\n' "$UP_CLEAN" | tail -n 30 | tr '\n' ';')"
fi

flush_alerts "$LABEL"
exit 0
"#,
        );
    }
    Ok(s)
}

fn render_py(spec: &NewJob, file: &str, label: &str, tier: &str, timeout: u32) -> Result<String> {
    let script = spec
        .script_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("py template: spec.script_path is required"))?;
    let script = script.trim();
    if script.is_empty() {
        bail!("py template: spec.script_path is empty");
    }
    if script.contains('\'') || script.contains('\n') || script.contains('"') {
        bail!("py template: script_path may not contain quotes or newlines: {script}");
    }
    if script.contains("..") {
        bail!("py template: script_path may not contain '..': {script}");
    }

    let doc: Vec<&str> = vec![
        "Runs a Python script in its OWN virtualenv, so this job's dependencies",
        "can never be broken by an unrelated system/brew python upgrade.",
        "",
        "The venv is created on first run and reused afterwards. Requirements",
        "are installed only when a requirements file sits next to the script and",
        "its contents changed since the last install (hash recorded in the venv),",
        "so a nominal run touches the network not at all.",
        "",
        "Offline skips SILENTLY when the venv still has to be built or updated —",
        "a nightly false alarm for being offline would poison the alert channel.",
        "An already-usable venv runs offline just fine.",
        "",
        "The script inherits MOPS_DRY_RUN, so it",
        "this wrapper additionally creates and installs NOTHING in dry mode.",
        "Anything the script prints on stdout is log detail: only a non-zero exit",
        "(or a venv/install failure) alerts.",
    ];

    let mut s = header(
        file,
        tier,
        timeout,
        "py",
        &format!("python watchdog ({script})"),
        &doc,
    );
    s.push_str(&format!("LABEL=\"{label}\"\n"));
    s.push_str("DRY=\"${MOPS_DRY_RUN:-0}\"\n");
    s.push_str(&format!(
        "# Script to run, relative to the automations root (the folder containing jobs/).\nPY_SCRIPT=\"$SCRIPT_DIR/../{script}\"\n"
    ));
    s.push_str("# Optional requirements file: <script>-requirements.txt beside the script.\n");
    s.push_str("REQ_FILE=\"${PY_SCRIPT%.py}-requirements.txt\"\n");
    s.push_str("PYTHON=\"${MOPS_PYTHON:-/usr/bin/python3}\"\n");
    s.push_str(&format!(
        "VENV_DIR=\"${{MOPS_VENV_DIR:-$HOME/.cache/machine-ops/venvs/{label}}}\"\n"
    ));
    s.push_str("VENV_PY=\"$VENV_DIR/bin/python3\"\n");
    s.push_str("PROBE_HOST=\"pypi.org\"\n\n");
    s.push_str(online_probe());
    s.push_str(
        r#"if [[ ! -f "$PY_SCRIPT" ]]; then
    alert "[$LABEL] python script missing: $PY_SCRIPT"
    log_line "[$LABEL] script missing ($PY_SCRIPT) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

if [[ ! -x "$PYTHON" ]]; then
    alert "[$LABEL] python interpreter not found at $PYTHON"
    log_line "[$LABEL] interpreter missing ($PYTHON) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

# Requirements fingerprint: install only when the file actually changed.
REQ_HASH=""
REQ_STAMP="$VENV_DIR/.machine-ops-requirements-sha"
if [[ -f "$REQ_FILE" ]]; then
    REQ_HASH="$(shasum -a 256 "$REQ_FILE" < /dev/null 2>/dev/null | awk '{print $1}')"
fi
NEED_VENV=0
NEED_INSTALL=0
if [[ ! -x "$VENV_PY" ]]; then
    NEED_VENV=1
fi
if [[ -n "$REQ_HASH" ]]; then
    if [[ ! -f "$REQ_STAMP" ]] || [[ "$(cat "$REQ_STAMP" 2>/dev/null)" != "$REQ_HASH" ]]; then
        NEED_INSTALL=1
    fi
fi

if [[ "$DRY" == "1" ]]; then
    # Preview: build nothing, install nothing. With no usable venv there is
    # nothing to run either, so say what would happen and stay silent.
    if (( NEED_VENV == 1 )); then
        log_line "[$LABEL] dry run: would create venv at $VENV_DIR and install requirements; script not run"
        flush_alerts "$LABEL"
        exit 0
    fi
    if (( NEED_INSTALL == 1 )); then
        log_line "[$LABEL] dry run: would install changed requirements from $REQ_FILE (skipped)"
    fi
else
    if (( NEED_VENV == 1 || NEED_INSTALL == 1 )); then
        if ! is_online; then
            log_line "[$LABEL] skipped: offline (venv/requirements work needs $PROBE_HOST, staying silent) dry_run=$DRY"
            flush_alerts "$LABEL"
            exit 0
        fi
    fi
    if (( NEED_VENV == 1 )); then
        if ! mkdir -p "$(dirname "$VENV_DIR")" 2>/dev/null; then
            alert "[$LABEL] cannot create venv parent directory for $VENV_DIR"
            log_line "[$LABEL] venv parent mkdir failed ($VENV_DIR) dry_run=$DRY"
            flush_alerts "$LABEL"
            exit 0
        fi
        VENV_OUT="$("$PYTHON" -m venv "$VENV_DIR" < /dev/null 2>&1)"
        VENV_RC=$?
        if [[ "$VENV_RC" != "0" || ! -x "$VENV_PY" ]]; then
            alert "[$LABEL] venv creation failed rc=$VENV_RC at $VENV_DIR"
            alert "$(printf '%s\n' "$VENV_OUT" | strip_ansi | tail -n 10)"
            log_line "[$LABEL] venv creation failed rc=$VENV_RC dry_run=$DRY"
            flush_alerts "$LABEL"
            exit 0
        fi
        log_line "[$LABEL] created venv at $VENV_DIR"
        if [[ -n "$REQ_HASH" ]]; then
            NEED_INSTALL=1
        fi
    fi
    if (( NEED_INSTALL == 1 )); then
        PIP_OUT="$("$VENV_PY" -m pip install --disable-pip-version-check --quiet -r "$REQ_FILE" < /dev/null 2>&1)"
        PIP_RC=$?
        if [[ "$PIP_RC" != "0" ]]; then
            alert "[$LABEL] pip install -r $REQ_FILE exited rc=$PIP_RC"
            alert "$(printf '%s\n' "$PIP_OUT" | strip_ansi | tail -n 10)"
            log_line "[$LABEL] requirements install failed rc=$PIP_RC dry_run=$DRY"
            flush_alerts "$LABEL"
            exit 0
        fi
        printf '%s\n' "$REQ_HASH" > "$REQ_STAMP" 2>/dev/null || true
        log_line "[$LABEL] installed requirements from $REQ_FILE"
    fi
fi

if [[ ! -x "$VENV_PY" ]]; then
    alert "[$LABEL] venv interpreter missing at $VENV_PY"
    log_line "[$LABEL] venv interpreter missing ($VENV_PY) dry_run=$DRY"
    flush_alerts "$LABEL"
    exit 0
fi

# The script decides for itself what a preview means; it sees the same flag
# under MOPS_DRY_RUN.
export MOPS_DRY_RUN="$DRY"
RUN_OUT="$("$VENV_PY" "$PY_SCRIPT" < /dev/null 2>&1)"
RUN_RC=$?
RUN_CLEAN="$(printf '%s\n' "$RUN_OUT" | strip_ansi)"

log_line "[$LABEL] script=$PY_SCRIPT rc=$RUN_RC dry_run=$DRY"
if [[ -n "$RUN_CLEAN" ]]; then
    log_line "[$LABEL] script output: $(printf '%s\n' "$RUN_CLEAN" | tail -n 30 | tr '\n' ';')"
fi

if [[ "$RUN_RC" != "0" ]]; then
    alert "[$LABEL] python script exited rc=$RUN_RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$RUN_CLEAN" | tail -n 15)"
fi

flush_alerts "$LABEL"
exit 0
"#,
    );
    Ok(s)
}

fn render_sh(_spec: &NewJob, file: &str, label: &str, tier: &str, timeout: u32) -> String {
    let doc: Vec<&str> = vec![
        "Blank scaffold. Put the real work in do_work (apply) and preview_work",
        "(dry run), then decide what deserves an alert.",
        "",
        "The contract, in one line: a nominal run prints NOTHING. log_line every",
        "run's one-line summary; alert ONLY on change, regression, or error.",
        "Anything printed on stdout is treated as an alert by bin/run.",
        "",
        "If this job touches the network, probe reachability first and skip",
        "SILENTLY when offline (see jobs/30-brew-update.sh's is_online) — a",
        "nightly false alarm for being offline would poison the alert channel.",
        "If it can block indefinitely, keep the `# timeout:` line above honest.",
    ];
    let mut s = header(file, tier, timeout, "sh", "TODO describe what this watches", &doc);
    s.push_str(&format!("LABEL=\"{label}\"\n"));
    s.push_str("DRY=\"${MOPS_DRY_RUN:-0}\"\n\n");
    s.push_str(
        r#"# preview_work — read-only preview for MOPS_DRY_RUN=1. Must mutate
# NOTHING: no installs, no writes, no index refreshes.
preview_work() {
    # TODO: report what an apply run would change.
    printf 'nothing to preview yet\n'
}

# do_work — the real thing. Every command that could prompt reads /dev/null;
# never sudo, never a password.
do_work() {
    # TODO: the real work goes here.
    printf 'nothing to do yet\n'
}

if [[ "$DRY" == "1" ]]; then
    OUT="$(preview_work < /dev/null 2>&1)"
    RC=$?
else
    OUT="$(do_work < /dev/null 2>&1)"
    RC=$?
fi
CLEAN="$(printf '%s\n' "$OUT" | strip_ansi)"

# Always one line in the run log, silent or not.
log_line "[$LABEL] rc=$RC dry_run=$DRY summary=$(printf '%s' "$CLEAN" | head -n 1)"

if [[ "$RC" != "0" ]]; then
    alert "[$LABEL] exited rc=$RC (dry_run=$DRY)"
    alert "$(printf '%s\n' "$CLEAN" | tail -n 15)"
    log_line "[$LABEL] output tail: $(printf '%s\n' "$CLEAN" | tail -n 30 | tr '\n' ';')"
fi

# TODO: alert on the CHANGE or REGRESSION this job exists to catch — an exit
# code alone is rarely the interesting signal.

flush_alerts "$LABEL"
exit 0
"#,
    );
    s
}

/// Tiny greedy wrapper for comment prose (no dependencies to add).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

// ---------------------------------------------------------------------------
// Deletion
// ---------------------------------------------------------------------------

/// Remove the LaunchAgent and the console.json entry (caller does that);
/// here, move the script to the Trash when `keep_script` is false.
///
/// Trash, not unlink: a job script is hand-written work, and "delete the
/// automation" must never mean "destroy the only copy". The user can put it
/// back from the Finder.
pub fn delete(root: &Path, file: &str, keep_script: bool) -> Result<()> {
    if keep_script {
        return Ok(());
    }
    let path = jobs_dir(root).join(file);
    if !path.exists() {
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; refusing to delete {file} outright"))?;
    let trash = std::env::var_os("MOPS_TRASH_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".Trash"));
    std::fs::create_dir_all(&trash)
        .with_context(|| format!("create trash dir {}", trash.display()))?;

    let dest = unique_trash_path(&trash, file);
    // Same volume: a rename is the move. Across volumes rename fails with
    // EXDEV, so fall back to copy-then-remove.
    match std::fs::rename(&path, &dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(&path, &dest)
                .with_context(|| format!("copy {} to {}", path.display(), dest.display()))?;
            std::fs::remove_file(&path)
                .with_context(|| format!("remove {} after copying to Trash", path.display()))?;
            Ok(())
        }
    }
}

fn unique_trash_path(trash: &Path, file: &str) -> PathBuf {
    let first = trash.join(file);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (file.to_string(), String::new()),
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let candidate = trash.join(format!("{stem} {stamp}{ext}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2..10_000u32 {
        let c = trash.join(format!("{stem} {stamp} {n}{ext}"));
        if !c.exists() {
            return c;
        }
    }
    trash.join(format!("{stem} {stamp} {}{ext}", std::process::id()))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// `bash -n` always; `shellcheck` when present on PATH. Returns the
/// collected diagnostics — empty means clean.
pub fn validate(path: &Path) -> Result<Vec<String>> {
    let mut diags: Vec<String> = Vec::new();
    if !path.is_file() {
        diags.push(format!("{}: not a file", path.display()));
        return Ok(diags);
    }

    let bash = Command::new("/bin/bash")
        .arg("-n")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run bash -n {}", path.display()))?;
    if !bash.status.success() {
        for line in collect_lines(&bash.stderr, &bash.stdout) {
            diags.push(format!("bash -n: {line}"));
        }
        if diags.is_empty() {
            diags.push(format!("bash -n: failed with {}", bash.status));
        }
    }

    // The metadata contract is part of validity: a job whose line 2 does not
    // parse ALERTS on every dispatcher run instead of quietly skipping.
    let content = std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let mut lines = content.lines();
    let _shebang = lines.next();
    match lines.next().and_then(parse_tier_line) {
        Some(_) => {}
        None => diags.push(
            "metadata: line 2 must be a whole-line '# tier: <name>' comment (bin/run alerts otherwise)"
                .to_string(),
        ),
    }

    if have("shellcheck") {
        let sc = Command::new("shellcheck")
            .arg("--severity=warning")
            .arg("--shell=bash")
            .arg(path)
            .stdin(Stdio::null())
            .output();
        if let Ok(sc) = sc {
            if !sc.status.success() {
                for line in collect_lines(&sc.stdout, &sc.stderr) {
                    diags.push(format!("shellcheck: {line}"));
                }
            }
        }
    }
    Ok(diags)
}

fn collect_lines(primary: &[u8], fallback: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(primary);
    let mut out: Vec<String> = text
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if out.is_empty() {
        out = String::from_utf8_lossy(fallback)
            .lines()
            .map(|l| l.trim_end().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    out
}

fn have(cmd: &str) -> bool {
    Command::new("/usr/bin/env")
        .args(["sh", "-c", &format!("command -v {cmd} >/dev/null 2>&1")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Timeout rewriting
// ---------------------------------------------------------------------------

/// Rewrite the `# timeout:` line in place (inserting it at line 3 when
/// absent), preserving the rest of the file byte for byte.
///
/// Byte-level on purpose: the file may be the user's hand-written script,
/// and a round-trip through a lossy string conversion (or a line-ending
/// rewrite) would be a silent edit nobody asked for.
pub fn set_timeout(path: &Path, secs: u32) -> Result<()> {
    if !(1..=86400).contains(&secs) {
        bail!("timeout {secs}s out of range (bin/run accepts 1..=86400)");
    }
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut chunks = split_keep_terminators(&data);

    // Terminator style is copied from a neighbouring line so a CRLF file
    // stays CRLF.
    let newline: Vec<u8> = chunks
        .iter()
        .find_map(|c| {
            if c.ends_with(b"\r\n") {
                Some(b"\r\n".to_vec())
            } else if c.ends_with(b"\n") {
                Some(b"\n".to_vec())
            } else {
                None
            }
        })
        .unwrap_or_else(|| b"\n".to_vec());

    let mut replacement = format!("# timeout: {secs}").into_bytes();
    replacement.extend_from_slice(&newline);

    let line3_is_timeout = chunks
        .get(2)
        .map(|c| {
            let text = String::from_utf8_lossy(c);
            let text = text.trim_end_matches(['\n', '\r']);
            meta_value(text, "timeout").is_some()
        })
        .unwrap_or(false);

    if line3_is_timeout {
        chunks[2] = replacement;
    } else if chunks.len() >= 2 {
        // Insert as the new line 3, directly under the tier line. If line 2
        // was the last line and lacked a terminator, give it one first.
        if !chunks[1].ends_with(b"\n") {
            chunks[1].extend_from_slice(&newline);
        }
        chunks.insert(2, replacement);
    } else {
        // Degenerate file (shebang only, or empty): append rather than
        // inventing a tier line we were not asked to write.
        if let Some(last) = chunks.last_mut() {
            if !last.ends_with(b"\n") {
                last.extend_from_slice(&newline);
            }
        }
        chunks.push(replacement);
    }

    let out: Vec<u8> = chunks.concat();
    std::fs::write(path, &out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Split into lines, each keeping its own terminator (the last may have none).
fn split_keep_terminators(data: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, b) in data.iter().enumerate() {
        if *b == b'\n' {
            out.push(data[start..=i].to_vec());
            start = i + 1;
        }
    }
    if start < data.len() {
        out.push(data[start..].to_vec());
    }
    out
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Run one job through bin/run-job, capturing stdout/stderr and timing.
/// stdin MUST be /dev/null. `dry` sets MOPS_DRY_RUN=1.
/// bin/run-job exits 0 even when the job speaks — stdout is the alert
/// channel, not the exit code — so derive Status via Status::derive.
///
/// `tool_root` supplies `bin/run-job`; `automations_root` supplies the job
/// script. The child's working directory is the AUTOMATIONS root: any
/// relative path a job writes is user data, and must never land inside the
/// tool's install directory (Principle II).
pub fn run(tool_root: &Path, automations_root: &Path, file: &str, dry: bool) -> Result<RunOutcome> {
    let runner = crate::paths::run_job_path(tool_root);
    if !runner.is_file() {
        bail!("bin/run-job missing at {}", runner.display());
    }
    let job = jobs_dir(automations_root).join(file);
    if !job.is_file() {
        bail!("job script missing at {}", job.display());
    }

    let started = std::time::Instant::now();
    // /bin/bash explicitly: run-job must work even if the exec bit is lost.
    let out = Command::new("/bin/bash")
        .arg(&runner)
        .arg(&job)
        .current_dir(automations_root)
        .env("MOPS_TOOL_ROOT", tool_root)
        .env("MOPS_AUTOMATIONS_ROOT", automations_root)
        .env("MOPS_DRY_RUN", if dry { "1" } else { "0" })
        .stdin(Stdio::null()) // no job may ever prompt
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("run {} {}", runner.display(), job.display()))?;
    let duration_s = started.elapsed().as_secs() as i64;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let wrapper_rc = out.status.code().unwrap_or(-1);

    // bin/run-job exits 0 for any COMPLETED run, so the real outcome lives in
    // its alert text: it prints "[TIMED OUT after <n>s limit; …]" on a
    // timeout and "job exited <rc>" / "[also exited <rc>]" when the job
    // itself failed. A non-zero wrapper rc means the wrapper could not run
    // at all (bad args, unreadable job, root/sudo) — a failure either way.
    let timed_out = stdout.contains("[TIMED OUT after");
    let rc = if wrapper_rc != 0 {
        wrapper_rc
    } else if timed_out {
        124
    } else {
        job_rc_from_alerts(&stdout).unwrap_or(0)
    };

    Ok(RunOutcome {
        status: Status::derive(rc, timed_out, &stdout),
        rc,
        duration_s,
        stdout,
        stderr,
        timed_out,
    })
}

/// Pull the job's own exit code out of bin/run-job's alert text.
fn job_rc_from_alerts(stdout: &str) -> Option<i32> {
    for line in stdout.lines() {
        let t = line.trim();
        let digits = t
            .strip_prefix("job exited ")
            .or_else(|| t.strip_prefix("[also exited ").map(|r| r.trim_end_matches(']')));
        if let Some(d) = digits {
            if let Ok(n) = d.trim().parse::<i32>() {
                if n != 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Schedule;

    // Temp dirs only: a test must never write into the real jobs/ directory.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let base = std::env::temp_dir().join(format!(
                "machine-ops-jobs-test-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(base.join("jobs")).unwrap();
            Tmp(base)
        }
        fn root(&self) -> &Path {
            &self.0
        }
        fn jobs(&self) -> PathBuf {
            self.0.join("jobs")
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn spec(kind: &str, title: &str) -> NewJob {
        NewJob {
            title: title.into(),
            group: "Tests".into(),
            kind: kind.into(),
            schedule: Schedule::Daily { times: vec!["02:00".into()] },
            timeout: 900,
            script_path: None,
            manager: None,
            subcommand: None,
        }
    }

    // --- anchored tier parsing ------------------------------------------

    #[test]
    fn tier_accepts_the_shapes_bin_run_accepts() {
        assert_eq!(parse_tier_line("# tier: daily").as_deref(), Some("daily"));
        assert_eq!(parse_tier_line("#tier:daily").as_deref(), Some("daily"));
        assert_eq!(parse_tier_line("   #   tier:   weekly   ").as_deref(), Some("weekly"));
        assert_eq!(parse_tier_line("\t# tier: weekly\t").as_deref(), Some("weekly"));
        assert_eq!(parse_tier_line("# tier: daily\r").as_deref(), Some("daily"));
    }

    #[test]
    fn tier_rejects_executable_code_that_merely_contains_the_pattern() {
        // The whole point of the anchor: this is code, not metadata.
        assert!(parse_tier_line("echo \"# tier: daily\"").is_none());
        assert!(parse_tier_line("printf '# tier: daily\\n'").is_none());
        assert!(parse_tier_line("junk # tier: daily").is_none());
        assert!(parse_tier_line("# tier: daily # trailing").is_none());
        assert!(parse_tier_line("# tier: daily weekly").is_none());
        assert!(parse_tier_line("# tier: Daily").is_none()); // [a-z]+ only
        assert!(parse_tier_line("# tier: daily2").is_none());
        assert!(parse_tier_line("# tier:").is_none());
        assert!(parse_tier_line("# tiers: daily").is_none());
        assert!(parse_tier_line("# timeout: 10").is_none());
        assert!(parse_tier_line("").is_none());
    }

    // --- anchored timeout parsing ---------------------------------------

    #[test]
    fn timeout_accepts_in_range_values() {
        assert_eq!(parse_timeout_line("# timeout: 1"), Some(1));
        assert_eq!(parse_timeout_line("#timeout:3600"), Some(3600));
        assert_eq!(parse_timeout_line("  #  timeout:  86400  "), Some(86400));
        // Leading zeros are stripped before the length check, like bin/run.
        assert_eq!(parse_timeout_line("# timeout: 0900"), Some(900));
        assert_eq!(parse_timeout_line("# timeout: 000000000600"), Some(600));
    }

    #[test]
    fn timeout_out_of_range_or_malformed_means_default() {
        for line in [
            "# timeout: 0",
            "# timeout: 86401",
            "# timeout: 99999",
            "# timeout: 100000",
            "# timeout: 999999999999",
            "# timeout: -5",
            "# timeout: 10s",
            "# timeout: abc",
            "# timeout:",
            "# timeout: 10 20",
            "echo \"# timeout: 10\"",
            "junk # timeout: 10",
            "# tier: daily",
        ] {
            assert!(
                parse_timeout_line(line).is_none(),
                "{line:?} must not parse as a timeout"
            );
        }
    }

    #[test]
    fn read_meta_resolves_default_timeout_and_loud_missing_tier() {
        let t = Tmp::new("meta");
        let ok = t.jobs().join("10-ok.sh");
        std::fs::write(&ok, "#!/usr/bin/env bash\n# tier: weekly\n# timeout: 60\nMOLE=/bin/mole\n").unwrap();
        let m = read_meta(&ok).unwrap();
        assert_eq!(m.file, "10-ok.sh");
        assert_eq!(m.label, "10-ok");
        assert_eq!(m.tier.as_deref(), Some("weekly"));
        assert_eq!(m.timeout, 60);
        assert_eq!(m.kind, "mole");

        // Tier on the wrong line is NOT a tier: bin/run would alert.
        let bad = t.jobs().join("20-bad.sh");
        std::fs::write(&bad, "#!/usr/bin/env bash\n# timeout: 60\n# tier: daily\necho hi\n").unwrap();
        let m = read_meta(&bad).unwrap();
        assert!(m.tier.is_none());
        assert_eq!(m.timeout, DEFAULT_TIMEOUT);

        // An out-of-range override silently falls back to the default.
        let big = t.jobs().join("30-big.sh");
        std::fs::write(&big, "#!/usr/bin/env bash\n# tier: daily\n# timeout: 999999\n").unwrap();
        assert_eq!(read_meta(&big).unwrap().timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn scan_is_filename_sorted_and_only_shell_files() {
        let t = Tmp::new("scan");
        for f in ["30-c.sh", "10-a.sh", "20-b.sh"] {
            std::fs::write(t.jobs().join(f), "#!/usr/bin/env bash\n# tier: daily\n").unwrap();
        }
        std::fs::write(t.jobs().join("notes.txt"), "ignore me").unwrap();
        std::fs::write(t.jobs().join(".hidden.sh"), "#!/usr/bin/env bash\n").unwrap();
        let files: Vec<String> = scan(t.root()).unwrap().into_iter().map(|m| m.file).collect();
        assert_eq!(files, vec!["10-a.sh", "20-b.sh", "30-c.sh"]);
    }

    // --- slug + number allocation ---------------------------------------

    #[test]
    fn slugify_collapses_everything_unsafe() {
        assert_eq!(slugify("Brew Update"), "brew-update");
        assert_eq!(slugify("  Mole: optimize!! "), "mole-optimize");
        assert_eq!(slugify("Tidy ~/Downloads (weekly)"), "tidy-downloads-weekly");
        assert_eq!(slugify("CPU   load   check"), "cpu-load-check");
        assert_eq!(slugify("Backup_2024"), "backup-2024");
        assert_eq!(slugify("---"), "");
    }

    #[test]
    fn create_picks_a_non_colliding_number_prefix() {
        let t = Tmp::new("numbers");
        for f in ["10-mole-optimize.sh", "20-mole-clean.sh", "30-brew-update.sh"] {
            std::fs::write(t.jobs().join(f), "#!/usr/bin/env bash\n# tier: daily\n").unwrap();
        }
        let f1 = create(t.root(), &spec("sh", "Disk Watch")).unwrap();
        assert_eq!(f1, "40-disk-watch.sh");
        let f2 = create(t.root(), &spec("sh", "Another Thing")).unwrap();
        assert_eq!(f2, "50-another-thing.sh");
        // Same title again: the number must move, never overwrite.
        let f3 = create(t.root(), &spec("sh", "Disk Watch")).unwrap();
        assert_eq!(f3, "60-disk-watch.sh");
        assert!(t.jobs().join(&f1).exists() && t.jobs().join(&f3).exists());
    }

    #[test]
    fn created_scripts_are_executable_and_carry_the_metadata_lines() {
        let t = Tmp::new("contract");
        let mut s = spec("sh", "Weekly Thing");
        s.schedule = Schedule::Weekly { days: vec!["mon".into()], times: vec!["03:00".into()] };
        s.timeout = 1234;
        let file = create(t.root(), &s).unwrap();
        let path = t.jobs().join(&file);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#!/usr/bin/env bash");
        assert_eq!(parse_tier_line(lines[1]).as_deref(), Some("weekly"));
        assert_eq!(parse_timeout_line(lines[2]), Some(1234));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "generated job must be executable");
        }
        // An out-of-range requested timeout falls back to the default.
        let mut s2 = spec("sh", "Bad Timeout");
        s2.timeout = 999_999;
        let f2 = create(t.root(), &s2).unwrap();
        let text2 = std::fs::read_to_string(t.jobs().join(f2)).unwrap();
        assert_eq!(parse_timeout_line(text2.lines().nth(2).unwrap()), Some(DEFAULT_TIMEOUT));
    }

    // --- templates -------------------------------------------------------

    fn all_four() -> Vec<NewJob> {
        let mut mole = spec("mole", "Mole Optimize");
        mole.subcommand = Some("optimize".into());
        let mut mole_clean = spec("mole", "Mole Clean");
        mole_clean.subcommand = Some("clean".into());
        let mut py = spec("py", "Report Generator");
        py.script_path = Some("tools/report.py".into());
        let mut out = vec![mole, mole_clean, py, spec("sh", "Blank Scaffold")];
        for m in ["brew", "npm", "pipx", "cargo", "rustup", "gem"] {
            let mut p = spec("pkg", &format!("{m} updates"));
            p.manager = Some(m.into());
            out.push(p);
        }
        out
    }

    #[test]
    fn every_template_is_syntactically_valid_bash() {
        let t = Tmp::new("bash-n");
        for s in all_four() {
            let file = create(t.root(), &s).unwrap();
            let path = t.jobs().join(&file);
            let out = Command::new("/bin/bash")
                .arg("-n")
                .arg(&path)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "bash -n failed for {} ({}/{:?}):\n{}",
                file,
                s.kind,
                s.manager,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn every_template_follows_the_house_contract() {
        let t = Tmp::new("house");
        for s in all_four() {
            let file = create(t.root(), &s).unwrap();
            let text = std::fs::read_to_string(t.jobs().join(&file)).unwrap();
            let why = |what: &str| format!("{file} ({}) missing {what}", s.kind);

            assert!(text.starts_with("#!/usr/bin/env bash\n"), "{}", why("shebang"));
            assert!(text.contains("\nset -u\n"), "{}", why("set -u"));
            assert!(
                text.contains(
                    "MACOPS_LIB=\"${MOPS_TOOL_ROOT:-$SCRIPT_DIR/..}/lib\""
                ),
                "{}",
                why("tool-root-relative MACOPS_LIB")
            );
            assert!(
                text.contains("source \"$MACOPS_LIB/common.sh\""),
                "{}",
                why("lib/common.sh source")
            );
            assert!(text.contains(&format!("LABEL=\"{}\"", file.trim_end_matches(".sh"))), "{}", why("LABEL"));
            assert!(text.contains("DRY=\"${MOPS_DRY_RUN:-0}\""), "{}", why("dry-run support"));
            assert!(
                !text.contains("MACHINE_OPS"),
                "{}",
                why("no legacy MACHINE_OPS_* names")
            );
            assert!(text.contains("< /dev/null"), "{}", why("stdin redirect"));
            assert!(text.contains("log_line "), "{}", why("log_line summary"));
            assert!(text.contains("alert "), "{}", why("alert"));
            assert!(
                text.trim_end().ends_with("flush_alerts \"$LABEL\"\nexit 0"),
                "{}",
                why("flush_alerts + exit 0 ending")
            );
            // Never sudo, never a password. Not negotiable — checked on the
            // CODE, since a comment may well explain why sudo is forbidden.
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!code.contains("sudo"), "{file} must never invoke sudo");
            assert!(
                !code.to_lowercase().contains("password"),
                "{file} must not touch passwords"
            );

            // Network-touching templates must probe and skip silently.
            let networked = s.kind == "pkg" || s.kind == "py";
            assert_eq!(
                code.contains("is_online"),
                networked,
                "{file}: reachability probe presence should match kind {}",
                s.kind
            );
            if networked {
                assert!(text.contains("skipped: offline"), "{}", why("silent offline skip"));
                // The offline path logs and leaves, with no alert queued.
                let offline_block = text
                    .split("skipped: offline")
                    .nth(1)
                    .unwrap()
                    .split("exit 0")
                    .next()
                    .unwrap();
                assert!(
                    !offline_block.contains("alert "),
                    "{file}: offline must never alert"
                );
            }
        }
    }

    /// The real contract, executed: every generated template must run to
    /// completion against the repo's own lib/common.sh and print NOTHING.
    /// Stubs stand in for the external tools; the network probe is forced
    /// offline, which must be a silent skip rather than a nightly alarm.
    #[test]
    fn every_template_is_silent_on_a_nominal_dry_run() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let common = repo.join("lib/common.sh");
        if !common.is_file() {
            return; // nothing to execute against
        }
        let t = Tmp::new("silent");
        std::fs::create_dir_all(t.root().join("lib")).unwrap();
        std::fs::create_dir_all(t.root().join("tools")).unwrap();
        std::fs::copy(&common, t.root().join("lib/common.sh")).unwrap();
        std::fs::write(t.root().join("tools/noop.py"), "print()\n").unwrap();
        // A stub that says nothing and succeeds: stands in for mole/brew/npm.
        let stub = t.root().join("stub.sh");
        std::fs::write(&stub, "#!/usr/bin/env bash\nexit 0\n").unwrap();
        make_executable(&stub).unwrap();
        let log = t.root().join("run.log");

        let mut specs = all_four();
        for s in specs.iter_mut() {
            if s.kind == "py" {
                s.script_path = Some("tools/noop.py".into());
            }
        }
        for s in specs {
            let file = create(t.root(), &s).unwrap();
            let out = Command::new("/bin/bash")
                .arg(t.jobs().join(&file))
                .env("MOPS_LOG", &log)
                .env("MOPS_DRY_RUN", "1")
                .env("MOPS_FORCE_OFFLINE", "1") // offline must be silent
                .env("MOPS_MOLE", &stub)
                .env("MOPS_PKG_BIN", &stub)
                // Keep the child hermetic: the template must find lib/ via the
                // same-root fallback, not a tool root another test set.
                .env_remove("MOPS_TOOL_ROOT")
                .env_remove("MOPS_AUTOMATIONS_ROOT")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(0),
                "{file} ({}) must exit 0; stderr:\n{}",
                s.kind,
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                out.stdout.is_empty(),
                "{file} ({}) broke silent-on-success; stdout:\n{}",
                s.kind,
                String::from_utf8_lossy(&out.stdout)
            );
        }
        // Silent or not, every run leaves a line in the log.
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.lines().count() >= 10, "run log:\n{logged}");
    }

    /// Apply mode, end to end, against a stub manager: a successful upgrade
    /// must stay silent and leave the counts in the run log, while a failing
    /// one must speak exactly once.
    #[test]
    fn pkg_template_apply_mode_is_silent_on_success_and_speaks_on_failure() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let common = repo.join("lib/common.sh");
        if !common.is_file() {
            return;
        }
        let t = Tmp::new("pkg-apply");
        std::fs::create_dir_all(t.root().join("lib")).unwrap();
        std::fs::copy(&common, t.root().join("lib/common.sh")).unwrap();
        let log = t.root().join("run.log");

        // A stub brew: two formulae outdated, upgrade succeeds (or fails
        // when STUB_FAIL is set).
        let stub = t.root().join("brew-stub");
        std::fs::write(
            &stub,
            "#!/usr/bin/env bash\nset -u\ncase \"${1:-}\" in\n  update) exit 0 ;;\n  outdated) printf 'wget\\njq\\n'; exit 0 ;;\n  upgrade) if [[ -n \"${STUB_FAIL:-}\" ]]; then printf 'boom\\n'; exit 9; fi; exit 0 ;;\nesac\nexit 0\n",
        )
        .unwrap();
        make_executable(&stub).unwrap();

        let mut s = spec("pkg", "Brew Updates");
        s.manager = Some("brew".into());
        let file = create(t.root(), &s).unwrap();
        let job = t.jobs().join(&file);

        let out = Command::new("/bin/bash")
            .arg(&job)
            .env("MOPS_LOG", &log)
            .env("MOPS_PKG_BIN", &stub)
            .env_remove("MOPS_DRY_RUN")
            .env_remove("MOPS_FORCE_OFFLINE")
            .env_remove("MOPS_TOOL_ROOT")
            .env_remove("MOPS_AUTOMATIONS_ROOT")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(
            out.stdout.is_empty(),
            "a successful upgrade must stay silent; got:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains("upgraded=2 (wget jq)"), "run log:\n{logged}");

        let out = Command::new("/bin/bash")
            .arg(&job)
            .env("MOPS_LOG", &log)
            .env("MOPS_PKG_BIN", &stub)
            .env("STUB_FAIL", "1")
            .env_remove("MOPS_TOOL_ROOT")
            .env_remove("MOPS_AUTOMATIONS_ROOT")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(out.status.code(), Some(0), "a speaking job still exits 0");
        assert!(said.contains("upgrade exited rc=9"), "stdout:\n{said}");
        assert!(said.starts_with("== "), "alerts carry the label header:\n{said}");
    }

    #[test]
    fn templates_round_trip_to_their_kind() {
        let t = Tmp::new("kinds");
        for s in all_four() {
            let file = create(t.root(), &s).unwrap();
            let m = read_meta(&t.jobs().join(&file)).unwrap();
            assert_eq!(m.kind, s.kind, "{file} should read back as kind {}", s.kind);
            assert_eq!(m.tier.as_deref(), Some("daily"));
        }
    }

    #[test]
    fn template_rejects_bad_inputs() {
        let t = Tmp::new("bad");
        let mut bad_sub = spec("mole", "Mole What");
        bad_sub.subcommand = Some("nuke".into());
        assert!(create(t.root(), &bad_sub).is_err());

        let mut bad_mgr = spec("pkg", "Mystery");
        bad_mgr.manager = Some("apt".into());
        assert!(create(t.root(), &bad_mgr).is_err());

        assert!(create(t.root(), &spec("pkg", "No Manager")).is_err());
        assert!(create(t.root(), &spec("py", "No Script")).is_err());
        assert!(create(t.root(), &spec("ruby", "Wrong Kind")).is_err());

        let mut traversal = spec("py", "Escape");
        traversal.script_path = Some("../../etc/passwd.py".into());
        assert!(create(t.root(), &traversal).is_err());

        let mut quoted = spec("py", "Injected");
        quoted.script_path = Some("a\"; rm -rf /; echo \".py".into());
        assert!(create(t.root(), &quoted).is_err());
    }

    #[test]
    fn real_repo_jobs_still_parse() {
        // The exemplars in the repo this console manages must read cleanly;
        // read-only, and skipped when the repo is not where we expect.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !root.join("jobs").is_dir() {
            return;
        }
        let metas = scan(&root).unwrap();
        assert!(!metas.is_empty());
        for m in &metas {
            assert!(m.tier.is_some(), "{} has no parseable tier", m.file);
            assert!(KINDS.contains(&m.kind.as_str()), "{} got kind {}", m.file, m.kind);
        }
        let brew = metas.iter().find(|m| m.file == "30-brew-update.sh");
        if let Some(b) = brew {
            assert_eq!(b.timeout, 3600);
            // Its HOLD list *mentions* mole in comments; the kind must not
            // be decided by prose.
            assert_eq!(b.kind, "pkg");
        }
        if let Some(mo) = metas.iter().find(|m| m.file == "10-mole-optimize.sh") {
            assert_eq!(mo.kind, "mole");
            assert_eq!(mo.timeout, DEFAULT_TIMEOUT); // no line-3 override
        }
    }

    // --- set_timeout -----------------------------------------------------

    #[test]
    fn set_timeout_replaces_line_three_byte_for_byte() {
        let t = Tmp::new("set-replace");
        let p = t.jobs().join("10-x.sh");
        let tail = "\n# doc\n\nset -u\nLABEL=\"x\"\n# timeout: 7 (not line 3, stays put)\nexit 0\n";
        let before = format!("#!/usr/bin/env bash\n# tier: daily\n# timeout: 60{tail}");
        std::fs::write(&p, &before).unwrap();
        set_timeout(&p, 4242).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(after, format!("#!/usr/bin/env bash\n# tier: daily\n# timeout: 4242{tail}"));
        assert_eq!(read_meta(&p).unwrap().timeout, 4242);
    }

    #[test]
    fn set_timeout_inserts_at_line_three_when_absent() {
        let t = Tmp::new("set-insert");
        let p = t.jobs().join("10-y.sh");
        let before = "#!/usr/bin/env bash\n# tier: daily\n# machine-ops job: y.\nexit 0\n";
        std::fs::write(&p, before).unwrap();
        set_timeout(&p, 600).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            after,
            "#!/usr/bin/env bash\n# tier: daily\n# timeout: 600\n# machine-ops job: y.\nexit 0\n"
        );
        let m = read_meta(&p).unwrap();
        assert_eq!(m.timeout, 600);
        assert_eq!(m.tier.as_deref(), Some("daily"));
    }

    #[test]
    fn set_timeout_preserves_crlf_and_trailing_bytes() {
        let t = Tmp::new("set-crlf");
        let p = t.jobs().join("10-z.sh");
        // CRLF file with no final newline, and a stray NUL-free binary-ish tail.
        let before = "#!/usr/bin/env bash\r\n# tier: weekly\r\n# timeout: 30\r\necho déjà vu\r\nexit 0";
        std::fs::write(&p, before.as_bytes()).unwrap();
        set_timeout(&p, 90).unwrap();
        let after = std::fs::read(&p).unwrap();
        assert_eq!(
            String::from_utf8(after).unwrap(),
            "#!/usr/bin/env bash\r\n# tier: weekly\r\n# timeout: 90\r\necho déjà vu\r\nexit 0"
        );
        assert_eq!(read_meta(&p).unwrap().timeout, 90);

        // Insertion keeps CRLF too.
        let q = t.jobs().join("20-z.sh");
        std::fs::write(&q, "#!/usr/bin/env bash\r\n# tier: daily\r\nexit 0\r\n".as_bytes()).unwrap();
        set_timeout(&q, 15).unwrap();
        assert_eq!(
            std::fs::read_to_string(&q).unwrap(),
            "#!/usr/bin/env bash\r\n# tier: daily\r\n# timeout: 15\r\nexit 0\r\n"
        );
    }

    #[test]
    fn set_timeout_does_not_mistake_code_for_metadata() {
        let t = Tmp::new("set-code");
        let p = t.jobs().join("10-w.sh");
        // Line 3 LOOKS like a timeout but is executable code: it must be
        // pushed down, not rewritten.
        let before = "#!/usr/bin/env bash\n# tier: daily\necho \"# timeout: 10\"\n";
        std::fs::write(&p, before).unwrap();
        set_timeout(&p, 120).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "#!/usr/bin/env bash\n# tier: daily\n# timeout: 120\necho \"# timeout: 10\"\n"
        );
    }

    #[test]
    fn set_timeout_rejects_out_of_range() {
        let t = Tmp::new("set-range");
        let p = t.jobs().join("10-r.sh");
        let before = "#!/usr/bin/env bash\n# tier: daily\n# timeout: 60\n";
        std::fs::write(&p, before).unwrap();
        assert!(set_timeout(&p, 0).is_err());
        assert!(set_timeout(&p, 86_401).is_err());
        // The file is untouched by a rejected request.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    }

    // --- validate / delete ----------------------------------------------

    #[test]
    fn validate_flags_broken_syntax_and_missing_tier() {
        let t = Tmp::new("validate");
        let bad = t.jobs().join("10-broken.sh");
        std::fs::write(&bad, "#!/usr/bin/env bash\n# tier: daily\nif [ 1; then\n").unwrap();
        let diags = validate(&bad).unwrap();
        assert!(diags.iter().any(|d| d.starts_with("bash -n:")), "{diags:?}");

        let no_tier = t.jobs().join("20-no-tier.sh");
        std::fs::write(&no_tier, "#!/usr/bin/env bash\necho hi\n").unwrap();
        let diags = validate(&no_tier).unwrap();
        assert!(diags.iter().any(|d| d.starts_with("metadata:")), "{diags:?}");
    }

    #[test]
    fn generated_templates_validate_clean() {
        let t = Tmp::new("validate-templates");
        for s in all_four() {
            let file = create(t.root(), &s).unwrap();
            let diags = validate(&t.jobs().join(&file)).unwrap();
            assert!(diags.is_empty(), "{file} ({}) diagnostics: {diags:?}", s.kind);
        }
    }

    #[test]
    fn delete_moves_the_script_to_the_trash() {
        let t = Tmp::new("delete");
        let trash = t.root().join("Trash");
        let file = create(t.root(), &spec("sh", "Throwaway")).unwrap();
        let path = t.jobs().join(&file);
        let body = std::fs::read_to_string(&path).unwrap();

        // keep_script leaves it exactly where it is.
        delete(t.root(), &file, true).unwrap();
        assert!(path.exists());

        // SAFETY: the trash dir is redirected into the temp root, so no test
        // ever drops files into the real ~/.Trash.
        unsafe { std::env::set_var("MOPS_TRASH_DIR", &trash) };
        delete(t.root(), &file, false).unwrap();
        assert!(!path.exists(), "script should have left jobs/");
        assert_eq!(std::fs::read_to_string(trash.join(&file)).unwrap(), body);

        // A second file of the same name must not clobber the first.
        std::fs::write(&path, "#!/usr/bin/env bash\n# tier: daily\n# second\n").unwrap();
        delete(t.root(), &file, false).unwrap();
        let in_trash: Vec<_> = std::fs::read_dir(&trash).unwrap().flatten().collect();
        assert_eq!(in_trash.len(), 2, "both copies should survive in the Trash");
        assert_eq!(std::fs::read_to_string(trash.join(&file)).unwrap(), body);
        unsafe { std::env::remove_var("MOPS_TRASH_DIR") };

        // Deleting what is already gone is not an error.
        delete(t.root(), "99-nope.sh", false).unwrap();
    }

    // --- run outcome mapping --------------------------------------------

    #[test]
    fn job_rc_is_read_out_of_run_job_alert_text() {
        assert_eq!(job_rc_from_alerts("== x ==\njob exited 3\n"), Some(3));
        assert_eq!(job_rc_from_alerts("== x ==\nsomething\n[also exited 2]\n"), Some(2));
        assert_eq!(job_rc_from_alerts("== x ==\nplain alert text\n"), None);
        assert_eq!(job_rc_from_alerts("job exited 0\n"), None);
    }

    #[test]
    fn run_derives_status_from_stdout_not_the_exit_code() {
        let t = Tmp::new("run");
        // The wrapper lives in a SEPARATE tool root; the job script lives in
        // the automations root. This is the split under test.
        let tool = t.root().join("tool");
        std::fs::create_dir_all(tool.join("bin")).unwrap();
        std::fs::create_dir_all(tool.join("lib")).unwrap();
        // A stub run-job: exits 0 always, speaks only what the fixture file
        // says — exactly the trap this contract exists to avoid. The fixture
        // is a file, not an env var, so parallel tests cannot race on it.
        std::fs::write(
            tool.join("bin/run-job"),
            "#!/usr/bin/env bash\nset -u\ncat \"$(dirname \"$0\")/../say.txt\"\nexit 0\n",
        )
        .unwrap();
        let job = t.jobs().join("10-stub.sh");
        std::fs::write(&job, "#!/usr/bin/env bash\n# tier: daily\nexit 0\n").unwrap();

        let cases = [
            ("", Status::Quiet, 0),
            ("== 10-stub ==\nsomething changed\n", Status::Spoke, 0),
            ("== 10-stub ==\njob exited 7\n", Status::Fail, 7),
            ("== 10-stub ==\n[TIMED OUT after 900s limit; process group killed]\n", Status::Hang, 124),
        ];
        for (say, want_status, want_rc) in cases {
            std::fs::write(tool.join("say.txt"), say).unwrap();
            let out = run(&tool, t.root(), "10-stub.sh", false).unwrap();
            assert_eq!(out.status, want_status, "stdout {say:?}");
            assert_eq!(out.rc, want_rc, "stdout {say:?}");
            assert_eq!(out.timed_out, want_status == Status::Hang);
        }

        // Dry run passes the flag through under its only accepted name.
        std::fs::write(
            tool.join("bin/run-job"),
            "#!/usr/bin/env bash\nset -u\nprintf 'dry=%s\\n' \"${MOPS_DRY_RUN:-unset}\"\nexit 0\n",
        )
        .unwrap();
        let dry_out = run(&tool, t.root(), "10-stub.sh", true).unwrap().stdout;
        assert!(dry_out.contains("dry=1"), "stdout: {dry_out}");
        let apply_out = run(&tool, t.root(), "10-stub.sh", false).unwrap().stdout;
        assert!(apply_out.contains("dry=0"), "stdout: {apply_out}");

        // A missing job or missing wrapper is an error, not a silent "quiet".
        assert!(run(&tool, t.root(), "99-missing.sh", false).is_err());
    }
}
