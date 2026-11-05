export const css = (name: string): string =>
  getComputedStyle(document.body).getPropertyValue(name).trim() || "#888";

export interface Fit {
  ctx: CanvasRenderingContext2D;
  w: number;
  h: number;
}

/** Sizes the backing store for devicePixelRatio; returns null when no 2D context (jsdom). */
export function fit(canvas: HTMLCanvasElement, cssHeight: number): Fit | null {
  let ctx: CanvasRenderingContext2D | null = null;
  try {
    ctx = canvas.getContext("2d");
  } catch {
    ctx = null;
  }
  if (!ctx) return null;
  const w = canvas.clientWidth || canvas.getBoundingClientRect().width;
  if (!w) return null;
  const d = window.devicePixelRatio || 1;
  canvas.style.height = cssHeight + "px";
  const pw = Math.round(w * d);
  const ph = Math.round(cssHeight * d);
  if (canvas.width !== pw) canvas.width = pw;
  if (canvas.height !== ph) canvas.height = ph;
  ctx.setTransform(d, 0, 0, d, 0, 0);
  ctx.clearRect(0, 0, w, cssHeight);
  return { ctx, w, h: cssHeight };
}

export function roundTopRect(
  x: CanvasRenderingContext2D, a: number, b: number, w: number, h: number, r: number,
): void {
  if (h <= 0) return;
  r = Math.min(r, h / 2, w / 2);
  x.beginPath();
  x.moveTo(a, b + h);
  x.lineTo(a, b + r);
  x.quadraticCurveTo(a, b, a + r, b);
  x.lineTo(a + w - r, b);
  x.quadraticCurveTo(a + w, b, a + w, b + r);
  x.lineTo(a + w, b + h);
  x.closePath();
  x.fill();
}
