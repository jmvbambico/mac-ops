/**
 * App shell controller.
 *
 * index.html owns the static chrome; this module is the only thing that
 * populates it. It routes between the six views, keeps the sidebar, toolbar and
 * status bar in sync with store.ts, and owns the command palette and the four
 * action dialogs. Views stay pure renderers — every user action is forwarded
 * to store.ts or api.ts here.
 */
import { backend, errText, setBackend } from "./api";
import { closeLayer, inputValue, numberValue, setLayer, $ } from "./dom";
import { fakeBackend } from "./fake";
import {
  DAYS,
  esc,
  fmtDur,
  groupsOf,
  hhmm,
  parseHHMM,
  staggerPlan,
} from "./format";
import { IC } from "./icons";
import {
  S,
  SAVED_VIEWS,
  addTime,
  applyTimeout,
  applyTitle,
  attnJobs,
  clearDetailDrafts,
  clearStaggerDrafts,
  deleteJob,
  jobByFile,
  jobs,
  loadDetailRuns,
  loadFleet,
  loadLogs,
  loadPaths,
  loadProfile,
  mirrorDraft,
  mutate,
  onChange,
  primeView,
  pruneRuns,
  refreshJobs,
  saveAutomationsRoot,
  setEnabled,
  setEvery,
  setMeta,
  setSchedule,
  toast,
  updateTimes,
  validate,
  visible,
} from "./store";
import type { SavedView, ViewName } from "./store";
import type { Job, RunOutcome, Schedule, ScheduleMode, Status } from "./types";
import { STATUS_META, statusOf } from "./types";
import { skeletonRows } from "./views/common";
import { vDetail } from "./views/detail";
import { vJobs } from "./views/jobs";
import { vLogs } from "./views/logs";
import { vOverview } from "./views/overview";
import { vSchedule } from "./views/schedule";
import { StaggerCtx, vStagger } from "./views/stagger";
import { vSettings } from "./views/settings";

/* ------------------------------- chrome ------------------------------- */

const NAV: [ViewName, string][] = [
  ["overview", "Overview"],
  ["jobs", "Automations"],
  ["schedule", "Schedule"],
  ["logs", "Logs"],
  ["settings", "Settings"],
];

const TITLES: Record<ViewName, [string, string]> = {
  overview: ["Overview", "exceptions first — quiet automations stay out of the way"],
  jobs: ["Automations", "search, filter, group and act in bulk"],
  schedule: ["Schedule", "where the load actually lands across the day"],
  logs: ["Logs", "per-job files plus the shared summary log"],
  settings: ["Settings", "scale, privileges and safety rails"],
  detail: ["Automation", "configuration, history and log"],
};

const THEME_KEY = "mops.theme";

function savedTheme(): "light" | "dark" | null {
  try {
    const t = localStorage.getItem(THEME_KEY);
    return t === "light" || t === "dark" ? t : null;
  } catch {
    return null;
  }
}

function effectiveTheme(): "light" | "dark" {
  const t = document.documentElement.dataset.theme;
  if (t === "light" || t === "dark") return t;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme: "light" | "dark" | "system"): void {
  if (theme === "system") {
    delete document.documentElement.dataset.theme;
    try {
      localStorage.removeItem(THEME_KEY);
    } catch {
      /* storage can be unavailable; the theme still applies */
    }
  } else {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch {
      /* storage can be unavailable; the theme still applies */
    }
  }
  render();
}

