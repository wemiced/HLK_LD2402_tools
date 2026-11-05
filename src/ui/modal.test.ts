import { beforeEach, expect, it } from "vitest";
import { setLocale } from "../i18n";
import { app } from "../state/store";
import { openModal } from "./modal";

const wait = () => new Promise((r) => setTimeout(r, 5));
const actions = [
  { label: "A", kind: "ghost" as const, value: "a" },
  { label: "B", kind: "primary" as const, value: "b" },
];
const open = () => openModal({ title: "T", body: document.createTextNode("body"), actions });
const btns = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".modal button"));
const key = (k: string, shiftKey = false) => document.dispatchEvent(new KeyboardEvent("keydown", { key: k, shiftKey, cancelable: true }));
let opener: HTMLButtonElement;

beforeEach(() => {
  setLocale("en");
  document.body.innerHTML = "<button id=o>open</button>";
  opener = document.getElementById("o") as HTMLButtonElement;
  opener.focus();
});

it("focuses first action, traps Tab, restores focus on close", async () => {
  const p = open();
  const [x, a, b] = btns();
  expect(document.activeElement).toBe(a);
  key("Tab"); expect(document.activeElement).toBe(b);
  key("Tab"); expect(document.activeElement).toBe(x);
  key("Tab", true); expect(document.activeElement).toBe(b);
  expect(document.querySelector(".modal")!.getAttribute("aria-modal")).toBe("true");
  b.click();
  expect(await p).toBe("b");
  expect(document.activeElement).toBe(opener);
});

it("Esc, backdrop and X resolve null; listeners are removed", async () => {
  let p = open(); key("Escape"); expect(await p).toBeNull();
  p = open(); document.querySelector<HTMLElement>(".modal-backdrop")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); expect(await p).toBeNull();
  p = open(); btns()[0].click(); expect(await p).toBeNull();
  expect(document.querySelector(".modal")).toBeNull();
  const ev = new KeyboardEvent("keydown", { key: "Tab", cancelable: true });
  document.dispatchEvent(ev);
  expect(ev.defaultPrevented).toBe(false);
});

it("only one modal at a time; disconnect closes it", async () => {
  app.set({ connection: "connected" });
  const p = open();
  expect(await open()).toBeNull();
  expect(document.querySelectorAll(".modal").length).toBe(1);
  app.set({ connection: "disconnected" });
  expect(await p).toBeNull();
  await wait();
  expect(document.querySelector(".modal")).toBeNull();
});
