import { ask } from "@tauri-apps/plugin-dialog";
import { readParams, snapshotStatus, writeParams } from "../api";
import { t } from "../i18n";
import { isDirty } from "../state/draft";
import { app, onConnected, onDisconnect } from "../state/store";
import type { SensorParams } from "../types";
import { errorMessageOf } from "./errors";
import { toast } from "./notify";

let epoch = 0;
onConnected(() => { epoch++; });
onDisconnect(() => { epoch++; });

/** Snapshot of the connection generation, to detect a stale async result. */
export const currentEpoch = (): number => epoch;
/** True while `ep` is still the current connection and it is up. */
export const isLive = (ep: number): boolean => ep === epoch && app.get().connection === "connected";

/** Runs `fn` as the single in-flight parameters operation (sets paramsBusy). */
export async function guarded(fn: () => Promise<void>): Promise<void> {
  const s = app.get();
  if (s.paramsBusy || s.calibrating || s.autoGaining) return;
  app.set({ paramsBusy: true });
  try { await fn(); } finally { app.set({ paramsBusy: false }); }
}

let statusSeq = 0;
/** Fetches the snapshot status for the current connection into the store (stale results dropped). */
export async function refreshSnapshotStatus(): Promise<void> {
  if (app.get().connection !== "connected") return;
  const ep = epoch;
  const seq = ++statusSeq;
  try {
    const status = await snapshotStatus();
    if (isLive(ep) && seq === statusSeq) app.set({ snapshots: status });
  } catch {
    /* disconnected or unavailable: the Restore button simply stays disabled */
  }
}
/** Drops any snapshot-status request still in flight. */
export function dropPendingSnapshotStatus(): void {
  statusSeq++;
}

/**
 * Writes `params`, rereads the sensor and sets sensor+draft. The Rust side backs
 * up the current settings first. Stale results (disconnect / reconnect) are dropped.
 * The snapshot status is refreshed after every attempt: a write can fail after
 * its backup was saved, leaving the sensor values (and their refresh) unchanged.
 */
export async function performWrite(params: SensorParams, successKey: string): Promise<void> {
  const ep = epoch;
  try {
    await writeParams(params);
    let fresh;
    try {
      fresh = await readParams();
    } catch {
      if (isLive(ep)) {
        app.set({ sensor: params, draft: params });
        toast("error", t("params.writtenRereadFailed"));
      }
      return;
    }
    if (!isLive(ep)) return;
    app.set({ sensor: fresh, draft: fresh });
    toast("success", t(successKey));
  } catch (e) {
    if (isLive(ep)) toast("error", errorMessageOf(e));
  } finally {
    if (isLive(ep)) void refreshSnapshotStatus();
  }
}

/** True when it is safe to proceed (clean, or the user agreed to discard). */
export async function confirmDiscardIfDirty(): Promise<boolean> {
  if (!isDirty(app.get().sensor, app.get().draft)) return true;
  try {
    return await ask(t("params.confirmDiscard"), { title: t("params.discardTitle"), kind: "warning" });
  } catch {
    return false;
  }
}
