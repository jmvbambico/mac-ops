/**
 * Application state and the loaders that fill it from the backend.
 *
 * Rules this module enforces:
 *  - a failed command NEVER blanks a view: the previous data is kept and an
 *    error string is parked next to it for the view to render;
 *  - `list_jobs` failing with nothing loaded yet is the one fatal case, and it
 *    renders a full-view error with a Retry;
 *  - nothing here touches the DOM. `onChange` is how the UI learns to repaint.
 */
import { backend, errText } from "./api";
import type { ResolvedPathsInfo } from "./api";
import type { DayStat, Job, Run, Schedule, Status } from "./types";
import { hhmm, localLoadProfile, needsAttention, nextRun, parseHHMM } from "./format";
import { lastDuration, statusOf } from "./types";

export type ViewName = "overview" | "jobs" | "schedule" | "logs" | "settings" | "detail";
export type SavedView = "all" | "attention" | "disabled" | "never" | "slow" | "due";

export interface Async<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
}

const idle = <T>(): Async<T> => ({ data: null, error: null, loading: false });

export interface Toast {
  msg: string;
  kind: "ok" | "warn" | "err";
  id: number;
}

export const S = {
  // data
  jobs: idle<Job[]>(),
  fleet: idle<DayStat[]>(),
  profile: idle<number[]>(),
  logs: idle<Run[]>(),
  detailRuns: idle<Run[]>(),
  diagnostics: idle<string[]>(),
  paths: idle<ResolvedPathsInfo>(),
  /**
   * Inline state for the Settings root editor. `draft` is what the input
   * renders: typing mirrors into it, and a rejected value stays visible
   * instead of snapping back to the old root. `saved` is the value the last
   * successful save wrote to `config.json`; when an environment override is
   * active it differs from the effective root, and the panel shows both rather
   * than letting the saved value disappear. After a successful save the draft
   * keeps that persisted value, so saving again cannot write the effective
   * env root into the config file.
   */
  pathsEdit: {
    error: null as string | null,
    busy: false,
    draft: null as string | null,
    saved: null as string | null,
  },
  /**
   * Drafts for the text inputs whose value is the user's in-progress typing.
   * Typing mirrors into this map (see `mirrorDraft`) so an unrelated repaint
   * — a toast auto-dismissing, a background load — rebuilds the field from
   * what was typed instead of stale model data. The field still feeds the
   * same command it always did; only its rendering source changed.
   */
  drafts: {} as Record<string, string>,

  // navigation
  view: "overview" as ViewName,
  selected: null as string | null,
  savedView: "all" as SavedView,
  groupFilter: null as string | null,
  collapsed: new Set<string>(),

  // automations view
  q: "",
  statusF: new Set<Status>(),
  picked: new Set<string>(),
  groupBy: "group" as "group" | "flat",

  // logs view
  logFilter: "all",
  logQuery: "",
  logGroup: true,

  // chrome
  palOpen: false,
  palQ: "",
  busy: null as string | null,
  toast: null as Toast | null,
};

/**
 * The text inputs whose value is the user's in-progress typing. Typing
 * mirrors into `S.drafts` instead of being re-read from the model on the
 * next repaint. This is the same shape as the `#root-input` fix: mirror the
 * keystroke, and deliberately do NOT repaint — repainting the input being
 * typed into is exactly what loses the caret and the value.
 */
export const DRAFT_INPUT_IDS = [
  "d-title",
  "d-newtime",
  "d-timeout",
  "d-every",
  "prune-days",
  "s-start",
  "s-end",
  "s-max",
] as const;

/** Drafts for the detail view are scoped to the automation being edited. */
const DETAIL_DRAFT_IDS = new Set<string>(["d-title", "d-newtime", "d-timeout", "d-every"]);

const draftKey = (id: string): string =>
  DETAIL_DRAFT_IDS.has(id) ? `${S.selected ?? ""}::${id}` : id;

/** The value currently typed into `id`, if the user has typed into it. */
export function getDraft(id: string): string | undefined {
  return S.drafts[draftKey(id)];
}

