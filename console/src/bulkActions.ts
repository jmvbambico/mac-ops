/**
 * Controller logic for the three bulk actions.
 *
 * They share three rules, and this module is where those rules live so the
 * actions cannot drift apart again:
 *  - one file's rejection never stops the rest (`applyBulk`);
 *  - the selection survives a partial failure so the user can retry it, and is
 *    cleared only when every write succeeded;
 *  - a fleet refresh that fails after the writes is reported, never swallowed.
 *
 * Rendering stays in main.ts: callers pass `paint`, so these functions keep the
 * status bar and table in sync without touching the DOM themselves.
 */
import { backend, errText } from "./api";
import { applyBulk, bulkEnabledToast, bulkRunToast, planBulk, withRefreshFailure } from "./bulk";
import type { BulkToast } from "./bulk";
import { S, refreshJobs, toast } from "./store";
import { applyBulkGroup, bulkGroupToast, planBulkGroup } from "./views/group";

export interface BulkPaint {
  /** Repaint the view (table, toolbar and chrome). */
  view(): void;
  /** Repaint the status bar only; used before the work starts. */
  chrome(): void;
  /** Called once a group move settles, before the refresh. */
  groupDone?(): void;
}

/** Drop the busy label, re-read the fleet, then report — including a refresh failure. */
async function settle(outcome: BulkToast, paint: BulkPaint, beforeRefresh?: () => void): Promise<void> {
  S.busy = null;
  beforeRefresh?.();
  await refreshJobs();
  const final = withRefreshFailure(outcome, S.jobs.error);
  toast(final.msg, final.kind);
  paint.view();
}

export async function bulkSetEnabled(
  files: string[],
  enabled: boolean,
  paint: BulkPaint,
): Promise<void> {
  const check = planBulk(files);
  if (!check.ok) {
    toast(check.reason, "warn");
    return;
  }
  const list = check.files;
  const what = `${enabled ? "Enabled" : "Disabled"} ${list.length} automation${list.length === 1 ? "" : "s"}`;
  S.busy = what;
  paint.view();
  let outcome: BulkToast;
  try {
    const result = await applyBulk(list, (file) => backend().setEnabled(file, enabled));
    outcome = bulkEnabledToast(result, enabled);
    if (!result.failed.length) S.picked.clear();
  } catch (err) {
    // Keep the selection: nothing is known to have succeeded, so the user can
    // retry exactly what they picked.
    outcome = { msg: `${what} failed — ${errText(err)}`, kind: "err" };
  }
  await settle(outcome, paint);
}

export async function bulkRun(files: string[], dry: boolean, paint: BulkPaint): Promise<void> {
  const check = planBulk(files);
  if (!check.ok) {
    toast(check.reason, "warn");
    return;
  }
  const list = check.files;
  S.busy = `${dry ? "Dry-running" : "Running"} ${list.length} automation${list.length === 1 ? "" : "s"}`;
  paint.chrome();
  let outcome: BulkToast;
  try {
    const result = await applyBulk(list, (file) => backend().runJob(file, dry));
    outcome = bulkRunToast(result, dry);
    if (!result.failed.length) S.picked.clear();
  } catch (err) {
    outcome = { msg: `${dry ? "Dry run" : "Run"} failed — ${errText(err)}`, kind: "err" };
  }
  await settle(outcome, paint);
}

export async function bulkSetGroup(files: string[], raw: string, paint: BulkPaint): Promise<void> {
  const check = planBulkGroup(files, raw);
  if (!check.ok) {
    toast(check.reason, "warn");
    return;
  }
  const { group } = check.plan;
  S.busy = `Moving ${check.plan.files.length} automation${check.plan.files.length === 1 ? "" : "s"} to ${group}`;
  paint.chrome();
  let outcome: BulkToast;
  try {
    const result = await applyBulkGroup(check.plan.files, group, (file, title, next) =>
      backend().setMeta(file, title, next),
    );
    outcome = bulkGroupToast(result, group);
    if (!result.failed.length) S.picked.clear();
  } catch (err) {
    outcome = { msg: `Move to group failed — ${errText(err)}`, kind: "err" };
  }
  await settle(outcome, paint, () => paint.groupDone?.());
}
