import type { FrameDto, SensorParams } from '../types';

const GATE_M = 0.7;
const MAX_DB = 95;

const clampNum = (v: number, lo: number, hi: number): number =>
  Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : lo;
const cleanDb = (v: number): number =>
  Math.round(clampNum(v, 0, MAX_DB) * 100) / 100;

export function gatesInRange(maxDistanceM: number): number {
  const n = Math.ceil(maxDistanceM / GATE_M - 1e-9);
  return Math.min(16, Math.max(1, Number.isFinite(n) ? n : 1));
}

export function isDirty(sensor: SensorParams | null, draft: SensorParams | null): boolean {
  if (!sensor || !draft) return false;
  if (Math.abs(sensor.maxDistanceM - draft.maxDistanceM) > 0.05) return true;
  if (sensor.disappearDelayS !== draft.disappearDelayS) return true;
  const differs = (a: number[], b: number[]) =>
    a.length !== b.length || a.some((v, i) => Math.abs(v - b[i]) > 0.005);
  return differs(sensor.triggerDb, draft.triggerDb) || differs(sensor.microDb, draft.microDb);
}

export function previewPresence(frame: FrameDto, p: SensorParams): 'moving' | 'still' | 'none' {
  const n = gatesInRange(p.maxDistanceM);
  let still = false;
  for (let i = 0; i < n; i++) {
    if (frame.motionDb[i] > p.triggerDb[i]) return 'moving';
    if (frame.microDb[i] > p.microDb[i]) still = true;
  }
  return still ? 'still' : 'none';
}

export function setThreshold(
  p: SensorParams,
  series: 'trigger' | 'micro',
  gate: number,
  db: number,
): SensorParams {
  const key = series === 'trigger' ? 'triggerDb' : 'microDb';
  const arr = p[key].slice();
  arr[gate] = cleanDb(db);
  return { ...p, [key]: arr };
}

export function clampParams(p: SensorParams): SensorParams {
  return {
    maxDistanceM: Math.round(clampNum(p.maxDistanceM, 0.7, 10) * 10) / 10,
    disappearDelayS: Math.round(clampNum(p.disappearDelayS, 0, 65535)),
    triggerDb: p.triggerDb.map(cleanDb),
    microDb: p.microDb.map(cleanDb),
  };
}
