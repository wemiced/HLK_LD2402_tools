import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

const dir = __dirname;
const load = (f: string): Record<string, string> => JSON.parse(readFileSync(join(dir, f), "utf8"));
const files = readdirSync(dir).filter((f) => f.endsWith(".json"));
const en = load("en.json");

it("every locale has exactly the keys of en", () => {
  for (const f of files) {
    expect(Object.keys(load(f)).sort(), f).toEqual(Object.keys(en).sort());
  }
});

it("every data-i18n / t() key in index.html and ui/*.ts exists in en", () => {
  const root = join(dir, "..");
  const srcs = [join(root, "..", "index.html")];
  for (const f of readdirSync(join(root, "ui"))) if (f.endsWith(".ts") && !f.endsWith(".test.ts")) srcs.push(join(root, "ui", f));
  const re = /(?:\bt\(\s*|data-i18n(?:-title)?="|i18n:\s*)["'`]([a-zA-Z0-9_.]+)["'`]?/g;
  const missing: string[] = [];
  for (const s of srcs) {
    const text = readFileSync(s, "utf8");
    for (const m of text.matchAll(re)) if (!m[1].endsWith(".") && !(m[1] in en)) missing.push(`${s}: ${m[1]}`);
  }
  expect(missing).toEqual([]);
});

it("every DeviceError kind has an errors.<kind> string", () => {
  const types = readFileSync(join(dir, "..", "types.ts"), "utf8");
  const block = types.slice(types.indexOf("export type DeviceError"), types.indexOf("export type ConfigError"));
  const kinds = [...block.matchAll(/kind: "(\w+)"/g)].map((m) => m[1]);
  expect(kinds.length).toBe(8);
  for (const k of kinds) expect(en[`errors.${k}`], k).toBeTruthy();
  for (const k of ["notAConfig", "unsupportedVersion", "outOfRange"]) expect(en[`errors.config.${k}`], k).toBeTruthy();
});
