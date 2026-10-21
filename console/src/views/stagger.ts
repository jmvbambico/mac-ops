/**
 * The auto-stagger modal — the window that says when the fleet may start and
 * how many automations may run at once.
 *
 * It is a view like the others: a pure renderer. The window inputs keep the
 * value the user is typing (see `getDraft`) so an unrelated repaint cannot
 * reset them.
 */
import { esc, hhmm, localLoadProfile } from "../format";
import { IC } from "../icons";
import { S, getDraft, jobByFile, jobs } from "../store";

export interface StaggerCtx {
  files: string[];
  start: number;
  end: number;
  max: number;
  plan: Map<string, number> | null;
}

export function peakConcurrency(): number {
  return Math.max(0, ...(S.profile.data ?? localLoadProfile(jobs())));
}

export function vStagger(ctx: StaggerCtx): string {
  const preview = ctx.plan
    ? [...ctx.plan.entries()]
        .sort((a, b) => a[1] - b[1])
        .map(([file, min]) => {
          const j = jobByFile(file);
          return `<div class="row" style="padding:5px 0;border-top:.5px solid var(--line-soft)">
            <span class="mono" style="font-size:11.5px;min-width:46px">${hhmm(min)}</span>
            <span class="trunc" style="font-size:12px">${esc(j?.title ?? file)}</span>
            <span class="muted mono" style="font-size:11px;margin-left:auto">${esc(file)}</span>
          </div>`;
        })
        .join("")
    : `<span class="muted" style="font-size:11.5px">Preview to see the proposed start times.</span>`;

  return `
    <div class="scrim" data-close><div class="modal" style="width:min(560px,100%)">
      <h3>Auto-stagger ${ctx.files.length} automations</h3>
      <p class="muted" style="margin:4px 0 14px;font-size:12.5px">Spreads start times across a window so no
        two long jobs overlap. Longest first, so the heavy work lands while the machine is idlest.</p>
      <div class="grid g2">
        <div class="field" style="margin:0"><label>Window start</label>
          <input class="inp" id="s-start" value="${esc(getDraft("s-start") ?? hhmm(ctx.start))}" style="height:30px"></div>
        <div class="field" style="margin:0"><label>Window end</label>
          <input class="inp" id="s-end" value="${esc(getDraft("s-end") ?? hhmm(ctx.end))}" style="height:30px"></div>
      </div>
      <div class="field"><label>Max concurrent</label>
        <input class="inp" type="number" id="s-max" min="1" max="20" value="${getDraft("s-max") ?? ctx.max}"
          style="height:30px;width:80px">
        <div class="hint">Peak today is ${peakConcurrency()}.</div></div>
      <div class="card" style="margin-top:11px;overflow:hidden;padding:8px 11px">${preview}</div>
      <div class="note" style="margin-top:13px"><span style="color:var(--accent);flex:0 0 auto">${IC.check}</span>
        <div>Rewrites <span class="mono">StartCalendarInterval</span> in each LaunchAgent and the schedule in
        <span class="mono">state/console.json</span>. Reversible — the previous layout is kept.</div></div>
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        <button class="btn" data-close>Cancel</button>
        <button class="btn" data-stagger-preview>Preview</button>
        <button class="btn pri" data-stagger-apply ${ctx.plan ? "" : "disabled"}>${IC.stagger} Apply layout</button>
      </div>
    </div></div>`;
}
