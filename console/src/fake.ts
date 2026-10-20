/**
 * An in-memory Backend built from the prototype's 38-automation fixture.
 *
 * This exists so the UI and every pure helper can be exercised with no Tauri
 * runtime and no Rust (the backend modules are still `todo!()`). It is NEVER a
 * silent fallback for a failing real backend — a failed invoke must surface as
 * an error in the UI. Activate it explicitly with `?fake=1` in the URL.
 */
import type { Backend } from "./api";
import type { DayStat, Job, NewJob, Run, RunOutcome, Schedule, Status } from "./types";

type RawRow = [
  number, // leading number in the filename
  string, // slug
  string, // title
  string, // group
  string, // kind
  string, // schedule shorthand
  number, // timeout
  Status, // last status
  number, // duration
  string, // summary
];

const RAW: RawRow[] = [
  [10, "mole-optimize", "Mole optimize", "System", "mole", "02:00", 900, "quiet", 18, "score 94->94 disk 63G->63G applied=3 rc=0"],
  [11, "mole-clean", "Mole clean", "System", "mole", "02:05", 900, "quiet", 91, "reclaimed=unknown items=14 rc=0"],
  [12, "disk-audit", "Disk space audit", "System", "sh", "02:10", 300, "quiet", 6, "free=63G threshold=15G rc=0"],
  [13, "timemachine-check", "Time Machine check", "System", "sh", "03:00", 600, "spoke", 12, "last backup 4d ago (threshold 2d) rc=0"],
  [14, "log-rotate", "System log rotate", "System", "sh", "04:00", 300, "quiet", 3, "rotated=2 pruned=180MB rc=0"],
  [15, "font-cache", "Font cache rebuild", "System", "sh", "w:Sun 03:30", 600, "quiet", 22, "rebuilt=1 rc=0"],
  [16, "spotlight-audit", "Spotlight index audit", "System", "sh", "04:10", 900, "quiet", 31, "volumes=2 indexed=2 rc=0"],
  [30, "brew-update", "Homebrew formulae", "Package managers", "pkg", "02:20", 3600, "spoke", 19, "upgraded=0 held=3 casks_outdated=5 rc=1"],
  [31, "brew-cask-report", "Homebrew casks (report)", "Package managers", "pkg", "02:40", 900, "quiet", 11, "outdated=5 upgraded=0 report-only rc=0"],
  [32, "npm-global-audit", "npm globals audit", "Package managers", "pkg", "02:45", 900, "quiet", 14, "packages=13 outdated=4 report-only rc=0"],
  [33, "pipx-update", "pipx packages", "Package managers", "pkg", "02:50", 1200, "quiet", 27, "upgraded=1 rc=0"],
  [34, "cargo-update", "cargo installs", "Package managers", "pkg", "03:10", 2400, "quiet", 64, "upgraded=0 rc=0"],
  [35, "rustup-update", "rustup toolchains", "Package managers", "pkg", "03:20", 1800, "quiet", 41, "stable unchanged rc=0"],
  [36, "gem-update", "RubyGems", "Package managers", "pkg", "03:25", 1200, "off", 0, "disabled 2026-09-30"],
  [37, "go-install-update", "Go tools", "Package managers", "pkg", "03:30", 1800, "quiet", 38, "tools=6 upgraded=1 rc=0"],
  [38, "uv-tool-update", "uv tools", "Package managers", "pkg", "03:35", 900, "quiet", 9, "tools=4 upgraded=0 rc=0"],
  [40, "skills-update", "Skills CLI", "Package managers", "pkg", "02:55", 900, "quiet", 8, "skills=9 updated=none rc=0"],
  [50, "git-repo-gc", "Git repo gc", "Developer", "sh", "w:Sun 05:00", 3600, "quiet", 142, "repos=23 gc=23 saved=1.1G rc=0"],
  [51, "docker-prune", "Docker prune", "Developer", "sh", "05:20", 1800, "off", 0, "disabled 2026-10-02"],
  [52, "xcode-derived", "Xcode DerivedData", "Developer", "sh", "05:25", 900, "quiet", 17, "removed=3.4G rc=0"],
  [53, "node-modules-sweep", "node_modules sweep", "Developer", "sh", "w:Sun 05:30", 3600, "quiet", 203, "stale=11 removed=6.2G rc=0"],
  [54, "venv-prune", "Stale venv prune", "Developer", "py", "05:40", 900, "quiet", 12, "venvs=18 pruned=2 rc=0"],
  [55, "dotfiles-reconcile", "Dotfiles reconcile", "Developer", "py", "06:00", 600, "fail", 4, "drift detected in 2 files rc=1"],
  [60, "restic-daily", "Restic snapshot", "Backups", "sh", "01:00", 7200, "quiet", 412, "added=1.2G snapshots=186 rc=0"],
  [61, "restic-check", "Restic integrity", "Backups", "sh", "w:Sun 01:30", 7200, "quiet", 688, "no errors rc=0"],
  [62, "borg-weekly", "Borg archive", "Backups", "sh", "w:Mon 01:45", 7200, "hang", 7200, "TIMED OUT after 7200s"],
  [63, "photos-export", "Photos export", "Backups", "py", "23:30", 3600, "quiet", 221, "exported=48 rc=0"],
  [64, "db-dump", "Postgres dump", "Backups", "sh", "23:50", 1800, "quiet", 46, "dbs=3 size=890MB rc=0"],
  [70, "expense-parse", "Expense parser", "Scripts", "py", "07:00", 600, "quiet", 11, "parsed=12 rc=0"],
  [71, "rss-digest", "RSS digest", "Scripts", "py", "07:10", 600, "quiet", 8, "items=94 sent=1 rc=0"],
  [72, "invoice-fetch", "Invoice fetch", "Scripts", "py", "07:20", 900, "quiet", 19, "invoices=3 rc=0"],
  [73, "ledger-sync", "Ledger sync", "Scripts", "py", "07:30", 900, "none", 0, "never run"],
  [74, "screenshot-tidy", "Screenshot tidy", "Scripts", "py", "12:00", 300, "quiet", 4, "moved=23 rc=0"],
  [80, "tor-check", "Tor gateway check", "Network & security", "sh", "i:6", 300, "quiet", 5, "circuit ok latency=240ms rc=0"],
  [81, "cert-expiry", "TLS cert expiry", "Network & security", "py", "08:00", 600, "spoke", 7, "1 cert expires in 12d rc=0"],
  [82, "ssh-key-audit", "SSH key audit", "Network & security", "sh", "w:Sun 08:10", 600, "quiet", 6, "keys=4 all ed25519 rc=0"],
  [83, "firewall-audit", "Firewall audit", "Network & security", "sh", "08:20", 600, "off", 0, "disabled 2026-08-14"],
];

