import { connect, disconnect, listPorts, onDisconnected } from "../api";
import { availableLocales, getLocale, onLocaleChange, renderI18n, setLocale, t } from "../i18n";
import { isDirty } from "../state/draft";
import { app, subscribeState, type ViewId } from "../state/store";
import { errorMessageOf } from "./errors";
import { toast } from "./notify";
import { confirmDiscardIfDirty } from "./paramsPanel";

const THEME_KEY = "ld2402.theme";
const ic = (d: string) => `<svg class="i" viewBox="0 0 24 24">${d}</svg>`;
const VIEWS: { id: ViewId; icon: string }[] = [
  { id: "dashboard", icon: ic('<rect x="3" y="3" width="7" height="9" rx="2"/><rect x="14" y="3" width="7" height="5" rx="2"/><rect x="14" y="12" width="7" height="9" rx="2"/><rect x="3" y="16" width="7" height="5" rx="2"/>') },
  { id: "calibration", icon: ic('<path d="M12 3v3M12 18v3M3 12h3M18 12h3"/><circle cx="12" cy="12" r="5"/>') },
  { id: "firmware", icon: ic('<path d="M12 3v12M7 10l5 5 5-5"/><path d="M4 21h16"/>') },
];
const MOON = ic('<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>');
const SUN = ic('<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>');
const GLOBE = ic('<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>');

function storeGet(k: string): string | null {
  try { return localStorage.getItem(k); } catch { return null; }
}
function storeSet(k: string, v: string): void {
  try { localStorage.setItem(k, v); } catch { /* ignore */ }
}

