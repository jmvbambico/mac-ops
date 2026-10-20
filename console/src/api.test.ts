import { describe, expect, it } from "vitest";
import { errText, parseResolvedPaths } from "./api";

const valid = {
  tool_root: "/opt/mac-ops/tool",
  automations_root: "~/.mops",
  config_path: "~/.config/mops/config.json",
  source: "env",
};

describe("parseResolvedPaths", () => {
  it("passes a valid payload through", () => {
    expect(parseResolvedPaths(valid)).toEqual(valid);
  });

  it("accepts every known source", () => {
    for (const source of ["env", "config", "default"] as const) {
      expect(parseResolvedPaths({ ...valid, source }).source).toBe(source);
    }
  });

  it("rejects a payload that is not an object", () => {
    expect(() => parseResolvedPaths(null)).toThrow(/not an object/);
    expect(() => parseResolvedPaths(undefined)).toThrow(/not an object/);
    expect(() => parseResolvedPaths("nope")).toThrow(/not an object/);
    expect(() => parseResolvedPaths(42)).toThrow(/not an object/);
  });

  it("rejects a missing path field", () => {
    const { tool_root: _drop, ...rest } = valid;
    expect(() => parseResolvedPaths(rest)).toThrow(/tool_root/);
  });

  it("rejects an empty path field", () => {
    expect(() => parseResolvedPaths({ ...valid, automations_root: "" })).toThrow(/automations_root/);
  });

  it("rejects a whitespace-only path field", () => {
    expect(() => parseResolvedPaths({ ...valid, config_path: "   " })).toThrow(/config_path/);
  });

  it("rejects a non-string path field", () => {
    expect(() => parseResolvedPaths({ ...valid, tool_root: 7 })).toThrow(/tool_root/);
    expect(() => parseResolvedPaths({ ...valid, config_path: null })).toThrow(/config_path/);
    expect(() => parseResolvedPaths({ ...valid, automations_root: ["a"] })).toThrow(/automations_root/);
  });

  it("degrades an unrecognised source to null but keeps the paths", () => {
    const out = parseResolvedPaths({ ...valid, source: "somewhere-else" });
    expect(out.source).toBeNull();
    expect(out.tool_root).toBe(valid.tool_root);
    expect(out.automations_root).toBe(valid.automations_root);
    expect(out.config_path).toBe(valid.config_path);
  });

  it("degrades a missing source to null but keeps the paths", () => {
    const { source: _drop, ...rest } = valid;
    const out = parseResolvedPaths(rest);
    expect(out.source).toBeNull();
    expect(out.tool_root).toBe(valid.tool_root);
  });
});

describe("errText", () => {
  it("renders strings, errors and objects as display text", () => {
    expect(errText("boom")).toBe("boom");
    expect(errText(new Error("boom"))).toBe("boom");
    expect(errText({ message: "boom" })).toBe("boom");
    expect(errText({ a: 1 })).toBe('{"a":1}');
    expect(errText(7)).toBe("7");
  });
});