function parseShorthand(s: string): Schedule {
  if (s.startsWith("w:")) {
    const [d, t] = s.slice(2).split(" ");
    return { mode: "weekly", days: [d], times: [t] };
  }
  if (s.startsWith("i:")) return { mode: "interval", every_hours: Number(s.slice(2)) };
  return { mode: "daily", times: [s] };
}

/** Deterministic pseudo-random, so fixture history is stable across renders. */
function makeRnd(seed: number): () => number {
  let s = seed;
  return () => (s = (s * 1103515245 + 12345) % 2147483648) / 2147483648;
}

const DAY_MS = 86_400_000;

function buildJobs(): Job[] {
  const rnd = makeRnd(7);
  return RAW.map(([n, slug, title, group, kind, sched, timeout, status, dur, summary], idx) => {
    const enabled = status !== "off";
    const schedule = parseShorthand(sched);
    const file = `${n}-${slug}.sh`;
    const last: Run | null =
      status === "none"
        ? null
        : {
            id: 10_000 + idx,
            job: file,
            started_at: Math.floor((Date.now() - 3 * 3600 * 1000) / 1000),
            duration_s: dur,
            rc: status === "fail" ? 1 : status === "hang" ? 124 : status === "spoke" ? 1 : 0,
            status,
            dry_run: false,
            summary,
            stdout: status === "quiet" ? "" : `== ${slug} ==\n[${slug}] ${summary}`,
          };
    // burn the generator the same way the prototype did, keeping the history
    // of every fixture job stable regardless of render order
    for (let i = 0; i < 30; i++) rnd();
    return {
      file,
      label: slug,
      title,
      group,
      kind,
      tier: schedule.mode === "weekly" ? "weekly" : "daily",
      timeout,
      enabled,
      schedule,
      last,
    };
  });
}

