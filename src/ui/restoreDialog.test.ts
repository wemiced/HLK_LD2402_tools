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
    snapshotStatus: vi.fn(),
    previewRestore: vi.fn(),
    ask: vi.fn(),
    toast: vi.fn(),
  };
});

vi.mock("../api", () => ({
  ApiError: h.ApiError,
  readParams: h.readParams,
  writeParams: h.writeParams,
  exportConfig: h.exportConfig,
  importConfig: h.importConfig,
  snapshotStatus: h.snapshotStatus,
  previewRestore: h.previewRestore,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: h.ask, save: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, close: vi.fn() }) }));
vi.mock("./notify", () => ({ toast: h.toast, showBanner: vi.fn(), hideBanner: vi.fn() }));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { mountParamsPanel } from "./paramsPanel";

const wait = (ms = 20) => new Promise((r) => setTimeout(r, ms));
const params = { maxDistanceM: 5, disappearDelayS: 30, triggerDb: Array(16).fill(40), microDb: Array(16).fill(30) };
const restored = { ...params, maxDistanceM: 3 };
const meta = (kind: "original" | "backup") => ({ kind, savedAt: "2026-11-05T10:00:00Z", file: "x.json" });
const status = { original: meta("original"), latestBackup: meta("backup"), unknownSerial: false, originalError: null };
const preview = { meta: meta("original"), params: restored, clamped: [], unknownSerial: false };
let el: HTMLElement;
const restoreBtn = () => el.querySelector<HTMLButtonElement>('[data-role="restore"]')!;
const modal = () => document.querySelector(".modal");
const modalBtn = (re: RegExp) => Array.from(document.querySelectorAll<HTMLButtonElement>(".modal button")).find((b) => re.test(b.textContent ?? ""))!;

beforeEach(async () => {
  setLocale("en");
  for (const m of [h.readParams, h.writeParams, h.snapshotStatus, h.previewRestore, h.ask, h.toast]) m.mockReset();
  h.readParams.mockResolvedValue(restored);
  h.writeParams.mockResolvedValue(undefined);
  h.previewRestore.mockResolvedValue(preview);
  h.ask.mockResolvedValue(true);
  app.set({
    connection: "connected",
    info: { firmware: "v1", serial: "1", capabilities: { permanentSave: true, autoGain: true, serialText: true }, power: "ok" } as never,
    sensor: params, draft: params, calibrating: false, autoGaining: false, paramsBusy: false, snapshots: status,
  });
  document.body.innerHTML = '<div id="p"></div>';
  el = document.getElementById("p")!;
  mountParamsPanel(el);
  await wait();
});

it("restore_button_disabled_without_snapshots", () => {
  expect(restoreBtn().disabled).toBe(false);
  app.set({ snapshots: { ...status, original: null, latestBackup: null } });
  expect(restoreBtn().disabled).toBe(true);
  app.set({ snapshots: null });
  expect(restoreBtn().disabled).toBe(true);
  app.set({ snapshots: status, calibrating: true });
  expect(restoreBtn().disabled).toBe(true);
});

it("restore_original_flow_writes_preview_params", async () => {
  restoreBtn().click();
  await wait();
  expect(modal()!.getAttribute("role")).toBe("dialog");
  expect(modal()!.textContent).toContain("Settings at first connection");
  expect(modal()!.textContent).toContain("Before the last change");
  modalBtn(/first connection/).click();
  await wait();
  expect(h.previewRestore).toHaveBeenCalledWith("original");
  modalBtn(/^Restore$/).click();
  await wait();
  expect(h.writeParams).toHaveBeenCalledWith(restored);
  expect(h.readParams).toHaveBeenCalled();
  expect(app.get().sensor).toEqual(restored);
  expect(h.toast).toHaveBeenCalledWith("success", "Settings restored.");
  expect(modal()).toBeNull();
});

it("only_present_choices_are_offered", async () => {
  app.set({ snapshots: { ...status, original: null } });
  restoreBtn().click();
  await wait();
  expect(modal()!.textContent).not.toContain("first connection");
  expect(modal()!.textContent).toContain("Before the last change");
});

it("restore_shows_clamped_fields_and_unknown_serial_warning", async () => {
  h.previewRestore.mockResolvedValue({
    ...preview,
    unknownSerial: true,
    clamped: [{ field: "maxDistanceM", from: 12, to: 10 }, { field: "triggerDb[3]", from: 99, to: 95 }],
  });
  restoreBtn().click();
  await wait();
  modalBtn(/Before the last change/).click();
  await wait();
  const text = modal()!.textContent!;
  expect(text).toContain("no serial number");
  expect(text).toContain("another sensor");
  expect(text).toContain("outside the manual's ranges");
  expect(text).toContain("Maximum distance: 12.0 m → 10.0 m");
  expect(text).toContain("Gate 4 trigger threshold: 99.00 dB → 95.00 dB");
  expect(modal()!.querySelector(".callout.danger")).not.toBeNull();
});

it("restore_cancel_sends_nothing", async () => {
  restoreBtn().click();
  await wait();
  modalBtn(/first connection/).click();
  await wait();
  modalBtn(/^Cancel$/).click();
  await wait();
  expect(h.writeParams).not.toHaveBeenCalled();
  expect(modal()).toBeNull();
  // Esc on the first dialog also cancels
  restoreBtn().click();
  await wait();
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  await wait();
  expect(modal()).toBeNull();
  expect(h.previewRestore).toHaveBeenCalledTimes(1);
  expect(h.writeParams).not.toHaveBeenCalled();
});

it("restore_disconnect_mid_write", async () => {
  let release!: () => void;
  h.writeParams.mockReturnValue(new Promise<void>((r) => { release = r; }));
  restoreBtn().click();
  await wait();
  modalBtn(/first connection/).click();
  await wait();
  modalBtn(/^Restore$/).click();
  await wait();
  expect(h.writeParams).toHaveBeenCalled();
  app.set({ connection: "disconnected", info: null });
  release();
  await wait();
  expect(h.toast).not.toHaveBeenCalled();
  expect(app.get().sensor).toEqual(params);
  expect(app.get().draft).toEqual(params);
});

it("restore_error_keeps_draft_and_toasts", async () => {
  const draft = { ...params, maxDistanceM: 6 };
  app.set({ draft });
  h.writeParams.mockRejectedValue(new h.ApiError({ kind: "backupFailed", message: "disk full" }));
  restoreBtn().click();
  await wait();
  modalBtn(/first connection/).click();
  await wait();
  modalBtn(/^Restore$/).click();
  await wait();
  expect(h.ask).toHaveBeenCalled(); // dirty draft: discard confirmation first
  expect(h.toast).toHaveBeenCalledWith("error", expect.stringContaining("disk full"));
  expect(app.get().draft).toEqual(draft);
});
