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
    cb: {} as Record<string, (p: never) => void>,
    startCalibration: vi.fn(),
    autoGain: vi.fn(),
    readParams: vi.fn(),
    setStreaming: vi.fn(),
    toast: vi.fn(),
    confirm: vi.fn(),
  };
});

vi.mock("../api", () => {
  const reg = (name: string) => vi.fn(async (cb: (p: never) => void) => { h.cb[name] = cb; return () => {}; });
  return {
    ApiError: h.ApiError,
    startCalibration: h.startCalibration,
    autoGain: h.autoGain,
    readParams: h.readParams,
    setStreaming: h.setStreaming,
    onCalibProgress: reg("progress"),
    onCalibDone: reg("done"),
    onCalibFailed: reg("failed"),
    onAutoGainDone: reg("gain"),
  };
});
vi.mock("./notify", () => ({ toast: h.toast, showBanner: vi.fn(), hideBanner: vi.fn() }));
vi.mock("./paramsPanel", () => ({ confirmDiscardIfDirty: h.confirm }));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { mountCalibration, mountCalibrationCard } from "./calibration";

const wait = (ms = 30) => new Promise((r) => setTimeout(r, ms));
const params = { maxDistanceM: 5, disappearDelayS: 30, triggerDb: Array(16).fill(40), microDb: Array(16).fill(30) };
const info = (autoGain: boolean) => ({ firmware: "v3.3.5", serial: "1", power: "ok", capabilities: { autoGain } }) as never;
let el: HTMLElement;
const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;

beforeEach(async () => {
  setLocale("en");
  for (const f of [h.startCalibration, h.autoGain, h.setStreaming, h.toast]) f.mockReset().mockResolvedValue(undefined);
  h.readParams.mockReset().mockResolvedValue(params);
  h.confirm.mockReset().mockResolvedValue(true);
  app.set({ connection: "connected", info: info(true), sensor: null, draft: null, streaming: false, calibrating: false, autoGaining: false, paramsBusy: false });
  document.body.innerHTML = '<section id="c"></section>';
  el = document.getElementById("c")!;
  mountCalibration(el);
  mountCalibrationCard(el.appendChild(document.createElement("div")));
  await wait();
});

it("progress_updates_bar", async () => {
  q("start").click();
  await wait();
  h.cb.progress(40 as never);
  expect(q("bar").style.width).toBe("40%");
  expect(q("pct").textContent).toContain("40");
});

it("done_shows_gates", async () => {
  q("start").click();
  await wait();
  h.cb.done({ gates: [1, 10, 15] } as never);
  await wait();
  expect(el.querySelectorAll(".gates .bad").length).toBe(3);
  expect(el.querySelectorAll(".gates span").length).toBe(16);
  expect(h.readParams).toHaveBeenCalled();
  expect(app.get().sensor).toEqual(params);
  expect(app.get().draft).toEqual(params);
});

it("done with no gates says no interference", async () => {
  q("start").click();
  await wait();
  h.cb.done({ gates: [] } as never);
  await wait();
  expect(el.querySelectorAll(".gates .bad").length).toBe(0);
  expect(q("result-title").textContent).toBe("No interference");
});

it("pauses streaming and restores it afterwards", async () => {
  app.set({ streaming: true });
  q("start").click();
  await wait();
  expect(h.setStreaming).toHaveBeenCalledWith(false);
  expect(h.startCalibration).toHaveBeenCalledWith({ trigger: 3, hold: 3, micro: 3 });
  h.cb.done({ gates: [] } as never);
  await wait();
  expect(h.setStreaming).toHaveBeenLastCalledWith(true);
  expect(app.get().streaming).toBe(true);
});

it("calib failed returns to prepare with an error toast and restores streaming", async () => {
  app.set({ streaming: true });
  q("start").click();
  await wait();
  h.cb.failed({ error: { kind: "busy" } } as never);
  await wait();
  expect(h.toast).toHaveBeenCalledWith("error", "The sensor is busy with another operation.");
  expect(q("step-prepare").hidden).toBe(false);
  expect(q<HTMLButtonElement>("start").disabled).toBe(false);
  expect(h.setStreaming).toHaveBeenLastCalledWith(true);
});

it("shows a connect notice and disables Start when disconnected; reset on disconnect", async () => {
  q("start").click();
  await wait();
  expect(q("step-measure").hidden).toBe(false);
  app.set({ connection: "disconnected", info: null });
  await wait();
  expect(q("step-prepare").hidden).toBe(false);
  expect(q("notice").hidden).toBe(false);
  expect(q<HTMLButtonElement>("start").disabled).toBe(true);
  expect(h.toast).not.toHaveBeenCalled();
});

it("does not start when discarding a dirty draft is refused", async () => {
  h.confirm.mockResolvedValue(false);
  app.set({ sensor: params, draft: { ...params, maxDistanceM: 6 } });
  q("start").click();
  await wait();
  expect(h.startCalibration).not.toHaveBeenCalled();
});

it("auto_gain_disabled_on_old_fw", async () => {
  app.set({ info: info(false) });
  const b = q<HTMLButtonElement>("autogain");
  expect(b.disabled).toBe(true);
  expect(b.title).toBe("Requires firmware 3.3.5 or later");
});

