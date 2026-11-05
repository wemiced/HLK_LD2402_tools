import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CalibCoefficients,
  CalibDone,
  CalibFailed,
  DeviceInfo,
  Disconnected,
  FrameDto,
  PortInfo,
  RestorePreview,
  SensorParams,
  SnapshotKind,
  SnapshotStatus,
} from "./types";

export class ApiError extends Error {
  kind: string;
  data: Record<string, unknown>;
  constructor(data: Record<string, unknown>) {
    const kind = typeof data.kind === "string" ? data.kind : "unknown";
    super(kind);
    this.name = "ApiError";
    this.kind = kind;
    this.data = data;
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (e instanceof ApiError) throw e;
    if (e && typeof e === "object") throw new ApiError(e as Record<string, unknown>);
    throw new ApiError({ kind: "io", message: String(e) });
  }
}

export const listPorts = (probe: boolean) => call<PortInfo[]>("list_ports", { probe });
export const connect = (path: string) => call<DeviceInfo>("connect", { path });
export const disconnect = () => call<void>("disconnect");
export const readParams = () => call<SensorParams>("read_params");
export const writeParams = (params: SensorParams) => call<void>("write_params", { params });
export const setStreaming = (on: boolean) => call<void>("set_streaming", { on });
export const startCalibration = (coeffs: CalibCoefficients) =>
  call<void>("start_calibration", { coeffs });
export const autoGain = () => call<void>("auto_gain");
export const exportConfig = (path: string, params: SensorParams) =>
  call<void>("export_config", { path, params });
export const importConfig = (path: string) => call<SensorParams>("import_config", { path });
export const snapshotStatus = () => call<SnapshotStatus>("snapshot_status");
export const previewRestore = (kind: SnapshotKind) =>
  call<RestorePreview>("preview_restore", { kind });

const on =
  <T>(name: string) =>
  (cb: (payload: T) => void): Promise<UnlistenFn> =>
    listen<T>(name, (e) => cb(e.payload));

export const onFrame = on<FrameDto>("device://frame");
export const onCalibProgress = on<number>("device://calib-progress");
export const onCalibDone = on<CalibDone>("device://calib-done");
export const onCalibFailed = on<CalibFailed>("device://calib-failed");
export const onAutoGainDone = on<null>("device://auto-gain-done");
export const onDisconnected = on<Disconnected>("device://disconnected");
