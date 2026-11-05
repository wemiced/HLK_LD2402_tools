import { formatNumber, t } from "../i18n";
import { css, fit } from "./canvasUtil";

export interface Sample { t: number; d: number }
export const WINDOW_MS = 60000;
const HEIGHT = 140;
const P = { l: 34, r: 8, t: 8, b: 8 };
const D_MAX = 10;

/** Keeps samples with t >= now - windowMs (boundary kept). */
export function pruneWindow(samples: Sample[], now: number, windowMs = WINDOW_MS): Sample[] {
  return samples.filter((s) => s.t >= now - windowMs);
}

export function createDistanceChart(canvas: HTMLCanvasElement): {
  push(t: number, distanceM: number | null): void;
  render(): void;
  reset(): void;
} {
  let samples: Sample[] = [];
  let now = 0;

  function render(): void {
    const f = fit(canvas, HEIGHT);
    if (!f) return;
    const { ctx: x, w, h } = f;
    const iw = w - P.l - P.r;
    const ih = h - P.t - P.b;
    x.font = "11px Inter, system-ui, sans-serif";
    x.lineWidth = 1;
    const C = { border: css("--border"), muted: css("--muted"), accent: css("--accent"), soft: css("--accent-soft") };
    x.strokeStyle = C.border;
    x.fillStyle = C.muted;
    for (let v = 0; v <= D_MAX; v += 2.5) {
      const y = P.t + ih - (v / D_MAX) * ih;
      x.beginPath(); x.moveTo(P.l, y); x.lineTo(w - P.r, y); x.stroke();
      x.fillText(formatNumber(v, v % 1 ? 1 : 0) + t("units.m"), 2, y + 4);
    }
    const pts = samples.filter((s) => Number.isFinite(s.d));
    if (!pts.length) return;
    const X = (t: number) => P.l + ((t - (now - WINDOW_MS)) / WINDOW_MS) * iw;
    const Y = (d: number) => P.t + ih - (Math.min(D_MAX, Math.max(0, d)) / D_MAX) * ih;
    // contiguous runs (gaps where the sensor saw nobody)
    const runs: Sample[][] = [];
    let cur: Sample[] = [];
    for (const s of samples) {
      if (Number.isFinite(s.d)) cur.push(s);
      else if (cur.length) { runs.push(cur); cur = []; }
    }
    if (cur.length) runs.push(cur);
    for (const run of runs) {
      x.beginPath();
      run.forEach((s, i) => (i ? x.lineTo(X(s.t), Y(s.d)) : x.moveTo(X(s.t), Y(s.d))));
      x.strokeStyle = C.accent;
      x.lineWidth = 2;
      x.lineJoin = "round";
      x.stroke();
      x.lineTo(X(run[run.length - 1].t), P.t + ih);
      x.lineTo(X(run[0].t), P.t + ih);
      x.closePath();
      x.fillStyle = C.soft;
      x.fill();
    }
    x.lineWidth = 1;
  }

  return {
    push(t, distanceM) {
      now = Math.max(now, t);
      samples.push({ t, d: distanceM === null ? NaN : distanceM });
      samples = pruneWindow(samples, now);
    },
    render,
    reset() {
      samples = [];
      now = 0;
    },
  };
}