/** Mirror a keystroke into the draft map. Returns false for other inputs. */
export function mirrorDraft(id: string, value: string): boolean {
  if (!DRAFT_INPUT_IDS.includes(id as (typeof DRAFT_INPUT_IDS)[number])) return false;
  S.drafts[draftKey(id)] = value;
  return true;
}

/** Forget a draft — used when the field it belongs to is rebuilt from scratch. */
export function clearDraft(id: string): void {
  delete S.drafts[draftKey(id)];
}

/** Forget the detail-view drafts of one automation (when it is left behind). */
export function clearDetailDrafts(file: string | null): void {
  if (!file) return;
  for (const id of DETAIL_DRAFT_IDS) delete S.drafts[`${file}::${id}`];
}

/**
 * Forget the stagger window drafts. A modal opened afresh starts from the
 * defaults, not from a previous session's half-typed window.
 */
export function clearStaggerDrafts(): void {
  clearDraft("s-start");
  clearDraft("s-end");
  clearDraft("s-max");
}

type Listener = () => void;
let listener: Listener = () => {};
export function onChange(fn: Listener): void {
  listener = fn;
}
export function changed(): void {
  listener();
}

let toastSeq = 0;
let toastTimer: number | undefined;
export function toast(msg: string, kind: Toast["kind"] = "ok"): void {
  S.toast = { msg, kind, id: ++toastSeq };
  if (toastTimer !== undefined) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    S.toast = null;
    changed();
  }, kind === "err" ? 7000 : 3200) as unknown as number;
  changed();
}

export const jobs = (): Job[] => S.jobs.data ?? [];
export const jobByFile = (file: string | null): Job | undefined => (file ? jobs().find((j) => j.file === file) : undefined);
export const attnJobs = (): Job[] => jobs().filter(needsAttention);

export const SAVED_VIEWS: Record<SavedView, { label: string; pick: (all: Job[]) => Job[] }> = {
  all: { label: "All automations", pick: (all) => all },
  attention: { label: "Needs attention", pick: (all) => all.filter(needsAttention) },
  disabled: { label: "Disabled", pick: (all) => all.filter((j) => !j.enabled) },
  never: { label: "Never run", pick: (all) => all.filter((j) => statusOf(j) === "none") },
  slow: {
    label: "Slowest",
    pick: (all) => [...all].filter((j) => j.enabled).sort((a, b) => lastDuration(b) - lastDuration(a)).slice(0, 10),
  },
  due: {
    label: "Due next",
    pick: (all) => {
      const now = new Date();
      return [...all]
        .filter((j) => j.enabled && nextRun(j, now))
        .sort((a, b) => (nextRun(a, now) as Date).getTime() - (nextRun(b, now) as Date).getTime())
        .slice(0, 10);
    },
  },
};

/** The automations the Automations table should show, after every filter. */
export function visible(): Job[] {
  let list = SAVED_VIEWS[S.savedView].pick(jobs());
  if (S.groupFilter) list = list.filter((j) => j.group === S.groupFilter);
  if (S.statusF.size) list = list.filter((j) => S.statusF.has(statusOf(j)));
  if (S.q) {
    const s = S.q.toLowerCase();
    list = list.filter((j) =>
      (j.title + j.file + j.label + (j.last?.summary ?? "") + j.group).toLowerCase().includes(s),
    );
  }
  return list;
}

/* ----------------------------- loaders ----------------------------- */

async function load<T>(slot: Async<T>, fn: () => Promise<T>, repaint = true): Promise<T | null> {
  slot.loading = true;
  slot.error = null;
  if (repaint) changed();
  try {
    const data = await fn();
    slot.data = data;
    slot.error = null;
    return data;
  } catch (err) {
    // keep whatever we already had; park the message next to it
    slot.error = errText(err);
    return null;
  } finally {
    slot.loading = false;
    if (repaint) changed();
  }
}

export const refreshJobs = (): Promise<Job[] | null> => load(S.jobs, () => backend().listJobs());

export const loadFleet = (days = 30): Promise<DayStat[] | null> => load(S.fleet, () => backend().fleetHistory(days));

export async function loadProfile(): Promise<void> {
  await load(S.profile, () => backend().loadProfile());
  if (S.profile.error && !S.profile.data) {
    // Graceful degradation: the same number the backend would have produced,
    // computed from the schedules we already hold.
    S.profile.data = localLoadProfile(jobs());
    changed();
  }
}

