import type { DeviceInfo, FrameDto, SensorParams } from "../types";

export function createStore<T extends object>(initial: T) {
  let state = initial;
  const subs = new Set<(s: T, prev: T) => void>();
  return {
    get: () => state,
    set(patch: Partial<T>) {
      const prev = state;
      state = { ...state, ...patch };
      subs.forEach((fn) => fn(state, prev));
    },
    subscribe(fn: (s: T, prev: T) => void) {
      subs.add(fn);
      return () => {
        subs.delete(fn);
      };
    },
  };
}

export type ViewId = "dashboard" | "calibration" | "firmware";

export interface AppState {
  connection: "disconnected" | "connecting" | "connected";
  port: string | null;
  info: DeviceInfo | null;
  view: ViewId;
  sensor: SensorParams | null;
  draft: SensorParams | null;
  streaming: boolean;
  /** True while an automatic calibration is measuring (other commands would be Busy). */
  calibrating: boolean;
  /** True from an auto-gain request until its completion report (or timeout). */
  autoGaining: boolean;
  /** True while the parameter panel runs a read/write/import sequence. */
  paramsBusy: boolean;
  lastFrame: FrameDto | null;
  series: "trigger" | "micro";
}

export const app = createStore<AppState>({
  connection: "disconnected",
  port: null,
  info: null,
  view: "dashboard",
  sensor: null,
  draft: null,
  streaming: false,
  calibrating: false,
  autoGaining: false,
  paramsBusy: false,
  lastFrame: null,
  series: "trigger",
});

/** True when an update changed nothing but the live frame (10 Hz): most views can skip it. */
export function onlyFrameChanged(s: AppState, prev: AppState): boolean {
  return (Object.keys(s) as (keyof AppState)[]).every((k) => k === "lastFrame" || s[k] === prev[k]);
}

/** Subscribe, skipping frame-only updates. */
export const subscribeState = (fn: () => void) =>
  app.subscribe((s, prev) => { if (!onlyFrameChanged(s, prev)) fn(); });

/** Draft edits are allowed only while connected and no device operation could overwrite them. */
export const canEdit = (s: AppState): boolean =>
  s.connection === "connected" && !s.paramsBusy && !s.calibrating && !s.autoGaining;

function onTransition(to: AppState["connection"], fn: () => void): () => void {
  let prev = app.get().connection;
  return app.subscribe((s) => {
    const was = prev;
    prev = s.connection;
    if (s.connection === to && was !== to) fn();
  });
}

/** Called each time the connection becomes 'connected'. */
export const onConnected = (fn: () => void) => onTransition("connected", fn);
/** Called each time the connection returns to 'disconnected'. */
export const onDisconnect = (fn: () => void) => onTransition("disconnected", fn);