export function mountShell(root: HTMLElement): void {
  document.body.dataset.theme = storeGet(THEME_KEY) === "light" ? "light" : "dark";

  root.innerHTML = `
<nav class="rail">
  <div class="logo">LD</div>
  ${VIEWS.map((v) => `<button data-view="${v.id}" data-i18n-aria="nav.${v.id}">${v.icon}<span data-i18n="nav.${v.id}"></span></button>`).join("")}
  <div class="spacer"></div>
  <button id="lang-btn" data-i18n-aria="nav.language">${GLOBE}<span data-i18n="nav.language"></span></button>
  <button id="theme-toggle" data-i18n-aria="nav.theme">${MOON}<span data-i18n="nav.theme"></span></button>
</nav>
<div class="langmenu" id="lang-menu"></div>
<main>
  <header>
    <div><h1 id="title"></h1><div class="sub" id="sub"></div></div>
    <div class="grow"></div>
    <div class="pill"><span class="dot" id="conn-dot"></span><select id="port-select"></select><button class="iconbtn" id="refresh-ports" data-i18n-title="conn.refresh" data-i18n-aria="conn.refresh">↻</button></div>
    <button class="btn" id="connect-btn"></button>
  </header>
  <div id="banners"></div>
  ${VIEWS.map((v) => `<section class="view content" data-view="${v.id}"></section>`).join("")}
</main>`;

  const $ = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel)!;
  const select = $<HTMLSelectElement>("#port-select");
  const connectBtn = $<HTMLButtonElement>("#connect-btn");
  const langMenu = $<HTMLElement>("#lang-menu");

  // --- ports
  async function refreshPorts(): Promise<void> {
    const keep = select.value;
    let ports: Awaited<ReturnType<typeof listPorts>> = [];
    try { ports = await listPorts(true); } catch { /* keep empty list */ }
    select.innerHTML = "";
    for (const p of ports) {
      const o = document.createElement("option");
      o.value = p.path;
      o.dataset.detected = String(p.detected === true);
      o.dataset.label = p.label;
      select.appendChild(o);
    }
    // keep the user's choice while it is still listed; otherwise preselect
    const kept = ports.find((p) => p.path === keep);
    const first = kept ?? ports.find((p) => p.detected === true) ?? ports[0];
    if (first) select.value = first.path;
    renderPortLabels();
    update();
  }
  function renderPortLabels(): void {
    for (const o of Array.from(select.options)) {
      if (o.dataset.label === undefined) o.remove();
      else o.textContent = o.dataset.label + (o.dataset.detected === "true" ? " — " + t("ports.detected") : "");
    }
    if (!select.options.length) {
      const o = document.createElement("option");
      o.value = "";
      o.textContent = t("ports.none");
      select.appendChild(o);
    }
  }

  // --- render from state
  let shownTheme: string | undefined;
  function update(): void {
    const s = app.get();
    const theme = document.body.dataset.theme;
    if (theme !== shownTheme) {
      shownTheme = theme;
      $("#theme-toggle").querySelector("svg")!.outerHTML = theme === "light" ? SUN : MOON;
    }
    root.querySelectorAll<HTMLElement>(".rail [data-view]").forEach((b) => b.classList.toggle("on", b.dataset.view === s.view));
    root.querySelectorAll<HTMLElement>("section[data-view]").forEach((e) => e.classList.toggle("on", e.dataset.view === s.view));
    $("#title").textContent = t("nav." + s.view);
    $("#sub").textContent = s.info ? `HLK-LD2402 · ${s.info.firmware} · SN ${s.info.serial}` : "HLK-LD2402";
    const dot = $("#conn-dot");
    dot.classList.toggle("on", s.connection === "connected");
    dot.classList.toggle("busy", s.connection === "connecting");
    connectBtn.classList.toggle("primary", s.connection !== "connected");
    connectBtn.disabled = s.connection === "connecting" || (s.connection === "disconnected" && !select.value);
    connectBtn.textContent = t(s.connection === "connected" ? "conn.disconnect" : s.connection === "connecting" ? "conn.connecting" : "conn.connect");
    select.disabled = s.connection !== "disconnected";
    $<HTMLButtonElement>("#refresh-ports").disabled = s.connection !== "disconnected";
  }

  // --- connection flow
  // bumped by device://disconnected while connecting: the pending result is stale
  let connectEpoch = 0;
  async function toggleConnection(): Promise<void> {
    const s = app.get();
    if (s.connection === "connecting") return;
    if (s.connection === "connected") {
      if (isDirty(s.sensor, s.draft) && !(await confirmDiscardIfDirty())) return;
      try { await disconnect(); } catch { /* state is reset regardless */ }
      app.set({ connection: "disconnected", info: null, streaming: false, lastFrame: null });
      return;
    }
    if (!select.value) return;
    if (isDirty(s.sensor, s.draft) && !(await confirmDiscardIfDirty())) return;
    app.set({ connection: "connecting", port: select.value });
    const ep = ++connectEpoch;
    try {
      const info = await connect(select.value);
      if (ep !== connectEpoch) {
        // the link dropped before the answer arrived: the core no longer holds it
        app.set({ connection: "disconnected", info: null });
        toast("error", t("errors.disconnected"));
        return;
      }
      app.set({ info, connection: "connected" });
    } catch (e) {
      app.set({ connection: "disconnected", info: null });
      toast("error", errorMessageOf(e));
    }
  }

  // --- events
  root.querySelectorAll<HTMLElement>(".rail [data-view]").forEach((b) =>
    b.addEventListener("click", () => app.set({ view: b.dataset.view as ViewId })),
  );
  $("#refresh-ports").addEventListener("click", () => {
    if (app.get().connection === "disconnected") void refreshPorts();
  });
  connectBtn.addEventListener("click", () => void toggleConnection());
  let lastPort = "";
  select.addEventListener("focus", () => { lastPort = select.value; });
  select.addEventListener("change", async () => {
    if (lastPort && select.value !== lastPort && !(await confirmDiscardIfDirty())) select.value = lastPort;
    lastPort = select.value;
    update();
  });
  $("#theme-toggle").addEventListener("click", () => {
    const next = document.body.dataset.theme === "light" ? "dark" : "light";
    document.body.dataset.theme = next;
    storeSet(THEME_KEY, next);
    update();
  });

  function renderLangMenu(): void {
    langMenu.innerHTML = "";
    for (const l of availableLocales()) {
      const b = document.createElement("button");
      b.textContent = l.name;
      b.classList.toggle("on", l.code === getLocale());
      b.addEventListener("click", () => {
        setLocale(l.code);
        langMenu.classList.remove("open");
      });
      langMenu.appendChild(b);
    }
  }
  $("#lang-btn").addEventListener("click", (e) => {
    e.stopPropagation();
    renderLangMenu();
    langMenu.classList.toggle("open");
  });
  document.addEventListener("click", () => langMenu.classList.remove("open"));

  onLocaleChange(() => {
    renderI18n(root);
    renderPortLabels();
    update();
  });
  subscribeState(update);
  void onDisconnected(() => {
    // during a connect, the connect result reports the outcome (one toast)
    if (app.get().connection === "connecting") { connectEpoch++; return; }
    if (app.get().connection === "disconnected") return;
    app.set({ connection: "disconnected", info: null, streaming: false });
    toast("error", t("errors.disconnected"));
  });

  renderI18n(root);
  update();
  void refreshPorts();
}
