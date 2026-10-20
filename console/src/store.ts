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
import { localLoadProfile, needsAttention, nextRun } from "./format";
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

/** Filesystem diagnostics. Read-only: the config is edited outside the GUI. */
export const loadPaths = (): Promise<ResolvedPathsInfo | null> => load(S.paths, () => backend().resolvedPaths());

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