export function fakeBackend(): Backend {
  let jobs = buildJobs();
  const rnd = makeRnd(11);

  const find = (file: string): Job => {
    const j = jobs.find((x) => x.file === file);
    if (!j) throw `no such automation: ${file}`;
    return j;
  };

  const synthRuns = (j: Job, limit: number): Run[] => {
    const out: Run[] = [];
    for (let i = 0; i < limit; i++) {
      const r = rnd();
      const status: Status = !j.enabled
        ? "off"
        : i === 0 && j.last
          ? j.last.status
          : r > 0.96
            ? "spoke"
            : r > 0.945
              ? "fail"
              : r > 0.94
                ? "none"
                : "quiet";
      out.push({
        id: 1000 * (i + 1) + j.file.length,
        job: j.file,
        started_at: Math.floor((Date.now() - i * DAY_MS) / 1000),
        duration_s: Math.max(1, Math.round((j.last?.duration_s ?? 10) * (0.7 + r * 0.6))),
        rc: status === "fail" ? 1 : status === "hang" ? 124 : 0,
        status,
        dry_run: false,
        summary: j.last?.summary ?? "",
        stdout: status === "quiet" ? "" : `[${j.label}] ${j.last?.summary ?? ""}`,
      });
    }
    return out; // newest first
  };

  return {
    async listJobs() {
      return jobs.map((j) => ({ ...j }));
    },
    async resolvedPaths() {
      return {
        tool_root: "/opt/mac-ops/tool",
        automations_root: "~/mac-ops",
        config_path: "~/.config/mops/config.json",
        source: "default" as const,
      };
    },
    async setEnabled(file, enabled) {
      find(file).enabled = enabled;
    },
    async setSchedule(file, schedule) {
      find(file).schedule = schedule;
    },
    async setTimeout(file, secs) {
      find(file).timeout = secs;
    },
    async setMeta(file, title, group) {
      const j = find(file);
      if (title !== null) j.title = title;
      if (group !== null) j.group = group;
    },
    async createJob(spec: NewJob) {
      const file = `99-${spec.title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}.sh`;
      jobs.push({
        file,
        label: file.replace(/^\d+-/, "").replace(/\.sh$/, ""),
        title: spec.title,
        group: spec.group,
        kind: spec.kind,
        tier: spec.schedule.mode === "weekly" ? "weekly" : "daily",
        timeout: spec.timeout,
        enabled: false,
        schedule: spec.schedule,
        last: null,
      });
      return file;
    },
    async deleteJob(file) {
      jobs = jobs.filter((j) => j.file !== file);
    },
    async runJob(file, dry) {
      const j = find(file);
      return {
        status: dry ? "quiet" : (j.last?.status ?? "quiet"),
        rc: j.last?.rc ?? 0,
        duration_s: j.last?.duration_s ?? 1,
        stdout: dry ? "" : (j.last?.stdout ?? ""),
        stderr: "",
        timed_out: false,
      } satisfies RunOutcome;
    },
    async runsFor(file, limit) {
      return synthRuns(find(file), limit);
    },
    async fleetHistory(days) {
      const groups = [...new Set(jobs.map((j) => j.group))];
      const out: DayStat[] = [];
      for (let d = days - 1; d >= 0; d--) {
        const day = new Date(Date.now() - d * DAY_MS).toISOString().slice(0, 10);
        for (const group of groups) {
          const list = jobs.filter((j) => j.group === group && j.enabled);
          const r = rnd();
          const worst = list.find((j) => j.last && j.last.status !== "quiet");
          const status: Status =
            d === 0 && worst?.last ? worst.last.status : r > 0.95 ? "spoke" : r > 0.93 ? "fail" : "quiet";
          out.push({ day, group, status: list.length ? status : "off" });
        }
      }
      return out;
    },
    async searchLogs(q, limit) {
      const all = jobs.flatMap((j) => synthRuns(j, 3));
      const hay = q.toLowerCase();
      return all
        .filter((r) => !q || (r.summary + r.job + r.stdout).toLowerCase().includes(hay))
        .sort((a, b) => b.started_at - a.started_at)
        .slice(0, limit);
    },
    async loadProfile() {
      const mins = new Array<number>(1440).fill(0);
      for (const j of jobs) {
        if (!j.enabled) continue;
        const starts =
          j.schedule.mode === "interval"
            ? Array.from({ length: Math.ceil(24 / Math.max(1, j.schedule.every_hours)) }, (_, i) => i * Math.max(1, j.schedule.mode === "interval" ? j.schedule.every_hours : 1) * 60)
            : j.schedule.mode === "manual"
              ? []
              : j.schedule.times.map((t) => {
                  const [hh, mm] = t.split(":").map(Number);
                  return hh * 60 + mm;
                });
        const len = Math.max(1, Math.round((j.last?.duration_s ?? 60) / 60));
        for (const s of starts) for (let i = 0; i < len; i++) mins[(s + i) % 1440]++;
      }
      return mins;
    },
    async validateJob() {
      return [];
    },
    async pruneRuns() {
      return 0;
    },
    async reindex() {
      return jobs.length;
    },
  };
}
