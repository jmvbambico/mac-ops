import { describe, expect, it } from "vitest";
import {
  DAYS,
  counts,
  esc,
  fmtDur,
  groupsOf,
  hhmm,
  localLoadProfile,
  needsAttention,
  nextRun,
  parseHHMM,
  sbar,
  scheduleStarts,
  staggerPlan,
  untilStr,
} from "./format";
import type { Job, Run } from "./types";

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

describe("esc", () => {
  it("escapes the HTML metacharacters that matter in attributes and text", () => {
    expect(esc('<script>alert("x")</script>')).toBe("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;");
    expect(esc("a & b")).toBe("a &amp; b");
    expect(esc("<>&\"")).toBe("&lt;&gt;&amp;&quot;");
  });

  it("escapes single quotes for defence in depth", () => {
    expect(esc("it's")).toBe("it&#39;s");
    expect(esc("' onmouseover='x")).toBe("&#39; onmouseover=&#39;x");
  });

  it("stringifies non-strings", () => {
    expect(esc(42)).toBe("42");
    expect(esc(null)).toBe("null");
    expect(esc(undefined)).toBe("undefined");
  });
});

describe("fmtDur", () => {
  it("renders seconds, minutes and hours", () => {
    expect(fmtDur(0)).toBe("0s");
    expect(fmtDur(59)).toBe("59s");
    expect(fmtDur(60)).toBe("1m 0s");
    expect(fmtDur(61)).toBe("1m 1s");
    expect(fmtDur(3599)).toBe("59m 59s");
    expect(fmtDur(3600)).toBe("1h 0m");
    expect(fmtDur(3660)).toBe("1h 1m");
    expect(fmtDur(7200)).toBe("2h 0m");
  });
});

describe("hhmm / parseHHMM", () => {
  it("formats a minute-of-day as HH:MM", () => {
    expect(hhmm(0)).toBe("00:00");
    expect(hhmm(120)).toBe("02:00");
    expect(hhmm(605)).toBe("10:05");
    expect(hhmm(1439)).toBe("23:59");
  });

  it("wraps hours past 24 back into the day", () => {
    expect(hhmm(1440)).toBe("00:00");
    expect(hhmm(1500)).toBe("01:00");
  });

  it("parses valid HH:MM input", () => {
    expect(parseHHMM("00:00")).toBe(0);
    expect(parseHHMM("02:00")).toBe(120);
    expect(parseHHMM("23:59")).toBe(1439);
    expect(parseHHMM("7:05")).toBe(425);
    expect(parseHHMM("  7:05  ")).toBe(425);
  });

  it("rejects invalid HH:MM input", () => {
    expect(parseHHMM("24:00")).toBeNull();
    expect(parseHHMM("12:60")).toBeNull();
    expect(parseHHMM("12:5")).toBeNull();
    expect(parseHHMM("1200")).toBeNull();
    expect(parseHHMM("ab:cd")).toBeNull();
    expect(parseHHMM("")).toBeNull();
    expect(parseHHMM("-1:00")).toBeNull();
  });

  it("round-trips every minute of the day", () => {
    for (let m = 0; m < 1440; m++) expect(parseHHMM(hhmm(m))).toBe(m);
  });
});

describe("scheduleStarts", () => {
  it("returns nothing for a manual schedule", () => {
    expect(scheduleStarts({ mode: "manual" })).toEqual([]);
  });

  it("returns the daily start minutes", () => {
    expect(scheduleStarts({ mode: "daily", times: ["02:00", "14:30"] })).toEqual([120, 870]);
  });

  it("drops invalid times", () => {
    expect(scheduleStarts({ mode: "daily", times: ["02:00", "99:99"] })).toEqual([120]);
  });

  it("spaces an interval schedule across the day", () => {
    expect(scheduleStarts({ mode: "interval", every_hours: 6 })).toEqual([0, 360, 720, 1080]);
  });

  it("never divides by zero on a zero interval", () => {
    const starts = scheduleStarts({ mode: "interval", every_hours: 0 });
    expect(starts).toHaveLength(24);
    expect(starts[0]).toBe(0);
  });
});

