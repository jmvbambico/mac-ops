/** Thin DOM accessors. Keeps the views and modals free of querySelector noise. */
export const $ = <T extends Element = HTMLElement>(sel: string): T | null => document.querySelector<T>(sel);

export const layerEl = (): HTMLElement => {
  const el = document.getElementById("layer");
  if (!el) throw new Error("#layer missing from index.html");
  return el;
};

export const setLayer = (html: string): void => {
  layerEl().innerHTML = html;
};

export const closeLayer = (): void => {
  layerEl().innerHTML = "";
};

export const inputValue = (sel: string): string => {
  const el = $<HTMLInputElement | HTMLSelectElement>(sel);
  return el ? el.value : "";
};

export const numberValue = (sel: string, fallback: number): number => {
  const n = Number(inputValue(sel));
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : fallback;
};
