import { t } from "../i18n";
import { app, subscribeState } from "../state/store";

let toastBox: HTMLElement | null = null;

export function showBanner(id: string, kind: "warning" | "info", html: string): void {
  const host = document.getElementById("banners");
  if (!host) return;
  hideBanner(id);
  const el = document.createElement("div");
  el.className = `banner ${kind}`;
  el.dataset.banner = id;
  el.innerHTML = html;
  host.appendChild(el);
}

export function hideBanner(id: string): void {
  document.querySelectorAll(`[data-banner="${id}"]`).forEach((e) => e.remove());
}

export function toast(kind: "error" | "success" | "info", message: string): void {
  if (!toastBox || !toastBox.isConnected) {
    toastBox = document.createElement("div");
    toastBox.className = "toasts";
    document.body.appendChild(toastBox);
  }
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  el.addEventListener("click", () => el.remove());
  toastBox.appendChild(el);
  if (kind !== "error") setTimeout(() => el.remove(), 5000);
}

const esc = (v: string) => v.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/** Orange, dismissible banner under the header while the sensor reports a noisy power supply. */
export function installPowerBanner(): void {
  const sync = () => {
    const s = app.get();
    const on = s.connection === "connected" && s.info?.power === "interference";
    const shown = !!document.querySelector('[data-banner="power"]');
    if (s.connection !== "connected") dismissed = false;
    if (!on) return hideBanner("power");
    if (shown || dismissed) return;
    showBanner("power", "warning", `⚠️ <b>${esc(t("banner.power.title"))}</b> — <span>${esc(t("banner.power.body"))}</span><div class="grow"></div><button class="btn sm ghost">${esc(t("banner.dismiss"))}</button>`);
    document.querySelector('[data-banner="power"] button')?.addEventListener("click", () => {
      dismissed = true;
      hideBanner("power");
    });
  };
  // dismissal lasts for the connection; reset when it drops
  let dismissed = false;
  subscribeState(sync);
  sync();
}

/** Warning banner when the first-connection snapshot could not be saved; dismissible per connection. */
export function installSnapshotBanner(): void {
  let dismissed = false;
  const sync = () => {
    const s = app.get();
    if (s.connection !== "connected") dismissed = false;
    const err = s.connection === "connected" ? s.snapshots?.originalError : null;
    const el = document.querySelector('[data-banner="snapshot"]');
    if (!err) return hideBanner("snapshot");
    if (dismissed) return;
    const text = t("banner.snapshot.body", { message: err });
    if (el && el.querySelector("span")?.textContent === text) return;
    showBanner("snapshot", "warning", `⚠️ <b>${esc(t("banner.snapshot.title"))}</b> — <span>${esc(text)}</span><div class="grow"></div><button class="btn sm ghost">${esc(t("banner.dismiss"))}</button>`);
    document.querySelector('[data-banner="snapshot"] button')?.addEventListener("click", () => {
      dismissed = true;
      hideBanner("snapshot");
    });
  };
  subscribeState(sync);
  sync();
}
