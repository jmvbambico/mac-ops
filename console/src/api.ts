/**
 * The whole backend surface, in one place.
 *
 * Every Rust command returns `Result<T, String>`, so every invoke here rejects
 * with a string on failure. Nothing in this module swallows an error: callers
 * decide whether that means an error banner or a toast.
 *
 * `Backend` is an interface and `setBackend()` swaps the implementation, so the
 * data layer is testable with no Tauri runtime at all (the Rust modules are
 * still `todo!()`; see `fakeBackend` in fake.ts).
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DayStat, Job, NewJob, Run, RunLogBatch, RunOutcome, Schedule } from "./types";

export type PathSource = "env" | "config" | "default";

/**
 * Roots the backend resolved at startup. Settings renders these and can write
 * the automations root to the config file; the change applies on restart,
 * and `$MOPS_AUTOMATIONS_ROOT` still wins while it is set.
 */
export interface ResolvedPaths {
  tool_root: string;
  automations_root: string;
  config_path: string;
  source: PathSource;
}

/**
 * What Settings renders after validation. `source` is null when the backend
 * sent an unrecognised value: the paths are still sound, so keep them and omit
 * the badge rather than hiding working diagnostics.
 */
export interface ResolvedPathsInfo extends Omit<ResolvedPaths, "source"> {
  source: PathSource | null;
}

/**
 * Narrow guard for the one command whose payload crosses an untyped boundary.
 * A malformed path field rejects into the normal async error state; only an
 * unrecognised `source` is tolerated, because it cannot invalidate the paths.
 */
export function parseResolvedPaths(raw: unknown): ResolvedPathsInfo {
  if (!raw || typeof raw !== "object") throw new Error("resolved_paths: response is not an object");
  const r = raw as Record<string, unknown>;
  const text = (key: string): string => {
    const value = r[key];
    if (typeof value !== "string" || value.trim() === "") {
      throw new Error(`resolved_paths: ${key} is missing or empty`);
    }
    return value;
  };
  const paths = {
    tool_root: text("tool_root"),
    automations_root: text("automations_root"),
    config_path: text("config_path"),
  };
  const source = r.source;
  return {
    ...paths,
    source: source === "env" || source === "config" || source === "default" ? source : null,
  };
}

/** Stop a live run-log subscription. */
export type Unsubscribe = () => void;

export interface Backend {
  listJobs(): Promise<Job[]>;
  resolvedPaths(): Promise<ResolvedPathsInfo>;
  /** Persist a new automations root; resolves to the paths the next launch uses. */
  setAutomationsRoot(path: string): Promise<ResolvedPathsInfo>;
  setEnabled(file: string, enabled: boolean): Promise<void>;
  setSchedule(file: string, schedule: Schedule): Promise<void>;
  setTimeout(file: string, secs: number): Promise<void>;
  setMeta(file: string, title: string | null, group: string | null): Promise<void>;
  createJob(spec: NewJob): Promise<string>;
  deleteJob(file: string, keepScript: boolean): Promise<void>;
  runJob(file: string, dry: boolean): Promise<RunOutcome>;
  /**
   * Captured per-job output from `logs/job-<slug>.out.log`, written by the
   * job's `log_output` calls near the end of a run. Fetch it AFTER `runJob`
   * resolves, not while the job is in flight. An empty string means the job
   * has not opted in — the ordinary case, not an error.
   */
  jobOutput(file: string): Promise<string>;
  /**
   * Subscribe to the lines an on-demand run appends to the shared run log.
   * Resolves to an unsubscribe function. Streaming is best-effort: a failure
   * to subscribe must never fail the run itself.
   */
  subscribeRunLog(onBatch: (batch: RunLogBatch) => void): Promise<Unsubscribe>;
  runsFor(file: string, limit: number): Promise<Run[]>;
  fleetHistory(days: number): Promise<DayStat[]>;
  searchLogs(q: string, limit: number): Promise<Run[]>;
  loadProfile(): Promise<number[]>;
  validateJob(file: string): Promise<string[]>;
  pruneRuns(keepDays: number): Promise<number>;
  reindex(): Promise<number>;
}

/**
 * Real backend. Argument keys are camelCase because tauri's command macro
 * defaults to `ArgumentCase::Camel` when mapping the JS payload onto the Rust
 * parameter names (`keep_script` <- `keepScript`).
 */
export const tauriBackend: Backend = {
  listJobs: () => invoke<Job[]>("list_jobs"),
  resolvedPaths: () => invoke<unknown>("resolved_paths").then(parseResolvedPaths),
  setAutomationsRoot: (path) =>
    invoke<unknown>("set_automations_root", { path }).then(parseResolvedPaths),
  setEnabled: (file, enabled) => invoke<void>("set_enabled", { file, enabled }),
  setSchedule: (file, schedule) => invoke<void>("set_schedule", { file, schedule }),
  setTimeout: (file, secs) => invoke<void>("set_timeout", { file, secs }),
  setMeta: (file, title, group) => invoke<void>("set_meta", { file, title, group }),
  createJob: (spec) => invoke<string>("create_job", { spec }),
  deleteJob: (file, keepScript) => invoke<void>("delete_job", { file, keepScript }),
  runJob: (file, dry) => invoke<RunOutcome>("run_job", { file, dry }),
  jobOutput: (file) => invoke<string>("read_job_output", { file }),
  subscribeRunLog: (onBatch) =>
    listen<RunLogBatch>("run-log", (event) => {
      const payload = event.payload;
      if (payload && typeof payload.job === "string" && Array.isArray(payload.lines)) {
        onBatch(payload);
      }
    }),
  runsFor: (file, limit) => invoke<Run[]>("runs_for", { file, limit }),
  fleetHistory: (days) => invoke<DayStat[]>("fleet_history", { days }),
  searchLogs: (q, limit) => invoke<Run[]>("search_logs", { q, limit }),
  loadProfile: () => invoke<number[]>("load_profile"),
  validateJob: (file) => invoke<string[]>("validate_job", { file }),
  pruneRuns: (keepDays) => invoke<number>("prune_runs", { keepDays }),
  reindex: () => invoke<number>("reindex"),
};

let current: Backend = tauriBackend;

export function setBackend(b: Backend): void {
  current = b;
}

export function backend(): Backend {
  return current;
}

/** Normalises anything a rejected invoke can throw into a display string. */
export function errText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object") {
    const m = (err as { message?: unknown }).message;
    if (typeof m === "string") return m;
    try {
      return JSON.stringify(err);
    } catch {
      /* fall through */
    }
  }
  return String(err);
}
