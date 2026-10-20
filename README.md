# MacOps

A console for the scheduled maintenance automations on your Mac — package
manager updates, cleanup jobs, custom scripts — with one idea at its centre:

> **Silence is success.** A healthy run prints nothing. Automations speak only
> on change, regression, or error.

That is what keeps a list of forty automations from becoming notification
noise, and it is why the GUI renders *Quiet* as a green, first-class state
rather than an empty row.

> **Status: early.** The runner is in daily use; the Tauri console is still
> being built. Interfaces will move.

## What you get

- **A runner** (`bin/run`, `bin/run-job`) — bash, launchd-scheduled, with a
  per-job timeout that kills the whole process group, so no job can hang
  forever and silence the watchdog.
- **A console** (`console/`) — a Tauri app to add, schedule, enable, run and
  debug automations: per-job logs, run history, a 24-hour load view that shows
  when your jobs collide, and bulk actions for when there are many.
- **Examples** (`examples/jobs/`) — real automations to copy: Homebrew
  formulae with a hold list, a cleanup job, a CLI updater.

## Design rules worth knowing before you extend it

- **Never root, never sudo.** Everything runs as you, under a *user*
  LaunchAgent. Tools needing privilege request it themselves and degrade
  cleanly without it.
- **Fail safe toward running.** A corrupt state file means everything stays
  enabled. An automation that silently never runs is the worst outcome here.
- **launchd, not cron.** A missed run (laptop asleep at 02:00) is coalesced
  and fires once on wake — not zero times, and not as a catch-up burst.
- **The tool and your automations are separate.** This repo is the tool. Your
  jobs, logs and state live in your own directory. See
  [Two roots](#two-roots) below.

## Two roots

MacOps resolves two independent directories and never assumes they are the
same:

| Root | Holds | Default |
|---|---|---|
| **Tool root** | `bin/`, `lib/` — the installed code | derived from the script/executable location |
| **Automations root** | `jobs/`, `logs/`, `state/` — your data | `~/.mops` |

Resolution, first hit wins:

- **Tool root:** `$MOPS_TOOL_ROOT`, otherwise upward from the running
  executable and then the current directory (looking for `bin/run` and
  `lib/common.sh`).
- **Automations root:** `$MOPS_AUTOMATIONS_ROOT`, otherwise
  `automations_root` in `~/.config/mops/config.json`, otherwise `~/.mops`.

`~/.config/mops/config.json` looks like:

```json
{"automations_root": "/Volumes/Backup/macops"}
```

A missing config file is normal; a malformed one falls through to `~/.mops`
so maintenance keeps running. `bin/run` and `bin/run-job` export
`MOPS_TOOL_ROOT`, which is how a job finds `lib/common.sh` even when
its own directory is the automations root.

The two roots may legitimately be the **same** directory — a git checkout used
in place, with `jobs/` alongside `bin/`. Point
`MOPS_AUTOMATIONS_ROOT` at that checkout and everything keeps working;
nothing writes user data into the tool directory just because they coincide.

### Environment variables

Canonical names:

| Variable | Meaning |
|---|---|
| `MOPS_TOOL_ROOT` | the installed code (`bin/`, `lib/`) |
| `MOPS_AUTOMATIONS_ROOT` | your `jobs/`, `logs/` and `state/` |
| `MOPS_LOG` | the run log path |
| `MOPS_DRY_RUN` | `1` previews; `0` applies |
| `MOPS_RUNS` | where structured run records are appended |
| `MOPS_RUN_STDOUT_CAP` | cap on captured stdout recorded per run |

**Deprecated:** the old `MACHINE_OPS_*` names still work and will be **removed
in 0.4.0**. During the transition `bin/run` and `bin/run-job` export BOTH names
with identical values, so a job that reads the legacy name behaves exactly as
it did. Reads prefer `MOPS_*` when both are set. The alias exists for the
owner's own automations, which live in a separate repository this rename
cannot reach — not for third-party users. When those automations have
migrated, the legacy names go.

`install.sh` resolves the automations root when it renders the LaunchAgent and
pins it in the plist, so launchd and `bin/run` always agree.

## Adding an automation

One file in your automations directory:

```bash
#!/usr/bin/env bash
# tier: daily
# timeout: 900
set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The tool root is exported by bin/run/bin/run-job; the fallback supports a
# checkout used in place (jobs/ beside lib/).
MACOPS_LIB="${MOPS_TOOL_ROOT:-${MACHINE_OPS_TOOL_ROOT:-$SCRIPT_DIR/..}}/lib"
source "$MACOPS_LIB/common.sh"
LABEL="my-thing"

# ... do the work, honouring MOPS_DRY_RUN=1 ...

log_line "[$LABEL] summary always goes to the log"
alert "only on change, regression or error"   # omit and the run stays silent
flush_alerts "$LABEL"
exit 0
```

Line 2 sets the tier, line 3 the timeout. Both are parsed as anchored full
lines, so commented-out code can never be mistaken for metadata.

## Contributing

Read [`AGENTS.md`](AGENTS.md) first — it is the constitution, and it is what
reviewers judge against. Gates live in
[`.agents/orchestration.yaml`](.agents/orchestration.yaml). Branch from `dev`,
PR back into `dev`.

## License

MIT — see [LICENSE](LICENSE).
