// Mirrors the Rust DTOs (camelCase on the wire).

export type PresenceKind = "none" | "moving" | "still";

export interface FrameDto {
  presence: PresenceKind;
  distanceM: number;
  /** 16 motion gate energies in dB. */
  motionDb: number[];
  /** 16 micro-motion gate energies in dB. */
  microDb: number[];
}

export interface SensorParams {
  maxDistanceM: number;
  disappearDelayS: number;
  triggerDb: number[];
  microDb: number[];
}

export interface CalibCoefficients {
  trigger: number;
  hold: number;
  micro: number;
}

export interface Capabilities {
  permanentSave: boolean;
  autoGain: boolean;
  serialText: boolean;
}

export type PowerStatus = "notTested" | "ok" | "interference";

export interface DeviceInfo {
  firmware: string;
  serial: string;
  capabilities: Capabilities;
  power: PowerStatus;
}

export interface PortInfo {
  path: string;
  label: string;
  usbSerial: boolean;
  /** null when not probed. */
  detected: boolean | null;
}

export type DeviceError =
  | { kind: "permissionDenied"; path: string }
  | { kind: "notFound"; path: string }
  | { kind: "inUse"; path: string }
  | { kind: "notLd2402"; path: string }
  | { kind: "timeout"; word: number }
  | { kind: "nack"; word: number; status: number }
  | { kind: "unsupported"; feature: string; required: string }
  | { kind: "disconnected" }
  | { kind: "busy" }
  | { kind: "forbidden"; word: number }
  | { kind: "io"; message: string }
  | { kind: "backupFailed"; message: string };

export type ConfigError =
  | { kind: "config"; reason: "notAConfig" }
  | { kind: "config"; reason: "unsupportedVersion"; version: number }
  | { kind: "config"; reason: "outOfRange"; field: string };

export type SnapshotKind = "original" | "backup";

export interface SnapshotMeta {
  kind: SnapshotKind;
  /** RFC3339 UTC. */
  savedAt: string;
  /** File name only. */
  file: string;
}

export interface SnapshotStatus {
  original: SnapshotMeta | null;
  /** For a blank serial, only a backup taken during this connection. */
  latestBackup: SnapshotMeta | null;
  unknownSerial: boolean;
  /** Why the first-connection snapshot could not be saved, if it failed. */
  originalError: string | null;
}

/** A value that was out of range in the snapshot and was clamped. */
export interface ClampedField {
  field: string;
  from: number;
  to: number;
}

export interface RestorePreview {
  meta: SnapshotMeta;
  params: SensorParams;
  clamped: ClampedField[];
  unknownSerial: boolean;
}

export type ApiErrorData = DeviceError | ConfigError;

export interface CalibDone {
  gates: number[];
}

export interface CalibFailed {
  error: DeviceError;
}

export interface Disconnected {
  reason: string;
}
