import { beforeEach, expect, it } from "vitest";
import { setLocale } from "../i18n";
import { app } from "../state/store";
import { installPowerBanner } from "./notify";

const info = (power: string) => ({ firmware: "v3.3.5", serial: "1", power, capabilities: { autoGain: true } }) as never;
const banner = () => document.querySelector('[data-banner="power"]');

document.body.innerHTML = '<div id="banners"></div>';
setLocale("en");
installPowerBanner();

beforeEach(() => app.set({ connection: "disconnected", info: null }));

it("shows on interference after connect, dismiss sticks even when info is re-set, hides on disconnect, re-shows next connect", () => {
  app.set({ connection: "connected", info: info("interference") });
  expect(banner()).not.toBeNull();
  banner()!.querySelector("button")!.click();
  expect(banner()).toBeNull();
  app.set({ info: info("interference") });
  expect(banner()).toBeNull();
  app.set({ connection: "disconnected", info: null });
  expect(banner()).toBeNull();
  app.set({ connection: "connected", info: info("interference") });
  expect(banner()).not.toBeNull();
});

it("is not shown when power is ok, and its content is plain escaped text", () => {
  app.set({ connection: "connected", info: info("ok") });
  expect(banner()).toBeNull();
  app.set({ info: info("interference") });
  expect(banner()!.querySelectorAll("script,img").length).toBe(0);
  expect(banner()!.textContent).toContain("Disturbed power supply");
});
