import { formatNumber, onLocaleChange, renderI18n } from "../i18n";
import { gatesInRange, setThreshold } from "../state/draft";
import { app, canEdit } from "../state/store";

const GATE_M = 0.7;

/** Parse a user-typed dB value ("," or "." decimal); null when invalid. */
export function parseDb(raw: string): number | null {
  const s = raw.trim().replace(",", ".");
  if (!/^(\d+(\.\d*)?|\.\d+)$/.test(s)) return null;
  const v = Number(s);
  return v >= 0 && v <= 95 ? v : null;
}

export function mountGateTable(drawer: HTMLElement): { open(): void; close(): void } {
  drawer.classList.add("drawer");
  drawer.innerHTML = `
<div class="row"><h3 data-i18n="table.title"></h3><div class="grow"></div><button class="btn ghost sm" data-role="close" data-i18n-aria="table.close">✕</button></div>
<p class="hint" style="margin:6px 0 12px" data-i18n="table.hint"></p>
<table><thead><tr><th data-i18n="table.gate"></th><th data-i18n="table.distance"></th><th data-i18n="table.trigger"></th><th data-i18n="table.micro"></th></tr></thead><tbody></tbody></table>`;
  const tbody = drawer.querySelector("tbody")!;
  const inputs: HTMLInputElement[][] = [[], []];
  const rows: HTMLTableRowElement[] = [];
  for (let i = 0; i < 16; i++) {
    const tr = tbody.insertRow();
    rows.push(tr);
    tr.innerHTML = `<td>${i + 1}</td><td class="mono" data-role="range"></td><td><input inputmode="decimal"></td><td><input inputmode="decimal"></td>`;
    tr.querySelectorAll("input").forEach((inp, k) => {
      inputs[k].push(inp);
      inp.addEventListener("input", () => {
        const v = parseDb(inp.value);
        inp.classList.toggle("invalid", v === null);
        if (v === null) return;
        const s = app.get();
        const base = s.draft ?? s.sensor;
        if (base && canEdit(s)) app.set({ draft: setThreshold(base, k === 0 ? "trigger" : "micro", i, v) });
      });
      inp.addEventListener("blur", () => { inp.classList.remove("invalid"); sync(); });
    });
  }

  function sync(): void {
    const s = app.get();
    const p = s.draft ?? s.sensor;
    const n = p ? gatesInRange(p.maxDistanceM) : 16;
    const editable = !!p && canEdit(s);
    rows.forEach((tr, i) => {
      tr.classList.toggle("off", i >= n);
      tr.querySelector('[data-role="range"]')!.textContent =
        `${formatNumber(i * GATE_M, 1)}–${formatNumber((i + 1) * GATE_M, 1)} m`;
    });
    (["triggerDb", "microDb"] as const).forEach((key, k) => {
      inputs[k].forEach((inp, i) => {
        inp.disabled = !editable;
        if (document.activeElement === inp) return;
        inp.value = p ? formatNumber(p[key][i], 2) : "";
      });
    });
  }

  const dr = drawer;
  dr.querySelector('[data-role="close"]')!.addEventListener("click", () => dr.classList.remove("open"));
  app.subscribe(sync);
  onLocaleChange(() => { renderI18n(dr); sync(); });
  renderI18n(dr);
  sync();
  return {
    open: () => dr.classList.add("open"),
    close: () => dr.classList.remove("open"),
  };
}
