/**
 * Overview — EXCEPTION FIRST.
 *
 * This view never lists the healthy fleet. It lists what needs review and
 * represents everything else as a count, because "quiet" is success and a
 * console that scrolls past 37 green rows is a console nobody reads.
 */
import { IC } from "../icons";
import { counts, esc, fmtDur, groupsOf, nextRun, sbar, schedText, untilStr } from "../format";
import { STATUS_META, lastDuration, statusOf } from "../types";
import type { DayStat, Job, Status } from "../types";
import { S, attnJobs, jobs } from "../store";
import { asyncPanel, fatal, statCard } from "./common";

const HEAT_STATUSES: Status[] = ["quiet", "spoke", "fail", "hang", "off"];

function heatmap(stats: DayStat[], groups: { name: string }[]): string {
  const days = [...new Set(stats.map((s) => s.day))].sort().slice(-30);
  // NUL cannot occur in a group name or a day label, so this composite key
  // cannot collide the way ":" or "-" could.
  const byKey = new Map(stats.map((s) => [`${s.group}\0${s.day}`, s.status]));
  const rows = groups.length ? groups.map((g) => g.name) : [...new Set(stats.map((s) => s.group))].sort();

  if (!days.length) {
    return `<div class="empty" style="padding:26px 16px">
      <div class="t">No history yet</div>
      <div class="muted" style="font-size:12px">The heatmap fills in as runs are recorded.</div></div>`;
  }

  return `<div class="fleet">
    ${rows
      .map((name) => {
        const cells = days.map((d) => {
          const st = byKey.get(`${name}\0${d}`);
          const cls = st ?? "";
          const label = st ? STATUS_META[st].t : "no run recorded";
          return `<i class="${cls}" title="${esc(name)} · ${esc(d)} — ${esc(label)}"></i>`;
        });
        return `<div class="fleet-row">
          <div class="fleet-name">${esc(name)}</div>
          <div class="fleet-cells">${cells.join("")}</div></div>`;
      })
      .join("")}
  </div>
  <div class="muted" style="font-size:11px;margin-top:10px">
    Rolled up by group — a day is coloured by its worst outcome. Open a group to see per-automation rows.
  </div>`;
}

