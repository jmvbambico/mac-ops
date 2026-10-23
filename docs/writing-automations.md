# Writing automations

An automation is **one executable file** in your automations root:

```
~/.mops/jobs/50-disk-audit.sh
```

One file per automation. No framework, no registry, no config to update — the
runner discovers `jobs/*.sh` by globbing, and the console reads the same
directory.

This guide covers the contract every job must honour, in bash and in Python.

---

## The contract in one page

| Rule | Why |
|---|---|
| A nominal run prints **NOTHING** to stdout | stdout **is** the alert channel. Anything printed is treated as an alert. |
| Line 2 is `# tier: <name>` | Two parsers read it — `bin/run` and the console. Missing or malformed alerts loudly. |
| Line 3 is `# timeout: <secs>` (optional) | Range 1..86400, default 900. Bounds the run. |
| Honour `MOPS_DRY_RUN=1` | A dry run must change nothing. |
| Never `sudo`, never prompt | Jobs run unattended under a user LaunchAgent. |
| Exit 0 unless the job itself failed | The exit code is not the alert channel, but it still means something. |

> **A change that makes a nominal run print something is a defect, even when
> the text is correct.** A fleet of 40 automations that each say one helpful
> thing per night is 40 notifications a night, which is zero signal.

---

## The header, and why it is strict

```bash
#!/usr/bin/env bash
# tier: daily
# timeout: 900
# disk-audit: warn before the boot volume gets tight.
```

- **Line 2 — `# tier:`** decides which sweep runs it. `bin/run daily` runs
  every job whose tier is `daily`. Missing or malformed is surfaced loudly
  rather than silently skipped.
- **Line 3 — `# timeout:`** is optional. Outside 1..86400 it falls back to 900
  with a logged note.

Both are matched as **anchored full lines**. Executable code that merely
*contains* the pattern does not validate:

```bash
echo "# tier: daily"   # ← not metadata, and correctly not treated as such
```

`bin/run` and the console's `jobs.rs` both parse these. **Change one, change
the other** — a disagreement means the GUI shows something different from what
actually runs.

---

## The bash skeleton

```bash
#!/usr/bin/env bash
# tier: daily
# timeout: 900
# disk-audit: warn before the boot volume gets tight.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# lib/common.sh belongs to the TOOL root, which may differ from this job's
# automations root. bin/run and bin/run-job export MOPS_TOOL_ROOT.
MACOPS_LIB="${MOPS_TOOL_ROOT:-$SCRIPT_DIR/..}/lib"
# shellcheck disable=SC1091 # path resolved at runtime from the tool root
if ! source "$MACOPS_LIB/common.sh"; then
    printf '50-disk-audit: cannot load lib/common.sh\n' >&2
    exit 1
fi

LABEL="disk-audit"
DRY="${MOPS_DRY_RUN:-0}"
THRESHOLD_GB=15

FREE_GB="$(df -g / | awk 'NR==2 {print $4}')"

if (( FREE_GB < THRESHOLD_GB )); then
    alert "[$LABEL] free space ${FREE_GB}G is below ${THRESHOLD_GB}G"
fi

log_line "[$LABEL] free=${FREE_GB}G threshold=${THRESHOLD_GB}G rc=0 dry_run=$DRY"

flush_alerts "$LABEL"
exit 0
```

Three details that are easy to get wrong:

**`set -u`, not `set -e`.** A watchdog that aborts halfway through leaves you
with no summary and no alert. Handle failures explicitly instead.

**Resolve `lib/` from `MOPS_TOOL_ROOT`.** The tool root and your automations
root are independent. The `$SCRIPT_DIR/..` fallback only covers a checkout used
in place.

**Always end with `flush_alerts "$LABEL"`.** Alerts are queued, not printed, so
the job decides *once* at the end whether it has anything worth surfacing.

---

## The three output channels

This is the part that makes or breaks an automation.

### 1. `alert` + `flush_alerts` — stdout, the alert channel

```bash
alert "[$LABEL] 3 certificates expire within 14 days"
...
flush_alerts "$LABEL"
```

Queues a line and emits it **once**, at the end, on stdout. Reserve this for
**change, regression, or error**. If it fires on a healthy run, it is a defect.

### 2. `log_line` — the durable one-line summary

```bash
log_line "[$LABEL] certs=12 expiring=0 rc=0 dry_run=$DRY"
```

Appends one timestamped line to `logs/machine-ops.log`. **Never touches
stdout.** Log one of these on *every* run, healthy or not — it is what the
console shows as the run's summary, and what a rebuild recovers from.