function renderChrome(): void {
  const all = jobs();
  const groups = groupsOf(all);
  const attn = attnJobs().length;

  const tbStatus = $("#tb-status");
  if (tbStatus) {
    tbStatus.innerHTML = attn
      ? `<span class="badge spoke">${IC.bell} ${attn} need review</span>`
      : `<span class="badge quiet">All quiet</span>`;
  }

  const themeBtn = $("#btn-theme");
  if (themeBtn) themeBtn.innerHTML = effectiveTheme() === "dark" ? IC.sun : IC.moon;
  const refreshBtn = $("#btn-refresh");
  if (refreshBtn) refreshBtn.innerHTML = IC.refresh;
  const brand = $("#brand-mark");
  if (brand) brand.innerHTML = IC.mop;
  const addIc = $("#add-ic");
  if (addIc) addIc.innerHTML = IC.plus;

  const nav = $("#nav");
  if (nav) {
    nav.innerHTML = NAV.map(
      ([k, label]) => `
      <button class="nav-item" aria-current="${S.view === k}" data-nav="${k}">
        <span class="ic">${IC[k]}</span>${label}
        ${k === "jobs" ? `<span class="cnt">${all.length}</span>` : ""}
      </button>`,
    ).join("");
  }

  const navViews = $("#nav-views");
  if (navViews) {
    navViews.innerHTML = (Object.keys(SAVED_VIEWS) as SavedView[])
      .map((k) => {
        const count = SAVED_VIEWS[k].pick(all).length;
        const current = S.view === "jobs" && S.savedView === k && !S.groupFilter;
        return `<button class="nav-item" aria-current="${current}" data-view="${k}"
          style="min-height:27px;font-size:12.5px">
          ${k === "attention" && count ? '<span class="dot spoke"></span>' : '<span style="width:8px"></span>'}
          ${esc(SAVED_VIEWS[k].label)}<span class="cnt">${count}</span>
        </button>`;
      })
      .join("");
  }

  const navGroups = $("#nav-groups");
  if (navGroups) {
    navGroups.innerHTML = groups
      .map((g) => {
        const open = !S.collapsed.has(g.name);
        const bad = g.jobs.filter((j) => j.enabled && ["spoke", "fail", "hang"].includes(statusOf(j))).length;
        return `<div class="grp">
          <button class="grp-h" aria-expanded="${open}" data-grp="${esc(g.name)}">
            <span class="chev">${IC.chev}</span>
            <span class="trunc">${esc(g.name)}</span>
            ${bad ? '<span class="dot spoke"></span>' : ""}
            <span class="cnt">${g.jobs.length}</span>
          </button>
          <div class="grp-body" ${open ? "" : "hidden"}>
            ${
              open
                ? g.jobs
                    .map(
                      (j) => `<button class="sb-job ${
                        S.selected === j.file && S.view === "detail" ? "sel" : ""
                      } ${j.enabled ? "" : "dis"}" data-job="${esc(j.file)}">
                        <span class="dot ${statusOf(j)}"></span>
                        <span class="nm trunc">${esc(j.title)}</span>
                      </button>`,
                    )
                    .join("")
                : ""
            }
          </div>
        </div>`;
      })
      .join("");
  }

  const statusbar = $("#statusbar");
  if (statusbar) {
    const enabled = all.filter((j) => j.enabled).length;
    const sweep = all.reduce((a, j) => a + (j.enabled ? (j.last?.duration_s ?? 0) : 0), 0);
    const right = S.busy
      ? `<span class="row" style="gap:5px;margin-left:auto"><span class="spin">${IC.refresh}</span>${esc(
          S.busy,
        )}</span>`
      : `<span class="mono" style="margin-left:auto">${esc(S.paths.data?.automations_root ?? "MacOps")}</span>`;
    statusbar.innerHTML = `
      <span>${enabled} of ${all.length} enabled</span><span>·</span>
      <span>${groups.length} group${groups.length === 1 ? "" : "s"}</span><span>·</span>
      <span>total runtime ${fmtDur(sweep)}/day</span>
      ${right}`;
  }
}

function renderToolbar(): void {
  const el = $("#toolbar");
  if (!el) return;
  const [title, sub] = TITLES[S.view];
  el.innerHTML = `<h2>${esc(title)}</h2><span class="sub">${esc(sub)}</span><div class="spacer"></div>
    ${S.view === "jobs" ? `<button class="btn pri" id="btn-add2">${IC.plus} New automation</button>` : ""}
    ${S.view === "schedule" ? `<button class="btn" data-bulk="stagger">${IC.stagger} Auto-stagger</button>` : ""}`;
}

function loadingCard(): string {
  return `<div class="card pad col" style="gap:9px">${skeletonRows(5)}</div>`;
}

function renderView(): void {
  const el = $("#view");
  if (!el) return;
  if (!S.jobs.data && !S.jobs.error) {
    el.innerHTML = loadingCard();
    return;
  }
  switch (S.view) {
    case "overview":
      el.innerHTML = vOverview();
      break;
    case "jobs":
      el.innerHTML = vJobs();
      break;
    case "schedule":
      el.innerHTML = vSchedule();
      break;
    case "logs":
      el.innerHTML = vLogs();
      break;
    case "settings":
      el.innerHTML = vSettings();
      break;
    case "detail":
      el.innerHTML = vDetail();
      break;
  }
}

