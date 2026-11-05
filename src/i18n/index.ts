type Messages = Record<string, string>;

const KEY = "ld2402.locale";
const catalogs = new Map<string, { name: string; messages: Messages }>();
const modules = import.meta.glob("../locales/*.json", { eager: true }) as Record<
  string,
  { default: Messages } | Messages
>;
for (const [path, mod] of Object.entries(modules)) {
  const code = path.replace(/^.*\/(.+)\.json$/, "$1");
  const messages = ((mod as { default?: Messages }).default ?? mod) as Messages;
  catalogs.set(code, { name: messages["_meta.name"] ?? code, messages });
}

let current = "en";
const listeners = new Set<(code: string) => void>();

/** Adds (or replaces) a catalog at runtime. */
export function registerLocale(code: string, name: string, messages: Messages): void {
  catalogs.set(code, { name, messages });
}

export function t(key: string, params?: Record<string, string | number>): string {
  const raw = catalogs.get(current)?.messages[key] ?? catalogs.get("en")?.messages[key] ?? key;
  if (!params) return raw;
  return raw.replace(/\{(\w+)\}/g, (m, k: string) => (k in params ? String(params[k]) : m));
}

export function getLocale(): string {
  return current;
}

export function availableLocales(): { code: string; name: string }[] {
  return [...catalogs.entries()]
    .map(([code, c]) => ({ code, name: c.name }))
    .sort((a, b) => a.code.localeCompare(b.code));
}

export function renderI18n(root: ParentNode = document): void {
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n!);
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    el.title = t(el.dataset.i18nTitle!);
  });
}

export function onLocaleChange(fn: (code: string) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function setLocale(code: string): void {
  if (!catalogs.has(code)) code = "en";
  current = code;
  try {
    localStorage.setItem(KEY, code);
  } catch {
    /* storage unavailable */
  }
  document.documentElement.lang = code;
  renderI18n();
  listeners.forEach((fn) => fn(code));
}

export function initLocale(): void {
  let stored: string | null = null;
  try {
    stored = localStorage.getItem(KEY);
  } catch {
    /* ignore */
  }
  const nav = (typeof navigator !== "undefined" ? navigator.language : "en").slice(0, 2).toLowerCase();
  const pick = [stored, nav].find((c): c is string => !!c && catalogs.has(c)) ?? "en";
  setLocale(pick);
}

export function formatNumber(n: number, digits: number): string {
  return new Intl.NumberFormat(current, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(n);
}
