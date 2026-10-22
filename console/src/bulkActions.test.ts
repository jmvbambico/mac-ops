import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "./api";
import type { Backend } from "./api";
import { bulkRun, bulkSetEnabled, bulkSetGroup } from "./bulkActions";
import type { BulkPaint } from "./bulkActions";
import { fakeBackend } from "./fake";
import { S } from "./store";

const A = "60-restic-daily.sh";
const B = "61-restic-check.sh";

const paint = (): BulkPaint => ({ view: vi.fn(), chrome: vi.fn(), groupDone: vi.fn() });

const backendWith = (over: Partial<Backend> = {}): Backend => ({ ...fakeBackend(), ...over });

const failingRefresh = (): Backend =>
  backendWith({
    listJobs: async () => {
      throw new Error("index unavailable");
    },
  });

beforeEach(() => {
  vi.useFakeTimers();
  S.jobs = { data: null, error: null, loading: false };
  S.picked = new Set<string>();
  S.busy = null;
  S.toast = null;
  setBackend(fakeBackend());
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("bulk actions — a failed fleet refresh is surfaced", () => {
  it("reports it after a fully successful bulk enable", async () => {
    setBackend(failingRefresh());
    S.picked = new Set([A]);

    await bulkSetEnabled([A], true, paint());

    expect(S.jobs.error).toBe("index unavailable");
    expect(S.toast?.kind).toBe("err");
    expect(S.toast?.msg).toBe("Enabled 1 automation — fleet refresh failed: index unavailable");
    expect([...S.picked]).toEqual([]);
  });

  it("reports it after a fully successful bulk run", async () => {
    setBackend(failingRefresh());
    S.picked = new Set([A]);

    await bulkRun([A], false, paint());

    expect(S.jobs.error).toBe("index unavailable");
    expect(S.toast?.kind).toBe("err");
    expect(S.toast?.msg).toBe("Ran 1 automation · all quiet — fleet refresh failed: index unavailable");
    expect([...S.picked]).toEqual([]);
  });

  it("reports it after a fully successful group move", async () => {
    setBackend(failingRefresh());
    S.picked = new Set([A]);

    await bulkSetGroup([A], "Backups", paint());

    expect(S.jobs.error).toBe("index unavailable");
    expect(S.toast?.kind).toBe("err");
    expect(S.toast?.msg).toContain("Moved 1 automation to “Backups”");
    expect(S.toast?.msg).toContain("fleet refresh failed: index unavailable");
    expect([...S.picked]).toEqual([]);
  });

  it("keeps both facts when a partial write failure and a refresh failure coincide", async () => {
    const fake = fakeBackend();
    const setEnabled = fake.setEnabled;
    setBackend({
      ...fake,
      setEnabled: async (file, enabled) => {
        if (file === B) throw new Error("read-only");
        return setEnabled(file, enabled);
      },
      listJobs: async () => {
        throw new Error("index unavailable");
      },
    });
    S.picked = new Set([A, B]);

    await bulkSetEnabled([A, B], true, paint());

    expect(S.toast?.kind).toBe("err");
    expect(S.toast?.msg).toContain(`Enabled 1 of 2 automations — failed: ${B} (read-only)`);
    expect(S.toast?.msg).toContain("fleet refresh also failed: index unavailable");
  });
});

describe("bulk actions — selection survives a partial failure", () => {
  it("keeps the picked set when a bulk enable partially fails", async () => {
    const fake = fakeBackend();
    const setEnabled = fake.setEnabled;
    setBackend({
      ...fake,
      setEnabled: async (file, enabled) => {
        if (file === B) throw new Error("read-only");
        return setEnabled(file, enabled);
      },
    });
    S.picked = new Set([A, B]);

    await bulkSetEnabled([A, B], true, paint());

    expect([...S.picked]).toEqual([A, B]);
    expect(S.toast?.kind).toBe("err");
  });

  it("clears the picked set when a bulk enable fully succeeds", async () => {
    S.picked = new Set([A, B]);

    await bulkSetEnabled([A, B], false, paint());

    expect([...S.picked]).toEqual([]);
    expect(S.toast?.kind).toBe("ok");
  });

  it("keeps the picked set when a bulk run partially fails", async () => {
    const fake = fakeBackend();
    const runJob = fake.runJob;
    setBackend({
      ...fake,
      runJob: async (file, dry) => {
        if (file === B) throw new Error("job busy");
        return runJob(file, dry);
      },
    });
    S.picked = new Set([A, B]);

    await bulkRun([A, B], false, paint());

    expect([...S.picked]).toEqual([A, B]);
    expect(S.toast?.kind).toBe("err");
  });

  it("clears the picked set when a bulk run fully succeeds", async () => {
    S.picked = new Set([A, B]);

    await bulkRun([A, B], true, paint());

    expect([...S.picked]).toEqual([]);
    expect(S.toast?.kind).toBe("ok");
  });

  it("keeps the picked set when a group move partially fails", async () => {
    const fake = fakeBackend();
    const setMeta = fake.setMeta;
    setBackend({
      ...fake,
      setMeta: async (file, title, group) => {
        if (file === B) throw new Error("write refused");
        return setMeta(file, title, group);
      },
    });
    S.picked = new Set([A, B]);

    await bulkSetGroup([A, B], "Backups", paint());

    expect([...S.picked]).toEqual([A, B]);
    expect(S.toast?.kind).toBe("err");
  });

  it("clears the picked set when a group move fully succeeds", async () => {
    S.picked = new Set([A, B]);

    await bulkSetGroup([A, B], "Backups", paint());

    expect([...S.picked]).toEqual([]);
    expect(S.toast?.kind).toBe("ok");
  });
});