function renderToasts(): void {
  const el = $("#toasts");
  if (!el) return;
  const t = S.toast;
  if (!t) {
    el.innerHTML = "";
    return;
  }
  const icon = t.kind === "err" ? IC.warn : t.kind === "warn" ? IC.warn : IC.check;
  el.innerHTML = `<div class="toast ${t.kind}"><span class="ic">${icon}</span><span class="m">${esc(t.msg)}</span></div>`;
}

function render(): void {
  const active = document.activeElement;
  const focusId = active instanceof HTMLElement && active.id ? active.id : null;
  const selStart = active instanceof HTMLInputElement ? active.selectionStart : null;
  const selEnd = active instanceof HTMLInputElement ? active.selectionEnd : null;

  renderChrome();
  renderToolbar();
  renderView();
  renderToasts();

  if (focusId) {
    const el = document.getElementById(focusId) as HTMLElement | null;
    if (el) {
      el.focus();
      if (el instanceof HTMLInputElement && selStart !== null) {
        try {
          el.setSelectionRange(selStart, selEnd ?? selStart);
        } catch {
          /* number inputs do not support selection ranges */
        }
      }
    }
  }
}

function go(view: ViewName, selected?: string | null): void {
  if (view === "detail" && !(selected ?? S.selected)) view = "jobs";
  const prev = S.selected;
  S.view = view;
  if (selected !== undefined) S.selected = selected;
  // Leaving an automation behind drops its half-typed drafts: they belong to
  // that automation's detail view, not to the next one.
  if (prev !== S.selected) clearDetailDrafts(prev);
  S.picked.clear();
  primeView();
  render();
  const scroller = $("#view");
  if (scroller) scroller.scrollTop = 0;
}

/* ---------------------------- command palette ---------------------------- */

let palIndex = 0;

function paletteHits(): Job[] {
  const q = S.palQ.trim().toLowerCase();
  return jobs()
    .filter((j) => `${j.title}${j.file}${j.group}${j.label}`.toLowerCase().includes(q))
    .slice(0, 40);
}

function openPalette(): void {
  S.palOpen = true;
  S.palQ = "";
  palIndex = 0;
  renderPalette();
}

function renderPalette(): void {
  if (!S.palOpen) return;
  const hits = paletteHits();
  const empty = S.jobs.loading
    ? `<div class="empty"><div class="t">Loading automations…</div></div>`
    : `<div class="empty"><div class="t">No match</div></div>`;

  setLayer(`
    <div class="scrim" style="align-items:flex-start" data-close>
      <div class="pal" role="dialog" aria-modal="true" aria-label="Jump to an automation">
        <input id="pal-q" placeholder="Jump to an automation…" value="${esc(S.palQ)}" autofocus>
        <div class="pal-list">
          ${
            hits.length
              ? hits
                  .map(
                    (j, i) => `<button class="pal-item ${i === palIndex ? "on" : ""}" data-job="${esc(j.file)}">
                      <span class="dot ${statusOf(j)}"></span>
                      <span class="trunc">${esc(j.title)}</span>
                      <span class="g">${esc(j.group)}</span>
                    </button>`,
                  )
                  .join("")
              : empty
          }
        </div>
      </div>
    </div>`);

  const inp = document.querySelector<HTMLInputElement>("#pal-q");
  if (inp) {
    inp.focus();
    inp.setSelectionRange(inp.value.length, inp.value.length);
  }
  document.querySelector(".pal-item.on")?.scrollIntoView({ block: "nearest" });
}

/* -------------------------------- modals -------------------------------- */

let runCtx: { file: string; dry: boolean; outcome: RunOutcome | null; busy: boolean } | null = null;

function closeModal(): void {
  S.palOpen = false;
  S.palQ = "";
  runCtx = null;
  staggerCtx = null;
  closeLayer();
}

function openRunModal(file: string, dry: boolean): void {
  const j = jobByFile(file);
  if (!j) {
    toast("That automation is gone", "warn");
    return;
  }
  S.palOpen = false;
  runCtx = { file, dry, outcome: null, busy: false };
  renderRunModal();
}

function outcomeText(o: RunOutcome): string {
  const body = [o.stdout.trim(), o.stderr.trim()].filter(Boolean).join("\n");
  return esc(body || "(no output on stdout — silent, which is success)");
}

