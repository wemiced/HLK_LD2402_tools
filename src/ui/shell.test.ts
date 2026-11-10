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
  const info = {
    firmware: "v3.3.5",
    serial: "1A2B3C4D",
    capabilities: { permanentSave: true, autoGain: true, serialText: true },
    power: "ok",
  };
  const ports = [
    { path: "/dev/ttyACM0", label: "ttyACM0", usbSerial: true, detected: false },
    { path: "/dev/ttyUSB0", label: "ttyUSB0", usbSerial: true, detected: true },
  ];
  return {
    ApiError,
    info,
    ports,
    disconnectedCb: null as null | ((p: { reason: string }) => void),
    listPorts: vi.fn(),
    connect: vi.fn(),
    disconnect: vi.fn(),
    ask: vi.fn(),
  };
});

vi.mock("../api", () => ({
  ApiError: h.ApiError,
  listPorts: h.listPorts,
  connect: h.connect,
  disconnect: h.disconnect,
  onDisconnected: vi.fn(async (cb: (p: { reason: string }) => void) => {
    h.disconnectedCb = cb;
    return () => {};
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: h.ask }));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { mountShell } from "./shell";

const flush = () => new Promise((r) => setTimeout(r, 0));
let root: HTMLElement;

async function mount(ports = h.ports) {
  h.listPorts.mockResolvedValue(ports);
  document.body.innerHTML = '<div id="app"></div>';
  root = document.getElementById("app")!;
  mountShell(root);
  await flush();
}
const connectBtn = () => root.querySelector<HTMLButtonElement>("#connect-btn")!;

beforeEach(async () => {
  localStorage.clear();
  document.querySelectorAll(".toasts").forEach((e) => e.remove());
  h.connect.mockReset().mockResolvedValue(h.info);
  h.disconnect.mockReset().mockResolvedValue(undefined);
  h.ask.mockReset().mockResolvedValue(true);
  app.set({ connection: "disconnected", info: null, port: null, view: "dashboard" });
  setLocale("en");
  await mount();
});

it("renders 2 view buttons and switches views", () => {
  const btns = root.querySelectorAll<HTMLButtonElement>(".rail button[data-view]");
  expect(btns.length).toBe(2);
  btns[1].click();
  expect(app.get().view).toBe("calibration");
  expect(root.querySelector('section[data-view="calibration"]')).not.toBeNull();
});

it("preselects the detected port and shows version/SN after connect", async () => {
  const sel = root.querySelector<HTMLSelectElement>("#port-select")!;
  expect(sel.value).toBe("/dev/ttyUSB0");
  expect(sel.selectedOptions[0].textContent).toContain("detected");
  connectBtn().click();
  await flush();
  expect(root.querySelector(".sub")!.textContent).toBe("HLK-LD2402 · v3.3.5 · SN 1A2B3C4D");
  expect(app.get().connection).toBe("connected");
});

it("connect failure shows an error toast with the path and returns to disconnected", async () => {
  h.connect.mockRejectedValue(new h.ApiError({ kind: "permissionDenied", path: "/dev/ttyUSB0" }));
  connectBtn().click();
  await flush();
  expect(app.get().connection).toBe("disconnected");
  const toast = document.querySelector(".toast.error")!;
  expect(toast.textContent).toContain("/dev/ttyUSB0");
  expect(toast.textContent).toContain("dialout");
});

it("Connect is disabled while connecting", async () => {
  let resolve!: (v: unknown) => void;
  h.connect.mockReturnValue(new Promise((r) => (resolve = r)));
  connectBtn().click();
  expect(app.get().connection).toBe("connecting");
  expect(connectBtn().disabled).toBe(true);
  connectBtn().click();
  expect(h.connect).toHaveBeenCalledTimes(1);
  resolve(h.info);
  await flush();
  expect(connectBtn().disabled).toBe(false);
});

it("device://disconnected event resets state and toasts", async () => {
  connectBtn().click();
  await flush();
  h.disconnectedCb!({ reason: "unplugged" });
  expect(app.get().connection).toBe("disconnected");
  expect(document.querySelector(".toast.error")!.textContent).toContain("disconnected");
});

it("user disconnect sets state directly without an event", async () => {
  connectBtn().click();
  await flush();
  connectBtn().click();
  await flush();
  expect(h.disconnect).toHaveBeenCalled();
  expect(app.get().connection).toBe("disconnected");
  expect(app.get().info).toBeNull();
});

it("discarding on disconnect drops the edits so reconnecting does not ask again", async () => {
  const sensor = { maxDistanceM: 5, disappearDelayS: 30, triggerDb: Array(16).fill(40), microDb: Array(16).fill(30) };
  connectBtn().click();
  await flush();
  app.set({ sensor, draft: { ...sensor, maxDistanceM: 6 } });
  h.ask.mockResolvedValue(false);
  connectBtn().click();
  await flush();
  expect(app.get().connection).toBe("connected");
  expect(app.get().draft!.maxDistanceM).toBe(6);
  h.ask.mockResolvedValue(true);
  connectBtn().click();
  await flush();
  expect(app.get().connection).toBe("disconnected");
  expect(app.get().draft).toEqual(sensor);
  h.ask.mockClear();
  connectBtn().click();
  await flush();
  expect(h.ask).not.toHaveBeenCalled();
  expect(app.get().connection).toBe("connected");
});

it("language switch re-renders shell text live", () => {
  expect(root.querySelector("#title")!.textContent).toBe("Dashboard");
  setLocale("fr");
  expect(root.querySelector("#title")!.textContent).toBe("Tableau de bord");
  expect(connectBtn().textContent).toBe("Connecter");
  expect(root.querySelector(".rail [data-view]")!.getAttribute("aria-label")).toBe("Tableau de bord");
});

it("theme toggle flips body data-theme, persists it and swaps the icon", () => {
  const btn = root.querySelector<HTMLButtonElement>("#theme-toggle")!;
  const before = document.body.dataset.theme;
  const iconBefore = btn.querySelector("svg")!.innerHTML;
  btn.click();
  expect(document.body.dataset.theme).not.toBe(before);
  expect(localStorage.getItem("ld2402.theme")).toBe(document.body.dataset.theme);
  expect(btn.querySelector("svg")!.innerHTML).not.toBe(iconBefore);
});

it("empty port list label is translated and survives a locale change", async () => {
  await mount([]);
  const sel = root.querySelector<HTMLSelectElement>("#port-select")!;
  expect(sel.options[0].textContent).toBe("No serial port found");
  setLocale("fr");
  expect(sel.options.length).toBe(1);
  expect(sel.options[0].textContent).toBe("Aucun port série trouvé");
});

it("a port held by another program gets its own message", async () => {
  h.connect.mockRejectedValue(new h.ApiError({ kind: "inUse", path: "/dev/ttyUSB0" }));
  connectBtn().click();
  await flush();
  const toast = document.querySelector(".toast.error")!;
  expect(toast.textContent).toBe(
    "/dev/ttyUSB0 is in use by another program (serial monitor, Arduino IDE, ModemManager…). Close it and try again.",
  );
});

it("refresh keeps the selected port and is disabled unless disconnected", async () => {
  const sel = root.querySelector<HTMLSelectElement>("#port-select")!;
  const refresh = root.querySelector<HTMLButtonElement>("#refresh-ports")!;
  sel.value = "/dev/ttyACM0";
  sel.dispatchEvent(new Event("change"));
  await flush();
  refresh.click();
  await flush();
  expect(sel.value).toBe("/dev/ttyACM0");
  // the selected port vanished: fall back to the detected one
  h.listPorts.mockResolvedValue([h.ports[1]]);
  refresh.click();
  await flush();
  expect(sel.value).toBe("/dev/ttyUSB0");
  h.listPorts.mockResolvedValue(h.ports);
  expect(refresh.disabled).toBe(false);
  connectBtn().click();
  await flush();
  expect(app.get().connection).toBe("connected");
  expect(refresh.disabled).toBe(true);
});

it("a device that never answers is reported as not an LD2402", async () => {
  h.connect.mockRejectedValue(new h.ApiError({ kind: "notLd2402", path: "/dev/ttyUSB0" }));
  connectBtn().click();
  await flush();
  expect(document.querySelector(".toast.error")!.textContent).toBe(
    "No LD2402 answered on /dev/ttyUSB0. Check the wiring (TX↔RX) and the power supply.",
  );
});

it("a disconnect during connect gives one toast and ignores a late success", async () => {
  let resolve!: (v: unknown) => void;
  h.connect.mockReturnValue(new Promise((r) => (resolve = r)));
  connectBtn().click();
  h.disconnectedCb!({ reason: "gone" });
  resolve(h.info);
  await flush();
  expect(app.get().connection).toBe("disconnected");
  expect(app.get().info).toBeNull();
  const toasts = document.querySelectorAll(".toast.error");
  expect(toasts.length).toBe(1);
  expect(toasts[0].textContent).toContain("disconnected");
});

it("a failed open that also emitted device://disconnected shows only the connect error", async () => {
  let reject!: (e: unknown) => void;
  h.connect.mockReturnValue(new Promise((_, r) => (reject = r)));
  connectBtn().click();
  h.disconnectedCb!({ reason: "gone" });
  reject(new h.ApiError({ kind: "notLd2402", path: "/dev/ttyUSB0" }));
  await flush();
  const toasts = document.querySelectorAll(".toast.error");
  expect(toasts.length).toBe(1);
  expect(toasts[0].textContent).toContain("No LD2402");
});

it("a frame-only update does not rewrite the shell (theme icon kept)", () => {
  const svg = root.querySelector("#theme-toggle svg");
  app.set({ lastFrame: { presence: "none", distanceM: 0, motionDb: [], microDb: [] } });
  expect(root.querySelector("#theme-toggle svg")).toBe(svg);
  app.set({ view: "calibration" });
  expect(root.querySelector("#theme-toggle svg")).toBe(svg);
  app.set({ lastFrame: null, view: "dashboard" });
});
