/**
 * The bulk "Move to group" modal.
 *
 * Like the stagger modal it is a pure renderer plus a small pair of helpers:
 * one validates what the user chose, one applies it file by file so a single
 * failure does not hide the rest. The controller (main.ts) owns the toast,
 * the busy state and the fleet refresh.
 */
import { applyBulk } from "../bulk";
import type { BulkFailure } from "../bulk";
import { esc } from "../format";
import { IC } from "../icons";

export interface GroupCtx {
  files: string[];
  groups: string[];
  value: string;
}

export function vGroupModal(ctx: GroupCtx): string {
  const n = ctx.files.length;
  const existing = ctx.groups.length
    ? `Existing: ${ctx.groups.map((g) => esc(g)).join(", ")}`
    : "No groups yet — the first name starts one.";
  return `
    <div class="scrim" data-close><div class="modal" style="width:min(480px,100%)">
      <h3>Move ${n} automation${n === 1 ? "" : "s"} to a group</h3>
      <p class="muted" style="margin:4px 0 14px;font-size:12.5px">Pick an existing group or type a new
        name. Only Group changes — titles, schedules and state stay as they are.</p>
      <div class="field" style="margin:0"><label>Group</label>
        <input class="inp" id="g-group" list="g-groups" value="${esc(ctx.value)}"
          placeholder="Group name" style="height:30px" autocomplete="off" spellcheck="false">
        <datalist id="g-groups">${ctx.groups
          .map((g) => `<option value="${esc(g)}"></option>`)
          .join("")}</datalist>
        <div class="hint">${existing}</div></div>
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        <button class="btn" data-close>Cancel</button>
        <button class="btn pri" data-group-apply="1">${IC.edit} Move</button>
      </div>
    </div></div>`;
}

export interface BulkGroupPlan {
  files: string[];
  group: string;
}

export type BulkGroupCheck =
  | { ok: true; plan: BulkGroupPlan }
  | { ok: false; reason: string };

/** Refuse the no-op writes before any file is touched. */
export function planBulkGroup(files: readonly string[], raw: string): BulkGroupCheck {
  const group = raw.trim();
  if (!files.length) return { ok: false, reason: "Select automations first" };
  if (!group) return { ok: false, reason: "Enter a group name" };
  return { ok: true, plan: { files: [...files], group } };
}

export type BulkGroupFailure = BulkFailure;

export interface BulkGroupResult {
  moved: string[];
  failed: BulkGroupFailure[];
}

/**
 * Move each file independently. One rejected write must not stop the others,
 * and the caller needs the names of the ones that failed, not just a count.
 */
export async function applyBulkGroup(
  files: readonly string[],
  group: string,
  setMeta: (file: string, title: string | null, group: string | null) => Promise<unknown>,
): Promise<BulkGroupResult> {
  const result = await applyBulk(files, (file) => setMeta(file, null, group));
  return { moved: result.done.map((d) => d.file), failed: result.failed };
}

/** The one place that decides whether a bulk move is a success or an error. */
export function bulkGroupToast(
  result: BulkGroupResult,
  group: string,
): { msg: string; kind: "ok" | "err" } {
  const total = result.moved.length + result.failed.length;
  if (!result.failed.length) {
    return {
      msg: `Moved ${result.moved.length} automation${result.moved.length === 1 ? "" : "s"} to “${group}”`,
      kind: "ok",
    };
  }
  const which = result.failed.map((f) => `${f.file} (${f.error})`).join(", ");
  return {
    msg: `Moved ${result.moved.length} of ${total} to “${group}” — failed: ${which}`,
    kind: "err",
  };
}
