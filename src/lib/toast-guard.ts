/**
 * Pure toast sanitization, deduplication, and burst-coalescing guards.
 * Prevents Sonner/WebView2 crashes caused by toast floods, oversized error strings,
 * or non-string payloads during bulk operations (delete, reset, import, GDI unload).
 */

export const MAX_TOAST_TITLE_LEN = 200;
export const MAX_TOAST_DESC_LEN = 480;
export const TOAST_DEDUPE_WINDOW_MS = 1_200;
export const TOAST_BURST_WINDOW_MS = 1_000;
export const TOAST_BURST_LIMIT = 5;
export const TOAST_BURST_ID = "fm-toast-burst";

/** Extract and safely truncate a plain-text toast string from unknown payloads. */
export function sanitizeToastText(value: unknown, maxLen: number): string {
  let raw = "";
  if (typeof value === "string") {
    raw = value;
  } else if (typeof value === "number" || typeof value === "boolean") {
    raw = String(value);
  } else if (value instanceof Error) {
    raw = value.message || value.name || "";
  } else if (value && typeof value === "object" && "message" in value) {
    const m = (value as { message?: unknown }).message;
    if (typeof m === "string") raw = m;
  }
  if (!raw) return "";
  // Strip ASCII control chars except newline/tab, collapse excessive whitespace.
  // eslint-disable-next-line no-control-regex
  const cleaned = raw.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "").replace(/\s+/g, " ").trim();
  if (cleaned.length <= maxLen) return cleaned;
  return `${cleaned.slice(0, Math.max(1, maxLen - 1)).trimEnd()}…`;
}

/** Stable short hash for deduping identical un-ID'd toasts within a short window. */
function hashKey(input: string): string {
  let h = 2166136261;
  for (let i = 0; i < input.length; i += 1) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0).toString(36);
}

export interface ToastGuardState {
  recentTimestamps: number[];
  recentKeys: Map<string, { at: number; id: string }>;
  burstSuppressed: number;
  burstResetAt: number;
}

export function createToastGuardState(): ToastGuardState {
  return {
    recentTimestamps: [],
    recentKeys: new Map(),
    burstSuppressed: 0,
    burstResetAt: 0,
  };
}

export type ToastGuardDecision =
  | { action: "pass"; id?: string | number }
  | { action: "coalesce"; id: string; summaryTitle: string; summaryDescription: string; count: number };

/**
 * Evaluate whether an incoming toast should pass through normally, reuse a dedupe ID,
 * or coalesce into a single burst-summary toast to protect Sonner/React from DOM floods.
 */
export function evaluateToastGuard(
  state: ToastGuardState,
  now: number,
  key: string,
  explicitId?: string | number,
): ToastGuardDecision {
  // Prune expired burst timestamps.
  const burstCutoff = now - TOAST_BURST_WINDOW_MS;
  while (state.recentTimestamps.length && state.recentTimestamps[0]! < burstCutoff) {
    state.recentTimestamps.shift();
  }
  if (state.burstResetAt && now >= state.burstResetAt) {
    state.burstSuppressed = 0;
    state.burstResetAt = 0;
  }

  // Prune expired dedupe keys (bounded size).
  const dedupeCutoff = now - TOAST_DEDUPE_WINDOW_MS;
  if (state.recentKeys.size > 40) {
    for (const [k, v] of state.recentKeys) {
      if (v.at < dedupeCutoff) state.recentKeys.delete(k);
    }
  }

  // Explicit IDs (progress/status toasts that update in place) never flood the stack.
  if (explicitId !== undefined) {
    return { action: "pass", id: explicitId };
  }

  // Identical un-ID'd toast inside the dedupe window reuses a deterministic ID (updates in place).
  const existing = state.recentKeys.get(key);
  if (existing && existing.at >= dedupeCutoff) {
    existing.at = now;
    return { action: "pass", id: existing.id };
  }

  // Too many distinct un-ID'd popups in < 1s — coalesce into ONE summary toast.
  if (state.recentTimestamps.length >= TOAST_BURST_LIMIT) {
    state.burstSuppressed += 1;
    state.burstResetAt = now + TOAST_BURST_WINDOW_MS * 2;
    const total = state.recentTimestamps.length + state.burstSuppressed;
    return {
      action: "coalesce",
      id: TOAST_BURST_ID,
      count: total,
      summaryTitle: `${total} updates in progress`,
      summaryDescription: "Open Notifications to view all activity.",
    };
  }

  const assignedId = `fm-dedupe-${hashKey(key)}`;
  state.recentKeys.set(key, { at: now, id: assignedId });
  state.recentTimestamps.push(now);
  return { action: "pass", id: assignedId };
}
