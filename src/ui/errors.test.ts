import { beforeEach, expect, it } from "vitest";
import { setLocale } from "../i18n";
import { errorMessage } from "./errors";

beforeEach(() => setLocale("en"));

it("unsupported names the feature in the user's language", () => {
  expect(errorMessage({ kind: "unsupported", feature: "autoGain", required: "3.3.5" })).toBe(
    "Auto gain adjustment is not supported by this firmware (requires 3.3.5).",
  );
  setLocale("fr");
  expect(errorMessage({ kind: "unsupported", feature: "permanentSave", required: "3.3.2" })).not.toContain("permanentSave");
});

it("timeout and nack show the command word as hex", () => {
  expect(errorMessage({ kind: "timeout", word: 0xff })).toBe("The sensor did not answer in time (command 0x00FF).");
  expect(errorMessage({ kind: "nack", word: 0x12, status: 1 })).toBe("The sensor rejected command 0x0012 (status 1).");
});