export function vOverview(): string {
  if (!S.jobs.data) return fatal("Could not read the automations", S.jobs.error ?? "loading…", "jobs");

  const all = jobs();
  const attn = attnJobs();
  const c = counts(all);
  const enabled = all.filter((j) => j.enabled);
  const now = new Date();
  const nextUp = [...all]
    .filter((j) => j.enabled && nextRun(j, now))
    .sort((a, b) => (nextRun(a, now) as Date).getTime() - (nextRun(b, now) as Date).getTime())
    .slice(0, 5);
  const sweep = enabled.reduce((a, j) => a + lastDuration(j), 0);
  const groups = groupsOf(all);
  const running = all.filter((j) => S.running.has(j.file));

  const attnRow = (j: Job, i: number): string => `
    <div class="row" style="padding:10px 13px;${i ? "border-top:.5px solid var(--line-soft)" : ""};cursor:pointer"
         data-open="${esc(j.file)}">
      <span class="dot ${S.running.has(j.file) ? "running" : statusOf(j)}"></span>
      <div class="col" style="gap:1px;min-width:0;flex:1 1 auto">
        <div class="row" style="gap:7px">
          <span style="font-weight:600;font-size:12.5px">${esc(j.title)}</span>
          ${
            S.running.has(j.file)
              ? `<span class="badge running"><span class="spin">${IC.refresh}</span>Running</span>`
              : `<span class="badge ${statusOf(j)}">${STATUS_META[statusOf(j)].t}</span>`
          }
          <span class="muted" style="font-size:11px">${esc(j.group)}</span>
        </div>
        <div class="mono dim trunc" style="font-size:11px">${esc(j.last?.summary ?? "")}</div>
      </div>
      <div class="row" style="gap:5px;flex:0 0 auto">
        <button class="ibtn" data-run="${esc(j.file)}" title="Run now">${IC.play}</button>
        <button class="ibtn" data-open="${esc(j.file)}" title="Configure">${IC.edit}</button>
      </div>
    </div>`;

  return `
  ${
    all.length === 0
      ? `<div class="card empty">
          <span class="muted">${IC.jobs}</span>
          <div class="t">No automations on disk</div>
          <div class="muted" style="font-size:12px">MacOps reads <span class="mono">jobs/*.sh</span> from your
            automations root (<span class="mono">~/.mops</span> by default, changeable in Settings). Add one
            and it appears here.</div>
          <button class="btn pri" data-add>${IC.plus} New automation</button>
        </div>`
      : ""
  }
  ${
    running.length
      ? `<div class="banner ok" data-running-strip="1">
          <span style="color:var(--accent)"><span class="spin">${IC.refresh}</span></span>
          <div class="col" style="gap:1px">
            <div class="ttl ok">${
              running.length === 1 ? "1 automation running" : `${running.length} automations running`
            }</div>
            <div class="ds">${running
              .map((j) => `<span class="mono">${esc(j.file)}</span>`)
              .join(" · ")}</div>
          </div>
        </div>`
      : ""
  }
  ${
    all.length
      ? `
  <div class="banner ${attn.length ? "warn" : "ok"}">
    <span style="color:var(--${attn.length ? "spoke" : "quiet"})">${attn.length ? IC.warn : IC.check}</span>
    <div class="col" style="gap:1px">
      <div class="ttl ${attn.length ? "warn" : "ok"}">${
        attn.length ? `${attn.length} of ${enabled.length} automations need review` : "All quiet"
      }</div>
      <div class="ds">${
        attn.length
          ? `The other ${enabled.length - attn.length} ran and said nothing. Only these are listed.`
          : "Every automation ran and said nothing. That is the success signal."
      }</div>
    </div>
    ${attn.length ? `<button class="btn pri" style="margin-left:auto" data-view="attention">Review all</button>` : ""}
  </div>

  <div class="grid g4" style="margin-top:12px">
    ${statCard("Automations", all.length, `${enabled.length} enabled · ${c.off ?? 0} off`)}
    ${statCard("Quiet last run", c.quiet ?? 0, `${Math.round(((c.quiet ?? 0) / all.length) * 100)}% of the fleet`)}
    ${statCard("Need review", attn.length, attn.length ? "spoke, failed or timed out" : "nothing outstanding")}
    ${statCard("Daily runtime", fmtDur(sweep), "sum of last durations")}
  </div>

  ${
    attn.length
      ? `
  <div class="sec-h"><h3>Needs attention</h3><span class="ln"></span>
    <span class="muted" style="font-size:11px">${attn.length} shown · everything else is quiet</span></div>
  <div class="card" style="overflow:hidden">${attn.map(attnRow).join("")}</div>`
      : ""
  }

  <div class="sec-h"><h3>Groups</h3><span class="ln"></span>
    <button class="btn sm ghost" data-nav="jobs">Open all</button></div>
  <div class="grid g2">
    ${groups
      .map(({ name, icon, jobs: list }) => {
        const bad = list.filter((j) => j.enabled && ["spoke", "fail", "hang"].includes(statusOf(j)));
        const nxt = list
          .filter((j) => j.enabled && nextRun(j, now))
          .sort((a, b) => (nextRun(a, now) as Date).getTime() - (nextRun(b, now) as Date).getTime())[0];
        return `<div class="card pad col" style="gap:9px" role="button" tabindex="0" data-grpopen="${esc(name)}">
        <div class="row">
          <span style="color:var(--ink-2)">${IC[icon] ?? IC.sh}</span>
          <span style="font-weight:600;letter-spacing:-.01em">${esc(name)}</span>
          <span class="badge ${bad.length ? "spoke" : "quiet"}" style="margin-left:auto">
            ${bad.length ? `${bad.length} to review` : "all quiet"}</span>
        </div>
        ${sbar(list)}
        <div class="row muted" style="font-size:11px">
          <span>${list.length} automations · ${list.filter((j) => j.enabled).length} enabled</span>
          <span style="margin-left:auto">${nxt ? `next ${untilStr(nextRun(nxt, now), now)}` : "none scheduled"}</span>
        </div>
      </div>`;
      })
      .join("")}
  </div>

  <div class="sec-h"><h3>Fleet health — last 30 days</h3><span class="ln"></span>
    <div class="row muted" style="font-size:11px;gap:11px">
      ${HEAT_STATUSES.map(
        (k) => `<span class="row" style="gap:5px"><span class="dot ${k}"></span>${STATUS_META[k].t}</span>`,
      ).join("")}
    </div></div>
  <div class="card pad">
    ${asyncPanel(S.fleet, "Fleet history unavailable", "fleet", (stats) => heatmap(stats, groups))}
  </div>

  <div class="sec-h"><h3>Up next</h3><span class="ln"></span>
    <button class="btn sm ghost" data-nav="schedule">Schedule</button></div>
  <div class="card" style="overflow:hidden">
    ${
      nextUp.length
        ? nextUp
            .map(
              (j, i) => `
      <div class="row" style="padding:9px 13px;${i ? "border-top:.5px solid var(--line-soft)" : ""};cursor:pointer"
           data-open="${esc(j.file)}">
        <span class="dot ${S.running.has(j.file) ? "running" : statusOf(j)}"></span>
        <span style="font-size:12.5px;font-weight:500;min-width:0" class="trunc">${esc(j.title)}</span>
        ${S.running.has(j.file) ? '<span class="badge running">Running</span>' : ""}
        <span class="muted" style="font-size:11px">${esc(j.group)}</span>
        <span class="muted mono" style="font-size:11px;margin-left:auto">${esc(schedText(j))}</span>
        <span style="font-size:11.5px;font-weight:600;min-width:72px;text-align:right">${untilStr(
          nextRun(j, now),
          now,
        )}</span>
      </div>`,
            )
            .join("")
        : `<div class="empty" style="padding:26px"><div class="t">Nothing scheduled</div>
            <div class="muted" style="font-size:12px">Every enabled automation is manual-only.</div></div>`
    }
  </div>`
      : ""
  }`;
}