function renderRunModal(): void {
  const ctx = runCtx;
  if (!ctx) return;
  const j = jobByFile(ctx.file);
  if (!j) {
    closeModal();
    render();
    return;
  }

  const st = statusOf(j);
  const quiet = st === "quiet";
  const o = ctx.outcome;
  const tone = ctx.dry ? "accent" : quiet ? "quiet" : "spoke";
  const preview = o
    ? outcomeText(o)
    : ctx.dry
      ? `[${esc(j.label)}] dry run — no changes made\n(no output on stdout — this run would be silent)`
      : quiet
        ? `[${esc(j.label)}] ${esc(j.last?.summary ?? "")}\n(no output on stdout — silent, which is success)`
        : `== ${esc(j.label)} ==\n[${esc(j.label)}] ${esc(j.last?.summary ?? "never run")}`;
  const badge = o
    ? `<span class="badge ${o.timed_out ? "hang" : o.status}">${o.timed_out ? "Timed out" : STATUS_META[o.status].t}</span>`
    : `<span class="badge ${ctx.dry ? "off" : quiet ? "quiet" : st}">${
        ctx.dry ? "preview" : STATUS_META[st].t
      }</span>`;

  setLayer(`
    <div class="scrim" data-close><div class="modal" style="width:min(620px,100%)">
      <div class="row" style="gap:9px">
        <span style="color:var(--${tone})">${ctx.dry ? IC.flask : quiet ? IC.check : IC.warn}</span>
        <div class="col" style="gap:1px"><h3 style="margin:0">${ctx.dry ? "Dry run" : "Run"} — ${esc(j.title)}</h3>
          <span class="muted mono" style="font-size:11.5px">bin/run-job jobs/${esc(j.file)}</span></div>
        ${badge}
      </div>
      <pre class="src" style="margin-top:13px">${preview}</pre>
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
            ? `<span class="row muted" style="font-size:11.5px;gap:6px"><span class="spin">${IC.refresh}</span>Running…</span>`
            : ""
        }
        <button class="btn" data-close>${o ? "Done" : "Cancel"}</button>
        ${
          o
            ? ""
            : `<button class="btn pri" data-run-confirm="1">${ctx.dry ? IC.flask : IC.play} ${
                ctx.dry ? "Dry run" : "Run now"
              }</button>`
        }
      </div>
    </div></div>`);
}

async function confirmRun(): Promise<void> {
  const ctx = runCtx;
  if (!ctx || ctx.busy) return;
  ctx.busy = true;
  S.busy = `${ctx.dry ? "Dry-running" : "Running"} ${ctx.file}`;
  renderChrome();
  renderRunModal();

  try {
    const o = await backend().runJob(ctx.file, ctx.dry);
    ctx.outcome = o;
    const spoke = o.status !== "quiet" && o.status !== "off";
    toast(
      `${ctx.dry ? "Dry run" : "Run"} ${ctx.file} — ${o.timed_out ? "timed out" : STATUS_META[o.status].t}`,
      spoke ? "warn" : "ok",
    );
    await refreshJobs();
  } catch (err) {
    toast(`Run failed — ${errText(err)}`, "err");
    closeModal();
  } finally {
    ctx.busy = false;
    S.busy = null;
    render();
    if (runCtx) renderRunModal();
  }
}

function openDeleteModal(file: string): void {
  const j = jobByFile(file);
  if (!j) return;
  S.palOpen = false;
  setLayer(`
    <div class="scrim" data-close><div class="modal" style="width:min(460px,100%)">
      <div class="row" style="gap:11px;align-items:flex-start">
        <span style="color:var(--fail);flex:0 0 auto">${IC.warn}</span>
        <div class="col" style="gap:5px"><h3 style="margin:0">Delete “${esc(j.title)}”?</h3>
          <p class="muted" style="margin:0;font-size:12.5px">Boots out
            <span class="mono">ai.machineops.${esc(j.label)}</span>, drops its entry from
            <span class="mono">state/console.json</span> and trashes
            <span class="mono">jobs/${esc(j.file)}</span>.</p></div>
      </div>
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        <button class="btn" data-close>Cancel</button>
        <button class="btn danger" data-confirmdel="${esc(j.file)}">Delete</button>
      </div>
    </div></div>`);
}

async function confirmDelete(file: string): Promise<void> {
  const ok = await deleteJob(file, false);
  if (!ok) return;
  closeModal();
  if (S.selected === file) {
    S.selected = null;
    go("jobs");
  } else {
    render();
  }
}

