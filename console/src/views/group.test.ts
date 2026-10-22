import { describe, expect, it, vi } from "vitest";
import { applyBulkGroup, bulkGroupToast, planBulkGroup, vGroupModal } from "./group";

const A = "60-restic-daily.sh";
const B = "61-restic-check.sh";
const C = "80-tor-check.sh";

describe("Move to group — the picker", () => {
  it("lists the existing groups and still allows a new name", () => {
    const html = vGroupModal({ files: [A, B], groups: ["Backups", "Developer"], value: "" });

    expect(html).toContain('<option value="Backups">');
    expect(html).toContain('<option value="Developer">');
    expect(html).toContain('list="g-groups"');
    expect(html).toContain('id="g-group"');
    expect(html).toContain("Move 2 automations");
  });

  it("says so when the fleet has no groups yet", () => {
    expect(vGroupModal({ files: [A], groups: [], value: "" })).toContain("No groups yet");
  });
});

describe("Move to group — refusals", () => {
  it("refuses a blank group name instead of firing writes", () => {
    expect(planBulkGroup([A], "   ")).toEqual({ ok: false, reason: "Enter a group name" });
  });

  it("refuses an empty selection", () => {
    expect(planBulkGroup([], "Backups")).toEqual({ ok: false, reason: "Select automations first" });
  });

  it("trims the typed name", () => {
    expect(planBulkGroup([A], "  Backups ")).toEqual({
      ok: true,
      plan: { files: [A], group: "Backups" },
    });
  });
});

describe("Move to group — applying", () => {
  it("calls setMeta once per picked file with title null and the chosen group", async () => {
    const setMeta = vi.fn(async () => {});

    const result = await applyBulkGroup([A, B, C], "Backups", setMeta);

    expect(setMeta).toHaveBeenCalledTimes(3);
    expect(setMeta).toHaveBeenNthCalledWith(1, A, null, "Backups");
    expect(setMeta).toHaveBeenNthCalledWith(2, B, null, "Backups");
    expect(setMeta).toHaveBeenNthCalledWith(3, C, null, "Backups");
    expect(result).toEqual({ moved: [A, B, C], failed: [] });
    expect(bulkGroupToast(result, "Backups")).toEqual({
      msg: "Moved 3 automations to “Backups”",
      kind: "ok",
    });
  });

  it("keeps going after a failure and names the files that failed", async () => {
    const setMeta = vi.fn(async (file: string) => {
      if (file === B) throw new Error("write refused");
    });

    const result = await applyBulkGroup([A, B, C], "Backups", setMeta);

    expect(result.moved).toEqual([A, C]);
    expect(result.failed).toEqual([{ file: B, error: "write refused" }]);

    const outcome = bulkGroupToast(result, "Backups");
    expect(outcome.kind).toBe("err");
    expect(outcome.msg).toContain("2 of 3");
    expect(outcome.msg).toContain(B);
    expect(outcome.msg).toContain("write refused");
  });
});
