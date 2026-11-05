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
  app.set({ connection: "disconnected", info: null, port: null, view: "dashboard" });
  setLocale("en");
  await mount();
});

it("renders 3 view buttons and switches views", () => {
  const btns = root.querySelectorAll<HTMLButtonElement>(".rail button[data-view]");
  expect(btns.length).toBe(3);
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
