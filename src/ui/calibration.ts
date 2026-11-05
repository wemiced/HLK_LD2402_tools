import { autoGain, onAutoGainDone, onCalibDone, onCalibFailed, onCalibProgress, readParams, setStreaming, startCalibration } from "../api";
import { formatNumber, onLocaleChange, renderI18n, t } from "../i18n";
import { isDirty } from "../state/draft";
import { app, onDisconnect } from "../state/store";
import { errorMessage, errorMessageOf } from "./errors";
import { toast } from "./notify";
import { confirmDiscardIfDirty } from "./paramsPanel";

const GATES = 16;
const CALIB_IDLE_MS = 15_000;
const AUTOGAIN_TIMEOUT_MS = 30_000;
// Coefficient range/default follow the manual ("higher = more sensitive").
// TODO(hardware): verify the range and default against a real sensor.
const COEF_MIN = 1, COEF_MAX = 20, COEF_STEP = 0.1, COEF_DEFAULT = 3;
const COEFS = [
  { id: "trigger", label: "calib.coefTrigger" },
  { id: "hold", label: "calib.coefHold" },
  { id: "micro", label: "calib.coefMicro" },
] as const;

const TEMPLATE = `
<div class="card calib">
  <h3 data-i18n="calib.title"></h3>
  <div class="hint notice" data-role="notice" data-i18n="calib.connectFirst" hidden></div>
  <div class="steps" data-role="steps">
    <div data-step="prepare"><span data-i18n="calib.step1"></span><br><b data-i18n="calib.step1Body"></b></div>
    <div data-step="measure"><span data-i18n="calib.step2"></span><br><b data-i18n="calib.step2Body"></b></div>
    <div data-step="result"><span data-i18n="calib.step3"></span><br><b data-i18n="calib.step3Body"></b></div>
  </div>
  <div data-role="step-prepare">
    <p class="hint" style="margin:10px 0 0" data-i18n="calib.hint"></p>
    ${COEFS.map((c) => `<div class="field"><div class="top"><label for="coef-${c.id}" data-i18n="${c.label}"></label><span class="val" data-role="val-${c.id}"></span></div>
      <input type="range" id="coef-${c.id}" data-role="coef-${c.id}" min="${COEF_MIN}" max="${COEF_MAX}" step="${COEF_STEP}" value="${COEF_DEFAULT}"></div>`).join("")}
    <div class="hint" data-i18n="calib.coefHint"></div>
    <div class="row" style="margin-top:16px"><button class="btn primary" data-role="start" data-i18n="calib.start"></button></div>
  </div>
  <div data-role="step-measure" hidden>
    <p class="hint" style="margin:10px 0 0" data-i18n="calib.measuring"></p>
    <div class="row" style="margin-top:16px"><div class="progress" style="flex:1" role="progressbar" aria-valuemin="0" aria-valuemax="100"><i data-role="bar"></i></div><span class="mono" data-role="pct"></span></div>
  </div>
</div>
<div class="card" data-role="step-result" hidden>
  <h3 data-role="result-title"></h3>
  <p class="hint" style="margin:6px 0" data-role="result-body"></p>
  <div class="hint" style="margin-top:10px" data-i18n="calib.gates"></div>
  <div class="gates" data-role="gates"></div>
  <div class="row" style="margin-top:14px"><button class="btn" data-role="retry" data-i18n="calib.retry"></button><button class="btn primary" data-role="dashboard" data-i18n="calib.viewDashboard"></button></div>
</div>`;

type Step = "prepare" | "measure" | "result";

