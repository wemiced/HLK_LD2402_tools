import { expect, it } from "vitest";
import { createStore, onlyFrameChanged, type AppState } from "./store";

it("subscribers get the previous state and can skip frame-only updates", () => {
  const s = createStore<Pick<AppState, "lastFrame" | "view">>({ lastFrame: null, view: "dashboard" });
  const seen: boolean[] = [];
  s.subscribe((next, prev) => seen.push(onlyFrameChanged(next as AppState, prev as AppState)));
  s.set({ lastFrame: { presence: "none", distanceM: 0, motionDb: [], microDb: [] } });
  s.set({ view: "firmware" });
  expect(seen).toEqual([true, false]);
});
