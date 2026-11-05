import type { DeviceInfo, FrameDto, SensorParams } from "../types";

export function createStore<T extends object>(initial: T) {
  let state = initial;
  const subs = new Set<(s: T) => void>();
  return {
    get: () => state,
    set(patch: Partial<T>) {
      state = { ...state, ...patch };
      subs.forEach((fn) => fn(state));
    },
    subscribe(fn: (s: T) => void) {
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
  lastFrame: null,
  series: "trigger",
});

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
