/**
 * Settings — scale, privileges, safety rails, plus the two maintenance
 * commands the backend exposes (prune_runs, reindex).
 */
import { IC } from "../icons";
import { esc, groupsOf, localLoadProfile } from "../format";
import { S, getDraft, jobs } from "../store";
import { asyncPanel, inlineErr } from "./common";

export function vSettings(): string {
  const all = jobs();
  const groups = groupsOf(all);
  const profile = S.profile.data ?? localLoadProfile(all);
  const peak = Math.max(0, ...profile);

  return `
  <div class="sec-h"><h3>Scale</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:11px">
    <dl class="kv">
      <dt>Automations</dt><dd>${all.length} across ${groups.length} group${groups.length === 1 ? "" : "s"}</dd>
      <dt>LaunchAgents</dt><dd>${all.filter((j) => j.enabled).length} loaded · one per enabled automation</dd>
      <dt>Log strategy</dt><dd>per-job <span class="mono">logs/job-&lt;slug&gt;.out.log</span> +
        shared summary log</dd>
      <dt>Peak concurrency</dt><dd>${peak} at once${
        S.profile.data ? "" : ' <span class="muted">(computed locally)</span>'
      }</dd>
    </dl>
    <div class="note">
      <span style="color:var(--accent);flex:0 0 auto">${IC.check}</span>
      <div><b>Groups are the scaling primitive.</b> They drive the sidebar, the rollup cards, the fleet
      heatmap and every bulk action. Without them MacOps degrades into a flat list the moment you pass
      roughly a dozen automations.</div>
    </div>
  </div>

  <div class="sec-h"><h3>Paths</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:11px">
    ${asyncPanel(S.paths, "Resolved paths unavailable", "paths", (p) => {
      // Only when the config file disagrees with the effective root (i.e. an
      // env override is winning). When they match, one path is enough.
      const saved = S.pathsEdit.saved && S.pathsEdit.saved !== p.automations_root ? S.pathsEdit.saved : null;
      return `
    <dl class="kv">
      <dt>Tool root</dt><dd class="mono">${esc(p.tool_root)}</dd>
      <dt>Automations root</dt><dd class="mono">${esc(p.automations_root)}${
        p.source ? `<span class="badge tier" style="margin-left:5px">${esc(p.source)}</span>` : ""
      }</dd>
      ${saved ? `<dt>Saved in config.json</dt><dd class="mono">${esc(saved)}</dd>` : ""}
      <dt>Config file</dt><dd class="mono">${esc(p.config_path)}</dd>
    </dl>
    ${
      p.source === "env"
        ? `<div class="note" style="background:var(--spoke-bg)">
            <span style="color:var(--spoke);flex:0 0 auto">${IC.warn}</span>
            <div><b><span class="mono">MOPS_AUTOMATIONS_ROOT</span> is set and overrides the config file.</b>
              Saving here records your choice in
              <span class="mono">config.json</span>, but the effective root stays
              <span class="mono">${esc(p.automations_root)}</span> until you unset that variable
              (and restart).</div>
          </div>`
        : ""
    }
    <div class="field" style="margin:0">
      <label for="root-input">Set the automations root</label>
      <div class="row" style="gap:7px;flex-wrap:wrap">
        <input class="inp mono" id="root-input" value="${esc(S.pathsEdit.draft ?? p.automations_root)}"
          spellcheck="false" autocomplete="off" style="flex:1 1 260px;min-width:190px">
        <button class="btn pri" data-set-root ${S.pathsEdit.busy ? "disabled" : ""}>
          ${S.pathsEdit.busy ? `${IC.refresh} Saving…` : `${IC.check} Save root`}</button>
      </div>
      <div class="hint">Absolute path. A directory that does not exist yet is created with
        <span class="mono">jobs/</span>, <span class="mono">logs/</span> and
        <span class="mono">state/</span>.</div>
      ${S.pathsEdit.error ? inlineErr("Could not save the automations root", S.pathsEdit.error) : ""}
    </div>
    <div class="note">
      <span style="color:var(--accent);flex:0 0 auto">${IC.check}</span>
      <div><b>A saved root takes effect when the console restarts.</b> The run database and cached
        jobs are tied to the root resolved at launch, so nothing is half-switched now.</div>
    </div>
    <div class="muted" style="font-size:11.5px">The tool root comes from
      <span class="mono">MOPS_TOOL_ROOT</span> or the executable's own location, and is not editable
      from the GUI. The automations root comes from <span class="mono">MOPS_AUTOMATIONS_ROOT</span>,
      then the <span class="mono">automations_root</span> key in
      <span class="mono">~/.config/mops/config.json</span>, then <span class="mono">~/.mops</span>.
      A checkout used in place (jobs/ beside bin/) is a supported automations root.</div>`;
    })}
  </div>

  <div class="sec-h"><h3>Run database</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:11px">
    <div class="muted" style="font-size:11.5px">Outcomes live in <span class="mono">state/console.db</span>.
      Reindex re-ingests <span class="mono">logs/machine-ops.log</span>; pruning drops run rows older than
      the window you keep.</div>
    <div class="row" style="gap:9px;flex-wrap:wrap">
      <button class="btn" data-reindex>${IC.refresh} Reindex from log</button>
      <span class="row" style="gap:6px">
        <input class="inp" type="number" id="prune-days" value="${getDraft("prune-days") ?? 90}" min="1" style="width:76px">
        <span class="muted" style="font-size:11.5px">days kept</span>
        <button class="btn" data-prune>${IC.trash} Prune runs</button>
      </span>
    </div>
  </div>

  <div class="sec-h"><h3>Privileges</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:11px">
    <div class="row"><span style="font-weight:600;font-size:12.5px">/etc/sudoers.d/mole</span>
      <span class="badge quiet" style="margin-left:auto">installed · mode 440</span></div>
    <dl class="kv">
      <dt>mole optimize</dt><dd><span class="badge quiet">Fully privileged</span></dd>
      <dt>mole clean</dt><dd><span class="badge off">User level only</span></dd>
    </dl>
    <div class="note" style="background:var(--spoke-bg)">
      <span style="color:var(--spoke);flex:0 0 auto">${IC.warn}</span>
      <div><b>Do not add <span class="mono">verifypw = any</span>.</b> It would let mole enter the system
      pass and then fail every deletion — <span class="mono">rm</span> and <span class="mono">find</span>
      are not granted. Use <b>Deep clean</b> instead.</div>
    </div>
  </div>

  <div class="sec-h"><h3>Safety</h3><span class="ln"></span></div>
  <div class="card pad col" style="gap:10px">
    <div class="note" style="background:var(--fail-bg)">
      <span style="color:var(--fail);flex:0 0 auto">${IC.warn}</span>
      <div><b>Never root, never sudo.</b> Every automation runs as you, under a user LaunchAgent. MacOps
      refuses to install a LaunchDaemon or run a job with elevated privileges.</div>
    </div>
    <dl class="kv">
      <dt>Default timeout</dt><dd class="mono">900s</dd>
      <dt>Kill grace</dt><dd class="mono">5s between TERM and KILL</dd>
      <dt>stdin</dt><dd class="mono">/dev/null on every job</dd>
    </dl>
  </div>

  <div class="sec-h"><h3>Appearance</h3><span class="ln"></span></div>
  <div class="card pad row">
    <span style="font-weight:600;font-size:12.5px">Theme</span>
    <div class="seg" style="margin-left:auto">
      ${(["light", "dark", "system"] as const)
        .map((t) => {
          const cur = document.documentElement.dataset.theme ?? "system";
          return `<button aria-pressed="${cur === t}" data-theme-set="${t}">${t[0].toUpperCase() + t.slice(1)}</button>`;
        })
        .join("")}
    </div>
  </div>`;
}
