import { beforeEach, expect, it } from "vitest";
import { setLocale, t } from "../i18n";
import { app } from "../state/store";
import { mountFirmware } from "./firmware";

let el: HTMLElement;
const set = (connected: boolean, caps = { permanentSave: true, autoGain: false, serialText: false }) =>
  app.set({ connection: connected ? "connected" : "disconnected", info: connected ? ({ firmware: "v3.3.2", serial: "1", power: "ok", capabilities: caps } as never) : null });

beforeEach(() => {
  setLocale("en");
  document.body.innerHTML = '<section id="f"></section>';
  el = document.getElementById("f")!;
  set(true);
  mountFirmware(el);
});

it("update_button_disabled_with_notice", () => {
  const btn = el.querySelector<HTMLButtonElement>('[data-role="update"]')!;
  expect(btn.disabled).toBe(true);
  expect(el.querySelector('[data-role="notice"]')!.textContent).toBe(t("firmware.unavailable"));
  expect(t("firmware.unavailable")).toBe("Firmware update is not available yet: Hi-Link has not published the update protocol.");
  expect(el.querySelector(".drop")).not.toBeNull();
  expect(el.querySelector('[data-role="current"]')!.textContent).toContain("v3.3.2");
});

it("feature_table_follows_capabilities", () => {
  const cells = () => [...el.querySelectorAll<HTMLElement>("[data-cap]")].map((c) => c.textContent);
  expect(cells()).toEqual(["✓", "✗", "✗"]);
  set(false);
  expect(cells()).toEqual(["—", "—", "—"]);
  expect(el.querySelector('[data-role="current"]')!.textContent).toContain("—");
});
