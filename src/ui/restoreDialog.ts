import { previewRestore } from "../api";
import { formatNumber, getLocale, t } from "../i18n";
import { app, canEdit, onConnected, onDisconnect, subscribeState } from "../state/store";
import type { ClampedField, RestorePreview, SnapshotKind, SnapshotMeta } from "../types";
import { errorMessageOf } from "./errors";
import { openModal } from "./modal";
import { toast } from "./notify";
import {
  confirmDiscardIfDirty, currentEpoch, dropPendingSnapshotStatus, guarded, isLive, performWrite, refreshSnapshotStatus,
} from "./writeFlow";

export function formatSavedAt(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return new Intl.DateTimeFormat(getLocale(), { dateStyle: "medium", timeStyle: "short" }).format(d);
}

const sourceLabel = (m: SnapshotMeta) =>
  t(m.kind === "original" ? "restore.original" : "restore.backup", { date: formatSavedAt(m.savedAt) });

/** Localized "label: value unit" parts for a clamped field name. */
function fieldLabel(field: string): { label: string; unit: string; digits: number } {
  const gate = /^(triggerDb|microDb)\[(\d+)\]$/.exec(field);
  if (gate) {
    const key = gate[1] === "triggerDb" ? "restore.gateTrigger" : "restore.gateMicro";
    return { label: t(key, { gate: Number(gate[2]) + 1 }), unit: t("units.db"), digits: 2 };
  }
  if (field === "maxDistanceM") return { label: t("params.maxDistance"), unit: t("units.m"), digits: 1 };
  if (field === "disappearDelayS") return { label: t("params.disappearDelay"), unit: t("units.s"), digits: 0 };
  return { label: field, unit: "", digits: 2 };
}

export function describeClamped(c: ClampedField): string {
  const f = fieldLabel(c.field);
  const v = (n: number) => `${formatNumber(n, f.digits)}${f.unit ? " " + f.unit : ""}`;
  return `${f.label}: ${v(c.from)} → ${v(c.to)}`;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function confirmBody(p: RestorePreview): HTMLElement {
  const box = el("div", "modal-stack");
  box.appendChild(el("p", "", t("restore.confirmBody", { source: sourceLabel(p.meta) })));
  if (p.unknownSerial) box.appendChild(el("div", "callout danger", t("restore.unknownSerial")));
  if (p.clamped.length) {
    const c = el("div", "callout warn");
    c.appendChild(el("p", "", t("restore.clamped")));
    const ul = el("ul", "clamped");
    for (const f of p.clamped) ul.appendChild(el("li", "mono", describeClamped(f)));
    c.appendChild(ul);
    box.appendChild(c);
  }
  box.appendChild(el("p", "hint", t("restore.backupNote")));
  return box;
}

/**
 * Keeps `app.snapshots` current: refreshed whenever the sensor values are
 * (re)loaded on a live connection (initial read, write, restore, calibration)
 * and after every write attempt (see `performWrite`), cleared on disconnect.
 */
export function installSnapshotStatus(): void {
  subscribeStateSensor();
  onConnected(() => { void refreshSnapshotStatus(); });
  onDisconnect(() => { dropPendingSnapshotStatus(); app.set({ snapshots: null }); });
}
function subscribeStateSensor(): void {
  let prev = app.get().sensor;
  subscribeState(() => {
    const s = app.get();
    const changed = s.sensor !== prev;
    prev = s.sensor;
    if (changed && s.sensor && s.connection === "connected") void refreshSnapshotStatus();
  });
}

let restoring = false;

/** Restore flow: choose a snapshot, review the preview, write it through the normal write path. */
export async function openRestoreDialog(): Promise<void> {
  if (restoring) return;
  const snaps = app.get().snapshots;
  if (!snaps || !canEdit(app.get())) return;
  restoring = true;
  const ep = currentEpoch();
  try {
    const actions: { label: string; kind: "option" | "ghost"; value: SnapshotKind | null }[] = [];
    if (snaps.original) actions.push({ label: t("restore.original", { date: formatSavedAt(snaps.original.savedAt) }), kind: "option", value: "original" });
    if (snaps.latestBackup) actions.push({ label: t("restore.backup", { date: formatSavedAt(snaps.latestBackup.savedAt) }), kind: "option", value: "backup" });
    if (!actions.length) return;
    actions.push({ label: t("restore.cancel"), kind: "ghost", value: null });

    const body = el("div", "modal-stack");
    body.appendChild(el("p", "", t("restore.chooseBody")));
    const kind = await openModal<SnapshotKind | null>({ title: t("restore.title"), body, actions });
    if (!kind || !isLive(ep)) return;

    let preview: RestorePreview;
    try {
      preview = await previewRestore(kind);
    } catch (e) {
      if (isLive(ep)) toast("error", errorMessageOf(e));
      return;
    }
    if (!isLive(ep)) return;
    const ok = await openModal<boolean>({
      title: t("restore.confirmTitle"),
      tone: preview.unknownSerial ? "warning" : "default",
      body: confirmBody(preview),
      actions: [
        { label: t("restore.cancel"), kind: "ghost", value: false },
        { label: t("restore.confirm"), kind: preview.unknownSerial ? "danger" : "primary", value: true },
      ],
    });
    if (!ok || !isLive(ep)) return;
    if (!(await confirmDiscardIfDirty()) || !isLive(ep)) return;
    if (!canEdit(app.get())) return;
    await guarded(() => performWrite(preview.params, "restore.done"));
  } finally {
    restoring = false;
  }
}
