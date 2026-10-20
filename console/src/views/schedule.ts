/**
 * Schedule — load, not lanes.
 *
 * The question this view answers is "where does the work actually land?", so it
 * draws concurrency across the day, names the contended windows and offers the
 * stagger assistant. The profile comes from `load_profile`; if that command
 * fails we fall back to the identical local computation (see store.loadProfile)
 * and say so.
 */
import { IC } from "../icons";
import { esc, fmtDur, groupsOf, hhmm, scheduleStarts } from "../format";
import { lastDuration } from "../types";
import { S, jobs } from "../store";
import { asyncPanel, statCard } from "./common";

export function vSchedule(): string {
  const all = jobs();
  const enabled = all.filter((j) => j.enabled);
  const busiest = groupsOf(all)
    .map((g) => {
      const list = g.jobs.filter((j) => j.enabled);
      return { name: g.name, secs: list.reduce((a, j) => a + lastDuration(j), 0), n: list.length };
    })
    .sort((a, b) => b.secs - a.secs);
  const longest = all.reduce((a, j) => Math.max(a, lastDuration(j)), 0);
  const longestJob = all.find((j) => lastDuration(j) === longest);

  const body = (mins: number[]): string => {
    const B = 48;
    const bw = Math.max(1, Math.floor(mins.length / B)); // 30-minute buckets
    const buckets = Array.from({ length: B }, (_, b) => Math.max(0, ...mins.slice(b * bw, (b + 1) * bw)));
    const peak = Math.max(...buckets, 1);
    const peakAll = Math.max(...mins, 0);
    const peakMin = mins.indexOf(peakAll);
    const hot = buckets.map((v, b) => [b, v] as [number, number]).filter(([, v]) => v >= 3);

    return `
  <div class="grid g4">
    ${statCard("Peak concurrency", peakAll, `at ${hhmm(Math.max(0, peakMin))}`)}
    ${statCard("Busiest window", hhmm(Math.floor(Math.max(0, peakMin) / 30) * 30), `${hot.length} windows ≥3 jobs`)}
    ${statCard("Scheduled starts", enabled.reduce((a, j) => a + scheduleStarts(j.schedule).length, 0), "per day")}
    ${statCard("Longest job", fmtDur(longest), longestJob ? esc(longestJob.title) : "—")}
  </div>

  <div class="sec-h"><h3>Load across the day</h3><span class="ln"></span>
    <span class="muted" style="font-size:11px">concurrent automations · 30-min buckets</span></div>
  <div class="card pad">
    <div class="histo">
      ${buckets
        .map(
          (v, b) => `<i class="${v >= 5 ? "crit" : v >= 3 ? "hot" : ""}"
        style="height:${Math.max(2, (v / peak) * 100)}%"
        title="${hhmm(b * bw)}–${hhmm((b + 1) * bw - 1)} — ${v} concurrent"></i>`,
        )
        .join("")}
    </div>
    <div class="histo-ax">${Array.from(
      { length: 8 },
      (_, i) => `<span>${String(i * 3).padStart(2, "0")}:00</span>`,
    ).join("")}</div>
    <div class="row" style="gap:14px;margin-top:11px;font-size:11px">
      <span class="row muted" style="gap:6px"><i style="width:9px;height:9px;border-radius:2px;background:var(--accent);opacity:.75"></i>1–2 concurrent</span>
      <span class="row muted" style="gap:6px"><i style="width:9px;height:9px;border-radius:2px;background:var(--spoke)"></i>3–4</span>
      <span class="row muted" style="gap:6px"><i style="width:9px;height:9px;border-radius:2px;background:var(--fail)"></i>5+</span>
    </div>
  </div>

  ${
    hot.length
      ? `
  <div class="sec-h"><h3>Contended windows</h3><span class="ln"></span>
    <button class="btn sm pri" data-bulk="stagger">${IC.stagger} Auto-stagger all</button></div>
  <div class="card" style="overflow:hidden">
    ${hot
      .slice(0, 6)
      .map(([b, v], i) => {
        const from = b * bw;
        const to = (b + 1) * bw;
        const overlap = enabled.filter((j) => scheduleStarts(j.schedule).some((s) => s >= from - 60 && s < to));
        return `<div class="row" style="padding:10px 13px;${i ? "border-top:.5px solid var(--line-soft)" : ""}">
        <span class="badge ${v >= 5 ? "fail" : "spoke"}">${v} at once</span>
        <span class="mono" style="font-size:12px">${hhmm(from)}–${hhmm(to - 1)}</span>
        <span class="muted trunc" style="font-size:11.5px;min-width:0">${overlap
          .slice(0, 4)
          .map((j) => esc(j.title))
          .join(", ")}${overlap.length > 4 ? ` +${overlap.length - 4}` : ""}</span>
        <button class="btn sm ghost" style="margin-left:auto"
          data-restage="${overlap.map((j) => esc(j.file)).join(",")}">Restage</button>
      </div>`;
      })
      .join("")}
  </div>
  <div class="note" style="margin-top:11px">
    <span style="color:var(--accent);flex:0 0 auto">${IC.warn}</span>
    <div><b>Why this matters at ${all.length} automations.</b> Each job holds its own timeout window, and
    <span class="mono">bin/run</span> walks the tier sequentially — so a crowded 02:00 turns a
    2-minute sweep into an hour, and one hung job blocks every job behind it. Auto-stagger spreads
    the selected automations across a window you choose, longest first.</div>
  </div>`
      : ""
  }

  <div class="sec-h"><h3>Runtime by group</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:11px">
    ${
      busiest.length
        ? busiest
            .map(
              (g) => `
      <div class="row" style="gap:11px">
        <span style="width:146px;flex:0 0 146px;font-size:12px" class="trunc">${esc(g.name)}</span>
        <div style="flex:1 1 auto;min-width:0"><div class="sbar" style="height:9px">
          <i class="quiet" style="width:${busiest[0].secs ? (g.secs / busiest[0].secs) * 100 : 0}%"></i></div></div>
        <span class="mono" style="font-size:11.5px;min-width:72px;text-align:right">${fmtDur(g.secs)}</span>
        <span class="muted" style="font-size:11px;min-width:30px;text-align:right">${g.n}</span>
      </div>`,
            )
            .join("")
        : `<span class="muted">Nothing enabled.</span>`
    }
  </div>`;
  };

  return asyncPanel(S.profile, "Load profile unavailable", "profile", body);
}
