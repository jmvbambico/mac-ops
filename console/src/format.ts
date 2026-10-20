/**
 * Pure helpers. No DOM, no invoke — everything here is a function of its
 * arguments, which is what makes the data layer testable with no backend.
 */
import type { Job, Run, Schedule, Status } from "./types";
import { ATTN, STATUS_META, lastDuration, statusOf } from "./types";

export const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export const esc = (s: unknown): string =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c] as string);

export const fmtDur = (s: number): string =>
  s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;

export const hhmm = (m: number): string =>
  `${String(Math.floor(m / 60) % 24).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;

export const parseHHMM = (t: string): number | null => {
  const m = /^(\d{1,2}):(\d{2})$/.exec(t.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  if (h > 23 || min > 59) return null;
  return h * 60 + min;
};

/** Minute-of-day start times a schedule produces in a 24h window. */
export function scheduleStarts(s: Schedule): number[] {
  switch (s.mode) {
    case "manual":
      return [];
    case "interval": {
      const step = Math.max(1, s.every_hours);
      const out: number[] = [];
      for (let h = 0; h < 24; h += step) out.push(h * 60);
      return out;
    }
    default:
      return s.times.map(parseHHMM).filter((m): m is number => m !== null);
  }
}

export const scheduleTimes = (s: Schedule): string[] => (s.mode === "daily" || s.mode === "weekly" ? s.times : []);
export const scheduleDays = (s: Schedule): string[] => (s.mode === "weekly" ? s.days : []);

export function schedText(j: Job): string {
  const s = j.schedule;
  if (s.mode === "interval") return `Every ${s.every_hours}h`;
  if (s.mode === "manual") return "Manual only";
  if (s.mode === "weekly") return `${s.days.join(", ")} ${s.times.join(", ")}`;
  return `Daily ${s.times.join(", ")}`;
}

/** The next wall-clock time this automation is due, or null. */
export function nextRun(j: Job, now = new Date()): Date | null {
  if (!j.enabled) return null;
  const s = j.schedule;
  if (s.mode === "manual") return null;
  if (s.mode === "interval") return new Date(now.getTime() + Math.max(1, s.every_hours) * 3600 * 1000);
  const cand: Date[] = [];
  for (let day = 0; day < 9; day++) {
    for (const t of s.times) {
      const mins = parseHHMM(t);
      if (mins === null) continue;
      const d = new Date(now);
      d.setDate(d.getDate() + day);
      d.setHours(Math.floor(mins / 60), mins % 60, 0, 0);
      if (d <= now) continue;
      if (s.mode === "weekly" && !s.days.includes(DAYS[(d.getDay() + 6) % 7])) continue;
      cand.push(d);
    }
  }
  return cand.sort((a, b) => a.getTime() - b.getTime())[0] ?? null;
}

export function untilStr(d: Date | null, now = new Date()): string {
  if (!d) return "—";
  const mins = Math.round((d.getTime() - now.getTime()) / 60000);
  const h = Math.floor(mins / 60);
  if (h >= 24) return `in ${Math.floor(h / 24)}d ${h % 24}h`;
  return h ? `in ${h}h ${mins % 60}m` : `in ${mins}m`;
}

export function fmtTime(epochSecs: number): string {
  const d = new Date(epochSecs * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
}

export function fmtDay(epochSecs: number): string {
  return new Date(epochSecs * 1000).toISOString().slice(0, 10);
}

export function counts(list: Job[]): Partial<Record<Status, number>> {
  return list.reduce<Partial<Record<Status, number>>>((a, j) => {
    const k = statusOf(j);
    a[k] = (a[k] ?? 0) + 1;
    return a;
  }, {});
}

export function sbar(list: Job[]): string {
  const c = counts(list);
  const n = list.length || 1;
  const order: Status[] = ["quiet", "spoke", "fail", "hang", "none", "off"];
  return `<div class="sbar">${order
    .filter((k) => c[k])
    .map((k) => `<i class="${k}" style="width:${((c[k] as number) / n) * 100}%" title="${c[k]} ${STATUS_META[k].t}"></i>`)
    .join("")}</div>`;
}

export const needsAttention = (j: Job): boolean => j.enabled && ATTN.includes(statusOf(j));

/**
 * Concurrency per minute-of-day, computed locally. The backend exposes the
 * same thing as `load_profile`; this is the fallback when that call fails and
 * the basis for the stagger assistant's "peak today" figure.
 */
export function localLoadProfile(jobs: Job[]): number[] {
  const mins = new Array<number>(1440).fill(0);
  for (const j of jobs) {
    if (!j.enabled) continue;
    const len = Math.max(1, Math.round(lastDuration(j) / 60));
    for (const s of scheduleStarts(j.schedule)) for (let i = 0; i < len; i++) mins[(s + i) % 1440]++;
  }
  return mins;
}

/** Groups, in a stable order, with the icon of their most common script kind. */
export function groupsOf(jobs: Job[]): { name: string; icon: string; jobs: Job[] }[] {
  const order: string[] = [];
  const byName = new Map<string, Job[]>();
  for (const j of jobs) {
    if (!byName.has(j.group)) {
      byName.set(j.group, []);
      order.push(j.group);
    }
    (byName.get(j.group) as Job[]).push(j);
  }
  order.sort((a, b) => a.localeCompare(b));
  return order.map((name) => {
    const list = byName.get(name) as Job[];
    const tally = new Map<string, number>();
    for (const j of list) tally.set(j.kind, (tally.get(j.kind) ?? 0) + 1);
    const icon = [...tally.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? "sh";
    return { name, icon, jobs: list };
  });
}

/** Severity class for a run, matching the prototype's log line styling. */
export function runSeverity(r: Run): "info" | "alert" | "err" {
  if (r.status === "fail" || r.status === "hang") return "err";
  if (r.status === "spoke") return "alert";
  return "info";
}

/**
 * Spread `jobs` across [startMin, endMin) so at most `maxConcurrent` start in
 * the same slot, longest first. Returns the proposed start minute per file.
 */
export function staggerPlan(jobs: Job[], startMin: number, endMin: number, maxConcurrent: number): Map<string, number> {
  const span = Math.max(1, endMin - startMin);
  const ordered = [...jobs].sort((a, b) => lastDuration(b) - lastDuration(a));
  const slots = Math.max(1, Math.ceil(ordered.length / Math.max(1, maxConcurrent)));
  const step = Math.max(1, Math.floor(span / slots));
  const plan = new Map<string, number>();
  ordered.forEach((j, i) => {
    const slot = Math.floor(i / Math.max(1, maxConcurrent));
    plan.set(j.file, (startMin + slot * step) % 1440);
  });
  return plan;
}
