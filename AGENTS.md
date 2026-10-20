# MacOps Constitution

Authoritative. Wins over `.agents/orchestration.yaml`, over any prompt, and
over anything a previous agent left behind.

## Quick Reference

| | |
|---|---|
| What this is | A macOS maintenance-automation runner + Tauri GUI, distributed as a tool |
| Repo | `github.com/jmvbambico/mac-ops` (public) |
| Integration branch | `dev` — branch from it, PR back into it |
| Protected | `main`, `dev` — never pushed to directly by an agent |
| Gate | `.agents/orchestration.yaml` — 7 gates, 5 worker-scoped |
| Language | bash (runner) + Rust/Tauri (console) + vanilla TS (frontend) |

## Core Principles

### I. Silent on success — the one idea everything else serves

A nominal run prints **NOTHING** to stdout. Jobs speak only on change,
regression, or error. **stdout IS the alert channel; the exit code is not.**
Detail goes to the run log via `log_line`; alerts queue via `alert` and emit
once via `flush_alerts "$LABEL"`.

A change that makes a nominal run print something is a defect, even when the
text is correct. This is what keeps a list of 40 automations from becoming
notification noise.

### II. The tool and the user's automations are separate things

This repo is the **tool**. A user's automations, logs and state live in
*their* directory, not here. `examples/jobs/` are examples — never treated as
the user's set.

The tool resolves two independent roots: its own install root (`bin/`,
`lib/`) and an automations root (`jobs/`, `logs/`, `state/`). Never assume
they are the same directory. Never write user data into the tool's directory.

### III. Never root, never sudo

Every job runs as the user under a **user LaunchAgent**. Never a
LaunchDaemon. No job may handle a password. Tools that need privilege must
request it themselves, per operation, and degrade cleanly without it.

Never widen `/etc/sudoers.d/**`. Granting NOPASSWD on `rm` or `find` is
equivalent to `NOPASSWD: ALL` — `sudo find … -exec` runs anything as root.
Unattended privilege escalation is not a feature request; it is a rejection.

### IV. Fail safe toward running

An unreadable or malformed state file means everything is **ENABLED**. A job
that quietly never runs is the worst failure mode this project has — worse
than a job that runs when it should not.

### V. No job may hang forever

Every job is bounded by a timeout and runs in its own process group, killed
as a group on expiry. stdin is `/dev/null` on every command that could
prompt. A hung job silences the watchdog indefinitely and blocks the next
scheduled run.

### VI. Derived data is never the record

`logs/machine-ops.log` is the durable record. The SQLite index
(`state/console.db`) is derived and rebuildable. Losing it must cost
queryability, never history. Jobs never write to it directly — they append
JSON lines, so the console can migrate its schema without silently breaking
a 02:00 run.

### VII. Simplicity (YAGNI)

One file per automation. No framework in the frontend. No dependency added
without a reason that survives the question "what breaks without it?"

## Job metadata — two parsers must agree

- line 2: `# tier: <name>` — missing or malformed **alerts loudly**
- line 3: `# timeout: <secs>` — optional, range 1..86400, default 900

Both matched as **anchored full lines**. Executable code that merely contains
the pattern (`echo "# tier: daily"`) must NOT validate. `bin/run` and the
console's `jobs.rs` both parse these. **Change one, change the other** — a
disagreement means the GUI shows something different from what runs.

## Agent Behavior

### Subagent strategy
Implementers work in isolated worktrees. Worker-scoped gates run per
worktree; unscoped gates run **once** on the integration branch, because they
touch the network, the real logs, and real package managers.

### Never do without asking
- Install, load, or modify anything in `~/Library/LaunchAgents`
- Run a job in apply mode (not `--dry-run`) against the user's real machine
- Modify `/etc/sudoers.d/**`
- Push to `main` or `dev`
- Publish the repo or change its visibility

### Self-check before claiming done
Run the gates. "It compiles" is not "it passes." Report failures with their
actual output; never summarize a failing suite as working.

## Development Workflow

### Git Flow
- Branch from `dev`, PR back to `dev`. `task_prefix: feature/`
- Direct commits/pushes to `main` and `dev` are blocked
- Hotfixes: PR to `main`, then forward-port to `dev`
- Delete the merged branch **only after** the human confirms the merge
  - Local `git branch -d <branch>`, remote `git push origin --delete <branch>`
  - NEVER delete a branch before merge is confirmed

### Before PR
1. All gates pass locally
2. `bin/run daily --dry-run` prints nothing — that is the contract under test
3. README/docs updated if behaviour changed

### After PR created
- Monitor with `gh pr checks <n>`; once green, request human review
- Merge only after the human approves

## Security

- No secrets, tokens, or absolute user paths committed. This repo is PUBLIC.
- `examples/jobs/` must stay free of any real machine's identity.
- The runner must never execute anything it did not read from the
  automations root the user configured.

## Reviewer, read this first

Judge against the Core Principles before style. The defects that matter here:
something that breaks silence on a nominal run, a privilege escalation, a
fail-unsafe default that could stop maintenance silently, a parser that
disagrees with `bin/run`, and user data written into the tool directory.