function openDeepModal(file: string): void {
  const j = jobByFile(file);
  if (!j) return;
  S.palOpen = false;
  setLayer(`
    <div class="scrim" data-close><div class="modal" style="width:min(600px,100%)">
      <div class="row" style="gap:10px;align-items:flex-start">
        <span style="color:var(--accent);flex:0 0 auto">${IC.shield}</span>
        <div class="col" style="gap:4px"><h3 style="margin:0">Deep clean — includes the system pass</h3>
          <p class="muted" style="margin:0;font-size:12.5px">Adds mole's system-level pass:
            <span class="mono">/Library/Caches</span>, <span class="mono">/Library/Logs/DiagnosticReports</span>,
            <span class="mono">/private/var/log</span>, <span class="mono">/private/var/db/diagnostics</span>.</p></div>
      </div>
      <div class="note" style="margin-top:14px"><span style="color:var(--accent);flex:0 0 auto">${IC.check}</span>
        <div><b>No password is handled here.</b> The console runs the automation through
        <span class="mono">bin/run-job</span> with stdin on <span class="mono">/dev/null</span>, exactly like a
        scheduled run. It cannot prompt for Touch ID or cache a sudo timestamp; parts of the system pass that
        need root are skipped by the job's own degrade path.</div></div>
      <div class="note" style="margin-top:9px;background:var(--fail-bg)">
        <span style="color:var(--fail);flex:0 0 auto">${IC.warn}</span>
        <div><b>Why it is a button, not a schedule.</b> Unattended, this would need NOPASSWD on
        <span class="mono">rm -rf</span> and <span class="mono">find</span> with unrestricted arguments.
        <span class="mono">sudo find … -exec</span> runs anything as root — equivalent to
        <span class="mono">NOPASSWD: ALL</span>.</div></div>
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        <button class="btn" data-close>Cancel</button>
        <button class="btn pri" data-confirmdeep="${esc(j.file)}">${IC.shield} Run deep clean</button>
      </div>
    </div></div>`);
}

function confirmDeep(file: string): void {
  closeModal();
  runCtx = { file, dry: false, outcome: null, busy: false };
  renderRunModal();
  void confirmRun();
}

let staggerCtx: StaggerCtx | null = null;

function openStaggerModal(): void {
  const list = S.picked.size ? jobs().filter((j) => S.picked.has(j.file)) : jobs().filter((j) => j.enabled);
  if (!list.length) {
    toast("Nothing to stagger — enable or select automations first", "warn");
    return;
  }
  S.palOpen = false;
  // A fresh modal starts from the defaults, not from a previous session's
  // half-typed window.
  clearStaggerDrafts();
  staggerCtx = { files: list.map((j) => j.file), start: 60, end: 300, max: 2, plan: null };
  renderStaggerModal();
}

function renderStaggerModal(): void {
  const ctx = staggerCtx;
  if (!ctx) return;
  setLayer(vStagger(ctx));
}

function readStaggerInputs(): boolean {
  const ctx = staggerCtx;
  if (!ctx) return false;
  const start = parseHHMM(inputValue("#s-start"));
  const end = parseHHMM(inputValue("#s-end"));
  if (start === null || end === null) {
    toast("Enter the window as HH:MM", "warn");
    return false;
  }
  if (end <= start) {
    toast("Window end must be after window start", "warn");
    return false;
  }
  ctx.start = start;
  ctx.end = end;
  ctx.max = Math.max(1, numberValue("#s-max", ctx.max));
  return true;
}

function planStagger(ctx: StaggerCtx): Map<string, number> {
  return staggerPlan(
    jobs().filter((j) => ctx.files.includes(j.file)),
    ctx.start,
    ctx.end,
    ctx.max,
  );
}

function previewStagger(): void {
  const ctx = staggerCtx;
  if (!ctx || !readStaggerInputs()) return;
  ctx.plan = planStagger(ctx);
  renderStaggerModal();
}

async function applyStagger(): Promise<void> {
  const ctx = staggerCtx;
  if (!ctx || !readStaggerInputs()) return;
  const plan = planStagger(ctx);
  ctx.plan = plan;

  S.busy = `Staggering ${plan.size} automation${plan.size === 1 ? "" : "s"}`;
  renderChrome();

  let applied = 0;
  try {
    for (const [file, min] of plan) {
      await backend().setSchedule(file, { mode: "daily", times: [hhmm(min)] });
      applied += 1;
    }
    toast(
      `Staggered ${applied} automation${applied === 1 ? "" : "s"} across ${hhmm(ctx.start)}–${hhmm(ctx.end)}`,
      "ok",
    );
    S.picked.clear();
  } catch (err) {
    toast(`Auto-stagger applied ${applied} of ${plan.size}, then failed — ${errText(err)}`, "err");
  } finally {
    S.busy = null;
    closeModal();
    render();
    await refreshJobs();
  }
}