export const loadLogs = (q: string, limit = 300): Promise<Run[] | null> => load(S.logs, () => backend().searchLogs(q, limit));

export const loadDetailRuns = (file: string, limit = 30): Promise<Run[] | null> =>
  load(S.detailRuns, () => backend().runsFor(file, limit));

export const validate = (file: string): Promise<string[] | null> => load(S.diagnostics, () => backend().validateJob(file));

/** Filesystem diagnostics, plus the current automations root for editing. */
export const loadPaths = (): Promise<ResolvedPathsInfo | null> => load(S.paths, () => backend().resolvedPaths());

/**
 * Persist a new automations root. The backend validates and writes
 * `~/.config/mops/config.json` atomically, then returns the paths the next
 * launch will resolve. We adopt that answer directly (rather than re-reading
 * `resolved_paths`, which still reports the running App's old root) and keep
 * any validation error inline so the panel can show it.
 *
 * Nothing is re-pointed live: the database and cached jobs belong to the
 * startup root, so the user is told a restart is required.
 */
export async function saveAutomationsRoot(path: string): Promise<boolean> {
  S.pathsEdit.busy = true;
  S.pathsEdit.error = null;
  S.pathsEdit.draft = path;
  changed();
  try {
    const resolved = await backend().setAutomationsRoot(path);
    S.paths = { data: resolved, error: null, loading: false };
    S.pathsEdit.error = null;
    // Remember what was persisted so the panel can show the config value even
    // when an env override keeps it inactive. Without an override the
    // re-resolved root IS the config value; with one, the backend still wrote
    // the trimmed input, so keep that.
    S.pathsEdit.saved = resolved.source === "env" ? path.trim() : resolved.automations_root;
    // Keep the persisted value in the editor. Clearing the draft would fall
    // back to the EFFECTIVE root, so under an env override the next Save would
    // silently write the env path into config.json instead of the saved one.
    S.pathsEdit.draft = S.pathsEdit.saved;
    toast("Automations root saved — restart the console to apply it", "ok");
    return true;
  } catch (err) {
    const msg = errText(err);
    S.pathsEdit.error = msg;
    toast(`Could not save the automations root — ${msg}`, "err");
    return false;
  } finally {
    S.pathsEdit.busy = false;
    changed();
  }
}

/** The data each view needs, kicked off when you navigate to it. */
export function primeView(): void {
  if (S.view === "overview") void loadFleet();
  if (S.view === "schedule") void loadProfile();
  if (S.view === "logs") void loadLogs(S.logQuery);
  if (S.view === "settings") void loadPaths();
  if (S.view === "detail" && S.selected) {
    S.detailRuns = idle<Run[]>();
    void loadDetailRuns(S.selected);
  }
}

/* ----------------------------- mutations ----------------------------- */

/**
 * Run a mutating command, then re-read the fleet. Errors become a toast —
 * the view keeps its data rather than going blank.
 */
export async function mutate(what: string, fn: () => Promise<unknown>): Promise<boolean> {
  S.busy = what;
  changed();
  try {
    await fn();
    S.busy = null;
    await refreshJobs();
    if (S.jobs.error) toast(`${what}: ${S.jobs.error}`, "err");
    else toast(what, "ok");
    return true;
  } catch (err) {
    S.busy = null;
    toast(`${what} failed — ${errText(err)}`, "err");
    return false;
  }
}

export const setEnabled = (file: string, enabled: boolean): Promise<boolean> =>
  mutate(`${enabled ? "Enabled" : "Disabled"} ${file}`, () => backend().setEnabled(file, enabled));

export const setSchedule = (file: string, schedule: Schedule): Promise<boolean> =>
  mutate(`Rescheduled ${file}`, () => backend().setSchedule(file, schedule));

export const setTimeoutSecs = (file: string, secs: number): Promise<boolean> =>
  mutate(`Timeout ${secs}s on ${file}`, () => backend().setTimeout(file, secs));

export const setMeta = (file: string, title: string | null, group: string | null): Promise<boolean> =>
  mutate(`Updated ${file}`, () => backend().setMeta(file, title, group));

