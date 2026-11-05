import { ApiError, onFrame, readParams, setStreaming } from "../api";
import { formatNumber, onLocaleChange, renderI18n, t } from "../i18n";
import { isDirty, previewPresence, setThreshold } from "../state/draft";
import { app, onConnected, onDisconnect } from "../state/store";
import type { PresenceKind } from "../types";
import { createDistanceChart } from "./distanceChart";
import { createEnergyChart } from "./energyChart";
import { toast } from "./notify";

const TEMPLATE = `
<div class="col">
  <div class="card status">
    <div class="orb idle" data-role="orb"><i></i></div>
    <div><div class="hint" data-role="label"></div><div class="big" data-role="dist"></div></div>
    <div class="kv"><div class="k" data-i18n="status.state"></div><div class="v" data-role="state"></div><div class="preview" data-role="preview" hidden></div></div>
    <div class="kv"><div class="k" data-i18n="status.absentSince"></div><div class="v mono" data-role="since"></div></div>
    <button class="btn sm" data-role="pause"></button>
  </div>
  <div class="card">
    <div class="row" style="margin-bottom:12px">
      <h3><span data-i18n="chart.energy"></span> <span class="hint">· <span data-i18n="chart.gateSize"></span></span></h3>
      <div class="legend">
        <span><b style="background:var(--accent)"></b><span data-i18n="chart.legend.energy"></span></span>
        <span><b style="background:var(--present)"></b><span data-i18n="chart.legend.trigger"></span></span>
        <span><b style="background:var(--micro)"></b><span data-i18n="chart.legend.micro"></span></span>
      </div>
    </div>
    <canvas data-role="bars"></canvas>
    <div class="hint" style="margin-top:8px" data-i18n="chart.tip"></div>
  </div>
  <div class="card">
    <div class="row" style="margin-bottom:12px"><h3><span data-i18n="chart.distance"></span> <span class="hint">· <span data-i18n="chart.last60"></span></span></h3></div>
    <canvas data-role="line"></canvas>
  </div>
</div>
<div class="col">
  <div data-slot="params"></div>
  <div data-slot="calib"></div>
</div>`;

/** Containers the parameter/calibration panels mount into. */
export function dashboardSlot(name: "params" | "calib", root: ParentNode = document): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[data-slot="${name}"]`);
}

const STATE_KEY: Record<PresenceKind, string> = {
  moving: "status.motion",
  still: "status.micro",
  none: "status.empty",
};

function errText(e: unknown): string {
  if (e instanceof ApiError) return t("errors." + e.kind, e.data as Record<string, string | number>);
  return t("errors.unknown");
}

export function mountDashboard(el: HTMLElement): void {
  el.innerHTML = TEMPLATE;
  renderI18n(el);
  const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;
  const orb = q("orb"), label = q("label"), dist = q("dist"), state = q("state");
  const preview = q("preview"), since = q("since"), pause = q<HTMLButtonElement>("pause");

  const energy = createEnergyChart(q<HTMLCanvasElement>("bars"), {
    onDrag(gate, db) {
      const s = app.get();
      const base = s.draft ?? s.sensor;
      if (!base) return;
      app.set({ draft: setThreshold(base, s.series, gate, db) });
    },
  });
  const line = createDistanceChart(q<HTMLCanvasElement>("line"));

  // --- time since absence
  let lastPresentAt: number | null = null;
  let firstFrameAt: number | null = null;

  // --- rendering (at most once per animation frame)
  let scheduled = false;
  const raf: (cb: () => void) => void =
    typeof requestAnimationFrame === "function" ? (cb) => requestAnimationFrame(cb) : (cb) => void setTimeout(cb, 16);
  function schedule(): void {
    if (scheduled) return;
    scheduled = true;
    raf(() => {
      scheduled = false;
      render();
    });
  }

  function render(): void {
    const s = app.get();
    const f = s.lastFrame;
    const connected = s.connection === "connected";
    const present = !!f && f.presence !== "none";
    orb.classList.toggle("idle", !f);
    orb.classList.toggle("clear", !present);
    if (!connected) {
      label.textContent = t("conn.notConnected");
      dist.textContent = "—";
      state.textContent = "—";
      since.textContent = "—";
    } else if (!f) {
      label.textContent = t("status.waiting");
      dist.textContent = "—";
      state.textContent = "—";
      since.textContent = "—";
    } else {
      label.textContent = present ? t("status.present") : t("status.nobody");
      dist.textContent = present ? `${formatNumber(f.distanceM, 2)} ${t("units.m")}` : "—";
      state.textContent = t(STATE_KEY[f.presence]);
      const ref = lastPresentAt ?? firstFrameAt;
      since.textContent = present || ref === null ? "—" : `${Math.max(0, Math.floor((Date.now() - ref) / 1000))} ${t("units.s")}`;
    }
    const p = s.draft ?? s.sensor;
    const showPreview = connected && !!f && !!p && isDirty(s.sensor, s.draft);
    preview.hidden = !showPreview;
    preview.textContent = showPreview ? t("status.preview", { state: t(STATE_KEY[previewPresence(f!, p!)]) }) : "";
    pause.disabled = !connected || s.calibrating;
    pause.textContent = (s.streaming ? "⏸ " : "▶ ") + t(s.streaming ? "status.pause" : "status.resume");
    energy.render(connected ? f : null, p, s.series);
    line.render();
  }

  // --- events
  // bumped on every connect/disconnect so stale async work can bail out
  let epoch = 0;
  const live = (e: number) => e === epoch && app.get().connection === "connected";

  pause.addEventListener("click", async () => {
    const ep = epoch;
    const next = !app.get().streaming;
    try {
      await setStreaming(next);
      if (live(ep)) app.set({ streaming: next });
    } catch (e) {
      if (live(ep)) toast("error", errText(e));
    }
  });

  app.subscribe(schedule);
  onLocaleChange(() => { renderI18n(el); schedule(); });
  const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(schedule) : null;
  if (ro) ro.observe(el);
  else window.addEventListener("resize", schedule);
  window.setInterval(() => { if (app.get().lastFrame) schedule(); }, 1000);

  void onFrame((f) => {
    const s = app.get();
    if (s.connection !== "connected") return;
    const now = Date.now();
    firstFrameAt ??= now;
    if (f.presence !== "none") lastPresentAt = now;
    line.push(now, f.presence !== "none" ? f.distanceM : null);
    app.set({ lastFrame: f });
  });

  onConnected(async () => {
    const ep = ++epoch;
    lastPresentAt = null;
    firstFrameAt = null;
    try {
      const params = await readParams();
      if (!live(ep)) return;
      app.set({ sensor: params, draft: params });
      await setStreaming(true);
      if (live(ep)) app.set({ streaming: true });
    } catch (e) {
      if (live(ep)) toast("error", errText(e));
    }
  });
  onDisconnect(() => {
    epoch++;
    line.reset();
    lastPresentAt = null;
    firstFrameAt = null;
    app.set({ lastFrame: null, streaming: false });
  });

  schedule();
}
