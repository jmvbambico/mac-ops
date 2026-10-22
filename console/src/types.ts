/**
 * Mirrors src-tauri/src/types.rs.
 *
 * Serialization notes that matter here:
 *  - `Status` is `#[serde(rename_all = "lowercase")]` -> plain lowercase strings.
 *  - `Schedule` is internally tagged: `#[serde(tag = "mode", rename_all = "lowercase")]`.
 *    Variant names are lowercased; FIELD names keep their Rust snake_case
 *    spelling, so the interval variant carries `every_hours`, not `everyHours`.
 *  - Command ARGUMENTS are a different story: tauri's command macro defaults to
 *    `ArgumentCase::Camel`, so `keep_script` must be sent as `keepScript`.
 *    See api.ts.
 */

/** Outcome vocabulary. `quiet` is SUCCESS: ran and said nothing. */
export type Status = "quiet" | "spoke" | "fail" | "hang" | "off" | "none";

export const STATUSES: Status[] = ["quiet", "spoke", "fail", "hang", "none", "off"];

/** Outcomes that mean a human should look. */
export const ATTN: Status[] = ["spoke", "fail", "hang"];

export type ScheduleMode = "daily" | "weekly" | "interval" | "manual";

export type Schedule =
  | { mode: "daily"; times: string[] }
  | { mode: "weekly"; days: string[]; times: string[] }
  | { mode: "interval"; every_hours: number }
  | { mode: "manual" };

export interface Run {
  id: number;
  job: string;
  /** unix epoch seconds */
  started_at: number;
  duration_s: number;
  rc: number;
  status: Status;
  dry_run: boolean;
  summary: string;
  stdout: string;
}

export interface Job {
  file: string; // "30-brew-update.sh"
  label: string; // "brew-update"
  title: string;
  group: string;
  kind: string; // mole | pkg | py | sh
  tier: string; // daily | weekly
  timeout: number; // seconds
  enabled: boolean;
  schedule: Schedule;
  last: Run | null;
}

/** One cell of the fleet heatmap: worst outcome for a group on a day. */
export interface DayStat {
  day: string; // YYYY-MM-DD
  group: string;
  status: Status;
}

export interface NewJob {
  title: string;
  group: string;
  kind: string; // mole | pkg | py | sh
  schedule: Schedule;
  timeout: number;
  /** Python template: path to the .py file, relative to the automations root (the folder that contains jobs/). */
  script_path?: string | null;
  /** Package template: brew | npm | pipx | cargo | rustup | gem */
  manager?: string | null;
  /** Mole template: optimize | clean */
  subcommand?: string | null;
}

export interface RunOutcome {
  status: Status;
  rc: number;
  duration_s: number;
  stdout: string;
  stderr: string;
  timed_out: boolean;
}

export const STATUS_META: Record<Status, { t: string; d: string }> = {
  quiet: { t: "Quiet", d: "Ran and said nothing — the healthy state" },
  spoke: { t: "Spoke", d: "Surfaced an alert on stdout" },
  fail: { t: "Failed", d: "Exited non-zero" },
  hang: { t: "Timed out", d: "Exceeded its timeout; process group killed" },
  off: { t: "Disabled", d: "Skipped by console state" },
  none: { t: "Not run", d: "No run recorded" },
};

/**
 * The status a job should be RENDERED as. Disabled wins over the last run, and
 * "no run recorded" is `none` — never confused with `quiet`, which is the
 * success signal of the silent-on-success contract.
 */
export function statusOf(j: Job): Status {
  if (!j.enabled) return "off";
  return j.last?.status ?? "none";
}

export function lastDuration(j: Job): number {
  return j.last?.duration_s ?? 0;
}

export function lastSummary(j: Job): string {
  if (j.last) return j.last.summary || "(no output — silent, which is success)";
  return j.enabled ? "never run" : "disabled";
}