it("auto gain shows busy state until done, then a success toast", async () => {
  const b = q<HTMLButtonElement>("autogain");
  expect(b.disabled).toBe(false);
  b.click();
  await wait();
  expect(h.autoGain).toHaveBeenCalled();
  expect(b.disabled).toBe(true);
  expect(b.classList.contains("busy")).toBe(true);
  h.cb.gain(null as never);
  expect(h.toast).toHaveBeenCalledWith("success", expect.any(String));
  expect(b.disabled).toBe(false);
});

it("auto gain error toasts and resets", async () => {
  h.autoGain.mockRejectedValue(new h.ApiError({ kind: "busy" }));
  const b = q<HTMLButtonElement>("autogain");
  b.click();
  await wait();
  expect(h.toast).toHaveBeenCalledWith("error", "The sensor is busy with another operation.");
  expect(b.disabled).toBe(false);
});

it("calibration watchdog: silence for 15 s aborts with a toast and restores streaming", async () => {
  app.set({ streaming: true });
  vi.useFakeTimers();
  try {
    q("start").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(app.get().calibrating).toBe(true);
    await vi.advanceTimersByTimeAsync(10_000);
    h.cb.progress(10 as never); // progress re-arms the watchdog
    await vi.advanceTimersByTimeAsync(10_000);
    expect(q("step-measure").hidden).toBe(false);
    await vi.advanceTimersByTimeAsync(5_001);
    expect(h.toast).toHaveBeenCalledWith("error", "The calibration did not finish: the sensor stopped responding.");
    expect(q("step-prepare").hidden).toBe(false);
    expect(app.get().calibrating).toBe(false);
    expect(h.setStreaming).toHaveBeenLastCalledWith(true);
  } finally {
    vi.useRealTimers();
  }
});

it("auto gain watchdog: no AutoGainDone within 30 s resets with an error toast", async () => {
  vi.useFakeTimers();
  try {
    const b = q<HTMLButtonElement>("autogain");
    b.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(b.getAttribute("aria-busy")).toBe("true");
    await vi.advanceTimersByTimeAsync(30_001);
    expect(h.toast).toHaveBeenCalledWith("error", "The gain adjustment did not finish in time.");
    expect(b.disabled).toBe(false);
    expect(b.getAttribute("aria-busy")).toBe("false");
  } finally {
    vi.useRealTimers();
  }
});

it("a second Start click while the discard dialog is open is ignored", async () => {
  app.set({ sensor: params, draft: { ...params, maxDistanceM: 6 } });
  h.confirm.mockImplementation(() => new Promise((r) => setTimeout(() => r(true), 40)));
  q("start").click();
  q("start").click();
  await wait(100);
  expect(h.confirm).toHaveBeenCalledTimes(1);
  expect(h.startCalibration).toHaveBeenCalledTimes(1);
});

it("auto gain, calibration and parameter I/O exclude each other", async () => {
  const gain = q<HTMLButtonElement>("autogain");
  const start = q<HTMLButtonElement>("start");
  gain.click();
  await wait();
  expect(app.get().autoGaining).toBe(true);
  expect(start.disabled).toBe(true);
  start.click();
  await wait();
  expect(h.startCalibration).not.toHaveBeenCalled();
  h.cb.gain(null as never);
  expect(app.get().autoGaining).toBe(false);
  expect(start.disabled).toBe(false);

  app.set({ paramsBusy: true });
  expect(start.disabled).toBe(true);
  expect(gain.disabled).toBe(true);
  app.set({ paramsBusy: false });

  start.click();
  await wait();
  expect(app.get().calibrating).toBe(true);
  expect(gain.disabled).toBe(true);
});

it("calibration watchdog is only re-armed by progress that increases", async () => {
  vi.useFakeTimers();
  try {
    q("start").click();
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(5_000);
    h.cb.progress(10 as never); // deadline now 20 s
    for (let i = 0; i < 3; i++) {
      await vi.advanceTimersByTimeAsync(4_000);
      h.cb.progress(10 as never); // repeated value: no re-arm
    }
    await vi.advanceTimersByTimeAsync(3_001);
    expect(h.toast).toHaveBeenCalledWith("error", "The calibration did not finish: the sensor stopped responding.");
    expect(q("step-prepare").hidden).toBe(false);
  } finally {
    vi.useRealTimers();
  }
});

it("a calibration is aborted after 120 s even if progress keeps moving", async () => {
  vi.useFakeTimers();
  try {
    q("start").click();
    await vi.advanceTimersByTimeAsync(0);
    for (let p = 5; p <= 55; p += 5) {
      await vi.advanceTimersByTimeAsync(10_000);
      h.cb.progress(p as never);
    }
    await vi.advanceTimersByTimeAsync(9_900); // 119.9 s
    expect(q("step-measure").hidden).toBe(false);
    await vi.advanceTimersByTimeAsync(101);
    expect(h.toast).toHaveBeenCalledWith("error", "The calibration did not finish: the sensor stopped responding.");
    expect(app.get().calibrating).toBe(false);
  } finally {
    vi.useRealTimers();
  }
});
