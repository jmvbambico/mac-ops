/**
 * Logs — the recorded runs, newest first.
 *
 * The backend's log surface is `search_logs(q, limit)`, which returns whole
 * `Run` rows rather than raw log lines, so a run renders as its one-line
 * summary plus, when it spoke, the stdout it emitted. Severity follows the
 * outcome: a quiet run is an unremarkable line, `spoke` is an alert, `fail`
 * and `hang` are errors. The text query is sent to the backend; the source
 * filter and the sweep grouping are applied here.
 */
import { IC } from "../icons";
import { esc, fmtDay, fmtTime, groupsOf, runSeverity } from "../format";
import { STATUS_META } from "../types";
import type { Run } from "../types";
import { S, attnJobs, jobByFile, jobs } from "../store";
import { asyncPanel } from "./common";

function line(sev: string, time: string, src: string, msg: string): string {
  return `<div class="lg ${sev}"><span class="t">${esc(time)}</span>
    <span class="s">${esc(src)}</span><span class="m">${esc(msg)}</span></div>`;
}

function runLines(r: Run): string {
  const j = jobByFile(r.job);
  const src = j?.label ?? r.job;
  const sev = runSeverity(r);
  const head = line(
    sev,
    fmtTime(r.started_at),
    src,
    `${r.summary || "(no output on stdout — silent, which is success)"}` +
      ` rc=${r.rc} dry_run=${r.dry_run ? 1 : 0}`,
  );
  const body = r.stdout
    .split("\n")
    .filter((l) => l.trim() !== "")
    .slice(0, 12)
    .map((l) => line("alert", fmtTime(r.started_at), `${src} stdout`, l))
    .join("");
  return head + body;
}

export function vLogs(): string {
  const all = jobs();
  const groups = groupsOf(all);

  const body = (runs: Run[]): string => {
    let rows = runs;
    if (S.logFilter === "alerts") rows = rows.filter((r) => r.status === "spoke" || r.status === "fail" || r.status === "hang");
    else if (S.logFilter !== "all") rows = rows.filter((r) => r.job === S.logFilter);
    rows = [...rows].sort((a, b) => b.started_at - a.started_at);

    const speaking = attnJobs().length;
    const enabled = all.filter((j) => j.enabled).length;

    let out = "";
    if (S.logGroup) {
      let day = "";
      for (const r of rows) {
        const d = fmtDay(r.started_at);
        if (d !== day) {
          day = d;
          const ofDay = rows.filter((x) => fmtDay(x.started_at) === d);
          const spoke = ofDay.filter((x) => x.status !== "quiet" && x.status !== "off").length;
          out += `<div class="runsep"><span class="dot ${spoke ? "spoke" : "quiet"}"></span>Sweep ${esc(d)}
            <span class="ln"></span>
            <span style="text-transform:none;letter-spacing:0">${spoke} of ${ofDay.length} runs spoke</span></div>`;
        }
        out += runLines(r);
      }
    } else {
      out = rows.map(runLines).join("");
    }

    return `
  <div class="card" style="overflow:hidden;padding:0">
    <div class="logbar">
      <input class="inp" id="log-q" placeholder="Search runs…" value="${esc(S.logQuery)}" style="width:180px">
      <select class="inp" id="log-src">
        <option value="all"${S.logFilter === "all" ? " selected" : ""}>All ${all.length} sources</option>
        <option value="alerts"${S.logFilter === "alerts" ? " selected" : ""}>Alerts only</option>
        ${groups
          .map(
            (g) => `<optgroup label="${esc(g.name)}">${g.jobs
              .map(
                (j) =>
                  `<option value="${esc(j.file)}"${S.logFilter === j.file ? " selected" : ""}>${esc(j.title)}</option>`,
              )
              .join("")}</optgroup>`,
          )
          .join("")}
      </select>
      <div class="seg">
        <button aria-pressed="${S.logGroup}" id="log-group">By sweep</button>
        <button aria-pressed="${!S.logGroup}" id="log-flat">Flat</button>
      </div>
      <span class="muted" style="font-size:11px;margin-left:auto">${rows.length} runs · ${speaking} of ${enabled}
        enabled automations are speaking</span>
      <button class="btn sm ghost" data-retry="logs">${IC.refresh} Reload</button>
    </div>
    <div class="loglist">
      ${out || '<div class="lg"><span class="m muted">No matching runs.</span></div>'}
    </div>
  </div>
  <div class="note" style="margin-top:12px">
    <span style="color:var(--spoke);flex:0 0 auto">${IC.warn}</span>
    <div><b>One 5 MB log will not survive ${all.length} automations.</b> At this size each automation writes its own
    <span class="mono">logs/job-&lt;slug&gt;.out.log</span> and the shared
    <span class="mono">machine-ops.log</span> keeps only the one-line-per-run summaries — otherwise a single
    chatty job rotates away everyone else's history. MacOps reads those summaries out of
    <span class="mono">state/console.db</span>; <b>Reindex</b> in Settings re-ingests the log.</div>
  </div>
  <div class="row muted" style="font-size:11px;gap:11px;margin-top:9px">
    ${(["quiet", "spoke", "fail", "hang"] as const)
      .map((k) => `<span class="row" style="gap:5px"><span class="dot ${k}"></span>${STATUS_META[k].t}</span>`)
      .join("")}
  </div>`;
  };

  return asyncPanel(S.logs, "Could not read the run log", "logs", body);
}
