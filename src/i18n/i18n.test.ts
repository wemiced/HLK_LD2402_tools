import { beforeEach, expect, it } from "vitest";
import { availableLocales, formatNumber, getLocale, initLocale, registerLocale, setLocale, t } from "./index";

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = "";
});

it("falls back to en then key", () => {
  registerLocale("xx", "Test", { "_meta.name": "Test" });
  setLocale("xx");
  expect(t("nav.dashboard")).toBe("Dashboard");
  expect(t("nope")).toBe("nope");
});

it("interpolates", () => {
  setLocale("en");
  expect(t("errors.permissionDenied", { path: "/dev/ttyUSB0" })).toContain("/dev/ttyUSB0");
});

it("initLocale uses stored, then navigator, then en", () => {
  Object.defineProperty(navigator, "language", { value: "fr-FR", configurable: true });
  initLocale();
  expect(getLocale()).toBe("fr");
  localStorage.clear();
  Object.defineProperty(navigator, "language", { value: "de-DE", configurable: true });
  initLocale();
  expect(getLocale()).toBe("en");
  localStorage.setItem("ld2402.locale", "fr");
  initLocale();
  expect(getLocale()).toBe("fr");
});

it("setLocale re-renders data-i18n nodes and persists", () => {
  document.body.innerHTML = '<span data-i18n="nav.dashboard"></span>';
  setLocale("fr");
  expect(document.body.textContent).toBe("Tableau de bord");
  expect(localStorage.getItem("ld2402.locale")).toBe("fr");
});

it("lists locales with names and formats numbers", () => {
  expect(availableLocales().map((l) => l.code)).toEqual(expect.arrayContaining(["en", "fr"]));
  setLocale("fr");
  expect(formatNumber(2.35, 2)).toBe("2,35");
});
