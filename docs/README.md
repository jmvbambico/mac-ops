# MacOps documentation

MacOps runs your macOS maintenance automations on a schedule and stays quiet
unless something needs you.

| Guide | For |
|---|---|
| **[Using the console](using-the-console.md)** | Driving the app: what each view is for, how to run, schedule, group and inspect automations. With screenshots. |
| **[Writing automations](writing-automations.md)** | Authoring a job in bash or Python: the header contract, the three output channels, dry runs, timeouts, privileges, and a pre-flight checklist. |

## Start here

**New to the app?** Read [Using the console](using-the-console.md). The
[Overview](using-the-console.md#overview--what-needs-you) section explains the
one idea the whole interface is built around.

**Adding an automation?** Go to
[Writing automations](writing-automations.md#the-contract-in-one-page) — the
contract fits on a page, and the rest of the guide is reference.

**Changing the tool itself?** Read `AGENTS.md` at the repo root first. It is
the constitution: it wins over any guide here, and over anything a previous
contributor left behind.

## The one idea

> **Silent on success.** A nominal run prints **NOTHING** to stdout. Jobs speak
> only on change, regression, or error.

stdout is the alert channel; the exit code is not. Detail goes to the run log,
alerts are queued and emitted once at the end of a run.

Everything else follows from this. The console is an exception list rather than
a wall of green ticks. A healthy fleet looks almost empty, and that is the
design working — it is what keeps forty automations from becoming forty
notifications a night.

## Two roots, never assumed to be one

| Root | Holds | Default |
|---|---|---|
| **Tool root** | `bin/`, `lib/` — the installed tool | the executable's own location |
| **Automations root** | `jobs/`, `logs/`, `state/` — your data | `~/.mops` |

The repository is the *tool*. Your automations, logs and state live in *your*
directory. `examples/jobs/` are examples to copy, never treated as your set.

## Also in this directory

- `design-prototype.html` — the approved visual prototype the console's markup
  and icon set were ported from. Reference for design work; not a running app.
- `images/` — screenshots used by these guides, captured from the console's
  built-in demo data.
