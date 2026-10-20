import { beforeEach, describe, expect, it, vi } from "vitest";
import { S, SAVED_VIEWS, jobs, visible } from "./store";
import type { SavedView } from "./store";
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

const setJobs = (list: Job[]): void => {
  S.jobs = { data: list, error: null, loading: false };
};

beforeEach(() => {
  S.jobs = { data: null, error: null, loading: false };
  S.savedView = "all";
  S.groupFilter = null;
  S.q = "";
  S.statusF = new Set<Status>();
  S.picked = new Set<string>();
  S.groupBy = "group";
});

describe("store state", () => {
  it("starts empty", () => {
    expect(jobs()).toEqual([]);
  });

  it("returns the loaded jobs", () => {
    const list = [job({ file: "a.sh" })];
    setJobs(list);
    expect(jobs()).toHaveLength(1);
  });
});

describe("SAVED_VIEWS", () => {
  const list: Job[] = [
    job({ file: "quiet.sh", title: "Quiet one", last: run({ status: "quiet", duration_s: 10 }) }),
    job({ file: "spoke.sh", title: "Spoke", last: run({ status: "spoke", duration_s: 20 }) }),
    job({ file: "fail.sh", title: "Failed", last: run({ status: "fail", duration_s: 30 }) }),
    job({ file: "hang.sh", title: "Hung", last: run({ status: "hang", duration_s: 40 }) }),
    job({ file: "never.sh", title: "Never", last: null }),
    job({ file: "off.sh", title: "Off", enabled: false, last: run({ status: "quiet", duration_s: 50 }) }),
  ];

  it("labels every view", () => {
    expect(Object.keys(SAVED_VIEWS)).toEqual(["all", "attention", "disabled", "never", "slow", "due"]);
    for (const v of Object.values(SAVED_VIEWS)) expect(v.label).toBeTruthy();
  });

  it("All returns everything", () => {
    expect(SAVED_VIEWS.all.pick(list)).toHaveLength(6);
  });

  it("Needs attention returns only spoke, fail and hang", () => {
    const got = SAVED_VIEWS.attention.pick(list).map((j) => j.file);
    expect(got).toEqual(["spoke.sh", "fail.sh", "hang.sh"]);
  });

  it("Disabled returns the disabled jobs", () => {
    expect(SAVED_VIEWS.disabled.pick(list).map((j) => j.file)).toEqual(["off.sh"]);
  });

  it("Never run returns jobs with no recorded run, not disabled ones", () => {
    expect(SAVED_VIEWS.never.pick(list).map((j) => j.file)).toEqual(["never.sh"]);
  });

  it("Slowest returns the ten slowest enabled jobs, longest first", () => {
    const many: Job[] = Array.from({ length: 14 }, (_, i) =>
      job({ file: `j${i}.sh`, last: run({ duration_s: (i + 1) * 10 }) }),
    );
    const got = SAVED_VIEWS.slow.pick(many);
    expect(got).toHaveLength(10);
    expect(got[0].file).toBe("j13.sh");
    expect(got[9].file).toBe("j4.sh");
  });

  it("Slowest skips disabled jobs", () => {
    const got = SAVED_VIEWS.slow.pick(list).map((j) => j.file);
    expect(got).not.toContain("off.sh");
    expect(got[0]).toBe("hang.sh");
  });

  it("Due next returns the soonest scheduled jobs", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 2, 10, 0, 0, 0));
    try {
      const scheduled: Job[] = [
        job({ file: "late.sh", schedule: { mode: "daily", times: ["23:00"] } }),
        job({ file: "early.sh", schedule: { mode: "daily", times: ["00:05"] } }),
        job({ file: "manual.sh", schedule: { mode: "manual" } }),
        job({ file: "off.sh", enabled: false, schedule: { mode: "daily", times: ["00:01"] } }),
      ];
      const got = SAVED_VIEWS.due.pick(scheduled).map((j) => j.file);
      expect(got).toEqual(["early.sh", "late.sh"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("Due next is capped at ten", () => {
    const many: Job[] = Array.from({ length: 15 }, (_, i) =>
      job({
        file: `j${i}.sh`,
        schedule: { mode: "daily", times: [`${String(i).padStart(2, "0")}:00`] },
      }),
    );
    expect(SAVED_VIEWS.due.pick(many)).toHaveLength(10);
  });
});

describe("visible", () => {
  const list: Job[] = [
    job({ file: "a.sh", title: "Brew update", group: "Packages", last: run({ status: "quiet" }) }),
    job({ file: "b.sh", title: "Docker prune", group: "Developer", last: run({ status: "fail" }) }),
    job({ file: "c.sh", title: "Disk audit", group: "Packages", last: null }),
    job({ file: "d.sh", title: "Xcode clean", group: "Developer", enabled: false }),
  ];

  it("returns everything with no filters", () => {
    setJobs(list);
    expect(visible()).toHaveLength(4);
  });

  it("applies the saved view", () => {
    setJobs(list);
    S.savedView = "attention" as SavedView;
    expect(visible().map((j) => j.file)).toEqual(["b.sh"]);
  });

  it("applies the group filter", () => {
    setJobs(list);
    S.groupFilter = "Packages";
    expect(visible().map((j) => j.file)).toEqual(["a.sh", "c.sh"]);
  });

  it("applies the status filter", () => {
    setJobs(list);
    S.statusF = new Set<Status>(["none"]);
    expect(visible().map((j) => j.file)).toEqual(["c.sh"]);
  });

  it("searches title, file, label, group and the last summary", () => {
    setJobs(list);
    S.q = "docker";
    expect(visible().map((j) => j.file)).toEqual(["b.sh"]);
    S.q = "PACKAGES";
    expect(visible().map((j) => j.file)).toEqual(["a.sh", "c.sh"]);
  });

  it("combines the filters", () => {
    setJobs(list);
    S.groupFilter = "Developer";
    S.savedView = "attention" as SavedView;
    expect(visible().map((j) => j.file)).toEqual(["b.sh"]);
  });

  it("returns nothing when nothing matches", () => {
    setJobs(list);
    S.q = "no-such-automation";
    expect(visible()).toEqual([]);
  });
});