Write them as `key=value` pairs. They are read by humans scanning a log and by
the console's parser.

### 3. `log_output` — the full captured output

```bash
CLEAN_OUT="$("$MOLE" clean < /dev/null 2>&1)"
CLEAN="$(printf '%s\n' "$CLEAN_OUT" | strip_ansi)"
log_output "$CLEAN"
```

Writes the complete output to `logs/job-<slug>.out.log`, which the console
shows in the run modal. Without this, a tool's long report is captured into a
shell variable, summarised into one line, and **thrown away**.

- Appends, so several calls per run are fine.
- ANSI escapes are stripped automatically.
- Capped at 256 KiB (`MOPS_JOB_OUT_CAP`); truncation is marked in the file.
- Truncated at the start of every run, so it always holds the current run.
- Silent no-op outside the runner — never writes to stdout, never fails you.

**Rule of thumb:** `alert` for *you must act*, `log_line` for *what happened in
one line*, `log_output` for *the full text, if you go looking*.

---

## Dry run, and the one dangerous default

```bash
DRY="${MOPS_DRY_RUN:-0}"
```

> `MOPS_DRY_RUN` **unset means apply mode.** The runner always exports an
> explicit `"1"` or `"0"`, so a correctly written job never sees it unset — but
> a job that reads some other variable name falls through to `0` and makes real
> changes while you believe you are previewing.

Honour it on every path that mutates anything:

```bash
if [[ "$DRY" == "1" ]]; then
    OUT="$("$TOOL" clean --dry-run < /dev/null 2>&1)"
else
    OUT="$("$TOOL" clean < /dev/null 2>&1)"
fi
```

A dry run must be genuinely read-only. The console's *Dry run* button, and
`bin/run daily --dry-run`, both depend on it.

---

## Timeouts and process groups

Every job runs in its own process group with `stdin` on `/dev/null`. On
timeout the **whole group** is `TERM`'d, graced 5 seconds, then `KILL`'d — so a
child process cannot outlive its parent and silence the sweep.

You do not implement any of this. You only choose `# timeout:` honestly:
long enough for a slow day, short enough that a hang is caught before the next
scheduled run.

Because `bin/run` walks a tier **sequentially**, one job's timeout budget
delays every job behind it. Use the console's **Schedule** view to see where
the load lands.

---

## Privileges

**Never `sudo`. Never handle a password.**

Jobs run as you, under a user LaunchAgent — never a LaunchDaemon. A tool that
needs privilege must request it itself, per operation, and degrade cleanly
without it. If a task cannot be done unattended without privilege, it is not a
task for this runner.

Widening `/etc/sudoers.d/**` is not a workaround: `NOPASSWD` on `find` or `rm`
is equivalent to `NOPASSWD: ALL`, because `sudo find … -exec` runs anything.

---

## Python automations

A Python automation is still **a bash job file** — it has to be, because the
tier and timeout contract lives in those header lines. The job file is a thin
wrapper that runs your `.py` in its own virtualenv.

Two files:

```
~/.mops/jobs/70-expense-parser.sh     ← the job (tier, timeout, logging)
~/.mops/scripts/expense-parser.py     ← the work
```

The script path is resolved **relative to the automations root** — the folder
that contains `jobs/`, not the `jobs/` directory itself.

### Let the console generate the wrapper

The venv wrapper has real subtleties: creating the environment on first run,
installing requirements only when their hash changes, skipping silently when
offline, and keeping all of it off stdout. Rather than hand-copying it, use
**New automation → `python script`** in the console and point it at your `.py`.

What it gives you:

| Behaviour | Detail |
|---|---|
| Isolated venv | `~/.cache/machine-ops/venvs/<label>` (override with `MOPS_VENV_DIR`) |
| Requirements | `<script>-requirements.txt` beside the script, installed only when changed |
| Offline | skips **silently** — a nightly false alarm for being offline poisons the alert channel |
| Dry run | the wrapper creates and installs nothing when `MOPS_DRY_RUN=1` |
| Output | anything your script prints to stdout is log detail; only a non-zero exit alerts |

### Writing the Python side

Your script is ordinary Python with one inversion to remember:

