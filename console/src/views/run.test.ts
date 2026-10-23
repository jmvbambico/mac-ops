import { beforeEach, describe, expect, it } from "vitest";
import { FakeRunLogBus, fakeBackend } from "../fake";
import { S } from "../store";
import type { Job, RunLogBatch } from "../types";
import {
  MAX_RUN_LOG_LINES,
  applyRunLogBatch,
  applyRunOutput,
  newRunCtx,
  vRunModal,
} from "./run";
import type { RunCtx } from "./run";

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

describe("run log retention cap", () => {
  it("keeps the newest lines and drops the oldest past the cap", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    const first = Array.from({ length: MAX_RUN_LOG_LINES }, (_, i) => `line ${i}`);
    applyRunLogBatch(ctx, { job: FILE, lines: first });
    expect(ctx.lines).toHaveLength(MAX_RUN_LOG_LINES);
    expect(ctx.dropped).toBe(0);

    applyRunLogBatch(ctx, { job: FILE, lines: ["newest"] });
    expect(ctx.lines).toHaveLength(MAX_RUN_LOG_LINES);
    expect(ctx.lines[MAX_RUN_LOG_LINES - 1]).toBe("newest");
    // The oldest line was dropped to make room.
    expect(ctx.lines[0]).toBe("line 1");
    expect(ctx.dropped).toBe(1);
  });

  it("shows a notice naming the durable log when lines were dropped", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    applyRunLogBatch(ctx, {
      job: FILE,
      lines: Array.from({ length: MAX_RUN_LOG_LINES + 3 }, (_, i) => `line ${i}`),
    });

    const html = vRunModal(ctx, job());
    expect(html).toContain("3 earlier lines not shown");
    expect(html).toContain("logs/machine-ops.log");
  });

  it("keeps the dropped count correct across several over-cap batches", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    const chunk = Array.from({ length: MAX_RUN_LOG_LINES }, (_, i) => `chunk ${i}`);
    applyRunLogBatch(ctx, { job: FILE, lines: chunk });
    expect(ctx.dropped).toBe(0);

    applyRunLogBatch(ctx, { job: FILE, lines: chunk });
    expect(ctx.dropped).toBe(MAX_RUN_LOG_LINES);
    expect(ctx.lines).toHaveLength(MAX_RUN_LOG_LINES);

    applyRunLogBatch(ctx, { job: FILE, lines: ["tail"] });
    expect(ctx.dropped).toBe(MAX_RUN_LOG_LINES + 1);

    const html = vRunModal(ctx, job());
    expect(html).toContain(`${MAX_RUN_LOG_LINES + 1} earlier lines not shown`);
    // The notice must not compound or be counted as a dropped line.
    expect(html).not.toContain(`${MAX_RUN_LOG_LINES + 2} earlier lines not shown`);
  });

  it("shows no notice at all while under the cap", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;

    applyRunLogBatch(ctx, {
      job: FILE,
      lines: Array.from({ length: MAX_RUN_LOG_LINES }, (_, i) => `line ${i}`),
    });

    const html = vRunModal(ctx, job());
    expect(html).not.toContain("earlier lines not shown");
  });
});

describe("the run modal's close button does not pretend to cancel", () => {
  it("says Cancel before the run starts", () => {
    const ctx = newRunCtx(FILE, false);
    const html = vRunModal(ctx, job());
    expect(html).toContain(">Cancel</button>");
    expect(html).not.toContain(">Close</button>");
    expect(html).not.toContain("continues in the background");
  });

  it("says Close while the run is in flight and says the run continues", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = true;
    const html = vRunModal(ctx, job());
    expect(html).toContain(">Close</button>");
    expect(html).not.toContain(">Cancel</button>");
    expect(html).not.toContain(">Done</button>");
    expect(html).toContain("This run continues in the background.");
  });

  it("says Done once there is an outcome and drops the background note", () => {
    const ctx = newRunCtx(FILE, false);
    ctx.busy = false;
    ctx.outcome = {
      status: "quiet",
      rc: 0,
      duration_s: 3,
      stdout: "",
      stderr: "",
      timed_out: false,
    };
    const html = vRunModal(ctx, job());
    expect(html).toContain(">Done</button>");
    expect(html).not.toContain(">Cancel</button>");
    expect(html).not.toContain(">Close</button>");
    expect(html).not.toContain("continues in the background");
  });
});

describe("Backend.subscribeRunLog seam", () => {  it("delivers batches through the fake and unsubscribes", async () => {
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

describe("captured per-job output", () => {
  const finished = (): RunCtx => {
    const ctx = newRunCtx(FILE, false);
    ctx.outcome = {
      status: "quiet",
      rc: 0,
      duration_s: 91,
      stdout: "",
      stderr: "",
      timed_out: false,
    };
    return ctx;
  };

  it("renders the captured report the one-line summaries hide", () => {
    const ctx = finished();
    const report = "== mole clean ==\n  -> removed 1.2G of caches\n  -> emptied Trash";
    expect(applyRunOutput(ctx, report)).toBe(true);

    const html = vRunModal(ctx, job());
    expect(html).toContain("removed 1.2G of caches");
    // Real output exists, so the honest-but-useless fallback must give way.
    expect(html).not.toContain("silent, which is success");
  });

  it("escapes captured output like the streamed lines", () => {
    const ctx = finished();
    applyRunOutput(ctx, "<script>alert('x')</script>");
    const html = vRunModal(ctx, job());
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("falls back cleanly when the job has not opted in", () => {
    const ctx = finished();
    expect(applyRunOutput(ctx, "")).toBe(false);
    expect(vRunModal(ctx, job())).toContain("silent, which is success");
  });

  it("keeps the live streamed lines when captured output is added", () => {
    const ctx = finished();
    applyRunLogBatch(ctx, { job: FILE, lines: ["streamed detail"] });
    applyRunOutput(ctx, "captured report body");

    const html = vRunModal(ctx, job());
    expect(html).toContain('data-run-stream="1"');
    expect(html).toContain("streamed detail");
    expect(html).toContain("captured report body");
  });

  it("reads through the fake backend, empty for a never-run job", async () => {
    const backend = fakeBackend();
    expect(await backend.jobOutput(FILE)).toContain("mole-clean");
    expect(await backend.jobOutput("73-ledger-sync.sh")).toBe("");
  });
});