describe("nextRun", () => {
  it("is null for a disabled job", () => {
    const j = job({ enabled: false, schedule: { mode: "daily", times: ["02:00"] } });
    expect(nextRun(j, new Date(2026, 2, 10, 1, 0))).toBeNull();
  });

  it("is null for a manual job", () => {
    expect(nextRun(job({ schedule: { mode: "manual" } }), new Date(2026, 2, 10, 1, 0))).toBeNull();
  });

  it("is null when no time in the schedule is valid", () => {
    const j = job({ schedule: { mode: "daily", times: ["99:99"] } });
    expect(nextRun(j, new Date(2026, 2, 10, 1, 0))).toBeNull();
  });

  it("picks the next daily time today when it is still ahead", () => {
    const now = new Date(2026, 2, 10, 1, 0);
    const d = nextRun(job({ schedule: { mode: "daily", times: ["02:00"] } }), now) as Date;
    expect(d.getDate()).toBe(10);
    expect(d.getHours()).toBe(2);
    expect(d.getMinutes()).toBe(0);
  });

  it("rolls to tomorrow when today's time has passed", () => {
    const now = new Date(2026, 2, 10, 3, 0);
    const d = nextRun(job({ schedule: { mode: "daily", times: ["02:00"] } }), now) as Date;
    expect(d.getDate()).toBe(11);
    expect(d.getHours()).toBe(2);
  });

  it("wraps over midnight for a 00:00 schedule", () => {
    const now = new Date(2026, 2, 10, 23, 59);
    const d = nextRun(job({ schedule: { mode: "daily", times: ["00:00"] } }), now) as Date;
    expect(d.getDate()).toBe(11);
    expect(d.getHours()).toBe(0);
    expect(d.getMinutes()).toBe(0);
  });

  it("treats the current minute as passed, not due", () => {
    const now = new Date(2026, 2, 10, 2, 0, 0, 0);
    const d = nextRun(job({ schedule: { mode: "daily", times: ["02:00"] } }), now) as Date;
    expect(d.getDate()).toBe(11);
  });

  it("chooses the earliest of several times", () => {
    const now = new Date(2026, 2, 10, 3, 0);
    const d = nextRun(job({ schedule: { mode: "daily", times: ["02:00", "14:30"] } }), now) as Date;
    expect(d.getHours()).toBe(14);
    expect(d.getMinutes()).toBe(30);
  });

  it("honours the weekly day set", () => {
    const now = new Date(2026, 2, 8, 23, 0); // a Sunday
    const j = job({ schedule: { mode: "weekly", days: ["Mon"], times: ["02:00"] } });
    const d = nextRun(j, now) as Date;
    expect(DAYS[(d.getDay() + 6) % 7]).toBe("Mon");
    expect(d.getTime()).toBeGreaterThan(now.getTime());
    expect(d.getTime()).toBeLessThanOrEqual(now.getTime() + 7 * 86400000);
  });

  it("adds hours for an interval schedule", () => {
    const now = new Date(2026, 2, 10, 1, 0);
    const d = nextRun(job({ schedule: { mode: "interval", every_hours: 6 } }), now) as Date;
    expect(d.getTime() - now.getTime()).toBe(6 * 3600 * 1000);
  });

  it("never returns a time in the past", () => {
    const now = new Date(2026, 2, 10, 12, 34, 56);
    const j = job({ schedule: { mode: "daily", times: ["00:01", "12:34", "23:59"] } });
    const d = nextRun(j, now) as Date;
    expect(d.getTime()).toBeGreaterThan(now.getTime());
  });
});

describe("untilStr", () => {
  const now = new Date(2026, 2, 10, 12, 0);
  it("renders a dash for no date", () => {
    expect(untilStr(null, now)).toBe("—");
  });
  it("renders minutes and hours", () => {
    expect(untilStr(new Date(now.getTime() + 30 * 60000), now)).toBe("in 30m");
    expect(untilStr(new Date(now.getTime() + 90 * 60000), now)).toBe("in 1h 30m");
  });
  it("renders days for distant dates", () => {
    expect(untilStr(new Date(now.getTime() + 30 * 3600 * 1000), now)).toBe("in 1d 6h");
  });
});

