import { beforeEach, expect, it, vi } from "vitest";

vi.mock("../api", () => ({
  listPorts: vi.fn(async () => [
    { path: "/dev/ttyACM0", label: "ttyACM0", usbSerial: true, detected: false },
    { path: "/dev/ttyUSB0", label: "ttyUSB0", usbSerial: true, detected: true },
  ]),
  connect: vi.fn(async () => ({
    firmware: "v3.3.5",
    serial: "1A2B3C4D",
    capabilities: { permanentSave: true, autoGain: true, serialText: true },
    power: "ok",
  })),
  disconnect: vi.fn(async () => {}),
  onDisconnected: vi.fn(async () => () => {}),
  ApiError: class extends Error {},
}));

import { setLocale } from "../i18n";
import { app } from "../state/store";
import { mountShell } from "./shell";

const flush = () => new Promise((r) => setTimeout(r, 0));
let root: HTMLElement;

beforeEach(async () => {
  document.body.innerHTML = '<div id="app"></div>';
  localStorage.clear();
  setLocale("en");
  root = document.getElementById("app")!;
  mountShell(root);
  await flush();
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
  root.querySelector<HTMLButtonElement>("#connect-btn")!.click();
  await flush();
  expect(root.querySelector(".sub")!.textContent).toBe("HLK-LD2402 · v3.3.5 · SN 1A2B3C4D");
  expect(app.get().connection).toBe("connected");
});

it("theme toggle flips body data-theme", () => {
  const before = document.body.dataset.theme;
  root.querySelector<HTMLButtonElement>("#theme-toggle")!.click();
  expect(document.body.dataset.theme).not.toBe(before);
});