```python
#!/usr/bin/env python3
"""Expense parser — summarise new receipts."""
import os
import sys

DRY = os.environ.get("MOPS_DRY_RUN", "0") == "1"

def main() -> int:
    receipts = find_new_receipts()
    if not receipts:
        return 0                     # silent: nothing changed

    if not DRY:
        archive(receipts)

    # Printing here becomes LOG DETAIL, not an alert — the wrapper captures it.
    print(f"parsed={len(receipts)} dry_run={int(DRY)}")

    if any(r.malformed for r in receipts):
        # Alert by FAILING. The wrapper alerts on a non-zero exit and attaches
        # the last 15 lines of your output, so say the useful thing last.
        print("malformed receipts found in 2 of 14")
        return 1
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

**Your script's output is captured by the wrapper and logged, not alerted.**
That is the opposite of the bash case, where stdout *is* the alert channel.
The wrapper runs you as `"$VENV_PY" "$PY_SCRIPT" < /dev/null 2>&1`, so stdout
and stderr are captured together — stderr is not a shortcut to an alert.

What the wrapper does with it:

| Your script | What happens |
|---|---|
| prints, exits `0` | output goes to `log_output` and a trimmed `log_line`; the run stays **quiet** |
| exits non-zero | an alert fires, carrying `rc` plus the **last 15 lines** of your output |

So to raise an alert from Python, **exit non-zero** — and put the line you want
a human to read at the end, because that is the part the alert quotes.

Keep `MOPS_DRY_RUN` honoured in Python too — the wrapper passes it through.

---

## Testing before you enable it

```bash
# 1. Syntax, under the shell CI actually uses (macOS /bin/bash is 3.2).
bash -n  ~/.mops/jobs/50-disk-audit.sh
/bin/bash -n ~/.mops/jobs/50-disk-audit.sh
shellcheck ~/.mops/jobs/50-disk-audit.sh

# 2. Dry run it, and check the silent contract yourself.
out=$(MOPS_DRY_RUN=1 bin/run-job ~/.mops/jobs/50-disk-audit.sh)
echo "rc=$?  stdout_bytes=${#out}"      # a healthy dry run: rc=0  stdout_bytes=0

# 3. Read what it actually recorded.
tail -5 ~/.mops/logs/machine-ops.log
cat     ~/.mops/logs/job-50-disk-audit.out.log
```

`stdout_bytes=0` on a healthy run is the assertion that matters. If it is
non-zero, your automation will alert every night.

Then enable it from the console — or leave it disabled and let the **Validate**
button check it first.

---

## Environment reference

Exported by `bin/run` / `bin/run-job`; read them, do not set them:

| Variable | Meaning |
|---|---|
| `MOPS_DRY_RUN` | `1` preview, `0` apply. Always explicitly set by the runner. |
| `MOPS_TOOL_ROOT` | Where `bin/` and `lib/` live. Resolve `lib/common.sh` from it. |
| `MOPS_AUTOMATIONS_ROOT` | Where `jobs/`, `logs/`, `state/` live. |
| `MOPS_LOG` | The shared run log. `log_line` writes here. |
| `MOPS_JOB_OUT` | This job's output file. `log_output` writes here. |
| `MOPS_JOB_OUT_CAP` | Byte cap for that file (default 262144). |
| `MOPS_VENV_DIR` | Override the venv location for a Python automation. |

### Helpers in `lib/common.sh`

| Helper | Use |
|---|---|
| `log_line <msg>` | one timestamped line to the run log |
| `alert <msg>` | queue an alert |
| `flush_alerts <label>` | emit queued alerts once, at the end |
| `log_output <text>` | full captured output for the console |
| `strip_ansi` | filter: remove colour escapes |
| `have <cmd>` | is a command available |
| `ts` | current timestamp in log format |

`record_run` exists in the same file but is called **by the runner**, not by
your job.

---

## Checklist

- [ ] Line 2 is `# tier: <name>`, line 3 is `# timeout: <secs>` if you need one
- [ ] `set -u`, and `lib/common.sh` sourced via `MOPS_TOOL_ROOT`
- [ ] A healthy run prints **zero bytes** to stdout
- [ ] Exactly one `log_line` summary per run, as `key=value` pairs
- [ ] `log_output` for any command output worth keeping
- [ ] `flush_alerts "$LABEL"` at the end
- [ ] `MOPS_DRY_RUN` honoured on every mutating path
- [ ] No `sudo`, no prompts, `stdin` never needed
- [ ] `bash -n`, `/bin/bash -n` and `shellcheck` all clean
- [ ] Dry-run verified before enabling

---

## See also

- [Using the console](using-the-console.md)
- `AGENTS.md` — the constitution these rules come from
- `examples/jobs/` — four real automations to copy
