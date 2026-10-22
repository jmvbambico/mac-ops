import { describe, expect, it, vi } from "vitest";
import { applyBulk, bulkEnabledToast, bulkRunToast, planBulk } from "./bulk";
import type { RunOutcome, Status } from "./types";

const A = "60-restic-daily.sh";
const B = "61-restic-check.sh";
const C = "80-tor-check.sh";

const outcome = (status: Status = "quiet"): RunOutcome => ({
  status,
  rc: 0,
  duration_s: 1,
  stdout: "",
  stderr: "",
  timed_out: false,
});

describe("bulk actions — refusals", () => {
  it("refuses an empty selection before touching any file", () => {
    expect(planBulk([])).toEqual({ ok: false, reason: "Select automations first" });
  });

  it("copies the picked files into a plan", () => {
    const picked = [A, B];
    const plan = planBulk(picked);
    expect(plan).toEqual({ ok: true, files: [A, B] });
    if (plan.ok) expect(plan.files).not.toBe(picked);
  });
});

describe("bulk actions — partial failure", () => {
  it("keeps running each automation after one fails", async () => {
    const runJob = vi.fn(async (file: string) => {
      if (file === B) throw new Error("job refused");
      return outcome();
    });

    const result = await applyBulk([A, B, C], (file) => runJob(file));

    expect(runJob).toHaveBeenCalledTimes(3);
    expect(runJob).toHaveBeenNthCalledWith(1, A);
    expect(runJob).toHaveBeenNthCalledWith(2, B);
    expect(runJob).toHaveBeenNthCalledWith(3, C);
    expect(result.done.map((d) => d.file)).toEqual([A, C]);
    expect(result.failed).toEqual([{ file: B, error: "job refused" }]);
  });

  it("names the failed run and reports N of M instead of plain success", async () => {
    const check = planBulk([A, B, C]);
    if (!check.ok) throw new Error("expected a plan");
    const result = await applyBulk(check.files, async (file) => {
      if (file === B) throw new Error("permission denied");
      return outcome();
    });

    const toast = bulkRunToast(result, false);
    expect(toast.kind).toBe("err");
    expect(toast.msg).toBe(`Ran 2 of 3 automations — failed: ${B} (permission denied)`);
  });

  it("names the failed write for a bulk disable", async () => {
    const result = await applyBulk([A, B], async (file) => {
      if (file === A) throw new Error("read-only");
    });

    const toast = bulkEnabledToast(result, false);
    expect(toast.kind).toBe("err");
    expect(toast.msg).toBe(`Disabled 1 of 2 automations — failed: ${A} (read-only)`);
  });
});

describe("bulk actions — full success", () => {
  it("reports a plain success for a quiet run", () => {
    const result = {
      done: [
        { file: A, value: outcome("quiet") },
        { file: B, value: outcome("off") },
      ],
      failed: [],
    };

    expect(bulkRunToast(result, false)).toEqual({
      msg: "Ran 2 automations · all quiet",
      kind: "ok",
    });
    expect(bulkRunToast(result, true)).toEqual({
      msg: "Dry ran 2 automations · all quiet",
      kind: "ok",
    });
  });

  it("counts the runs that spoke", () => {
    const result = {
      done: [
        { file: A, value: outcome("quiet") },
        { file: B, value: outcome("spoke") },
      ],
      failed: [],
    };

    expect(bulkRunToast(result, false)).toEqual({
      msg: "Ran 2 automations · 1 spoke",
      kind: "warn",
    });
  });

  it("reports a plain success for a bulk enable and disable", () => {
    const result = {
      done: [
        { file: A, value: undefined },
        { file: B, value: undefined },
        { file: C, value: undefined },
      ],
      failed: [],
    };

    expect(bulkEnabledToast(result, true)).toEqual({
      msg: "Enabled 3 automations",
      kind: "ok",
    });
    expect(bulkEnabledToast(result, false)).toEqual({
      msg: "Disabled 3 automations",
      kind: "ok",
    });
  });
});