describe("counts / sbar", () => {
  it("counts by rendered status", () => {
    const list = [
      job({ file: "a.sh", last: run({ status: "quiet" }) }),
      job({ file: "b.sh", last: run({ status: "quiet" }) }),
      job({ file: "c.sh", last: run({ status: "fail" }) }),
      job({ file: "d.sh", enabled: false, last: run({ status: "fail" }) }),
      job({ file: "e.sh", last: null }),
    ];
    expect(counts(list)).toEqual({ quiet: 2, fail: 1, off: 1, none: 1 });
  });

  it("counts an empty fleet as nothing", () => {
    expect(counts([])).toEqual({});
  });

  it("renders a status bar with one slice per present status", () => {
    const html = sbar([job({ file: "a.sh", last: run({ status: "quiet" }) })]);
    expect(html).toContain('class="sbar"');
    expect(html).toContain('class="quiet"');
    expect(html).not.toContain('class="fail"');
  });

  it("does not divide by zero on an empty fleet", () => {
    expect(sbar([])).toBe('<div class="sbar"></div>');
  });
});

describe("needsAttention", () => {
  it("is true for spoke, fail and hang", () => {
    for (const status of ["spoke", "fail", "hang"] as const) {
      expect(needsAttention(job({ last: run({ status }) }))).toBe(true);
    }
  });

  it("is false for quiet, none and off", () => {
    expect(needsAttention(job({ last: run({ status: "quiet" }) }))).toBe(false);
    expect(needsAttention(job({ last: null }))).toBe(false);
    expect(needsAttention(job({ enabled: false, last: run({ status: "fail" }) }))).toBe(false);
  });
});

describe("localLoadProfile", () => {
  it("is a full day of zeroes for an empty fleet", () => {
    const p = localLoadProfile([]);
    expect(p).toHaveLength(1440);
    expect(p.every((n) => n === 0)).toBe(true);
  });

  it("counts one minute for a short daily job", () => {
    const p = localLoadProfile([job({ last: run({ duration_s: 18 }) })]);
    expect(p[120]).toBe(1);
    expect(p[121]).toBe(0);
  });

  it("spans the rounded-up duration of a long job", () => {
    const p = localLoadProfile([job({ last: run({ duration_s: 90 }) })]);
    expect(p[120]).toBe(1);
    expect(p[121]).toBe(1);
    expect(p[122]).toBe(0);
  });

  it("ignores disabled jobs", () => {
    const p = localLoadProfile([job({ enabled: false, last: run({ duration_s: 90 }) })]);
    expect(p.every((n) => n === 0)).toBe(true);
  });

  it("counts every start of an interval schedule", () => {
    const p = localLoadProfile([
      job({ schedule: { mode: "interval", every_hours: 6 }, last: run({ duration_s: 60 }) }),
    ]);
    expect(p[0]).toBe(1);
    expect(p[360]).toBe(1);
    expect(p[720]).toBe(1);
    expect(p[1080]).toBe(1);
  });
});

describe("groupsOf", () => {
  it("returns nothing for an empty fleet", () => {
    expect(groupsOf([])).toEqual([]);
  });

  it("groups by name, sorted, keeping the input order inside a group", () => {
    const list = [
      job({ file: "b.sh", group: "Beta", kind: "sh" }),
      job({ file: "a.sh", group: "Alpha", kind: "sh" }),
      job({ file: "c.sh", group: "Beta", kind: "py" }),
    ];
    const g = groupsOf(list);
    expect(g.map((x) => x.name)).toEqual(["Alpha", "Beta"]);
    expect(g[1].jobs.map((j) => j.file)).toEqual(["b.sh", "c.sh"]);
  });

  it("picks the most common kind as the group icon", () => {
    const list = [
      job({ file: "a.sh", group: "G", kind: "py" }),
      job({ file: "b.sh", group: "G", kind: "sh" }),
      job({ file: "c.sh", group: "G", kind: "sh" }),
    ];
    expect(groupsOf(list)[0].icon).toBe("sh");
  });
});

