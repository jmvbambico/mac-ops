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
  jobs, logs and state live in your own directory.

## Adding an automation

One file in your automations directory:

```bash
#!/usr/bin/env bash
# tier: daily
# timeout: 900
set -u
source "$MACOPS_LIB/common.sh"
LABEL="my-thing"

# ... do the work, honouring MACHINE_OPS_DRY_RUN=1 ...

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
