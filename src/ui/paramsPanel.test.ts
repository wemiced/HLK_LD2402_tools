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
    readParams: vi.fn(),
    writeParams: vi.fn(),
    exportConfig: vi.fn(),
    importConfig: vi.fn(),
    ask: vi.fn(),
    save: vi.fn(),
    open: vi.fn(),
    toast: vi.fn(),
  };
});

vi.mock("../api", () => ({
  ApiError: h.ApiError,
  readParams: h.readParams,
  writeParams: h.writeParams,
  exportConfig: h.exportConfig,
  importConfig: h.importConfig,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: h.ask, save: h.save, open: h.open }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: vi.fn(async () => () => {}), destroy: vi.fn() }),
}));
vi.mock("./notify", () => ({ toast: h.toast, showBanner: vi.fn(), hideBanner: vi.fn() }));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { confirmDiscardIfDirty, mountParamsPanel } from "./paramsPanel";
import { mountGateTable } from "./gateTable";

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
const params = {
  maxDistanceM: 5,
  disappearDelayS: 30,
  triggerDb: Array(16).fill(40),
  microDb: Array(16).fill(30),
};
let el: HTMLElement;
const q = <T extends HTMLElement>(role: string) => el.querySelector<T>(`[data-role="${role}"]`)!;

beforeEach(async () => {
  setLocale("en");
  for (const m of [h.readParams, h.writeParams, h.exportConfig, h.importConfig, h.ask, h.save, h.open, h.toast]) m.mockReset();
  h.readParams.mockResolvedValue(params);
  h.writeParams.mockResolvedValue(undefined);
  h.ask.mockResolvedValue(true);
  app.set({
    connection: "connected",
    info: { firmware: "v1", serial: "1", capabilities: { permanentSave: true, autoGain: true, serialText: true }, power: "ok" } as never,
    sensor: params,
    draft: params,
    series: "trigger",
  });
  document.body.innerHTML = '<div id="p"></div><aside id="d"></aside>';
  el = document.getElementById("p")!;
  mountParamsPanel(el);
  await wait();
});

function slide(role: string, v: string) {
  const r = q<HTMLInputElement>(role);
  r.value = v;
  r.dispatchEvent(new Event("input"));
}

it("write_sends_draft_then_rereads", async () => {
  slide("dist", "6");
  expect(app.get().draft!.maxDistanceM).toBe(6);
  expect(q("dirty").classList.contains("show")).toBe(true);
  expect(q("perm").hidden).toBe(false);
  h.readParams.mockResolvedValue({ ...params, maxDistanceM: 6 });
  q<HTMLButtonElement>("write").click();
  await wait();
  expect(h.writeParams).toHaveBeenCalledWith(expect.objectContaining({ maxDistanceM: 6 }));
  expect(h.readParams).toHaveBeenCalled();
  expect(app.get().sensor!.maxDistanceM).toBe(6);
  expect(q("dirty").classList.contains("show")).toBe(false);
  expect(h.toast).toHaveBeenCalledWith("success", expect.any(String));
});

it("undo_restores_sensor_values", () => {
  slide("dist", "6");
  q<HTMLButtonElement>("undo").click();
  expect(app.get().draft).toEqual(params);
  expect(q("dirty").classList.contains("show")).toBe(false);
});

it("invalid_table_input_not_applied", () => {
  const d = document.getElementById("d")!;
  mountGateTable(d);
  const inputs = d.querySelectorAll<HTMLInputElement>("tbody input");
  expect(d.querySelectorAll("tbody tr").length).toBe(16);
  for (const bad of ["abc", "96"]) {
    inputs[0].value = bad;
    inputs[0].dispatchEvent(new Event("input"));
    expect(inputs[0].classList.contains("invalid")).toBe(true);
    expect(app.get().draft).toEqual(params);
  }
  inputs[0].value = "41,5";
  inputs[0].dispatchEvent(new Event("input"));
  expect(inputs[0].classList.contains("invalid")).toBe(false);
  expect(app.get().draft!.triggerDb[0]).toBe(41.5);
});

it("close_guard_cancel_keeps_window", async () => {
  expect(await confirmDiscardIfDirty()).toBe(true);
  expect(h.ask).not.toHaveBeenCalled();
  slide("dist", "6");
  h.ask.mockResolvedValue(false);
  expect(await confirmDiscardIfDirty()).toBe(false);
  expect(h.ask).toHaveBeenCalled();
});

it("import_sets_draft_and_error_toasts", async () => {
  h.open.mockResolvedValue("/x.json");
  h.importConfig.mockResolvedValue({ ...params, disappearDelayS: 99 });
  q<HTMLButtonElement>("import").click();
  await wait();
  expect(app.get().draft!.disappearDelayS).toBe(99);
  expect(app.get().sensor!.disappearDelayS).toBe(30);
  h.importConfig.mockRejectedValue(new h.ApiError({ kind: "config", reason: "notAConfig" }));
  q<HTMLButtonElement>("import").click();
  await wait();
  expect(h.toast).toHaveBeenCalledWith("error", expect.stringContaining("not an LD2402"));
});

it("seg_sets_series", () => {
  el.querySelectorAll<HTMLButtonElement>(".seg button")[1].click();
  expect(app.get().series).toBe("micro");
});
