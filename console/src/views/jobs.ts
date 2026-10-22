/**
 * Automations — the dense view: search, status chips, group buckets,
 * multi-select with bulk actions.
 */
import { IC } from "../icons";
import { esc, fmtDur, groupsOf, nextRun, schedText, untilStr } from "../format";
import { STATUS_META, STATUSES, lastDuration, statusOf } from "../types";
import type { Job } from "../types";
import { S, SAVED_VIEWS, jobs, visible } from "../store";
import { fatal } from "./common";

export function vJobs(): string {
  if (!S.jobs.data) return fatal("Could not read the automations", S.jobs.error ?? "loading…", "jobs");

  const all = jobs();
  const list = visible();
  const now = new Date();
  const buckets: [string | null, Job[]][] =
    S.groupBy === "group"
      ? groupsOf(list).map((g) => [g.name, g.jobs] as [string, Job[]])
      : [[null, list]];

  const row = (j: Job): string => {
    const st = statusOf(j);
    const running = S.running.has(j.file);
    return `<tr data-open="${esc(j.file)}" class="${S.picked.has(j.file) ? "pick" : ""}"
        ${running ? 'data-running="1"' : ""}>
      <td class="c" style="width:28px"><input type="checkbox" data-pick="${esc(j.file)}"
        ${S.picked.has(j.file) ? "checked" : ""}></td>
      <td class="c"><span class="dot ${running ? "running" : st}" title="${
        running ? "Running now" : STATUS_META[st].t
      }"></span></td>
      <td class="c" style="min-width:190px">
        <div class="nm trunc">${esc(j.title)}</div>
        <code class="mini">${esc(j.file)}</code></td>
      <td class="c mini">${esc(j.group)}</td>
      <td class="c mono" style="font-size:11.5px">${esc(schedText(j))}</td>
      <td class="c" style="white-space:nowrap">${
        j.enabled ? untilStr(nextRun(j, now), now) : '<span class="muted">—</span>'
      }</td>
      <td class="c">${lastDuration(j) ? fmtDur(lastDuration(j)) : '<span class="muted">—</span>'}
        <div class="mini">${j.timeout}s cap</div></td>
      <td class="c" style="max-width:220px"><div class="mono trunc" style="font-size:11px"
        title="${esc(j.last?.summary ?? "")}">${esc(j.last?.summary ?? (j.enabled ? "never run" : "disabled"))}</div></td>
      <td class="c">${
        running
          ? `<span class="badge running"><span class="spin">${IC.refresh}</span>Running</span>`
          : `<span class="badge ${st}">${STATUS_META[st].t}</span>`
      }</td>
      <td class="c"><div class="row" style="gap:2px;justify-content:flex-end">
        <button class="ibtn" data-run="${esc(j.file)}" title="Run now">${IC.play}</button>
        <button class="ibtn" data-dry="${esc(j.file)}" title="Dry run">${IC.flask}</button>
        <button class="ibtn" data-del="${esc(j.file)}" title="Delete">${IC.trash}</button>
      </div></td></tr>`;
  };

  return `
  ${
    S.picked.size
      ? `
  <div class="bulk">
    <span style="font-weight:600;font-size:12.5px">${S.picked.size} selected</span>
    <button class="btn sm" data-bulk="enable">Enable</button>
    <button class="btn sm" data-bulk="disable">Disable</button>
    <button class="btn sm" data-bulk="run">${IC.play} Run</button>
    <button class="btn sm" data-bulk="stagger">${IC.stagger} Auto-stagger</button>
    <button class="btn sm" data-bulk="group">Move to group…</button>
    <button class="btn sm" data-bulk="clear" style="margin-left:auto">Clear</button>
  </div>`
      : ""
  }

  <div class="row" style="gap:9px;margin-bottom:11px;flex-wrap:wrap">
    <div class="row" style="gap:6px;flex:1 1 230px;min-width:0">
      <span class="muted" style="margin-left:2px">${IC.search}</span>
      <input class="inp" id="j-q" placeholder="Search ${all.length} automations…"
        value="${esc(S.q)}" style="flex:1 1 auto;height:28px">
    </div>
    <div class="seg">
      <button aria-pressed="${S.groupBy === "group"}" data-groupby="group">By group</button>
      <button aria-pressed="${S.groupBy === "flat"}" data-groupby="flat">Flat</button>
    </div>
  </div>

  <div class="chips" style="margin-bottom:12px">
    ${STATUSES.map((k) => {
      const c = all.filter((j) => statusOf(j) === k).length;
      return c
        ? `<button class="chip" aria-pressed="${S.statusF.has(k)}" data-status="${k}">
        <span class="dot ${k}"></span>${STATUS_META[k].t}<span class="n">${c}</span></button>`
        : "";
    }).join("")}
    ${
      S.statusF.size || S.q || S.groupFilter || S.savedView !== "all"
        ? `<button class="chip" data-clearf>Clear filters</button>`
        : ""
    }
  </div>

  ${
    list.length
      ? `
  <div class="tbl-wrap" style="padding:0">
    <table class="jt">
      <thead><tr>
        <th style="width:28px"><input type="checkbox" data-pickall
          ${S.picked.size && S.picked.size === list.length ? "checked" : ""}></th>
        <th style="width:18px"></th><th>Automation</th><th>Group</th><th>Schedule</th>
        <th>Next</th><th>Duration</th><th>Last summary</th><th>Status</th><th style="width:112px"></th>
      </tr></thead>
      <tbody>
        ${buckets
          .map(
            ([label, js]) => `
          ${label ? `<tr class="gh-row"><td colspan="10">${esc(label)} · ${js.length}</td></tr>` : ""}
          ${js.map(row).join("")}`,
          )
          .join("")}
      </tbody>
    </table>
  </div>
  <div class="muted" style="font-size:11px;margin-top:9px">
    Showing ${list.length} of ${all.length}${S.groupFilter ? ` · group: ${esc(S.groupFilter)}` : ""}${
      S.savedView !== "all" ? ` · view: ${esc(SAVED_VIEWS[S.savedView].label)}` : ""
    }
  </div>`
      : `<div class="card empty">
      <span class="muted">${IC.search}</span>
      <div class="t">No automations match</div>
      <div class="muted" style="font-size:12px">Try clearing the search or status filters.</div>
      <button class="btn sm" data-clearf>Clear filters</button>
    </div>`
  }`;
}
