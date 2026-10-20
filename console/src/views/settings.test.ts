import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setBackend } from "../api";
import type { Backend, ResolvedPathsInfo } from "../api";
import { fakeBackend } from "../fake";
import { S, saveAutomationsRoot } from "../store";
import { vSettings } from "./settings";

const ENV_ROOT = "/Users/me/.mops";
const CONFIG_ROOT = "/Volumes/Work/macops";

const resolved = (over: Partial<ResolvedPathsInfo> = {}): ResolvedPathsInfo => ({
  tool_root: "/opt/mac-ops/tool",
  automations_root: CONFIG_ROOT,
  config_path: "/Users/me/.config/mops/config.json",
  source: "config",
  ...over,
});

const showPaths = (info: ResolvedPathsInfo): void => {
  S.paths = { data: info, error: null, loading: false };
};

/** A backend whose save resolves to `answer`, recording the value it received. */
const savingBackend = (answer: ResolvedPathsInfo): { backend: Backend; written: string[] } => {
  const written: string[] = [];
  return {
    written,
    backend: {
      ...fakeBackend(),
      setAutomationsRoot: async (path) => {
        written.push(path);
        return answer;
      },
    },
  };
};

beforeEach(() => {
  vi.stubGlobal("document", { documentElement: { dataset: {} } });
  S.jobs = { data: [], error: null, loading: false };
  S.profile = { data: null, error: null, loading: false };
  S.paths = { data: null, error: null, loading: false };
  S.pathsEdit = { error: null, busy: false, draft: null, saved: null };
  setBackend(fakeBackend());
});

afterEach(() => {
  vi.unstubAllGlobals();
  setBackend(fakeBackend());
});

describe("Settings paths — effective vs saved", () => {
  it("shows both roots and the override warning when env wins over a differing saved value", () => {
    showPaths(resolved({ automations_root: ENV_ROOT, source: "env" }));
    S.pathsEdit.saved = CONFIG_ROOT;

    const html = vSettings();

    // The effective root keeps reporting reality, with its source badge.
    expect(html).toContain(ENV_ROOT);
    expect(html).toContain('<span class="badge tier" style="margin-left:5px">env</span>');
    // The value written to config.json stays visible instead of vanishing.
    expect(html).toContain("Saved in config.json");
    expect(html).toContain(CONFIG_ROOT);
    expect(html).toContain("is set and overrides the config file");
  });

  it("does not repeat the path when the saved value matches the effective root", () => {
    showPaths(resolved({ automations_root: ENV_ROOT, source: "env" }));
    S.pathsEdit.saved = ENV_ROOT;

    const html = vSettings();

    expect(html).not.toContain("Saved in config.json");
    // The override is still real, so the warning must remain.
    expect(html).toContain("is set and overrides the config file");
  });

  it("does not duplicate the path when there is no override", () => {
    showPaths(resolved({ automations_root: CONFIG_ROOT, source: "config" }));
    S.pathsEdit.saved = CONFIG_ROOT;

    const html = vSettings();

    expect(html).not.toContain("Saved in config.json");
    expect(html).not.toContain("is set and overrides the config file");
    expect(html).toContain(`value="${CONFIG_ROOT}"`);
  });

  it("renders a rejected value as the draft with its error, leaving the saved value alone", async () => {
    showPaths(resolved());
    S.pathsEdit.saved = CONFIG_ROOT;
    setBackend({
      ...fakeBackend(),
      setAutomationsRoot: async () => {
        throw new Error("the automations root must be an absolute path (got relative/path)");
      },
    });

    const ok = await saveAutomationsRoot("relative/path");

    expect(ok).toBe(false);
    expect(S.pathsEdit.draft).toBe("relative/path");
    expect(S.pathsEdit.saved).toBe(CONFIG_ROOT);
    expect(S.pathsEdit.error).toContain("must be an absolute path");

    const html = vSettings();
    expect(html).toContain('value="relative/path"');
    expect(html).toContain("Could not save the automations root");
    expect(html).toContain("must be an absolute path");
  });
});

describe("Settings paths — saving", () => {
  it("shows the newly saved root in the input when there is no override", async () => {
    showPaths(resolved({ automations_root: ENV_ROOT, source: "default" }));
    const { backend } = savingBackend(resolved({ automations_root: CONFIG_ROOT, source: "config" }));
    setBackend(backend);

    const ok = await saveAutomationsRoot(CONFIG_ROOT);

    expect(ok).toBe(true);
    expect(S.paths.data?.automations_root).toBe(CONFIG_ROOT);
    expect(S.pathsEdit.draft).toBe(CONFIG_ROOT);
    expect(S.pathsEdit.saved).toBe(CONFIG_ROOT);

    const html = vSettings();
    expect(html).toContain(`value="${CONFIG_ROOT}"`);
    expect(html).not.toContain("Saved in config.json");
  });

  it("keeps the saved value visible after a save while an env override still wins", async () => {
    showPaths(resolved({ automations_root: ENV_ROOT, source: "env" }));
    const { backend, written } = savingBackend(resolved({ automations_root: ENV_ROOT, source: "env" }));
    setBackend(backend);

    const ok = await saveAutomationsRoot(`  ${CONFIG_ROOT}  `);

    expect(ok).toBe(true);
    expect(written).toEqual([`  ${CONFIG_ROOT}  `]);
    expect(S.pathsEdit.saved).toBe(CONFIG_ROOT);

    const html = vSettings();
    expect(html).toContain("Saved in config.json");
    expect(html).toContain(CONFIG_ROOT);
    expect(html).toContain(ENV_ROOT);
    expect(html).toContain("is set and overrides the config file");
    // The editor keeps the saved value, not the effective env root: saving
    // again must not silently overwrite config.json with the env path.
    expect(html).toContain(`value="${CONFIG_ROOT}"`);
    expect(html).not.toContain(`value="${ENV_ROOT}"`);
  });
});

describe("Settings paths — draft durability", () => {
  it("keeps a typed draft when an unrelated state change repaints the panel", () => {
    showPaths(resolved({ automations_root: ENV_ROOT, source: "env" }));
    const typed = "/Volumes/New/root";
    S.pathsEdit.draft = typed;

    // An auto-dismissing toast is the confirmed repaint trigger: clearing it
    // calls changed() -> render() -> vSettings().
    S.toast = { msg: "Automations root saved — restart the console to apply it", kind: "ok", id: 1 };
    S.toast = null;

    expect(vSettings()).toContain(`value="${typed}"`);
  });
});
