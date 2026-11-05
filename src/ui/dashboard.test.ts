import { beforeEach, expect, it, vi } from "vitest";

const h = vi.hoisted(() => {
  class ApiError extends Error {
    kind: string;
    data: Record<string, unknown>;
    constructor(data: Record<string, unknown>) {
      super(String(data.kind));
      this.kind = String(data.kind);
      this.data = data;
    }
  }
  return {
    ApiError,
    frameCb: null as null | ((f: unknown) => void),
    readParams: vi.fn(),
    setStreaming: vi.fn(),
    toast: vi.fn(),
  };
});

vi.mock("../api", () => ({
  ApiError: h.ApiError,
  readParams: h.readParams,
  setStreaming: h.setStreaming,
  writeParams: vi.fn(),
  exportConfig: vi.fn(),
  importConfig: vi.fn(),
  onFrame: vi.fn(async (cb: (f: unknown) => void) => {
    h.frameCb = cb;
    return () => {};
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(), save: vi.fn(), open: vi.fn() }));
vi.mock("./notify", () => ({ toast: h.toast, showBanner: vi.fn(), hideBanner: vi.fn() }));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { dashboardSlot, mountDashboard } from "./dashboard";
import { mountParamsPanel } from "./paramsPanel";

const wait = (ms = 60) => new Promise((r) => setTimeout(r, ms));
const params = {
  maxDistanceM: 5,
  disappearDelayS: 30,
  triggerDb: Array(16).fill(40),
  microDb: Array(16).fill(30),
};
HTMLCanvasElement.prototype.getContext = (() => null) as never;
let el: HTMLElement;
const text = (role: string) => el.querySelector(`[data-role="${role}"]`)!.textContent;

beforeEach(async () => {
  setLocale("en");
  h.readParams.mockReset().mockResolvedValue(params);
  h.setStreaming.mockReset().mockResolvedValue(undefined);
  app.set({ connection: "disconnected", sensor: null, draft: null, streaming: false, lastFrame: null });
  document.body.innerHTML = '<section id="d"></section>';
  el = document.getElementById("d")!;
  mountDashboard(el);
  await wait();
});

it("exposes empty slots and a neutral state when disconnected", () => {
  expect(dashboardSlot("params", el)).not.toBeNull();
  expect(dashboardSlot("calib", el)).not.toBeNull();
  expect(text("label")).toBe("Not connected");
  expect(el.querySelector<HTMLButtonElement>('[data-role="pause"]')!.disabled).toBe(true);
});

it("connects, loads params, streams, and shows the live state and preview", async () => {
  app.set({ connection: "connected" });
  await wait();
  expect(h.setStreaming).toHaveBeenCalledWith(true);
  expect(app.get().draft).toEqual(params);
  expect(text("label")).toBe("Waiting for data…");

  const frame = { presence: "none", distanceM: 0, motionDb: Array(16).fill(30), microDb: Array(16).fill(10) };
  h.frameCb!({ ...frame, presence: "moving", distanceM: 2.35 });
  await wait();
  expect(text("label")).toBe("Presence detected");
  expect(text("dist")).toBe("2.35 m");
  expect(text("state")).toBe("Motion");
  expect(el.querySelector<HTMLElement>('[data-role="preview"]')!.hidden).toBe(true);

  app.set({ draft: { ...params, triggerDb: Array(16).fill(10) } });
  await wait();
  const pv = el.querySelector<HTMLElement>('[data-role="preview"]')!;
  expect(pv.hidden).toBe(false);
  expect(pv.textContent).toBe("Preview with unsaved thresholds: Motion");

  app.set({ connection: "disconnected" });
  await wait();
  expect(app.get().lastFrame).toBeNull();
  expect(app.get().streaming).toBe(false);
  expect(app.get().draft).not.toBeNull();
});

it("disconnect while readParams is pending: no streaming, no toast", async () => {
  h.toast.mockReset();
  let resolve!: (p: unknown) => void;
  h.readParams.mockReset().mockReturnValue(new Promise((r) => (resolve = r)));
  app.set({ connection: "connected" });
  await wait();
  app.set({ connection: "disconnected" });
  resolve(params);
  await wait();
  expect(h.setStreaming).not.toHaveBeenCalled();
  expect(h.toast).not.toHaveBeenCalled();
  expect(app.get().streaming).toBe(false);
});

it("Pause resolving after disconnect leaves streaming false", async () => {
  app.set({ connection: "connected" });
  await wait();
  expect(app.get().streaming).toBe(true);
  let resolve!: () => void;
  h.setStreaming.mockReset().mockReturnValue(new Promise<void>((r) => (resolve = r)));
  el.querySelector<HTMLButtonElement>('[data-role="pause"]')!.click();
  app.set({ connection: "disconnected", streaming: false });
  resolve();
  await wait();
  expect(app.get().streaming).toBe(false);
});

it("disables pause/resume while a calibration is measuring", async () => {
  app.set({ connection: "connected" });
  await wait();
  const pause = el.querySelector<HTMLButtonElement>('[data-role="pause"]')!;
  expect(pause.disabled).toBe(false);
  app.set({ calibrating: true });
  await wait();
  expect(pause.disabled).toBe(true);
  app.set({ calibrating: false });
});

it("reconnect with a failing initial read drops the previous session's params", async () => {
  mountParamsPanel(dashboardSlot("params", el)!);
  app.set({ connection: "connected" });
  await wait();
  expect(app.get().sensor).toEqual(params);
  app.set({ connection: "disconnected" });
  await wait();
  h.toast.mockReset();
  h.readParams.mockReset().mockRejectedValue(new h.ApiError({ kind: "busy" }));
  app.set({ connection: "connected" });
  await wait();
  expect(app.get().sensor).toBeNull();
  expect(app.get().draft).toBeNull();
  expect(h.toast).toHaveBeenCalledWith("error", "The sensor is busy with another operation.");
  const panel = dashboardSlot("params", el)!;
  expect(panel.querySelector<HTMLButtonElement>('[data-role="write"]')!.disabled).toBe(true);
  expect(panel.querySelector<HTMLInputElement>('[data-role="dist"]')!.disabled).toBe(true);
  expect(panel.querySelector('[data-role="distVal"]')!.textContent).toBe("—");
});
