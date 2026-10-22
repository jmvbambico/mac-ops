/**
 * The on-demand run modal and its live stream state.
 *
 * Kept out of main.ts so the streaming behaviour is testable with no DOM and
 * no Tauri runtime. `RunCtx` is mutated by `applyRunLogBatch` as batches
 * arrive; `vRunModal` is a pure renderer over it.
 *
 * The stream is the shared run log (`logs/machine-ops.log`), not the child's
 * stdout. A nominal run prints nothing on stdout (AGENTS.md Principle I), so
 * stdout-only output would leave this modal blank for exactly the runs the
 * user wants to watch.
 */
import { esc, fmtDur } from "../format";
import { IC } from "../icons";
import type { Job, RunLogBatch, RunOutcome } from "../types";
import { STATUS_META, statusOf } from "../types";

export interface RunCtx {
  file: string;
  dry: boolean;
  outcome: RunOutcome | null;
  busy: boolean;
  /** Lines streamed from the run log since this run started. */
  lines: string[];
  /** How many streamed lines have been dropped by the retention cap. */
  dropped: number;
  /**
   * Captured command output from `logs/job-<slug>.out.log`, fetched after the
   * run finishes (the job calls `log_output` near the end). Empty means the
   * automation has not opted in — normal, not a failure.
   */
  output: string;
}

export function newRunCtx(file: string, dry: boolean): RunCtx {
  return { file, dry, outcome: null, busy: false, lines: [], dropped: 0, output: "" };
}

/**
 * How many streamed log lines are kept in memory for a run.
 *
 * A chatty job (or a scheduled run interleaving its lines into the same log)
 * would otherwise grow the array without limit, and every repaint rebuilds
 * the whole history into the `<pre>`. The cap keeps the newest lines; the
 * dropped ones are named to the user rather than silently vanishing, because
 * the durable record — `logs/machine-ops.log` — still has everything.
 */
export const MAX_RUN_LOG_LINES = 2000;

/**
 * The notice shown when lines have been dropped by the cap. The count is the
 * true number of dropped lines for the run, so it never compounds.
 */
export function truncationNotice(dropped: number): string {
  return `… ${dropped} earlier lines not shown (see logs/machine-ops.log)`;
}

/**
 * Append a streamed batch, but only when it belongs to THIS run. Returns true
 * when the modal needs a repaint. A batch for another automation is ignored:
 * a stale listener must never render another job's lines.
 */
export function applyRunLogBatch(ctx: RunCtx, batch: RunLogBatch): boolean {
  if (batch.job !== ctx.file || batch.lines.length === 0) return false;
  ctx.lines.push(...batch.lines);
  // Keep the tail, drop the head. The count of dropped lines is kept
  // separately so the notice stays accurate as more batches arrive.
  if (ctx.lines.length > MAX_RUN_LOG_LINES) {
    const excess = ctx.lines.length - MAX_RUN_LOG_LINES;
    ctx.lines.splice(0, excess);
    ctx.dropped += excess;
  }
  return true;
}

/**
 * Record the captured per-job output fetched after the run completes. Returns
 * true when the modal needs a repaint. An empty string is a legitimate answer
 * (the automation has not opted into `log_output`), so it is stored like any
 * other and simply contributes nothing to the body.
 */
export function applyRunOutput(ctx: RunCtx, output: string): boolean {
  if (ctx.output === output) return false;
  ctx.output = output;
  return true;
}

/**
 * The modal body: streamed log detail first, then the captured per-job output
 * (the long report the one-line summaries hide), then the child's own stdout /
 * stderr once the run finishes. The "silent, which is success" fallback is
 * kept only when there genuinely is nothing else to show.
 */
export function runBody(ctx: RunCtx, job: Job): string {
  const o = ctx.outcome;
  const notice = ctx.dropped > 0 ? truncationNotice(ctx.dropped) : "";
  const logText = [notice, ctx.lines.join("\n")].filter(Boolean).join("\n").trim();
  const capturedText = ctx.output.trim();
  const outText = o
    ? [o.stdout.trim(), o.stderr.trim()].filter(Boolean).join("\n")
    : "";
  const body = [logText, capturedText, outText].filter(Boolean).join("\n");
  if (body) return esc(body);

  if (o) return "(no output on stdout — silent, which is success)";
  if (ctx.dry) {
    return `[${esc(job.label)}] dry run — no changes made\n${
      ctx.busy ? "(waiting for log output…)" : "(no output on stdout — this run would be silent)"
    }`;
  }
  if (ctx.busy) {
    return `[${esc(job.label)}] starting…\n(waiting for log output…)`;
  }
  return `[${esc(job.label)}] ${esc(job.last?.summary ?? "")}\n(no output on stdout — silent, which is success)`;
}

export function vRunModal(ctx: RunCtx, job: Job): string {
  const st = statusOf(job);
  const quiet = st === "quiet";
  const o = ctx.outcome;
  const tone = ctx.dry ? "accent" : quiet ? "quiet" : "spoke";

  const badge = o
    ? `<span class="badge ${o.timed_out ? "hang" : o.status}">${
        o.timed_out ? "Timed out" : STATUS_META[o.status].t
      }</span>`
    : ctx.busy
      ? `<span class="badge running"><span class="spin">${IC.refresh}</span>Running</span>`
      : `<span class="badge ${ctx.dry ? "off" : quiet ? "quiet" : st}">${
          ctx.dry ? "preview" : STATUS_META[st].t
        }</span>`;

  return `
    <div class="scrim" data-close><div class="modal" data-running="${ctx.busy ? "1" : "0"}"
         style="width:min(620px,100%)">
      <div class="row" style="gap:9px">
        <span style="color:var(--${tone})">${ctx.dry ? IC.flask : quiet ? IC.check : IC.warn}</span>
        <div class="col" style="gap:1px"><h3 style="margin:0">${ctx.dry ? "Dry run" : "Run"} — ${esc(job.title)}</h3>
          <span class="muted mono" style="font-size:11.5px">bin/run-job jobs/${esc(job.file)}</span></div>
        ${badge}
      </div>
      <pre class="src" data-run-stream="1"
        style="margin-top:13px;max-height:46vh;overflow:auto;white-space:pre-wrap;overflow-wrap:anywhere">${runBody(ctx, job)}</pre>
      ${
        o
          ? `<div class="row muted" style="font-size:11px;margin-top:9px;gap:11px">
              <span class="mono">rc=${o.rc}</span>
              <span class="mono">${fmtDur(o.duration_s)}</span>
              ${o.timed_out ? '<span style="color:var(--hang)">timed out</span>' : ""}
            </div>`
          : ""
      }
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        ${
          ctx.busy
            ? `<span class="row muted" style="font-size:11.5px;gap:6px"><span class="spin">${IC.refresh}</span>Running…</span>
               <span class="muted" style="font-size:11.5px">This run continues in the background.</span>`
            : ""
        }
        <button class="btn" data-close>${o ? "Done" : ctx.busy ? "Close" : "Cancel"}</button>
        ${
          o
            ? ""
            : `<button class="btn pri" data-run-confirm="1">${ctx.dry ? IC.flask : IC.play} ${
                ctx.dry ? "Dry run" : "Run now"
              }</button>`
        }
      </div>
    </div></div>`;
}
