import { onLocaleChange, renderI18n, t } from "../i18n";
import { onDisconnect } from "../state/store";

export interface ModalAction<V> {
  label: string;
  /** "option" renders a full-width choice row (with optional `hint`). */
  kind: "primary" | "ghost" | "danger" | "option";
  value: V;
  hint?: string;
}

export interface ModalOptions<V> {
  title: string;
  body: Node;
  actions: ModalAction<V>[];
  /** Tone of the dialog: a warning tints the title. */
  tone?: "default" | "warning";
}

let seq = 0;
let closeCurrent: (() => void) | null = null;
// a modal never outlives its connection
onDisconnect(() => closeCurrent?.());

/**
 * In-app modal. Resolves with the chosen action's value, or null on Esc / backdrop / close.
 * Focus moves to the first action and returns to the previous element on close.
 */
export function openModal<V>(opts: ModalOptions<V>): Promise<V | null> {
  if (closeCurrent) return Promise.resolve(null);
  const previous = document.activeElement as HTMLElement | null;
  const id = `modal-${++seq}`;
  const backdrop = document.createElement("div");
  backdrop.className = "modal-backdrop";
  const dlg = document.createElement("div");
  dlg.className = "modal";
  dlg.setAttribute("role", "dialog");
  dlg.setAttribute("aria-modal", "true");
  dlg.setAttribute("aria-labelledby", `${id}-title`);

  const head = document.createElement("div");
  head.className = "row";
  const h = document.createElement("h3");
  h.id = `${id}-title`;
  h.textContent = opts.title;
  if (opts.tone === "warning") h.classList.add("warn");
  const grow = document.createElement("div");
  grow.className = "grow";
  const x = document.createElement("button");
  x.className = "btn ghost sm";
  x.textContent = "✕";
  x.setAttribute("aria-label", t("modal.close"));
  head.append(h, grow, x);

  const body = document.createElement("div");
  body.className = "modal-body";
  body.appendChild(opts.body);

  const foot = document.createElement("div");
  foot.className = "modal-actions";
  const options = opts.actions.some((a) => a.kind === "option");
  if (options) foot.classList.add("stack");

  dlg.append(head, body, foot);
  backdrop.appendChild(dlg);

  return new Promise<V | null>((resolve) => {
    let done = false;
    const finish = (v: V | null) => {
      if (done) return;
      done = true;
      closeCurrent = null;
      document.removeEventListener("keydown", onKey, true);
      off();
      backdrop.remove();
      if (previous && previous.isConnected) previous.focus();
      resolve(v);
    };
    const buttons = opts.actions.map((a) => {
      const b = document.createElement("button");
      b.className = a.kind === "option" ? "btn option" : `btn ${a.kind}`;
      const l = document.createElement("span");
      l.textContent = a.label;
      b.appendChild(l);
      if (a.hint) {
        const s = document.createElement("small");
        s.textContent = a.hint;
        b.appendChild(s);
      }
      b.addEventListener("click", () => finish(a.value));
      foot.appendChild(b);
      return b;
    });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        finish(null);
      } else if (e.key === "Tab") {
        const f = [x, ...buttons];
        const i = f.indexOf(document.activeElement as HTMLButtonElement);
        const next = e.shiftKey ? (i <= 0 ? f.length - 1 : i - 1) : (i + 1) % f.length;
        e.preventDefault();
        f[next].focus();
      }
    };
    const off = onLocaleChange(() => {
      renderI18n(dlg);
      x.setAttribute("aria-label", t("modal.close"));
    });
    closeCurrent = () => finish(null);
    x.addEventListener("click", () => finish(null));
    backdrop.addEventListener("mousedown", (e) => { if (e.target === backdrop) finish(null); });
    document.addEventListener("keydown", onKey, true);
    document.body.appendChild(backdrop);
    (buttons[0] ?? x).focus();
  });
}
