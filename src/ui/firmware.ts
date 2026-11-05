import { onLocaleChange, renderI18n, t } from "../i18n";
import { app, subscribeState } from "../state/store";
import type { Capabilities } from "../types";

// Firmware update is gated: the vendor protocol is undocumented (see
// docs/firmware-update-research.md). No flashing code, no file reading.
const FEATURES: { key: string; cap: keyof Capabilities; version: string }[] = [
  { key: "firmware.featSave", cap: "permanentSave", version: "≥ 3.3.2" },
  { key: "firmware.featGain", cap: "autoGain", version: "≥ 3.3.5" },
  { key: "firmware.featSerial", cap: "serialText", version: "≥ 3.3.5" },
];

const TEMPLATE = `
<div class="card">
  <div class="row"><h3 data-i18n="firmware.title"></h3><div class="grow"></div><span class="tag" data-role="current"></span></div>
  <div class="drop" data-role="drop" aria-disabled="true" style="margin-top:14px;opacity:.5">
    <span data-i18n="firmware.drop"></span><br><span class="hint" data-i18n="firmware.or"></span>
    <button class="btn sm" data-role="choose" disabled data-i18n="firmware.choose"></button>
  </div>
  <div class="row" style="margin-top:14px">
    <button class="btn primary" data-role="update" disabled data-i18n="firmware.update"></button>
    <span class="hint" data-i18n="firmware.keepPowered"></span>
  </div>
  <div class="hint notice" data-role="notice" style="margin-top:10px" data-i18n="firmware.unavailable"></div>
</div>
<div class="card"><h3 data-i18n="firmware.featuresTitle"></h3>
  <table style="margin-top:8px"><tr><th data-i18n="firmware.colFeature"></th><th data-i18n="firmware.colVersion"></th><th></th></tr>
  ${FEATURES.map((f) => `<tr><td data-i18n="${f.key}"></td><td class="mono">${f.version}</td><td data-cap="${f.cap}"></td></tr>`).join("")}
  </table>
  <div class="hint" style="margin-top:8px" data-i18n="firmware.olderHint"></div>
</div>`;

export function mountFirmware(el: HTMLElement): void {
  el.innerHTML = TEMPLATE;
  const cur = el.querySelector<HTMLElement>('[data-role="current"]')!;
  const render = (): void => {
    const info = app.get().info;
    cur.textContent = t("firmware.current", { version: info?.firmware || "—" });
    for (const c of el.querySelectorAll<HTMLElement>("[data-cap]")) {
      c.textContent = info ? (info.capabilities[c.dataset.cap as keyof Capabilities] ? "✓" : "✗") : "—";
    }
  };
  subscribeState(render);
  onLocaleChange(() => { renderI18n(el); render(); });
  renderI18n(el);
  render();
}
