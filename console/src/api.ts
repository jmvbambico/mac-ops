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
import type { DayStat, Job, NewJob, Run, RunOutcome, Schedule } from "./types";

/**
 * Roots the backend resolved at startup. Read-only diagnostics for Settings —
 * the automations root is configured through the environment or config file,
 * never from the GUI.
 */
export interface ResolvedPaths {
  tool_root: string;
  automations_root: string;
  config_path: string;
  source: "env" | "config" | "default";
}

export interface Backend {
  listJobs(): Promise<Job[]>;
  resolvedPaths(): Promise<ResolvedPaths>;
  setEnabled(file: string, enabled: boolean): Promise<void>;
  setSchedule(file: string, schedule: Schedule): Promise<void>;
  setTimeout(file: string, secs: number): Promise<void>;
  setMeta(file: string, title: string | null, group: string | null): Promise<void>;
  createJob(spec: NewJob): Promise<string>;
  deleteJob(file: string, keepScript: boolean): Promise<void>;
  runJob(file: string, dry: boolean): Promise<RunOutcome>;
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
  resolvedPaths: () => invoke<ResolvedPaths>("resolved_paths"),
  setEnabled: (file, enabled) => invoke<void>("set_enabled", { file, enabled }),
  setSchedule: (file, schedule) => invoke<void>("set_schedule", { file, schedule }),
  setTimeout: (file, secs) => invoke<void>("set_timeout", { file, secs }),
  setMeta: (file, title, group) => invoke<void>("set_meta", { file, title, group }),
  createJob: (spec) => invoke<string>("create_job", { spec }),
  deleteJob: (file, keepScript) => invoke<void>("delete_job", { file, keepScript }),
  runJob: (file, dry) => invoke<RunOutcome>("run_job", { file, dry }),
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
