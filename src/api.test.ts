import { beforeEach, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ApiError, connect, onFrame, previewRestore, setStreaming, snapshotStatus, writeParams } from "./api";

beforeEach(() => vi.resetAllMocks());

it("wraps rejected invokes in ApiError", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "permissionDenied", path: "/dev/ttyUSB0" });
  const err = await connect("/dev/ttyUSB0").catch((e) => e);
  expect(err).toBeInstanceOf(ApiError);
  expect(err.kind).toBe("permissionDenied");
  expect(err.data.path).toBe("/dev/ttyUSB0");
  expect(invoke).toHaveBeenCalledWith("connect", { path: "/dev/ttyUSB0" });
});

it("passes arguments by camelCase name", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  await setStreaming(true);
  expect(invoke).toHaveBeenCalledWith("set_streaming", { on: true });
});

it("event helpers unwrap the payload", async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockImplementation(async (_n, h) => {
    (h as (e: unknown) => void)({ payload: { presence: "none" } });
    return unlisten;
  });
  const cb = vi.fn();
  expect(await onFrame(cb)).toBe(unlisten);
  expect(cb).toHaveBeenCalledWith({ presence: "none" });
  expect(listen).toHaveBeenCalledWith("device://frame", expect.any(Function));
});

it("previewRestore invokes preview_restore with the kind", async () => {
  vi.mocked(invoke).mockResolvedValue({ meta: {}, params: {}, clamped: [], unknownSerial: false });
  await previewRestore("original");
  expect(invoke).toHaveBeenCalledWith("preview_restore", { kind: "original" });
});

it("snapshotStatus invokes snapshot_status", async () => {
  vi.mocked(invoke).mockResolvedValue({ original: null, latestBackup: null, unknownSerial: false, originalError: null });
  await snapshotStatus();
  expect(invoke).toHaveBeenCalledWith("snapshot_status", undefined);
});

it("a backupFailed rejection becomes an ApiError", async () => {
  vi.mocked(invoke).mockRejectedValue({ kind: "backupFailed", message: "x" });
  const err = await writeParams({ maxDistanceM: 5, disappearDelayS: 5, triggerDb: [], microDb: [] }).catch((e) => e);
  expect(err).toBeInstanceOf(ApiError);
  expect(err.kind).toBe("backupFailed");
  expect(err.data.message).toBe("x");
});