export const deleteJob = (file: string, keepScript: boolean): Promise<boolean> =>
  mutate(`Deleted ${file}`, () => backend().deleteJob(file, keepScript));

/* --------------------- actions that consume a draft --------------------- */

/**
 * The actions behind the buttons that read one of the draft inputs.
 *
 * Each one takes the value the user typed, runs the real command, and drops
 * that input's draft ONLY when the command succeeded.
 *
 * The rule matters in both directions. Before the drafts existed, the field
 * was rebuilt from the model on every repaint, so a committed action left it
 * empty and a failed one silently ate the text. Persisting the draft fixed
 * the second half, but it also stopped the first: a successful "Add time"
 * must still leave the box empty, or the next click adds the same time
 * twice. A rejected value — unparsable, or a write the backend refused —
 * must keep its text so the user can correct it.
 */

/** A number input's value as a positive integer, or the fallback. */
const positiveInt = (raw: string, fallback: number): number => {
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : fallback;
};

/**
 * Replace the times of the automation being edited. Resolves false when the
 * schedule could not take the change (wrong mode, or a refused write).
 */
export async function updateTimes(mutateTimes: (times: string[]) => string[]): Promise<boolean> {
  const j = jobByFile(S.selected);
  const s = j?.schedule;
  if (!j || !s || (s.mode !== "daily" && s.mode !== "weekly")) return false;
  const times = [...mutateTimes(s.times)];
  if (!times.length) {
    toast("A schedule needs at least one time", "warn");
    return false;
  }
  times.sort((a, b) => (parseHHMM(a) ?? 0) - (parseHHMM(b) ?? 0));
  return setSchedule(j.file, s.mode === "weekly" ? { mode: "weekly", days: s.days, times } : { mode: "daily", times });
}

/**
 * Add a time to the automation being edited.
 *
 * The #d-newtime draft is dropped only once the schedule write succeeded;
 * a rejected time keeps the user's text so it can be corrected.
 */
export async function addTime(raw: string): Promise<boolean> {
  const mins = parseHHMM(raw.trim());
  if (mins === null) {
    toast("Enter a time as HH:MM", "warn");
    return false;
  }
  const t = hhmm(mins);
  const ok = await updateTimes((times) => (times.includes(t) ? times : [...times, t]));
  if (!ok) return false;
  clearDraft("d-newtime");
  changed();
  return true;
}

/** Apply the interval length. Drops the #d-every draft once it is saved. */
export async function setEvery(raw: string): Promise<boolean> {
  const j = jobByFile(S.selected);
  if (!j) return false;
  const hours = Math.min(24, Math.max(1, positiveInt(raw.trim(), 6)));
  const ok = await setSchedule(j.file, { mode: "interval", every_hours: hours });
  if (!ok) return false;
  clearDraft("d-every");
  changed();
  return true;
}

/** Apply the timeout. Drops the #d-timeout draft once it is saved. */
export async function applyTimeout(raw: string): Promise<boolean> {
  const j = jobByFile(S.selected);
  if (!j) return false;
  const secs = Math.min(86400, Math.max(1, positiveInt(raw.trim(), j.timeout)));
  const ok = await setTimeoutSecs(j.file, secs);
  if (!ok) return false;
  clearDraft("d-timeout");
  changed();
  return true;
}

/** Rename the automation. Drops the #d-title draft once the rename lands. */
export async function applyTitle(raw: string): Promise<boolean> {
  const j = jobByFile(S.selected);
  if (!j) return false;
  const title = raw.trim();
  if (!title) {
    toast("Title cannot be empty", "warn");
    return false;
  }
  if (title === j.title) return true;
  const ok = await setMeta(j.file, title, null);
  if (!ok) return false;
  clearDraft("d-title");
  changed();
  return true;
}

/**
 * Prune the run database. Drops the #prune-days draft once the prune ran —
 * the window has been spent, and the field falls back to the default, which
 * is what the field showed before the drafts existed.
 */
export async function pruneRuns(raw: string): Promise<boolean> {
  const days = positiveInt(raw.trim(), 90);
  const ok = await mutate(`Pruned runs older than ${days} days`, () => backend().pruneRuns(days));
  if (!ok) return false;
  clearDraft("prune-days");
  changed();
  return true;
}
