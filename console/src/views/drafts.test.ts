import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeBackend } from "../fake";
import { DRAFT_INPUT_IDS, S, getDraft, mirrorDraft } from "../store";
import type { Job } from "../types";
import { vDetail } from "./detail";
import { vSettings } from "./settings";
import { vStagger } from "./stagger";

/**
 * The confirmed repaint trigger: a toast auto-dismissing after 3.2s calls
 * changed() -> render() -> the view is rebuilt from state. Anything the user
 * has typed but not mirrored into state is lost at that moment.
 */
const repaint = (): void => {
  S.toast = { msg: "saved", kind: "ok", id: 1 };
  S.toast = null;
};

const DAILY = "60-restic-daily.sh";
const INTERVAL = "80-tor-check.sh";
const OTHER = "61-restic-check.sh";

let fleet: Job[] = [];

const select = (file: string): void => {
  S.selected = file;
};

beforeEach(async () => {
  vi.stubGlobal("document", { documentElement: { dataset: {} } });
  fleet = await fakeBackend().listJobs();
  S.jobs = { data: fleet, error: null, loading: false };
  S.selected = null;
  S.drafts = {};
  S.profile = { data: null, error: null, loading: false };
  S.paths = { data: null, error: null, loading: false };
  S.pathsEdit = { error: null, busy: false, draft: null, saved: null };
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("draft inputs — the mirror", () => {
  it("mirrors typing into exactly the eight text inputs", () => {
    expect([...DRAFT_INPUT_IDS].sort()).toEqual([
      "d-every",
      "d-newtime",
      "d-timeout",
      "d-title",
      "prune-days",
      "s-end",
      "s-max",
      "s-start",
    ]);
    for (const id of DRAFT_INPUT_IDS) {
      expect(mirrorDraft(id, `typed-${id}`)).toBe(true);
      expect(getDraft(id)).toBe(`typed-${id}`);
    }
  });

  it("leaves the search fields alone", () => {
    expect(mirrorDraft("j-q", "text")).toBe(false);
    expect(mirrorDraft("log-q", "text")).toBe(false);
    expect(mirrorDraft("root-input", "/some/root")).toBe(false);
  });
});

describe("draft inputs — the detail view", () => {
  it("keeps a typed title when an unrelated repaint rebuilds the panel", () => {
    select(DAILY);
    mirrorDraft("d-title", "Restic snapshot (weekly)");

    repaint();

    expect(vDetail()).toContain('id="d-title" value="Restic snapshot (weekly)"');
  });

  it("keeps a typed timeout when an unrelated repaint rebuilds the panel", () => {
    select(DAILY);
    mirrorDraft("d-timeout", "9000");

    repaint();

    expect(vDetail()).toContain('value="9000" id="d-timeout"');
  });

  it("keeps a typed interval when an unrelated repaint rebuilds the panel", () => {
    select(INTERVAL);
    mirrorDraft("d-every", "12");

    repaint();

    expect(vDetail()).toContain('id="d-every"');
    expect(vDetail()).toContain('value="12"');
  });

  it("keeps a typed time when an unrelated repaint rebuilds the panel", () => {
    select(DAILY);
    mirrorDraft("d-newtime", "03:45");

    repaint();

    expect(vDetail()).toContain('id="d-newtime"');
    expect(vDetail()).toContain('value="03:45"');
  });

  it("scopes the detail drafts to the automation being edited", () => {
    select(DAILY);
    mirrorDraft("d-title", "Restic snapshot (weekly)");

    // Another automation must not inherit the half-typed title.
    select(OTHER);
    repaint();

    expect(vDetail()).not.toContain("Restic snapshot (weekly)");
    expect(vDetail()).toContain('id="d-title" value="Restic integrity"');
  });
});

describe("draft inputs — the settings panel", () => {
  it("keeps a typed prune window when an unrelated repaint rebuilds the panel", () => {
    mirrorDraft("prune-days", "30");

    repaint();

    expect(vSettings()).toContain('id="prune-days" value="30"');
  });
});

describe("draft inputs — the stagger modal", () => {
  const ctx = () => ({ files: [DAILY], start: 60, end: 300, max: 2, plan: null });

  it("keeps a typed window when the modal is rebuilt", () => {
    mirrorDraft("s-start", "07:15");
    mirrorDraft("s-end", "09:45");

    repaint();

    const html = vStagger(ctx());
    expect(html).toContain('id="s-start" value="07:15"');
    expect(html).toContain('id="s-end" value="09:45"');
  });

  it("keeps a typed concurrency limit when the modal is rebuilt", () => {
    mirrorDraft("s-max", "7");

    repaint();

    expect(vStagger(ctx())).toContain('id="s-max" min="1" max="20" value="7"');
  });
});