/* ------------------------------- actions ------------------------------- */

async function retry(what: string): Promise<void> {
  switch (what) {
    case "jobs": {
      const data = await refreshJobs();
      if (data) primeView();
      break;
    }
    case "fleet":
      await loadFleet();
      break;
    case "profile":
      await loadProfile();
      break;
    case "logs":
      await loadLogs(S.logQuery);
      break;
    case "detail":
      if (S.selected) await loadDetailRuns(S.selected);
      break;
    case "paths":
      await loadPaths();
      break;
    default:
      render();
  }
}

function reloadAll(): void {
  void refreshJobs().then((data) => {
    if (data) primeView();
  });
}

function toggleTheme(): void {
  applyTheme(effectiveTheme() === "dark" ? "light" : "dark");
}

async function bulkSetEnabled(files: string[], enabled: boolean): Promise<void> {
  if (!files.length) {
    toast("Select automations first", "warn");
    return;
  }
  S.picked.clear();
  await mutate(
    `${enabled ? "Enabled" : "Disabled"} ${files.length} automation${files.length === 1 ? "" : "s"}`,
    () => Promise.all(files.map((f) => backend().setEnabled(f, enabled))),
  );
  render();
}

async function bulkRun(files: string[], dry: boolean): Promise<void> {
  if (!files.length) {
    toast("Select automations first", "warn");
    return;
  }
  S.busy = `${dry ? "Dry-running" : "Running"} ${files.length} automation${files.length === 1 ? "" : "s"}`;
  renderChrome();
  let done = 0;
  let spoke = 0;
  try {
    for (const file of files) {
      const o = await backend().runJob(file, dry);
      done += 1;
      if (o.status !== "quiet" && o.status !== "off") spoke += 1;
    }
    toast(
      `${dry ? "Dry ran" : "Ran"} ${done} automation${done === 1 ? "" : "s"}${
        spoke ? ` · ${spoke} spoke` : " · all quiet"
      }`,
      spoke ? "warn" : "ok",
    );
    S.picked.clear();
  } catch (err) {
    toast(`${dry ? "Dry run" : "Run"} failed — ${errText(err)}`, "err");
  } finally {
    S.busy = null;
    await refreshJobs();
  }
}

function handleBulk(kind: string): void {
  switch (kind) {
    case "clear":
      S.picked.clear();
      render();
      break;
    case "enable":
      void bulkSetEnabled([...S.picked], true);
      break;
    case "disable":
      void bulkSetEnabled([...S.picked], false);
      break;
    case "run":
      void bulkRun([...S.picked], false);
      break;
    case "stagger":
      openStaggerModal();
      break;
    case "group":
      toast("Move to group is not available here yet — edit Group on an automation.", "warn");
      break;
  }
}

/* ------------------------- detail editing actions ------------------------- */

function currentJob(): Job | undefined {
  return jobByFile(S.selected);
}

function changeMode(mode: ScheduleMode): void {
  const j = currentJob();
  if (!j || j.schedule.mode === mode) return;
  const s = j.schedule;
  const times = s.mode === "daily" || s.mode === "weekly" ? s.times : ["02:00"];
  const days = s.mode === "weekly" && s.days.length ? s.days : ["Mon"];
  let next: Schedule;
  switch (mode) {
    case "daily":
      next = { mode: "daily", times };
      break;
    case "weekly":
      next = { mode: "weekly", days, times };
      break;
    case "interval":
      next = { mode: "interval", every_hours: s.mode === "interval" ? s.every_hours : 6 };
      break;
    case "manual":
      next = { mode: "manual" };
      break;
  }
  void setSchedule(j.file, next);
}

function toggleDay(day: string): void {
  const j = currentJob();
  const s = j?.schedule;
  if (!j || !s || s.mode !== "weekly") return;
  const days = s.days.includes(day) ? s.days.filter((d) => d !== day) : [...s.days, day];
  if (!days.length) {
    toast("A weekly schedule needs at least one day", "warn");
    return;
  }
  days.sort((a, b) => DAYS.indexOf(a) - DAYS.indexOf(b));
  void setSchedule(j.file, { mode: "weekly", days, times: s.times });
}

// The actions themselves live in store.ts: they own the draft handling —
// an input's draft is dropped only when the command it feeds succeeded.

/* -------------------------------- events -------------------------------- */

function layerHasContent(): boolean {
  const el = document.getElementById("layer");
  return !!el && el.childElementCount > 0;
}

