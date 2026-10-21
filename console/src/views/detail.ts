/**
 * Automation detail — configuration, history and log for one automation.
 * Every control here writes through a real command (set_enabled, set_schedule,
 * set_timeout, set_meta) and re-reads the fleet afterwards.
 */
import { IC } from "../icons";
import { esc, fmtDur, groupsOf, nextRun, scheduleDays, scheduleTimes, untilStr, DAYS, fmtDay } from "../format";
import { STATUS_META, lastDuration, statusOf } from "../types";
import type { Run } from "../types";
import { S, getDraft, jobByFile, jobs } from "../store";
import { asyncPanel, fatal, inlineErr } from "./common";

/**
 * Privilege facts for the mole-backed automations, verified against mole 1.54.0.
 * Static knowledge, not backend state — the console has no command for it.
 */
const PRIV: Record<string, { state: string; gate: string; ok: boolean; deep?: boolean; needs: string[]; note: string }> = {
  "mole-optimize": {
    state: "full",
    gate: "sudo -n true",
    ok: true,
    needs: ["/usr/bin/sfltool dumpbtm", "/usr/bin/true"],
    note: "Every privileged command optimize invokes is pinned in /etc/sudoers.d/mole, so all subtasks run unattended.",
  },
  "mole-clean": {
    state: "user",
    gate: "sudo -n -v",
    ok: false,
    deep: true,
    needs: ["/bin/rm -rf <any path>", "/usr/bin/find <any args>"],
    note: "The system pass needs root rm/find with unrestricted arguments — sudo find … -exec is arbitrary root execution, so granting it equals NOPASSWD: ALL. It stays user-level at 02:00 by design; run Deep clean to include it.",
  },
};

