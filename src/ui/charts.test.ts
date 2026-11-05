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

import { hitThreshold } from "./energyChart";
it("hitThreshold only matches near the line and inside the plot", () => {
  const y = dbToY(40, 200);
  expect(hitThreshold(y, 40, 200)).toBe(true);
  expect(hitThreshold(y + 8, 40, 200)).toBe(true);
  expect(hitThreshold(y + 9, 40, 200)).toBe(false);
  expect(hitThreshold(205, 0, 200)).toBe(false);
  expect(hitThreshold(y, undefined, 200)).toBe(false);
});

import { createEnergyChart } from "./energyChart";
it("a far pointerdown does not call onDrag, a near one does", () => {
  const c = document.createElement("canvas");
  c.getBoundingClientRect = () => ({ left: 0, top: 0, width: 342, height: 230, right: 342, bottom: 230, x: 0, y: 0, toJSON() {} });
  c.setPointerCapture = () => {};
  const calls: [number, number][] = [];
  const ch = createEnergyChart(c, { onDrag: (g, d) => calls.push([g, d]) });
  ch.render(null, { maxDistanceM: 5, disappearDelayS: 30, triggerDb: Array(16).fill(40), microDb: Array(16).fill(30) }, "trigger");
  const ev = (x: number, y: number) => { const e = new Event("pointerdown") as any; Object.assign(e, { clientX: x, clientY: y, button: 0, pointerId: 1 }); c.dispatchEvent(e); };
  ev(60, 8 + dbToY(10, 198));
  expect(calls).toEqual([]);
  ev(60, 8 + dbToY(40, 198));
  expect(calls.length).toBe(1);
});
