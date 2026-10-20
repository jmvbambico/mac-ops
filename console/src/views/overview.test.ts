import { beforeEach, describe, expect, it } from "vitest";
import { fakeBackend } from "../fake";
import { esc } from "../format";
import { S } from "../store";
import type { Job } from "../types";
import { vOverview } from "./overview";

let fleet: Job[] = [];

const loadFleet = async (): Promise<Job[]> => {
  const list = await fakeBackend().listJobs();
  return list;
};

/** The same fleet, but every job ran and said nothing — the healthy state. */
const allQuiet = (list: Job[]): Job[] =>
  list.map((j) => ({
    ...j,
    enabled: true,
    last: { ...(j.last ?? { id: 1, job: j.file, started_at: 0, duration_s: 5, rc: 0, dry_run: false, summary: "rc=0", stdout: "" }), status: "quiet" as const, rc: 0 },
  }));

const count = (html: string, needle: string): number => html.split(needle).length - 1;

beforeEach(async () => {
  fleet = await loadFleet();
  S.jobs = { data: null, error: null, loading: false };
  S.fleet = { data: null, error: null, loading: false };
  S.groupFilter = null;
  S.q = "";
  S.statusF = new Set();
  S.savedView = "all";
});

describe("the fixture", () => {
  it("is 37 jobs across 6 groups", () => {
    expect(fleet).toHaveLength(37);
    expect(new Set(fleet.map((j) => j.group)).size).toBe(6);
  });
});

describe("vOverview — exception first", () => {
  it("summarises a healthy fleet instead of enumerating it", () => {
    S.jobs = { data: allQuiet(fleet), error: null, loading: false };
    const html = vOverview();

    // every job in the fleet is quiet, so the only per-job rows allowed are
    // the five "up next" ones — never one row per job.
    const rows = count(html, 'data-open="');
    expect(rows).toBeLessThan(fleet.length);
    expect(rows).toBe(5);
  });

  it("lists no attention rows when nothing needs review", () => {
    S.jobs = { data: allQuiet(fleet), error: null, loading: false };
    const html = vOverview();
    expect(html).not.toContain("Needs attention");
    expect(html).toContain("All quiet");
  });

  it("lists only the jobs that need review when there are some", () => {
    const list = allQuiet(fleet);
    list[0] = { ...list[0], last: { ...list[0].last!, status: "fail", rc: 1 } };
    S.jobs = { data: list, error: null, loading: false };
    const html = vOverview();
    expect(html).toContain("Needs attention");
    expect(html).toContain("1 of 37 automations need review");
  });

  it("counts the quiet jobs as a percentage of the fleet", () => {
    S.jobs = { data: allQuiet(fleet), error: null, loading: false };
    const html = vOverview();
    expect(html).toContain("Every automation ran and said nothing");
    expect(html).toContain('class="num">37<');
  });
});

describe("vOverview — quiet is green", () => {
  it("uses the quiet class, not a warning one, for a healthy job", () => {
    S.jobs = { data: allQuiet(fleet), error: null, loading: false };
    const html = vOverview();
    expect(html).toContain('class="dot quiet"');
    expect(html).toContain('class="badge quiet"');
    expect(html).not.toContain('class="badge fail"');
    expect(html).not.toContain('class="badge spoke"');
    expect(html).toContain('class="banner ok"');
  });

  it("warns when a job spoke", () => {
    const list = allQuiet(fleet);
    list[0] = { ...list[0], last: { ...list[0].last!, status: "spoke", rc: 0 } };
    S.jobs = { data: list, error: null, loading: false };
    const html = vOverview();
    expect(html).toContain('class="banner warn"');
    expect(html).toContain('class="badge spoke"');
  });
});

describe("vOverview — escaping", () => {
  it("escapes a job title that carries HTML", () => {
    const list = allQuiet(fleet);
    const evil = '<script>alert("x")</script>';
    list[0] = {
      ...list[0],
      title: evil,
      last: { ...list[0].last!, status: "spoke", summary: evil },
    };
    S.jobs = { data: list, error: null, loading: false };
    const html = vOverview();
    expect(html).not.toContain("<script>");
    expect(html).not.toContain('alert("x")');
    expect(html).toContain("&lt;script&gt;");
  });
});

describe("vOverview — heatmap keys", () => {
  it("keeps groups distinct when their names contain separator characters", () => {
    const list = allQuiet(fleet)
      .slice(0, 2)
      .map((j, i) => ({ ...j, group: i === 0 ? "Power:Mgmt" : "Power-Mgmt" }));
    S.jobs = { data: list, error: null, loading: false };
    S.fleet = {
      data: [
        { day: "2026-03-10", group: "Power:Mgmt", status: "fail" },
        { day: "2026-03-10", group: "Power-Mgmt", status: "quiet" },
      ],
      error: null,
      loading: false,
    };

    const html = vOverview();
    const cells = (name: string): string => {
      const row = html
        .split('<div class="fleet-row">')
        .find((r) => r.includes(`<div class="fleet-name">${esc(name)}</div>`));
      return row?.match(/<div class="fleet-cells">([\s\S]*?)<\/div>/)?.[1] ?? "";
    };

    expect(cells("Power:Mgmt")).toContain('class="fail"');
    expect(cells("Power:Mgmt")).not.toContain('class="quiet"');
    expect(cells("Power-Mgmt")).toContain('class="quiet"');
  });
});

describe("vOverview — scale", () => {
  it("keeps the 37-job fleet grouped into 6 group cards", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    const html = vOverview();
    expect(count(html, 'data-grpopen="')).toBe(6);
  });

  it("renders one group card per group, in sorted order", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    const html = vOverview();
    const cards = [...html.matchAll(/data-grpopen="([^"]*)"/g)].map((m) => m[1]);
    const names = [...new Set(fleet.map((j) => j.group))].sort((a, b) => a.localeCompare(b));
    expect(cards).toEqual(names.map(esc));
  });
});
