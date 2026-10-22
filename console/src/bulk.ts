/**
 * Shared plumbing for bulk actions.
 *
 * Every bulk action applies to each automation independently: one rejected
 * write must not abandon the rest, and the toast has to name what failed
 * rather than reporting a blanket success. The per-file loop lives here; each
 * action keeps its own wording.
 */
import { errText } from "./api";
import type { RunOutcome } from "./types";

export interface BulkFailure {
  file: string;
  error: string;
}

export interface BulkDone<T> {
  file: string;
  value: T;
}

export interface BulkResult<T> {
  done: BulkDone<T>[];
  failed: BulkFailure[];
}

export interface BulkToast {
  msg: string;
  kind: "ok" | "warn" | "err";
}

export type BulkPlan = { ok: true; files: string[] } | { ok: false; reason: string };

/** Refuse an empty selection before any file is touched. */
export function planBulk(files: readonly string[]): BulkPlan {
  if (!files.length) return { ok: false, reason: "Select automations first" };
  return { ok: true, files: [...files] };
}

/**
 * Apply `fn` to each file independently, in order. A rejection is recorded
 * against its file and the remaining files still run.
 */
export async function applyBulk<T>(
  files: readonly string[],
  fn: (file: string) => Promise<T>,
): Promise<BulkResult<T>> {
  const done: BulkDone<T>[] = [];
  const failed: BulkFailure[] = [];
  for (const file of files) {
    try {
      done.push({ file, value: await fn(file) });
    } catch (err) {
      failed.push({ file, error: errText(err) });
    }
  }
  return { done, failed };
}

const plural = (n: number): string => (n === 1 ? "" : "s");

/** "N of M" plus every failing name and its error; null when nothing failed. */
function partialFailure(
  verb: string,
  done: number,
  failed: BulkFailure[],
): BulkToast | null {
  if (!failed.length) return null;
  const total = done + failed.length;
  const which = failed.map((f) => `${f.file} (${f.error})`).join(", ");
  return {
    msg: `${verb} ${done} of ${total} automation${plural(total)} — failed: ${which}`,
    kind: "err",
  };
}

/** The one place that decides whether a bulk run is a success or an error. */
export function bulkRunToast(result: BulkResult<RunOutcome>, dry: boolean): BulkToast {
  const verb = dry ? "Dry ran" : "Ran";
  const partial = partialFailure(verb, result.done.length, result.failed);
  if (partial) return partial;
  const spoke = result.done.filter(
    (d) => d.value.status !== "quiet" && d.value.status !== "off",
  ).length;
  return {
    msg: `${verb} ${result.done.length} automation${plural(result.done.length)}${
      spoke ? ` · ${spoke} spoke` : " · all quiet"
    }`,
    kind: spoke ? "warn" : "ok",
  };
}

/** The one place that decides whether a bulk enable/disable is a success or an error. */
export function bulkEnabledToast(result: BulkResult<unknown>, enabled: boolean): BulkToast {
  const verb = enabled ? "Enabled" : "Disabled";
  const partial = partialFailure(verb, result.done.length, result.failed);
  if (partial) return partial;
  return {
    msg: `${verb} ${result.done.length} automation${plural(result.done.length)}`,
    kind: "ok",
  };
}
