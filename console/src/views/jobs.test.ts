import { beforeEach, describe, expect, it } from "vitest";
import { fakeBackend } from "../fake";
import { S } from "../store";
import type { Job } from "../types";
import { vJobs } from "./jobs";

let fleet: Job[] = [];

const count = (html: string, needle: string): number => html.split(needle).length - 1;

beforeEach(async () => {
  fleet = await fakeBackend().listJobs();
  S.jobs = { data: null, error: null, loading: false };
  S.q = "";
  S.statusF = new Set();
  S.picked = new Set();
  S.groupFilter = null;
  S.savedView = "all";
  S.groupBy = "group";
});

describe("vJobs — grouping", () => {
  it("keeps the 37-job fleet grouped into 6 group headers", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    const html = vJobs();
    expect(count(html, 'class="gh-row"')).toBe(6);
  });

  it("renders one row per job", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    const html = vJobs();
    expect(count(html, 'data-open="')).toBe(37);
  });

  it("flattens the fleet on request", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    S.groupBy = "flat";
    const html = vJobs();
    expect(count(html, 'class="gh-row"')).toBe(0);
    expect(count(html, 'data-open="')).toBe(37);
  });

  it("reports how many of the fleet it is showing", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    expect(vJobs()).toContain("Showing 37 of 37");
  });

  it("shows an empty state when nothing matches", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    S.q = "no-such-automation";
    const html = vJobs();
    expect(html).toContain("No automations match");
    expect(count(html, 'data-open="')).toBe(0);
  });
});

describe("vJobs — quiet is green", () => {
  it("badges a quiet job with the quiet class", () => {
    S.jobs = { data: fleet, error: null, loading: false };
    const html = vJobs();
    expect(html).toContain('<span class="badge quiet">Quiet</span>');
    expect(html).toContain('<span class="dot quiet"');
  });

  it("badges a failed job with the fail class, not the quiet one", () => {
    const list = fleet.map((j) => ({ ...j }));
    const target = list.find((j) => j.last?.status === "fail")!;
    target.last = { ...target.last!, status: "fail" };
    S.jobs = { data: list, error: null, loading: false };
    const html = vJobs();
    expect(html).toContain('<span class="badge fail">Failed</span>');
  });
});

describe("vJobs — escaping", () => {
  it("escapes a job title carrying HTML", () => {
    const list = fleet.map((j) => ({ ...j }));
    const evil = '<script>alert("x")</script>';
    list[0] = { ...list[0], title: evil };
    S.jobs = { data: list, error: null, loading: false };
    const html = vJobs();
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain("&quot;x&quot;");
  });

  it("escapes a job file name in the row attributes", () => {
    const list = fleet.map((j) => ({ ...j }));
    list[0] = { ...list[0], file: 'a"b<c.sh' };
    S.jobs = { data: list, error: null, loading: false };
    const html = vJobs();
    expect(html).not.toContain('a"b<c.sh');
    expect(html).toContain("&quot;");
  });
});
