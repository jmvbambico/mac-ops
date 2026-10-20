/** Small render helpers shared by the views. */
import { IC } from "../icons";
import { esc } from "../format";
import type { Async } from "../store";

export const statCard = (label: string, value: string | number, sub: string): string =>
  `<div class="card pad col" style="gap:3px">
    <div class="lbl">${esc(label)}</div><div class="num">${esc(value)}</div>
    <div class="muted" style="font-size:11px">${sub}</div></div>`;

/** A whole view could not be rendered: say what broke and offer a retry. */
export const fatal = (what: string, msg: string, retry: string): string =>
  `<div class="card fatal">
    <span class="ic">${IC.warn}</span>
    <div class="t">${esc(what)}</div>
    <div class="muted" style="font-size:12px;max-width:430px">
      The command rejected. Nothing was changed and nothing is hidden — this is the backend's own message.</div>
    <pre>${esc(msg)}</pre>
    <button class="btn pri" data-retry="${esc(retry)}">${IC.refresh} Try again</button>
  </div>`;

/** A panel failed but the view is still useful: keep the panel, explain. */
export const inlineErr = (what: string, msg: string, retry?: string): string =>
  `<div class="inline-err">
    <span class="ic">${IC.warn}</span>
    <div><b>${esc(what)}</b> — ${esc(msg)}
      ${retry ? `<br><button class="btn sm" style="margin-top:7px" data-retry="${esc(retry)}">${IC.refresh} Retry</button>` : ""}
    </div></div>`;

export const skeletonRows = (n: number, h = 13): string =>
  Array.from({ length: n }, () => `<div class="skel" style="height:${h}px"></div>`).join("");

/**
 * Render an async slot: loading skeleton on first load, the error panel when it
 * failed with nothing cached, otherwise the data (possibly stale, with the
 * error shown above it).
 */
export function asyncPanel<T>(slot: Async<T>, what: string, retry: string, body: (data: T) => string): string {
  if (slot.data === null && slot.loading) return `<div class="card pad col" style="gap:7px">${skeletonRows(4)}</div>`;
  if (slot.data === null) return `<div class="card pad">${inlineErr(what, slot.error ?? "no data", retry)}</div>`;
  return `${slot.error ? inlineErr(`${what} (showing last known data)`, slot.error, retry) : ""}${body(slot.data)}`;
}
