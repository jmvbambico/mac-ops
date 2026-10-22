/**
 * The "New automation" modal — the window that creates a job file.
 *
 * It is a view like the others: a pure renderer over a plain form object, plus
 * the pure helpers that turn that form into a `NewJob` spec. The backend
 * (`create_job`) renders the script from a template, validates the generated
 * file and refuses to keep it when validation fails, so this module never has
 * to know what a job file looks like.
 *
 * Two rules shape what is here:
 *  - the form is validated HERE, before anything is sent: a blank title or a
 *    missing per-kind field is a UI mistake, not a backend error;
 *  - the new job is created DISABLED on purpose (the backend does it), and the
 *    user is told so — a job that appears but does not run must not look like
 *    a bug.
 */
import { errText } from "../api";
import { DAYS, esc, parseHHMM } from "../format";
import { IC } from "../icons";
import type { NewJob, Schedule, ScheduleMode } from "../types";

export type JobKind = "mole" | "pkg" | "py" | "sh";

export const KINDS: JobKind[] = ["mole", "pkg", "py", "sh"];

export const KIND_META: Record<JobKind, { label: string; hint: string }> = {
  mole: {
    label: "mole — cleanup pass",
    hint: "Runs mole unattended. It asks for privilege per operation itself — never through this console.",
  },
  pkg: {
    label: "package manager — update",
    hint: "Keeps a package manager's packages current. Offline skips silently.",
  },
  py: {
    label: "python script",
    hint: "Runs a Python script in its own virtualenv, so its dependencies stay isolated.",
  },
  sh: {
    label: "shell script — scaffold",
    hint: "A blank shell scaffold. You put the real work in it.",
  },
};

/** Package managers the `pkg` template knows how to drive. */
export const MANAGERS = ["brew", "npm", "pipx", "cargo", "rustup", "gem"];

/** Mole subcommands the `mole` template knows how to run. */
export const SUBCOMMANDS = ["optimize", "clean"];

export const SCHEDULE_MODES: { id: ScheduleMode; label: string }[] = [
  { id: "daily", label: "Daily" },
  { id: "weekly", label: "Weekly" },
  { id: "interval", label: "Every N hours" },
  { id: "manual", label: "Manual only" },
];

export interface NewJobForm {
  title: string;
  group: string;
  kind: JobKind;
  /** mole: optimize | clean */
  subcommand: string;
  /** pkg: brew | npm | pipx | cargo | rustup | gem */
  manager: string;
  /** py: path to the .py file, relative to the repo root */
  scriptPath: string;
  mode: ScheduleMode;
  /** HH:MM, used by the daily and weekly modes */
  time: string;
  /** one day of the week, used by the weekly mode */
  day: string;
  /** hours between runs, used by the interval mode */
  everyHours: number;
  timeout: number;
}

export function defaultForm(): NewJobForm {
  return {
    title: "",
    group: "",
    kind: "mole",
    subcommand: "",
    manager: "",
    scriptPath: "",
    mode: "daily",
    time: "02:00",
    day: "Mon",
    everyHours: 6,
    timeout: 900,
  };
}

/** The per-kind field, and only that field, belongs to each kind. */
export function needsKindField(kind: JobKind): "subcommand" | "manager" | "scriptPath" | null {
  if (kind === "mole") return "subcommand";
  if (kind === "pkg") return "manager";
  if (kind === "py") return "scriptPath";
  return null;
}

/**
 * Refuse a form that cannot produce a valid job. Returns the message to show,
 * or null when the form is good enough to send.
 */
export function validateForm(form: NewJobForm): string | null {
  if (!form.title.trim()) return "Give the automation a title";
  const need = needsKindField(form.kind);
  if (need === "subcommand" && !form.subcommand) return "Choose a mole subcommand";
  if (need === "manager" && !form.manager) return "Choose a package manager";
  if (need === "scriptPath" && !form.scriptPath.trim()) return "Give the path to the Python script";
  if (form.mode === "daily" || form.mode === "weekly") {
    if (parseHHMM(form.time.trim()) === null) return "Enter the start time as HH:MM";
  }
  if (form.mode === "interval" && (!Number.isFinite(form.everyHours) || form.everyHours < 1)) {
    return "The interval must be at least one hour";
  }
  if (!Number.isFinite(form.timeout) || form.timeout < 1 || form.timeout > 86400) {
    return "The timeout must be between 1 and 86400 seconds";
  }
  return null;
}

function toSchedule(form: NewJobForm): Schedule {
  switch (form.mode) {
    case "weekly":
      return { mode: "weekly", days: [form.day], times: [form.time.trim()] };
    case "interval":
      return { mode: "interval", every_hours: Math.floor(form.everyHours) };
    case "manual":
      return { mode: "manual" };
    default:
      return { mode: "daily", times: [form.time.trim()] };
  }
}

/**
 * The spec the backend receives. The three optional fields are always present:
 * the ones that do not belong to the chosen kind are null, so the payload
 * never carries a value the template would have to interpret.
 */
export function toSpec(form: NewJobForm): NewJob {
  return {
    title: form.title.trim(),
    group: form.group.trim() || "Ungrouped",
    kind: form.kind,
    schedule: toSchedule(form),
    timeout: Math.floor(form.timeout),
    script_path: form.kind === "py" ? form.scriptPath.trim() : null,
    manager: form.kind === "pkg" ? form.manager : null,
    subcommand: form.kind === "mole" ? form.subcommand : null,
  };
}

export type NewJobResult = { ok: true; file: string } | { ok: false; error: string };