const CLICKABLE =
  "[data-nav],[data-job],[data-open],[data-run],[data-dry],[data-del],[data-deep],[data-view],[data-grp]," +
  "[data-grpopen],[data-status],[data-groupby],[data-bulk],[data-mode],[data-wday],[data-setevery]," +
  "[data-addtime],[data-deltime],[data-settimeout],[data-settitle],[data-validate],[data-clearf],[data-close]," +
  "[data-confirmdel],[data-confirmdeep],[data-run-confirm],[data-stagger-preview],[data-stagger-apply]," +
  "[data-retry],[data-theme-set],[data-reindex],[data-prune],[data-set-root]," +
  "#btn-add,#btn-add2,#btn-theme,#btn-pal,#btn-refresh,#log-group,#log-flat";

document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement | null;
  if (!target) return;
  if (target.closest("input,select,textarea")) return;
  const t = target.closest<HTMLElement>(CLICKABLE);
  if (!t) return;
  const d = t.dataset;
  const id = t.id;

  if (id === "btn-theme") {
    toggleTheme();
    return;
  }
  if (id === "btn-pal") {
    openPalette();
    return;
  }
  if (id === "btn-refresh") {
    reloadAll();
    return;
  }
  if (id === "btn-add" || id === "btn-add2") {
    toast("Creating automations from the console is not available here yet.", "warn");
    return;
  }
  if (id === "log-group" || id === "log-flat") {
    S.logGroup = id === "log-group";
    render();
    return;
  }

  if (d.close !== undefined && (target === t || t.tagName === "BUTTON")) {
    closeModal();
    return;
  }
  if (d.confirmdel) {
    void confirmDelete(d.confirmdel);
    return;
  }
  if (d.confirmdeep) {
    confirmDeep(d.confirmdeep);
    return;
  }
  if (d.runConfirm !== undefined) {
    void confirmRun();
    return;
  }
  if (d.staggerPreview !== undefined) {
    previewStagger();
    return;
  }
  if (d.staggerApply !== undefined) {
    void applyStagger();
    return;
  }

  if (d.deep) {
    openDeepModal(d.deep);
    return;
  }
  if (d.del) {
    openDeleteModal(d.del);
    return;
  }
  if (d.run) {
    openRunModal(d.run, false);
    return;
  }
  if (d.dry) {
    openRunModal(d.dry, true);
    return;
  }

  if (d.grp) {
    if (S.collapsed.has(d.grp)) S.collapsed.delete(d.grp);
    else S.collapsed.add(d.grp);
    renderChrome();
    return;
  }
  if (d.grpopen) {
    S.groupFilter = d.grpopen;
    S.savedView = "all";
    go("jobs");
    return;
  }
  if (d.view) {
    const key = d.view as SavedView;
    if (!(key in SAVED_VIEWS)) return;
    S.savedView = key;
    S.groupFilter = null;
    S.statusF.clear();
    go("jobs");
    return;
  }
  if (d.status) {
    const status = d.status as Status;
    if (S.statusF.has(status)) S.statusF.delete(status);
    else S.statusF.add(status);
    render();
    return;
  }
  if (d.groupby) {
    S.groupBy = d.groupby === "flat" ? "flat" : "group";
    render();
    return;
  }
  if (d.clearf !== undefined) {
    S.statusF.clear();
    S.q = "";
    S.groupFilter = null;
    S.savedView = "all";
    render();
    return;
  }
  if (d.bulk) {
    handleBulk(d.bulk);
    return;
  }
  if (d.mode) {
    changeMode(d.mode as ScheduleMode);
    return;
  }
  if (d.wday) {
    toggleDay(d.wday);
    return;
  }
  if (d.setevery !== undefined) {
    void setEvery(inputValue("#d-every"));
    return;
  }
  if (d.addtime !== undefined) {
    void addTime(inputValue("#d-newtime"));
    return;
  }
  if (d.deltime) {
    void updateTimes((times) => times.filter((x) => x !== d.deltime));
    return;
  }
  if (d.settimeout !== undefined) {
    void applyTimeout(inputValue("#d-timeout"));
    return;
  }
  if (d.settitle !== undefined) {
    void applyTitle(inputValue("#d-title"));
    return;
  }
  if (d.validate) {
    void validate(d.validate);
    return;
  }
  if (d.nav) {
    S.groupFilter = null;
    go(d.nav as ViewName);
    return;
  }
  if (d.job || d.open) {
    closeModal();
    go("detail", d.job ?? d.open ?? null);
    return;
  }
  if (d.retry) {
    void retry(d.retry);
    return;
  }
  if (d.themeSet) {
    applyTheme(d.themeSet === "light" || d.themeSet === "dark" ? d.themeSet : "system");
    return;
  }
  if (d.reindex !== undefined) {
    void mutate("Reindexed from log", () => backend().reindex());
    return;
  }
  if (d.prune !== undefined) {
    void pruneRuns(inputValue("#prune-days"));
    return;
  }
  if (d.setRoot !== undefined) {
    void saveAutomationsRoot(inputValue("#root-input"));
  }
});

