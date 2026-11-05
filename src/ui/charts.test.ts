import { expect, it } from "vitest";
import { dbToY, gateAtX, yToDb } from "./energyChart";
import { pruneWindow } from "./distanceChart";

it("dbToY / yToDb map 0..95 dB onto the plot height", () => {
  expect(dbToY(95, 200)).toBe(0);
  expect(dbToY(0, 200)).toBe(200);
  expect(dbToY(500, 200)).toBe(0);
  expect(yToDb(dbToY(41.37, 200), 200)).toBe(41.37);
  expect(yToDb(-50, 200)).toBe(95);
  expect(yToDb(999, 200)).toBe(0);
});

it("gateAtX finds the gate under x", () => {
  expect(gateAtX(-1, 320)).toBeNull();
  expect(gateAtX(0, 320)).toBe(0);
  expect(gateAtX(319, 320)).toBe(15);
  expect(gateAtX(320, 320)).toBeNull();
});

it("pruneWindow drops samples older than 60 s and keeps the boundary", () => {
  const s = [{ t: 0, d: 1 }, { t: 39999, d: 1 }, { t: 40000, d: 2 }, { t: 100000, d: 3 }];
  expect(pruneWindow(s, 100000).map((v) => v.t)).toEqual([40000, 100000]);
  expect(pruneWindow(s, 60000).map((v) => v.t)).toEqual([0, 39999, 40000, 100000]);
});