export function mountCalibration(el: HTMLElement): void {
  el.innerHTML = TEMPLATE;
  renderI18n(el);
  const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;
  const coef: Record<string, HTMLInputElement> = {};
  for (const c of COEFS) coef[c.id] = q<HTMLInputElement>(`coef-${c.id}`);
  const start = q<HTMLButtonElement>("start");

  let step: Step = "prepare";
  let progress = 0;
  let gates: number[] = [];
  let wasStreaming = false;
  // bumped on every disconnect / new run so stale async work can bail out
  let epoch = 0;
  let starting = false;
  let watchdog: ReturnType<typeof setTimeout> | null = null;
  const stopWatchdog = () => { if (watchdog) clearTimeout(watchdog); watchdog = null; };
  const kick = () => {
    stopWatchdog();
    watchdog = setTimeout(() => void abort(t("calib.timeout")), CALIB_IDLE_MS);
  };
  const live = (e: number) => e === epoch && app.get().connection === "connected";

  function render(): void {
    const connected = app.get().connection === "connected";
    q("notice").hidden = connected;
    start.disabled = !connected;
    for (const c of COEFS) {
      q(`val-${c.id}`).textContent = formatNumber(Number(coef[c.id].value), 1);
      coef[c.id].disabled = step !== "prepare";
    }
    el.querySelectorAll<HTMLElement>("[data-step]").forEach((s) => s.classList.toggle("on", s.dataset.step === step));
    q("step-prepare").hidden = step !== "prepare";
    q("step-measure").hidden = step !== "measure";
    q("step-result").hidden = step !== "result";
    q("bar").style.width = `${progress}%`;
    q("pct").textContent = `${formatNumber(progress, 0)} %`;
    q("bar").parentElement!.setAttribute("aria-valuenow", String(progress));
    if (step === "result") renderResult();
  }

  function renderResult(): void {
    const box = q("gates");
    box.innerHTML = "";
    for (let i = 0; i < GATES; i++) {
      const s = document.createElement("span");
      s.textContent = formatNumber(i * 0.7, 1);
      if (gates.includes(i)) s.className = "bad";
      box.appendChild(s);
    }
    const clean = gates.length === 0;
    q("result-title").textContent = t(clean ? "calib.noInterference" : "calib.resultMinor");
    q("result-body").textContent = clean ? t("calib.noInterferenceBody") : t("calib.resultBody", { count: gates.length });
  }

  function goto(next: Step): void {
    step = next;
    if (next !== "measure") stopWatchdog();
    if (app.get().calibrating !== (next === "measure")) app.set({ calibrating: next === "measure" });
    render();
  }

  /** Restore the dashboard stream if it was running before calibration. */
  async function restoreStreaming(ep: number): Promise<void> {
    if (!wasStreaming) return;
    wasStreaming = false;
    if (!live(ep)) return;
    try {
      await setStreaming(true);
      if (live(ep)) app.set({ streaming: true });
    } catch (e) {
      if (live(ep)) toast("error", errorMessageOf(e));
    }
  }

  /** Calibration failed or went silent: back to Prepare with an error toast. */
  async function abort(message: string): Promise<void> {
    if (step !== "measure") return;
    const ep = epoch;
    toast("error", message);
    goto("prepare");
    await restoreStreaming(ep);
  }

  async function run(): Promise<void> {
    const s = app.get();
    if (starting || s.connection !== "connected" || step !== "prepare") return;
    starting = true;
    try {
      if (isDirty(s.sensor, s.draft) && !(await confirmDiscardIfDirty())) return;
    } finally {
      starting = false;
    }
    if (app.get().connection !== "connected" || step !== "prepare") return;
    const ep = ++epoch;
    progress = 0;
    gates = [];
    wasStreaming = app.get().streaming;
    goto("measure");
    kick();
    try {
      // the device answers Busy to other commands during calibration
      if (wasStreaming) {
        await setStreaming(false);
        if (live(ep)) app.set({ streaming: false });
      }
      if (!live(ep)) return;
      await startCalibration({ trigger: Number(coef.trigger.value), hold: Number(coef.hold.value), micro: Number(coef.micro.value) });
    } catch (e) {
      if (!live(ep)) return;
      toast("error", errorMessageOf(e));
      goto("prepare");
      await restoreStreaming(ep);
    }
  }

  start.addEventListener("click", () => void run());
  for (const c of COEFS) coef[c.id].addEventListener("input", render);
  q("retry").addEventListener("click", () => goto("prepare"));
  q("dashboard").addEventListener("click", () => { goto("prepare"); app.set({ view: "dashboard" }); });

  void onCalibProgress((p) => {
    if (step !== "measure") return;
    kick();
    progress = Math.max(0, Math.min(100, Math.round(p)));
    render();
  });
  void onCalibDone(async (d) => {
    if (step !== "measure") return;
    const ep = epoch;
    stopWatchdog();
    gates = d.gates;
    progress = 100;
    goto("result");
    try {
      const params = await readParams();
      if (live(ep)) app.set({ sensor: params, draft: params });
    } catch (e) {
      if (live(ep)) toast("error", errorMessageOf(e));
    }
    await restoreStreaming(ep);
  });
  void onCalibFailed(async (f) => {
    if (step !== "measure") return;
    await abort(errorMessage({ ...f.error }));
  });

  onDisconnect(() => {
    epoch++;
    stopWatchdog();
    wasStreaming = false;
    progress = 0;
    gates = [];
    goto("prepare");
  });
  app.subscribe(render);
  onLocaleChange(() => { renderI18n(el); render(); });
  render();
}

const CARD = `
<div class="card">
  <h3 data-i18n="calib.title"></h3>
  <p class="hint" style="margin:8px 0 14px" data-i18n="calib.hint"></p>
  <button class="btn primary" style="width:100%;justify-content:center" data-role="launch" data-i18n="calib.launch"></button>
  <button class="btn" style="width:100%;justify-content:center;margin-top:8px" data-role="autogain"><span class="spinner" hidden></span><span data-role="autogain-label"></span></button>
  <div class="hint" style="margin-top:6px" data-i18n="calib.autoGainHint"></div>
</div>`;

/** Dashboard card: navigate to the calibration page, or auto-adjust the gain. */
export function mountCalibrationCard(el: HTMLElement): void {
  el.innerHTML = CARD;
  renderI18n(el);
  const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;
  const btn = q<HTMLButtonElement>("autogain");
  let busy = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const clear = () => { if (timer) clearTimeout(timer); timer = null; };

  function render(): void {
    const s = app.get();
    const connected = s.connection === "connected";
    const supported = !!s.info?.capabilities.autoGain;
    btn.disabled = busy || !connected || !supported;
    btn.classList.toggle("busy", busy);
    btn.setAttribute("aria-busy", String(busy));
    btn.title = connected && !supported ? t("caps.requires", { version: "3.3.5" }) : "";
    btn.querySelector<HTMLElement>(".spinner")!.hidden = !busy;
    q("autogain-label").textContent = t(busy ? "calib.autoGaining" : "calib.autoGain");
  }

  q("launch").addEventListener("click", () => app.set({ view: "calibration" }));
  btn.addEventListener("click", async () => {
    if (btn.disabled) return;
    busy = true;
    render();
    clear();
    timer = setTimeout(() => {
      if (!busy) return;
      busy = false;
      render();
      toast("error", t("autogain.timeout"));
    }, AUTOGAIN_TIMEOUT_MS);
    try {
      await autoGain();
    } catch (e) {
      clear();
      busy = false;
      render();
      toast("error", errorMessageOf(e));
    }
  });
  void onAutoGainDone(() => {
    if (!busy) return;
    clear();
    busy = false;
    render();
    toast("success", t("calib.autoGainDone"));
  });
  onDisconnect(() => { clear(); busy = false; render(); });
  app.subscribe(render);
  onLocaleChange(() => { renderI18n(el); render(); });
  render();
}
