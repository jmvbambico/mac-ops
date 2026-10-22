import { beforeEach, describe, expect, it } from "vitest";
import { FakeRunLogBus, fakeBackend } from "../fake";
import { S } from "../store";
import type { Job, RunLogBatch } from "../types";
import { applyRunLogBatch, newRunCtx, vRunModal } from "./run";

const FILE = "11-mole-clean.sh";
let fleet: Job[] = [];
const job = (): Job => fleet.find((j) => j.file === FILE)!;

beforeEach(async () => {
  fleet = await fakeBackend().listJobs();
  S.jobs = { data: fleet, error: null, loading: false };
});

describe("on-demand run — live log stream", () => {
  it("renders streamed log lines live instead of the silent fallback", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    // Before any detail arrives the modal says it is waiting, not that the
    // run was silent — it is still in flight.
    expect(vRunModal(ctx, job())).toContain("waiting for log output");

    const changed = applyRunLogBatch(ctx, {
      job: FILE,
      lines: ["2026-10-22 19:20:27 [mole-clean] reclaimed=unknown items=8"],
    });
    expect(changed).toBe(true);

    const html = vRunModal(ctx, job());
    expect(html).toContain("reclaimed=unknown items=8");
    // Detail exists, so the honest-but-useless fallback must NOT be the body.
    expect(html).not.toContain("silent, which is success");
    expect(html).toContain('data-run-stream="1"');
  });

  it("shows a running indicator and clears it on completion, keeping the lines", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    let html = vRunModal(ctx, job());
    expect(html).toContain('data-running="1"');
    expect(html).toContain("Running");

    applyRunLogBatch(ctx, { job: FILE, lines: ["detail line from the log"] });
    ctx.busy = false;
    ctx.outcome = {
      status: "quiet",
      rc: 0,
      duration_s: 105,
      stdout: "",
      stderr: "",
      timed_out: false,
    };

    html = vRunModal(ctx, job());
    expect(html).toContain('data-running="0"');
    expect(html).not.toContain("Running…");
    expect(html).not.toContain('<span class="badge running">');
    expect(html).toContain("detail line from the log");
    expect(html).toContain("rc=0");
    expect(html).toContain("Quiet");
  });

  it("ignores a batch for a different job file", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    const changed = applyRunLogBatch(ctx, {
      job: "20-some-other-job.sh",
      lines: ["this belongs to another run"],
    });

    expect(changed).toBe(false);
    expect(ctx.lines).toEqual([]);
    expect(vRunModal(ctx, job())).not.toContain("this belongs to another run");
  });

  it("keeps the silent-success message when the log and stdout are empty", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.outcome = {
      status: "quiet",
      rc: 0,
      duration_s: 3,
      stdout: "",
      stderr: "",
      timed_out: false,
    };
    expect(vRunModal(ctx, job())).toContain("silent, which is success");
  });

  it("shows the child's late stdout alongside the streamed log detail", () => {
    const ctx = newRunCtx(FILE, false);
    applyRunLogBatch(ctx, { job: FILE, lines: ["log detail"] });
    ctx.outcome = {
      status: "spoke",
      rc: 0,
      duration_s: 4,
      stdout: "== mole-clean ==\nspoke",
      stderr: "",
      timed_out: false,
    };
    const html = vRunModal(ctx, job());
    expect(html).toContain("log detail");
    expect(html).toContain("spoke");
  });
});

describe("Backend.subscribeRunLog seam", () => {
  it("delivers batches through the fake and unsubscribes", async () => {
    const bus = new FakeRunLogBus();
    const backend = fakeBackend({ runLog: bus });
    const seen: RunLogBatch[] = [];

    const unsubscribe = await backend.subscribeRunLog((batch) => seen.push(batch));
    bus.emit({ job: FILE, lines: ["one"] });
    expect(seen).toEqual([{ job: FILE, lines: ["one"] }]);

    unsubscribe();
    expect(bus.size).toBe(0);
    bus.emit({ job: FILE, lines: ["two"] });
    expect(seen).toHaveLength(1);
  });
});
