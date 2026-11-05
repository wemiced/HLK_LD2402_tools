import { ApiError } from "../api";
import { t } from "../i18n";

/** Localized message for an error payload ({kind, ...data}). */
export function errorMessage(data: Record<string, unknown>): string {
  const kind = String(data.kind);
  const params = data as Record<string, string | number>;
  if (kind === "config") return t("errors.config." + String(data.reason), params);
  const key = "errors." + kind;
  return t(key, params) === key ? t("errors.unknown") : t(key, params);
}

export function errorMessageOf(e: unknown): string {
  return errorMessage(e instanceof ApiError ? { ...e.data, kind: e.kind } : { kind: "io", message: String(e) });
}