export function vDetail(): string {
  if (!S.jobs.data) return fatal("Could not read the automations", S.jobs.error ?? "loading…", "jobs");
  const j = jobByFile(S.selected);
  if (!j) {
    return `<div class="card empty">
      <div class="t">That automation is gone</div>
      <div class="muted" style="font-size:12px">It is no longer in <span class="mono">jobs/</span>.</div>
      <button class="btn" data-nav="jobs">Back to all automations</button></div>`;
  }

  const st = statusOf(j);
  const s = j.schedule;
  const p = PRIV[j.label];
  const now = new Date();
  const times = scheduleTimes(s);
  const wdays = scheduleDays(s);
  const groups = groupsOf(jobs()).map((g) => g.name);

  const historyPanel = (runs: Run[]): string => {
    const cells = [...runs].reverse(); // oldest -> newest, like the fleet strip
    return cells.length
      ? `<div class="fleet-cells">${cells
          .map(
            (r) =>
              `<i class="${r.status}" title="${esc(fmtDay(r.started_at))} — ${STATUS_META[r.status].t} · ${fmtDur(
                r.duration_s,
              )}"></i>`,
          )
          .join("")}</div>
        <div class="muted" style="font-size:11px">${cells.length} recorded run${cells.length === 1 ? "" : "s"}</div>`
      : `<div class="muted" style="font-size:11.5px">No runs recorded yet.</div>`;
  };

  const logPanel = (runs: Run[]): string =>
    runs.length
      ? runs
          .slice(0, 12)
          .map(
            (r) => `<div class="lg ${r.status === "quiet" ? "" : r.status === "spoke" ? "alert" : "err"}">
            <span class="t">${esc(fmtDay(r.started_at))}</span>
            <span class="m">${esc(r.summary || "(no output on stdout — silent, which is success)")}</span></div>`,
          )
          .join("")
      : '<div class="lg"><span class="m muted">No entries in the current window.</span></div>';

  return `
  <div class="row" style="margin-bottom:14px">
    <button class="ibtn" data-nav="jobs" title="Back">${IC.back}</button>
    <span style="color:var(--ink-2)">${IC[j.kind] ?? IC.sh}</span>
    <div class="col" style="gap:0;min-width:0">
      <div style="font-size:16px;font-weight:600;letter-spacing:-.018em" class="trunc">${esc(j.title)}</div>
      <div class="muted mono" style="font-size:11px">jobs/${esc(j.file)} · ${esc(j.group)}</div>
    </div>
    <span class="badge ${st}">${STATUS_META[st].t}</span>
    <span class="badge tier">${esc(j.tier)}</span>
    <div class="row" style="gap:6px;margin-left:auto">
      <button class="btn" data-run="${esc(j.file)}">${IC.play} Run now</button>
      <button class="btn" data-dry="${esc(j.file)}">${IC.flask} Dry run</button>
      <button class="btn" data-validate="${esc(j.file)}">${IC.shield} Validate</button>
      ${p?.deep ? `<button class="btn" data-deep="${esc(j.file)}">${IC.shield} Deep clean</button>` : ""}
      <button class="btn danger" data-del="${esc(j.file)}">${IC.trash}</button>
    </div>
  </div>

  ${
    S.diagnostics.error
      ? inlineErr("Validation failed to run", S.diagnostics.error)
      : S.diagnostics.data
        ? S.diagnostics.data.length
          ? `<div class="inline-err" style="margin-bottom:12px"><span class="ic">${IC.warn}</span>
              <div><b>${S.diagnostics.data.length} problem(s) in this script</b><pre class="src"
                style="margin-top:7px">${esc(S.diagnostics.data.join("\n"))}</pre></div></div>`
          : `<div class="note" style="margin-bottom:12px"><span style="color:var(--quiet);flex:0 0 auto">${IC.check}</span>
              <div><b>Script validates clean.</b> Header, tier and timeout lines are all well-formed.</div></div>`
        : ""
  }

  <div class="grid g2" style="align-items:start">
    <div class="col" style="gap:11px">
      <div class="card pad col" style="gap:11px">
        <div class="row"><span style="font-weight:600;font-size:12.5px">Enabled</span>
          <input class="sw" type="checkbox" ${j.enabled ? "checked" : ""} data-toggle="${esc(j.file)}"
            style="margin-left:auto"></div>
        <div class="muted" style="font-size:11.5px">Writes <span class="mono">state/console.json</span> and
          boots its LaunchAgent in or out. The script stays on disk.</div>
      </div>

      <div class="card pad col" style="gap:11px">
        <div style="font-weight:600;font-size:12.5px">When it runs</div>
        <div class="seg" style="align-self:flex-start">
          ${(["daily", "weekly", "interval", "manual"] as const)
            .map(
              (m) =>
                `<button aria-pressed="${s.mode === m}" data-mode="${m}">${m[0].toUpperCase() + m.slice(1)}</button>`,
            )
            .join("")}
        </div>
        ${
          s.mode === "weekly"
            ? `<div class="row" style="gap:4px;flex-wrap:wrap">
          ${DAYS.map(
            (d) =>
              `<button class="btn sm" data-wday="${d}" style="${
                wdays.includes(d) ? "background:var(--accent);color:#fff;border-color:transparent" : ""
              }">${d}</button>`,
          ).join("")}
        </div>`
            : ""
        }
        ${
          s.mode === "interval"
            ? `<div class="row"><input class="inp" type="number" min="1" max="24" value="${getDraft("d-every") ?? s.every_hours}"
                id="d-every" style="width:70px">
          <span class="muted">hours between runs</span>
          <button class="btn sm" data-setevery>Apply</button></div>`
            : ""
        }
        ${
          s.mode === "daily" || s.mode === "weekly"
            ? `
          <div class="row" style="flex-wrap:wrap;gap:6px">
            ${times
              .map(
                (t) =>
                  `<span class="pill mono">${esc(t)}<span class="x" data-deltime="${esc(t)}" title="Remove">×</span></span>`,
              )
              .join("")}
            <input class="inp mono" id="d-newtime" placeholder="HH:MM" style="width:74px"
              value="${esc(getDraft("d-newtime") ?? "")}">
            <button class="btn sm ghost" data-addtime>${IC.plus} Add time</button>
          </div>`
            : ""
        }
        ${
          s.mode === "manual"
            ? `<div class="muted" style="font-size:11.5px">Manual only — no LaunchAgent calendar interval is
                installed. Run it from here or from <span class="mono">bin/run-job</span>.</div>`
            : ""
        }
        <div class="row" style="border-top:.5px solid var(--line-soft);padding-top:10px">
          <span class="lbl">Next run</span>
          <span style="margin-left:auto;font-weight:600;font-size:12.5px">${
            j.enabled ? untilStr(nextRun(j, now), now) : '<span class="muted">disabled</span>'
          }</span>
        </div>
      </div>

      <div class="card pad col" style="gap:11px">
        <div style="font-weight:600;font-size:12.5px">Guard rails</div>
        <div class="field" style="margin:0"><label>Timeout</label>
          <div class="row"><input class="inp" type="number" min="1" value="${getDraft("d-timeout") ?? j.timeout}" id="d-timeout"
              style="width:92px">
            <span class="muted">seconds</span>
            <button class="btn sm" data-settimeout>Apply</button></div>
          <div class="hint">Written as <span class="mono"># timeout: ${j.timeout}</span> on line 3.
            On expiry the process group is TERM'd, graced 5s, then KILL'd.</div></div>
        <div class="field" style="margin:0"><label>Title</label>
          <div class="row"><input class="inp" id="d-title" value="${esc(getDraft("d-title") ?? j.title)}" style="flex:1 1 auto;max-width:260px">
            <button class="btn sm" data-settitle>Rename</button></div>
          <div class="hint">Display name only, stored in <span class="mono">state/console.json</span>. The
            file on disk keeps its name.</div></div>
        <div class="field" style="margin:0"><label>Group</label>
          <select class="inp" id="d-group" style="height:28px;align-self:flex-start">
            ${groups
              .map((g) => `<option ${g === j.group ? "selected" : ""}>${esc(g)}</option>`)
              .join("")}
            ${groups.includes(j.group) ? "" : `<option selected>${esc(j.group)}</option>`}
          </select>
          <div class="hint">Groups drive the sidebar, the rollups and bulk actions — the only thing that
            keeps ${jobs().length} automations navigable.</div></div>
      </div>

      ${
        p
          ? `
      <div class="card pad col" style="gap:10px">
        <div class="row"><span style="font-weight:600;font-size:12.5px">Privileges</span>
          <span class="badge ${p.ok ? "quiet" : "off"}" style="margin-left:auto">${
            p.ok ? "Fully privileged" : "User level only"
          }</span></div>
        <dl class="kv">
          <dt>Gate</dt><dd class="mono">${esc(p.gate)} → <span style="color:var(--${
            p.ok ? "quiet" : "spoke"
          })">${p.ok ? "passes" : "fails"}</span></dd>
          <dt>${p.ok ? "Uses" : "Would need"}</dt><dd class="mono" style="font-size:11.5px">${p.needs
            .map(esc)
            .join("<br>")}</dd>
        </dl>
        <div class="muted" style="font-size:11.5px;border-top:.5px solid var(--line-soft);padding-top:10px">${esc(
          p.note,
        )}</div>
        ${
          p.deep
            ? `<button class="btn" data-deep="${esc(j.file)}" style="align-self:flex-start">${IC.shield} Deep clean (asks for Touch ID)</button>`
            : ""
        }
      </div>`
          : ""
      }
    </div>

    <div class="col" style="gap:11px">
      <div class="card pad col" style="gap:10px">
        <div class="row"><span style="font-weight:600;font-size:12.5px">Last run</span>
          <span class="muted" style="margin-left:auto;font-size:11.5px">${
            j.last ? esc(fmtDay(j.last.started_at)) : "never"
          }</span></div>
        <dl class="kv">
          <dt>Outcome</dt><dd><span class="badge ${st}">${STATUS_META[st].t}</span>
            <span class="muted" style="font-size:11px">${esc(STATUS_META[st].d)}</span></dd>
          <dt>Exit code</dt><dd class="mono">${j.last ? j.last.rc : "—"}</dd>
          <dt>Duration</dt><dd>${lastDuration(j) ? fmtDur(lastDuration(j)) : "—"}
            <span class="muted">of ${j.timeout}s budget</span></dd>
          <dt>Summary</dt><dd class="mono" style="font-size:11.5px">${esc(
            j.last?.summary || (j.last ? "(no output on stdout — silent, which is success)" : "never run"),
          )}</dd>
        </dl>
        ${
          j.last && j.last.stdout.trim()
            ? `<pre class="src">${esc(j.last.stdout.trim())}</pre>`
            : ""
        }
      </div>
      <div class="card pad col" style="gap:9px">
        <div style="font-weight:600;font-size:12.5px">Recent runs</div>
        ${asyncPanel(S.detailRuns, "Run history unavailable", "detail", historyPanel)}
      </div>
      <div class="card pad col" style="gap:9px">
        <div style="font-weight:600;font-size:12.5px">Log — ${esc(j.label)}</div>
        <div class="loglist" style="max-height:230px;overflow:auto;margin:0 -11px">
          ${S.detailRuns.data ? logPanel(S.detailRuns.data) : '<div class="lg"><span class="m muted">Loading…</span></div>'}
        </div>
      </div>
    </div>
  </div>`;
}
