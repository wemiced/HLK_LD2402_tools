import { beforeEach, expect, it } from "vitest";
import { setLocale } from "../i18n";
import { app } from "../state/store";
import { installPowerBanner, installSnapshotBanner } from "./notify";

const info = (power: string) => ({ firmware: "v3.3.5", serial: "1", power, capabilities: { autoGain: true } }) as never;
const banner = () => document.querySelector('[data-banner="power"]');

document.body.innerHTML = '<div id="banners"></div>';
setLocale("en");
installPowerBanner();
installSnapshotBanner();

beforeEach(() => app.set({ connection: "disconnected", info: null, snapshots: null }));

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

const snap = (originalError: string | null) => ({ original: null, latestBackup: null, unknownSerial: false, originalError });
const sbanner = () => document.querySelector('[data-banner="snapshot"]');

it("snapshot_banner_shows_on_original_error", () => {
  app.set({ connection: "connected", snapshots: snap(null) });
  expect(sbanner()).toBeNull();
  app.set({ snapshots: snap("<img src=x> disk full") });
  expect(sbanner()).not.toBeNull();
  expect(sbanner()!.querySelectorAll("img").length).toBe(0);
  expect(sbanner()!.textContent).toContain("<img src=x> disk full");
  expect(sbanner()!.textContent).toContain("Writes stay protected by automatic backups");
  sbanner()!.querySelector("button")!.click();
  expect(sbanner()).toBeNull();
  app.set({ snapshots: snap("<img src=x> disk full") });
  expect(sbanner()).toBeNull();
  app.set({ connection: "disconnected", snapshots: null });
  app.set({ connection: "connected", snapshots: snap("again") });
  expect(sbanner()).not.toBeNull();
});
