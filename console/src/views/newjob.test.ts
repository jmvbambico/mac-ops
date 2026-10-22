import { afterEach, describe, expect, it } from "vitest";
import { backend, setBackend } from "../api";
import { fakeBackend } from "../fake";
import type { NewJob } from "../types";
import {
  NewJobForm,
  defaultForm,
  submitNewJob,
  toSpec,
  vNewJob,
  validateForm,
} from "./newjob";

/** A form with everything filled in, so a test can spoil exactly one field. */
const filled = (over: Partial<NewJobForm> = {}): NewJobForm => ({
  ...defaultForm(),
  title: "Disk space audit",
  group: "System",
  kind: "mole",
  subcommand: "optimize",
  manager: "brew",
  scriptPath: "scripts/audit.py",
  mode: "daily",
  time: "02:00",
  day: "Mon",
  everyHours: 6,
  timeout: 900,
  ...over,
});

/** The fake backend, with `create_job` recorded instead of executed. */
function recordingBackend(
  onCreate: (spec: NewJob) => Promise<string>,
): { created: NewJob[] } {
  const created: NewJob[] = [];
  const b = fakeBackend();
  b.createJob = async (spec) => {
    created.push(spec);
    return onCreate(spec);
  };
  setBackend(b);
  return { created };
}

afterEach(() => {
  setBackend(fakeBackend());
});

describe("vNewJob — the window renders for every kind", () => {
  it("shows the mole subcommand for a mole job, and nothing else", () => {
    const html = vNewJob(filled({ kind: "mole" }), null);
    expect(html).toContain('id="nj-subcommand"');
    expect(html).not.toContain('id="nj-manager"');
    expect(html).not.toContain('id="nj-script"');
  });

  it("shows the package manager for a pkg job", () => {
    const html = vNewJob(filled({ kind: "pkg" }), null);
    expect(html).toContain('id="nj-manager"');
    expect(html).not.toContain('id="nj-subcommand"');
    expect(html).not.toContain('id="nj-script"');
  });

  it("shows the script path for a py job", () => {
    const html = vNewJob(filled({ kind: "py" }), null);
    expect(html).toContain('id="nj-script"');
    expect(html).not.toContain('id="nj-subcommand"');
    expect(html).not.toContain('id="nj-manager"');
  });

  it("shows no per-kind field for a shell job", () => {
    const html = vNewJob(filled({ kind: "sh" }), null);
    expect(html).not.toContain('id="nj-subcommand"');
    expect(html).not.toContain('id="nj-manager"');
    expect(html).not.toContain('id="nj-script"');
  });

  it("carries every required field", () => {
    const html = vNewJob(filled(), null);
    for (const id of ["nj-title", "nj-group", "nj-kind", "nj-mode", "nj-time", "nj-timeout"]) {
      expect(html).toContain(`id="${id}"`);
    }
  });
});

describe("vNewJob — the user is told what happens next", () => {
  it("says the job is created disabled on purpose", () => {
    expect(vNewJob(filled(), null)).toContain("created disabled, on purpose");
  });

  it("says a shell job is a scaffold to edit", () => {
    expect(vNewJob(filled({ kind: "sh" }), null)).toContain("blank scaffold");
  });

  it("does not call a shell job a scaffold for the other kinds", () => {
    expect(vNewJob(filled({ kind: "mole" }), null)).not.toContain("blank scaffold");
  });
});

describe("validateForm — a bad form never reaches the backend", () => {
  it("refuses a blank title", () => {
    expect(validateForm(filled({ title: "   " }))).toBe("Give the automation a title");
  });

  it("refuses a missing mole subcommand", () => {
    expect(validateForm(filled({ kind: "mole", subcommand: "" }))).toBe("Choose a mole subcommand");
  });

  it("refuses a missing package manager", () => {
    expect(validateForm(filled({ kind: "pkg", manager: "" }))).toBe("Choose a package manager");
  });

  it("refuses a missing python script path", () => {
    expect(validateForm(filled({ kind: "py", scriptPath: "" }))).toBe("Give the path to the Python script");
  });

  it("refuses a time that is not HH:MM", () => {
    expect(validateForm(filled({ time: "2am" }))).toBe("Enter the start time as HH:MM");
  });

  it("refuses a timeout outside the allowed range", () => {
    expect(validateForm(filled({ timeout: 0 }))).toBe("The timeout must be between 1 and 86400 seconds");
  });

  it("accepts a complete form", () => {
    expect(validateForm(filled())).toBeNull();
  });
});

describe("submitNewJob — refused forms", () => {
  it("never calls the backend for a blank title", async () => {
    const { created } = recordingBackend(async () => "01-x.sh");
    const res = await submitNewJob(filled({ title: "" }), (spec) => backend().createJob(spec));
    expect(res.ok).toBe(false);
    expect(created).toHaveLength(0);
  });

  it("never calls the backend for a missing per-kind field", async () => {
    const { created } = recordingBackend(async () => "01-x.sh");
    const res = await submitNewJob(filled({ kind: "pkg", manager: "" }), (spec) =>
      backend().createJob(spec),
    );
    expect(res.ok).toBe(false);
    expect(created).toHaveLength(0);
  });
});

describe("submitNewJob — a well-formed form", () => {
  it("sends exactly the spec the backend expects", async () => {
    const { created } = recordingBackend(async () => "42-disk-space-audit.sh");
    const res = await submitNewJob(filled(), (spec) => backend().createJob(spec));

    expect(res).toEqual({ ok: true, file: "42-disk-space-audit.sh" });
    expect(created).toHaveLength(1);
    expect(created[0]).toEqual({
      title: "Disk space audit",
      group: "System",
      kind: "mole",
      schedule: { mode: "daily", times: ["02:00"] },
      timeout: 900,
      script_path: null,
      manager: null,
      subcommand: "optimize",
    });
  });

  it("sends the per-kind field of the chosen kind, and nulls for the rest", async () => {
    const { created } = recordingBackend(async () => "42-x.sh");
    await submitNewJob(filled({ kind: "pkg", manager: "brew" }), (spec) => backend().createJob(spec));
    expect(created[0].manager).toBe("brew");
    expect(created[0].subcommand).toBeNull();
    expect(created[0].script_path).toBeNull();
  });

  it("sends the weekly schedule with its day", async () => {
    const { created } = recordingBackend(async () => "42-x.sh");
    await submitNewJob(filled({ mode: "weekly", day: "Sat", time: "05:30" }), (spec) =>
      backend().createJob(spec),
    );
    expect(created[0].schedule).toEqual({ mode: "weekly", days: ["Sat"], times: ["05:30"] });
  });

  it("falls back to Ungrouped when no group was given", async () => {
    const { created } = recordingBackend(async () => "42-x.sh");
    await submitNewJob(filled({ group: "  " }), (spec) => backend().createJob(spec));
    expect(created[0].group).toBe("Ungrouped");
  });
});

describe("submitNewJob — a backend rejection", () => {
  it("surfaces the backend's own message", async () => {
    const msg = "generated script failed validation:\n- missing tier line";
    recordingBackend(async () => {
      throw msg;
    });
    const res = await submitNewJob(filled(), (spec) => backend().createJob(spec));
    expect(res.ok).toBe(false);
    expect(res.ok === false && res.error).toContain("generated script failed validation");
  });
});

describe("toSpec", () => {
  it("trims the title and the script path", () => {
    const spec = toSpec(filled({ title: "  Thing  ", kind: "py", scriptPath: " a.py " }));
    expect(spec.title).toBe("Thing");
    expect(spec.script_path).toBe("a.py");
  });
});
