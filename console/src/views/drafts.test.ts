import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../api";
import { fakeBackend } from "../fake";
import {
  DRAFT_INPUT_IDS,
  S,
  addTime,
  applyTimeout,
  applyTitle,
  clearStaggerDrafts,
  getDraft,
  mirrorDraft,
  pruneRuns,
  setEvery,
} from "../store";
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

/**
 * A committed action consumes the field it reads. The draft must go when
 * the command succeeded — otherwise the field still holds the value that was
 * just added, and the next click adds it twice — and must STAY when the
 * command was rejected, so the user can correct the text.
 */
describe("draft inputs — a committed action", () => {
  beforeEach(() => {
    setBackend(fakeBackend());
  });

  it("clears the #d-newtime draft after a successful add", async () => {
    select(DAILY);
    mirrorDraft("d-newtime", "03:45");

    const ok = await addTime("03:45");

    expect(ok).toBe(true);
    expect(getDraft("d-newtime")).toBeUndefined();
    // The field renders empty, and the time really landed on the schedule.
    expect(vDetail()).toContain('id="d-newtime"');
    expect(vDetail()).not.toContain('value="03:45"');
    expect(S.jobs.data?.find((j) => j.file === DAILY)?.schedule).toEqual({
      mode: "daily",
      times: ["01:00", "03:45"],
    });
  });

  it("keeps the #d-newtime draft when the time is rejected", async () => {
    select(DAILY);
    mirrorDraft("d-newtime", "25:99");

    const ok = await addTime("25:99");

    expect(ok).toBe(false);
    expect(getDraft("d-newtime")).toBe("25:99");
    expect(vDetail()).toContain('value="25:99"');
  });

  it("keeps the #d-newtime draft when the backend refuses the write", async () => {
    setBackend({
      ...fakeBackend(),
      setSchedule: async () => {
        throw new Error("write refused");
      },
    });
    select(DAILY);
    mirrorDraft("d-newtime", "03:45");

    const ok = await addTime("03:45");

    expect(ok).toBe(false);
    expect(getDraft("d-newtime")).toBe("03:45");
    expect(vDetail()).toContain('value="03:45"');
  });

  it("clears the #d-timeout draft after a successful apply", async () => {
    select(DAILY);
    mirrorDraft("d-timeout", "1800");

    const ok = await applyTimeout("1800");

    expect(ok).toBe(true);
    expect(getDraft("d-timeout")).toBeUndefined();
    // The field falls back to the persisted timeout — what it showed before
    // the drafts existed.
    expect(vDetail()).toContain('value="1800" id="d-timeout"');
  });

  it("keeps the #d-timeout draft when the apply fails", async () => {
    setBackend({
      ...fakeBackend(),
      setTimeout: async () => {
        throw new Error("write refused");
      },
    });
    select(DAILY);
    mirrorDraft("d-timeout", "1800");

    const ok = await applyTimeout("1800");

    expect(ok).toBe(false);
    expect(getDraft("d-timeout")).toBe("1800");
  });

  it("clears the #d-title draft after a successful rename", async () => {
    select(DAILY);
    mirrorDraft("d-title", "Restic snapshot (weekly)");

    const ok = await applyTitle("Restic snapshot (weekly)");

    expect(ok).toBe(true);
    expect(getDraft("d-title")).toBeUndefined();
    // The field falls back to the persisted title.
    expect(vDetail()).toContain('id="d-title" value="Restic snapshot (weekly)"');
  });

  it("keeps the #d-title draft when the rename fails", async () => {
    setBackend({
      ...fakeBackend(),
      setMeta: async () => {
        throw new Error("write refused");
      },
    });
    select(DAILY);
    mirrorDraft("d-title", "Restic snapshot (weekly)");

    const ok = await applyTitle("Restic snapshot (weekly)");

    expect(ok).toBe(false);
    expect(getDraft("d-title")).toBe("Restic snapshot (weekly)");
  });

  it("keeps the #d-title draft when the title is empty", async () => {
    select(DAILY);
    mirrorDraft("d-title", "  ");

    const ok = await applyTitle("  ");

    expect(ok).toBe(false);
    expect(getDraft("d-title")).toBe("  ");
  });

  it("clears the #d-every draft after a successful apply", async () => {
    select(INTERVAL);
    mirrorDraft("d-every", "12");

    const ok = await setEvery("12");

    expect(ok).toBe(true);
    expect(getDraft("d-every")).toBeUndefined();
    expect(vDetail()).toContain('id="d-every"');
    expect(vDetail()).toContain('value="12"');
  });

  it("keeps the #d-every draft when the apply fails", async () => {
    setBackend({
      ...fakeBackend(),
      setSchedule: async () => {
        throw new Error("write refused");
      },
    });
    select(INTERVAL);
    mirrorDraft("d-every", "12");

    const ok = await setEvery("12");

    expect(ok).toBe(false);
    expect(getDraft("d-every")).toBe("12");
  });

  it("clears the #prune-days draft after a successful prune", async () => {
    mirrorDraft("prune-days", "30");

    const ok = await pruneRuns("30");

    expect(ok).toBe(true);
    expect(getDraft("prune-days")).toBeUndefined();
    // The field falls back to the default window, which is what it showed
    // before the drafts existed.
    expect(vSettings()).toContain('id="prune-days" value="90"');
  });

  it("keeps the #prune-days draft when the prune fails", async () => {
    setBackend({
      ...fakeBackend(),
      pruneRuns: async () => {
        throw new Error("write refused");
      },
    });
    mirrorDraft("prune-days", "30");

    const ok = await pruneRuns("30");

    expect(ok).toBe(false);
    expect(getDraft("prune-days")).toBe("30");
  });

  it("starts a fresh stagger modal from the defaults", () => {
    mirrorDraft("s-start", "07:15");
    mirrorDraft("s-end", "09:45");
    mirrorDraft("s-max", "7");

    // What the modal does when it is opened afresh: the window is spent, the
    // next modal starts from the defaults.
    clearStaggerDrafts();

    expect(getDraft("s-start")).toBeUndefined();
    expect(getDraft("s-end")).toBeUndefined();
    expect(getDraft("s-max")).toBeUndefined();
    const html = vStagger({ files: [DAILY], start: 60, end: 300, max: 2, plan: null });
    expect(html).toContain('id="s-start" value="01:00"');
    expect(html).toContain('id="s-end" value="05:00"');
    expect(html).toContain('id="s-max" min="1" max="20" value="2"');
  });
});
