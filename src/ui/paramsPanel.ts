import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { exportConfig, importConfig, readParams } from "../api";
import { formatNumber, onLocaleChange, renderI18n, t } from "../i18n";
import { clampParams, isDirty } from "../state/draft";
import { app, canEdit, subscribeState } from "../state/store";
import { errorMessageOf } from "./errors";
import { mountGateTable } from "./gateTable";
import { toast } from "./notify";
import { openRestoreDialog } from "./restoreDialog";
import { confirmDiscardIfDirty, currentEpoch, guarded, isLive, performWrite } from "./writeFlow";

const TEMPLATE = `
<div class="card">
  <div class="row head"><h3 data-i18n="params.title"></h3><div class="grow"></div>
    <button class="btn ghost sm" data-role="restore" data-i18n="params.restore"></button>
    <button class="btn ghost sm" data-role="reload">↻ <span data-i18n="params.reread"></span></button></div>
  <div class="field">
    <div class="top"><label for="p-dist" data-i18n="params.maxDistance"></label><span class="val" data-role="distVal"></span></div>
    <input type="range" id="p-dist" min="0.7" max="10" step="0.1" data-role="dist">
  </div>
  <div class="field">
    <div class="top"><label for="p-delay" data-i18n="params.disappearDelay"></label>
      <span class="val"><input class="num" inputmode="numeric" data-role="delayNum"> <span data-i18n="units.s"></span></span></div>
    <input type="range" id="p-delay" min="0" max="600" step="1" data-role="delay">
  </div>
  <div class="field">
    <div class="top"><label data-i18n="params.thresholds"></label><span class="hint" data-i18n="params.gates"></span></div>
    <div class="seg"><button data-series="trigger" data-i18n="params.series.trigger"></button><button data-series="micro" data-i18n="params.series.micro"></button></div>
    <div class="row"><span class="hint" data-i18n="params.dragHint"></span><button class="btn sm" style="white-space:nowrap;flex:none" data-role="table" data-i18n="params.openTable"></button></div>
  </div>
  <div class="dirty" data-role="dirty">● <span data-i18n="params.dirty"></span><span class="tag" data-role="perm" data-i18n="params.permanentSave"></span><div class="grow"></div>
    <button class="btn ghost sm" data-role="undo" data-i18n="params.undo"></button>
    <button class="btn primary sm" data-role="write" data-i18n="params.write"></button></div>
  <div class="actions">
    <button class="btn" data-role="export" data-i18n="params.export"></button>
    <button class="btn" data-role="import" data-i18n="params.import"></button>
  </div>
</div>`;

const dirtyNow = () => isDirty(app.get().sensor, app.get().draft);

export { confirmDiscardIfDirty };

/** Intercept window close while there are unwritten changes. */
export async function installCloseGuard(): Promise<void> {
  try {
    const win = getCurrentWindow();
    let confirmed = false;
    await win.onCloseRequested(async (ev) => {
      if (confirmed || !dirtyNow()) return;
      ev.preventDefault();
      if (await confirmDiscardIfDirty()) {
        confirmed = true;
        await win.close();
      }
    });
  } catch {
    /* not running inside Tauri */
  }
}

