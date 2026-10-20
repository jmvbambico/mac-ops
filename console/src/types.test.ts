import { describe, expect, it } from "vitest";
import { ATTN, STATUSES, STATUS_META, lastDuration, lastSummary, statusOf } from "./types";
import type { Job, Run, Status } from "./types";

const run = (over: Partial<Run> = {}): Run => ({
  id: 1,
  job: "10-x.sh",
  started_at: 0,
  duration_s: 0,
  rc: 0,
  status: "quiet",
  dry_run: false,
  summary: "",
  stdout: "",
  ...over,
});

const job = (over: Partial<Job> = {}): Job => ({
  file: "10-x.sh",
  label: "x",
  title: "X",
  group: "System",
  kind: "sh",
  tier: "daily",
  timeout: 900,
  enabled: true,
  schedule: { mode: "daily", times: ["02:00"] },
  last: null,
  ...over,
});

describe("statusOf", () => {
  it("renders a job that printed nothing and exited 0 as quiet", () => {
    const j = job({ last: run({ status: "quiet", rc: 0, stdout: "" }) });
    expect(statusOf(j)).toBe("quiet");
    expect(statusOf(j)).not.toBe("fail");
    expect(statusOf(j)).not.toBe("none");
    expect(statusOf(j)).not.toBe("spoke");
  });

  it("renders an enabled job with no run as none", () => {
    expect(statusOf(job({ last: null }))).toBe("none");
  });

  it("renders a disabled job as off, whatever its last run was", () => {
    expect(statusOf(job({ enabled: false, last: null }))).toBe("off");
    expect(statusOf(job({ enabled: false, last: run({ status: "fail" }) }))).toBe("off");
    expect(statusOf(job({ enabled: false, last: run({ status: "quiet" }) }))).toBe("off");
  });

  it("passes the recorded status through", () => {
    for (const status of ["spoke", "fail", "hang"] as const) {
      expect(statusOf(job({ last: run({ status }) }))).toBe(status);
    }
  });

  it("never returns a status outside the vocabulary", () => {
    const all: Status[] = [...STATUSES, "off", "none"];
    for (const status of all) {
      expect(STATUSES).toContain(statusOf(job({ last: run({ status }) })));
    }
  });
});

describe("ATTN / STATUS_META", () => {
  it("treats only spoke, fail and hang as needing review", () => {
    expect(ATTN).toEqual(["spoke", "fail", "hang"]);
    expect(ATTN).not.toContain("quiet");
    expect(ATTN).not.toContain("none");
    expect(ATTN).not.toContain("off");
  });

  it("describes quiet as the healthy state", () => {
    expect(STATUS_META.quiet.t).toBe("Quiet");
    expect(STATUS_META.quiet.d).toMatch(/healthy/i);
  });
});

describe("lastDuration", () => {
  it("is zero when the job has never run", () => {
    expect(lastDuration(job({ last: null }))).toBe(0);
  });

  it("is the duration of the last run", () => {
    expect(lastDuration(job({ last: run({ duration_s: 142 }) }))).toBe(142);
    expect(lastDuration(job({ last: run({ duration_s: 0 }) }))).toBe(0);
  });
});

describe("lastSummary", () => {
  it("says never run for an enabled job with no run", () => {
    expect(lastSummary(job({ last: null }))).toBe("never run");
  });

  it("says disabled for a disabled job with no run", () => {
    expect(lastSummary(job({ enabled: false, last: null }))).toBe("disabled");
  });

  it("calls an empty summary a silent success", () => {
    expect(lastSummary(job({ last: run({ summary: "" }) }))).toBe("(no output — silent, which is success)");
  });

  it("passes a real summary through", () => {
    expect(lastSummary(job({ last: run({ summary: "rotated=2 rc=0" }) }))).toBe("rotated=2 rc=0");
  });
});
