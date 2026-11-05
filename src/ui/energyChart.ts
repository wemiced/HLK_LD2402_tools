import type { FrameDto, SensorParams } from "../types";
import { gatesInRange } from "../state/draft";
import { css, fit, roundTopRect } from "./canvasUtil";

export const Y_MAX = 95;
export const GATES = 16;
const HEIGHT = 230;
const P = { l: 34, r: 8, t: 8, b: 24 };

export function dbToY(db: number, plotH: number): number {
  return plotH - (Math.min(Y_MAX, Math.max(0, db)) / Y_MAX) * plotH;
}

export function yToDb(y: number, plotH: number): number {
  const db = (1 - y / plotH) * Y_MAX;
  return Math.round(Math.min(Y_MAX, Math.max(0, db)) * 100) / 100;
}

/** x is relative to the plot's left edge. */
export function gateAtX(x: number, plotW: number): number | null {
  if (!(x >= 0) || x >= plotW) return null;
  return Math.min(GATES - 1, Math.floor((x / plotW) * GATES));
}

export function createEnergyChart(
  canvas: HTMLCanvasElement,
  opts: { onDrag: (gate: number, db: number) => void },
): { render(frame: FrameDto | null, params: SensorParams | null, series: "trigger" | "micro"): void } {
  let last: { frame: FrameDto | null; params: SensorParams | null; series: "trigger" | "micro" } = {
    frame: null, params: null, series: "trigger",
  };
  let drag: number | null = null;

  const local = (e: PointerEvent) => {
    const r = canvas.getBoundingClientRect();
    return { x: e.clientX - r.left - P.l, y: e.clientY - r.top - P.t, plotW: r.width - P.l - P.r };
  };
  const plotH = HEIGHT - P.t - P.b;

  canvas.style.cursor = "ns-resize";
  canvas.style.touchAction = "none";
  canvas.addEventListener("pointerdown", (e) => {
    const p = local(e);
    const g = gateAtX(p.x, p.plotW);
    if (g === null) return;
    drag = g;
    try { canvas.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    opts.onDrag(g, yToDb(p.y, plotH));
  });
  canvas.addEventListener("pointermove", (e) => {
    if (drag === null) return;
    opts.onDrag(drag, yToDb(local(e).y, plotH));
  });
  const end = () => { drag = null; };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);

  function draw(): void {
    const f = fit(canvas, HEIGHT);
    if (!f) return;
    const { ctx: x, w, h } = f;
    const { frame, params, series } = last;
    const iw = w - P.l - P.r;
    const ih = h - P.t - P.b;
    const bw = iw / GATES;
    const inRange = params ? gatesInRange(params.maxDistanceM) : GATES;
    const energy = frame ? (series === "trigger" ? frame.motionDb : frame.microDb) : null;
    const sel = series === "trigger" ? params?.triggerDb : params?.microDb;
    const other = series === "trigger" ? params?.microDb : params?.triggerDb;
    const selColor = css(series === "trigger" ? "--present" : "--micro");
    const otherColor = css(series === "trigger" ? "--micro" : "--present");

    x.font = "11px Inter, system-ui, sans-serif";
    x.lineWidth = 1;
    x.strokeStyle = css("--border");
    x.fillStyle = css("--muted");
    for (let v = 0; v <= Y_MAX; v += 20) {
      const y = P.t + dbToY(v, ih);
      x.beginPath(); x.moveTo(P.l, y); x.lineTo(w - P.r, y); x.stroke();
      x.fillText(String(v), 4, y + 4);
    }
    for (let i = 0; i < GATES; i++) {
      const X = P.l + i * bw;
      x.globalAlpha = i < inRange ? 1 : 0.25;
      const e = energy?.[i];
      if (e !== undefined && Number.isFinite(e)) {
        const top = P.t + dbToY(e, ih);
        const g = x.createLinearGradient(0, top, 0, P.t + ih);
        g.addColorStop(0, css("--accent"));
        g.addColorStop(1, css("--accent-soft"));
        x.fillStyle = g;
        roundTopRect(x, X + bw * 0.18, top, bw * 0.64, P.t + ih - top, 5);
      }
      const line = (v: number | undefined, color: string, alpha: number, width: number) => {
        if (v === undefined) return;
        const y = P.t + dbToY(v, ih);
        const prev = x.globalAlpha;
        x.globalAlpha = prev * alpha;
        x.strokeStyle = color;
        x.lineWidth = width;
        x.beginPath(); x.moveTo(X + bw * 0.1, y); x.lineTo(X + bw * 0.9, y); x.stroke();
        x.globalAlpha = prev;
      };
      line(other?.[i], otherColor, 0.35, 2.5);
      line(sel?.[i], selColor, 1, 2.5);
      x.globalAlpha = 1;
      x.fillStyle = css("--muted");
      x.fillText((i * 0.7).toFixed(1), X + bw * 0.28, h - 6);
    }
    x.lineWidth = 1;
  }

  return {
    render(frame, params, series) {
      last = { frame, params, series };
      draw();
    },
  };
}