export function mountParamsPanel(el: HTMLElement): void {
  el.innerHTML = TEMPLATE;
  const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;
  const dist = q<HTMLInputElement>("dist"), delay = q<HTMLInputElement>("delay"), delayNum = q<HTMLInputElement>("delayNum");
  const distVal = q("distVal"), dirtyEl = q("dirty"), perm = q("perm");
  const writeBtn = q<HTMLButtonElement>("write"), undoBtn = q<HTMLButtonElement>("undo"), reloadBtn = q<HTMLButtonElement>("reload"), restoreBtn = q<HTMLButtonElement>("restore");
  const exportBtn = q<HTMLButtonElement>("export"), importBtn = q<HTMLButtonElement>("import");
  const segBtns = Array.from(el.querySelectorAll<HTMLButtonElement>(".seg button"));

  const drawer = document.createElement("aside");
  document.body.appendChild(drawer);
  const table = mountGateTable(drawer);

  const live = isLive;

  function render(): void {
    const s = app.get();
    const p = s.draft ?? s.sensor;
    const connected = s.connection === "connected";
    if (p) {
      dist.value = String(p.maxDistanceM);
      distVal.textContent = `${formatNumber(p.maxDistanceM, 1)} ${t("units.m")}`;
      delay.value = String(Math.min(600, p.disappearDelayS));
      if (document.activeElement !== delayNum) delayNum.value = String(p.disappearDelayS);
    } else {
      distVal.textContent = "—";
      if (document.activeElement !== delayNum) delayNum.value = "";
    }
    const dirty = dirtyNow();
    dirtyEl.classList.toggle("show", dirty);
    perm.hidden = !s.info?.capabilities.permanentSave;
    const edit = !!p && canEdit(s);
    dist.disabled = delay.disabled = delayNum.disabled = !edit;
    // the device answers Busy while a calibration or an auto-gain runs
    const blocked = s.paramsBusy || s.calibrating || s.autoGaining;
    writeBtn.disabled = blocked || !dirty || !connected;
    undoBtn.disabled = s.paramsBusy || !dirty;
    reloadBtn.disabled = blocked || !connected;
    const snaps = s.snapshots;
    restoreBtn.disabled = !canEdit(s) || !(snaps && (snaps.original || snaps.latestBackup));
    exportBtn.disabled = !p;
    importBtn.disabled = blocked || !connected || !s.sensor;
    segBtns.forEach((b) => b.classList.toggle("on", b.dataset.series === s.series));
  }

  function edit(patch: Partial<{ maxDistanceM: number; disappearDelayS: number }>): void {
    const s = app.get();
    const base = s.draft ?? s.sensor;
    if (!base || !canEdit(s)) return;
    app.set({ draft: clampParams({ ...base, ...patch }) });
  }
  dist.addEventListener("input", () => edit({ maxDistanceM: Number(dist.value) }));
  delay.addEventListener("input", () => edit({ disappearDelayS: Number(delay.value) }));
  delayNum.addEventListener("input", () => {
    const v = Number(delayNum.value.trim().replace(",", "."));
    if (delayNum.value.trim() === "" || !Number.isFinite(v)) return;
    edit({ disappearDelayS: v });
  });
  delayNum.addEventListener("blur", render);
  segBtns.forEach((b) => b.addEventListener("click", () => app.set({ series: b.dataset.series as "trigger" | "micro" })));
  q("table").addEventListener("click", () => table.open());

  writeBtn.addEventListener("click", () => void guarded(async () => {
    const draft = app.get().draft;
    if (draft) await performWrite(draft, "params.written");
  }));
  restoreBtn.addEventListener("click", () => void openRestoreDialog());

  undoBtn.addEventListener("click", () => {
    if (app.get().paramsBusy) return;
    app.set({ draft: app.get().sensor });
  });

  reloadBtn.addEventListener("click", () => void guarded(async () => {
    if (dirtyNow()) {
      const ok = await ask(t("params.confirmReload"), { title: t("params.discardTitle"), kind: "warning" }).catch(() => false);
      if (!ok) return;
    }
    const ep = currentEpoch();
    try {
      const fresh = await readParams();
      if (live(ep)) app.set({ sensor: fresh, draft: fresh });
    } catch (e) {
      if (live(ep)) toast("error", errorMessageOf(e));
    }
  }));

  exportBtn.addEventListener("click", async () => {
    const s = app.get();
    const p = s.draft ?? s.sensor;
    if (!p) return;
    try {
      const path = await save({ defaultPath: "ld2402-config.json", filters: [{ name: t("params.jsonFilter"), extensions: ["json"] }] });
      if (!path) return;
      await exportConfig(path, p);
      toast("success", t("params.exported"));
    } catch (e) {
      toast("error", errorMessageOf(e));
    }
  });

  importBtn.addEventListener("click", () => void guarded(async () => {
    try {
      const path = await open({ multiple: false, filters: [{ name: t("params.jsonFilter"), extensions: ["json"] }] });
      if (!path || Array.isArray(path)) return;
      const p = await importConfig(path);
      app.set({ draft: p });
      toast("info", t("params.imported"));
    } catch (e) {
      toast("error", errorMessageOf(e));
    }
  }));

  subscribeState(render);
  onLocaleChange(() => { renderI18n(el); render(); });
  renderI18n(el);
  render();
}
