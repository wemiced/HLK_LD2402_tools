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