/**
 * Validate, then create. A refused form never reaches the backend, and a
 * backend rejection comes back as the backend's own message — it knows things
 * the UI does not (which generated script failed validation, and why).
 */
export async function submitNewJob(
  form: NewJobForm,
  create: (spec: NewJob) => Promise<string>,
): Promise<NewJobResult> {
  const bad = validateForm(form);
  if (bad) return { ok: false, error: bad };
  try {
    const file = await create(toSpec(form));
    return { ok: true, file };
  } catch (err) {
    return { ok: false, error: errText(err) };
  }
}

/* --------------------------------- render --------------------------------- */

function kindField(form: NewJobForm): string {
  if (form.kind === "mole") {
    return `<div class="field"><label>mole subcommand</label>
      <select class="inp" id="nj-subcommand">
        <option value="">Choose…</option>
        ${SUBCOMMANDS.map(
          (s) => `<option value="${s}"${form.subcommand === s ? " selected" : ""}>${s}</option>`,
        ).join("")}
      </select></div>`;
  }
  if (form.kind === "pkg") {
    return `<div class="field"><label>package manager</label>
      <select class="inp" id="nj-manager">
        <option value="">Choose…</option>
        ${MANAGERS.map(
          (m) => `<option value="${m}"${form.manager === m ? " selected" : ""}>${m}</option>`,
        ).join("")}
      </select></div>`;
  }
  if (form.kind === "py") {
    return `<div class="field"><label>python script path</label>
      <input class="inp" id="nj-script" value="${esc(form.scriptPath)}" placeholder="scripts/thing.py">
      <div class="hint">Relative to the repository root, next to the job file.</div></div>`;
  }
  return "";
}

function scheduleFields(form: NewJobForm): string {
  const mode = `<div class="field" style="margin:0"><label>Schedule</label>
    <select class="inp" id="nj-mode">
      ${SCHEDULE_MODES.map(
        (m) => `<option value="${m.id}"${form.mode === m.id ? " selected" : ""}>${m.label}</option>`,
      ).join("")}
    </select></div>`;

  if (form.mode === "interval") {
    return `<div class="grid g2">${mode}
      <div class="field" style="margin:0"><label>Every (hours)</label>
        <input class="inp" type="number" id="nj-every" min="1" max="24" value="${esc(String(form.everyHours))}"></div>
    </div>`;
  }
  if (form.mode === "manual") return `<div class="grid g2">${mode}</div>`;
  if (form.mode === "weekly") {
    return `<div class="grid g2">${mode}
      <div class="field" style="margin:0"><label>Time</label>
        <input class="inp" id="nj-time" value="${esc(form.time)}" placeholder="HH:MM"></div>
      <div class="field" style="margin:0"><label>Day</label>
        <select class="inp" id="nj-day">
          ${DAYS.map((d) => `<option value="${d}"${form.day === d ? " selected" : ""}>${d}</option>`).join("")}
        </select></div>
    </div>`;
  }
  return `<div class="grid g2">${mode}
    <div class="field" style="margin:0"><label>Time</label>
      <input class="inp" id="nj-time" value="${esc(form.time)}" placeholder="HH:MM"></div>
  </div>`;
}

export function vNewJob(form: NewJobForm, error: string | null): string {
  const meta = KIND_META[form.kind];
  const errBlock = error
    ? `<div class="note" style="background:var(--fail-bg);margin-top:13px">
        <span style="color:var(--fail);flex:0 0 auto">${IC.warn}</span>
        <div>${esc(error)}</div></div>`
    : "";

  return `
    <div class="scrim" data-close><div class="modal" style="width:min(560px,100%)">
      <h3>New automation</h3>
      <p class="muted" style="margin:4px 0 14px;font-size:12.5px">Creates a job file in your automations root
        and registers it. The backend writes the script, checks it, and only then keeps it.</p>
      <div class="grid g2">
        <div class="field" style="margin:0"><label>Title</label>
          <input class="inp" id="nj-title" value="${esc(form.title)}" placeholder="Disk space audit"></div>
        <div class="field" style="margin:0"><label>Group</label>
          <input class="inp" id="nj-group" value="${esc(form.group)}" placeholder="Ungrouped"></div>
      </div>
      <div class="field"><label>Kind</label>
        <select class="inp" id="nj-kind">
          ${KINDS.map(
            (k) => `<option value="${k}"${form.kind === k ? " selected" : ""}>${KIND_META[k].label}</option>`,
          ).join("")}
        </select>
        <div class="hint">${esc(meta.hint)}</div></div>
      ${kindField(form)}
      ${scheduleFields(form)}
      <div class="field"><label>Timeout (seconds)</label>
        <input class="inp" type="number" id="nj-timeout" min="1" max="86400" value="${esc(String(form.timeout))}"
          style="width:120px"></div>
      ${
        form.kind === "sh"
          ? `<div class="note" style="margin-top:13px"><span style="color:var(--accent);flex:0 0 auto">${IC.warn}</span>
              <div>A blank scaffold: it prints “nothing to do yet” until you edit the file, so it cannot pass a
                clean dry run as it is.</div></div>`
          : ""
      }
      <div class="note" style="margin-top:13px"><span style="color:var(--accent);flex:0 0 auto">${IC.check}</span>
        <div><b>It is created disabled, on purpose.</b> A new automation only starts scheduling after a clean
          dry run — edit it, dry-run it, then enable it.</div></div>
      ${errBlock}
      <div class="row" style="margin-top:16px;justify-content:flex-end;gap:7px">
        <button class="btn" data-close>Cancel</button>
        <button class="btn pri" data-newjob-confirm="1">${IC.plus} Create automation</button>
      </div>
    </div></div>`;
}