document.addEventListener("change", (e) => {
  const el = e.target;
  if (el instanceof HTMLInputElement && el.dataset.toggle) {
    void setEnabled(el.dataset.toggle, el.checked);
    return;
  }
  if (el instanceof HTMLInputElement && el.dataset.pick) {
    if (el.checked) S.picked.add(el.dataset.pick);
    else S.picked.delete(el.dataset.pick);
    render();
    return;
  }
  if (el instanceof HTMLInputElement && el.dataset.pickall !== undefined) {
    if (el.checked) visible().forEach((j) => S.picked.add(j.file));
    else S.picked.clear();
    render();
    return;
  }
  if (el instanceof HTMLSelectElement && el.id === "log-src") {
    S.logFilter = el.value;
    render();
    return;
  }
  if (el instanceof HTMLSelectElement && el.id === "d-group" && S.selected) {
    const j = jobByFile(S.selected);
    if (j && j.group !== el.value) void setMeta(j.file, null, el.value);
  }
});

let logTimer: number | undefined;

function scheduleLogLoad(): void {
  if (logTimer !== undefined) clearTimeout(logTimer);
  logTimer = setTimeout(() => {
    void loadLogs(S.logQuery);
  }, 260) as unknown as number;
}

document.addEventListener("input", (e) => {
  const el = e.target;
  if (!(el instanceof HTMLInputElement)) return;
  if (el.id === "j-q") {
    S.q = el.value;
    render();
    return;
  }
  if (el.id === "log-q") {
    S.logQuery = el.value;
    render();
    scheduleLogLoad();
    return;
  }
  if (el.id === "pal-q") {
    S.palQ = el.value;
    palIndex = 0;
    renderPalette();
  }
  if (mirrorDraft(el.id, el.value)) {
    // Mirror the edit into the draft so an unrelated repaint (a toast
    // auto-dismiss, a background load) cannot reset what the user is typing.
    // Deliberately do NOT render() here: repainting the input being typed
    // into is exactly what loses the caret and the value.
    return;
  }
  if (el.id === "root-input") {
    // Mirror the edit into the draft so an unrelated repaint (a toast
    // auto-dismiss, a background load) cannot reset what the user is typing.
    // Unlike the search fields above, there is nothing to filter live, so
    // deliberately do NOT render() here — repainting the input being typed
    // into is exactly what loses the caret and the value.
    S.pathsEdit.draft = el.value;
  }
});

document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    openPalette();
    return;
  }
  if (e.key === "Escape") {
    if (S.palOpen || layerHasContent()) {
      e.preventDefault();
      closeModal();
    }
    return;
  }
  if (
    e.key === "Enter" &&
    e.target instanceof HTMLInputElement &&
    e.target.id === "root-input" &&
    !S.pathsEdit.busy
  ) {
    e.preventDefault();
    void saveAutomationsRoot(e.target.value);
    return;
  }
  if (!S.palOpen) return;

  const hits = paletteHits();
  if (e.key === "ArrowDown") {
    e.preventDefault();
    if (hits.length) {
      palIndex = (palIndex + 1) % hits.length;
      renderPalette();
    }
    return;
  }
  if (e.key === "ArrowUp") {
    e.preventDefault();
    if (hits.length) {
      palIndex = (palIndex - 1 + hits.length) % hits.length;
      renderPalette();
    }
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault();
    const hit = hits[palIndex] ?? hits[0];
    if (hit) {
      closeModal();
      go("detail", hit.file);
    }
  }
});

/* -------------------------------- bootstrap -------------------------------- */

function boot(): void {
  const params = new URLSearchParams(window.location.search);
  if (params.get("fake") === "1") setBackend(fakeBackend());

  const theme = savedTheme();
  if (theme) document.documentElement.dataset.theme = theme;

  onChange(render);
  render();

  void refreshJobs().then((data) => {
    if (data) primeView();
  });
}

boot();
