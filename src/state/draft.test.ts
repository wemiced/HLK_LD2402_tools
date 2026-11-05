import { describe, it, expect } from 'vitest';
import { gatesInRange, isDirty, previewPresence, setThreshold, clampParams } from './draft';
import type { SensorParams, FrameDto } from '../types';

const mk = (): SensorParams => ({
  maxDistanceM: 5.0,
  disappearDelayS: 5,
  triggerDb: Array(16).fill(40),
  microDb: Array(16).fill(30),
});
const frame = (): FrameDto => ({
  presence: 'none',
  distanceM: 0,
  motionDb: Array(16).fill(0),
  microDb: Array(16).fill(0),
});

describe('gatesInRange', () => {
  it('computes gate counts', () => {
    expect(gatesInRange(5.0)).toBe(8);
    expect(gatesInRange(0.7)).toBe(1);
    expect(gatesInRange(10)).toBe(15);
    expect(gatesInRange(0)).toBe(1);
    expect(gatesInRange(100)).toBe(16);
  });
});

describe('isDirty', () => {
  it('detects changes within tolerances', () => {
    expect(isDirty(mk(), mk())).toBe(false);
    const a = mk();
    a.triggerDb[0] += 0.001;
    expect(isDirty(mk(), a)).toBe(false);
    a.triggerDb[0] += 0.02;
    expect(isDirty(mk(), a)).toBe(true);
    const d = mk();
    d.maxDistanceM += 0.04;
    expect(isDirty(mk(), d)).toBe(false);
    d.maxDistanceM += 0.04;
    expect(isDirty(mk(), d)).toBe(true);
    const e = mk();
    e.disappearDelayS += 1;
    expect(isDirty(mk(), e)).toBe(true);
  });
  it('is false when either side is null', () => {
    expect(isDirty(null, mk())).toBe(false);
    expect(isDirty(mk(), null)).toBe(false);
  });
});

describe('previewPresence', () => {
  it('moving when motion exceeds trigger in range', () => {
    const f = frame();
    f.motionDb[2] = 50;
    expect(previewPresence(f, mk())).toBe('moving');
  });
  it('none when the exceeding gate is out of range', () => {
    const f = frame();
    f.motionDb[10] = 90;
    expect(previewPresence(f, mk())).toBe('none');
  });
  it('still when only micro exceeds', () => {
    const f = frame();
    f.microDb[1] = 35;
    expect(previewPresence(f, mk())).toBe('still');
  });
  it('moving wins over still; equal is not exceeding', () => {
    const f = frame();
    f.microDb[1] = 35;
    f.motionDb[0] = 40;
    expect(previewPresence(f, mk())).toBe('still');
    f.motionDb[3] = 41;
    expect(previewPresence(f, mk())).toBe('moving');
  });
});

describe('setThreshold', () => {
  it('clamps, rounds and does not mutate', () => {
    const p = mk();
    const q = setThreshold(p, 'micro', 3, 120);
    expect(q.microDb[3]).toBe(95);
    expect(p.microDb[3]).toBe(30);
    expect(setThreshold(p, 'trigger', 0, -5).triggerDb[0]).toBe(0);
    expect(setThreshold(p, 'trigger', 1, 12.3456).triggerDb[1]).toBe(12.35);
  });
});

describe('clampParams', () => {
  it('clamps all fields into a new object', () => {
    const p = mk();
    p.maxDistanceM = 20;
    p.disappearDelayS = 70000.7;
    p.triggerDb[0] = 120;
    p.microDb[1] = -3;
    p.triggerDb[2] = NaN;
    const c = clampParams(p);
    expect(c).not.toBe(p);
    expect(c.maxDistanceM).toBe(10);
    expect(c.disappearDelayS).toBe(65535);
    expect(c.triggerDb[0]).toBe(95);
    expect(c.microDb[1]).toBe(0);
    expect(c.triggerDb[2]).toBe(0);
    expect(p.maxDistanceM).toBe(20);
    expect(clampParams({ ...mk(), maxDistanceM: 0.12345 }).maxDistanceM).toBe(0.7);
    expect(clampParams({ ...mk(), maxDistanceM: 3.14 }).maxDistanceM).toBe(3.1);
    expect(clampParams({ ...mk(), maxDistanceM: NaN }).maxDistanceM).toBe(0.7);
    expect(clampParams({ ...mk(), disappearDelayS: 2.6 }).disappearDelayS).toBe(3);
  });
});