describe("staggerPlan", () => {
  const j = (file: string, duration: number): Job =>
    job({ file, last: run({ duration_s: duration }) });

  it("puts the longest job first", () => {
    const plan = staggerPlan([j("short.sh", 10), j("long.sh", 500)], 120, 300, 1);
    expect(plan.get("long.sh")).toBe(120);
    expect(plan.get("short.sh")).toBe(210);
  });

  it("keeps at most maxConcurrent jobs in a slot", () => {
    const jobs = [j("a.sh", 10), j("b.sh", 20), j("c.sh", 30), j("d.sh", 40)];
    const plan = staggerPlan(jobs, 0, 60, 2);
    expect(plan.get("d.sh")).toBe(0);
    expect(plan.get("c.sh")).toBe(0);
    expect(plan.get("b.sh")).toBe(30);
    expect(plan.get("a.sh")).toBe(30);
  });

  it("spreads over the window", () => {
    const jobs = [j("a.sh", 10), j("b.sh", 20)];
    const plan = staggerPlan(jobs, 100, 160, 1);
    expect(plan.get("b.sh")).toBe(100);
    expect(plan.get("a.sh")).toBe(130);
  });

  it("packs slots into shared minutes instead of overrunning a narrow window", () => {
    const jobs = [j("a.sh", 10), j("b.sh", 20), j("c.sh", 30), j("d.sh", 40)];
    const plan = staggerPlan(jobs, 0, 2, 1);
    expect(plan.get("d.sh")).toBe(0);
    expect(plan.get("c.sh")).toBe(0);
    expect(plan.get("b.sh")).toBe(1);
    expect(plan.get("a.sh")).toBe(1);
    for (const start of plan.values()) expect(start).toBeGreaterThanOrEqual(0);
    for (const start of plan.values()) expect(start).toBeLessThan(2);
  });

  it("keeps all 40 starts inside a 30-minute window", () => {
    const jobs = Array.from({ length: 40 }, (_, i) => j(`${String(i).padStart(2, "0")}.sh`, 100 - i));
    const plan = staggerPlan(jobs, 120, 150, 1);
    expect(plan.size).toBe(40);
    for (const start of plan.values()) {
      expect(start).toBeGreaterThanOrEqual(120);
      expect(start).toBeLessThan(150);
    }
    expect(Math.max(...plan.values())).toBe(149);
  });

  it("gives every slot its own minute when the window is exactly as wide", () => {
    const jobs = ["a.sh", "b.sh", "c.sh", "d.sh", "e.sh"].map((f, i) => j(f, 50 - i));
    const plan = staggerPlan(jobs, 120, 125, 1);
    expect([...plan.values()].sort((x, y) => x - y)).toEqual([120, 121, 122, 123, 124]);
  });

  it("starts a single job at the beginning of the window", () => {
    const plan = staggerPlan([j("a.sh", 10)], 120, 150, 1);
    expect(plan.get("a.sh")).toBe(120);
  });

  it("collapses an empty window to a single minute", () => {
    const jobs = [j("a.sh", 10), j("b.sh", 20), j("c.sh", 30)];
    const plan = staggerPlan(jobs, 300, 300, 1);
    for (const start of plan.values()) expect(start).toBe(300);
  });

  it("returns an empty plan for no jobs", () => {
    expect(staggerPlan([], 0, 60, 2).size).toBe(0);
  });

  it("does not divide by zero on a zero concurrency", () => {
    const plan = staggerPlan([j("a.sh", 10)], 0, 60, 0);
    expect(plan.get("a.sh")).toBe(0);
  });

  it("wraps a plan that runs past midnight into the day", () => {
    const plan = staggerPlan([j("a.sh", 10)], 1430, 1440, 1);
    expect(plan.get("a.sh")).toBe(1430);
  });
});
