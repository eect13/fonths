import { useSyncExternalStore } from "react";
import { toast } from "sonner";
import {
  createToastGuardState,
  evaluateToastGuard,
  MAX_TOAST_DESC_LEN,
  MAX_TOAST_TITLE_LEN,
  sanitizeToastText,
} from "./toast-guard";

export type NotificationKind = "success" | "error" | "info" | "warning";

export type NotificationEntry = {
  id: string;
  toastId?: string | number;
  kind: NotificationKind;
  title: string;
  description?: string;
  at: number;
  read: boolean;
};

const STORAGE_KEY = "font-manager:notifications-v1";
const MAX_ITEMS = 60;
const PERSIST_DEBOUNCE_MS = 300;

let items: NotificationEntry[] = loadInitial();
let installed = false;
let persistTimer: number | null = null;
const listeners = new Set<() => void>();
const toastGuard = createToastGuardState();

function loadInitial(): NotificationEntry[] {
  try {
    if (typeof window === "undefined") return [];
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (x): x is NotificationEntry =>
          Boolean(x) &&
          typeof x === "object" &&
          typeof (x as NotificationEntry).id === "string" &&
          typeof (x as NotificationEntry).title === "string" &&
          typeof (x as NotificationEntry).at === "number",
      )
      .slice(0, MAX_ITEMS);
  } catch {
    return [];
  }
}

export function flushNotificationsPersist() {
  if (persistTimer !== null && typeof window !== "undefined") {
    window.clearTimeout(persistTimer);
    persistTimer = null;
  }
  try {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
  } catch {
    /* ignore storage quota */
  }
}

function schedulePersist() {
  if (typeof window === "undefined") return;
  if (persistTimer !== null) return;
  persistTimer = window.setTimeout(() => {
    persistTimer = null;
    flushNotificationsPersist();
  }, PERSIST_DEBOUNCE_MS);
}

function emit() {
  schedulePersist();
  for (const fn of listeners) {
    try {
      fn();
    } catch {
      /* never let a subscriber crash toast dispatch */
    }
  }
}

export function recordNotification(
  kind: NotificationKind,
  title: unknown,
  opts?: { id?: string | number; description?: unknown },
) {
  const cleanTitle = sanitizeToastText(title, MAX_TOAST_TITLE_LEN);
  const cleanDesc = sanitizeToastText(opts?.description, MAX_TOAST_DESC_LEN) || undefined;
  if (!cleanTitle && !cleanDesc) return;
  const now = Date.now();
  const toastId = opts?.id;

  // Update existing entry in place when the same toast id updates (e.g. progress/status toasts).
  if (toastId !== undefined) {
    const idx = items.findIndex((n) => n.toastId === toastId);
    if (idx >= 0) {
      const next = items.slice();
      next[idx] = {
        ...next[idx]!,
        kind,
        title: cleanTitle || next[idx]!.title,
        description: cleanDesc,
        at: now,
        read: false,
      };
      // Move updated item to the top
      const [moved] = next.splice(idx, 1);
      items = [moved!, ...next];
      emit();
      return;
    }
  }

  const entry: NotificationEntry = {
    id: `n-${now}-${Math.random().toString(36).slice(2, 7)}`,
    toastId,
    kind,
    title: cleanTitle || cleanDesc || "Notification",
    description: cleanTitle ? cleanDesc : undefined,
    at: now,
    read: false,
  };
  items = [entry, ...items].slice(0, MAX_ITEMS);
  emit();
}

export function markAllNotificationsRead() {
  if (!items.some((n) => !n.read)) return;
  items = items.map((n) => (n.read ? n : { ...n, read: true }));
  emit();
}

export function removeNotification(id: string) {
  const next = items.filter((n) => n.id !== id);
  if (next.length === items.length) return;
  items = next;
  emit();
}

export function clearNotifications() {
  if (!items.length) return;
  items = [];
  flushNotificationsPersist();
  for (const fn of listeners) {
    try {
      fn();
    } catch {
      /* ignore */
    }
  }
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

function getSnapshot() {
  return items;
}

export function useNotifications(): NotificationEntry[] {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * Wrap sonner's toast methods once at app boot so every popup across the app
 * is captured automatically into the Notification Center history and protected
 * against burst floods, oversized strings, or uncaught render exceptions.
 */
export function installNotificationRecorder() {
  if (installed) return;
  installed = true;

  if (typeof window !== "undefined") {
    window.addEventListener("pagehide", flushNotificationsPersist);
  }

  type ToastMethod = (message: unknown, data?: Record<string, unknown>) => string | number;
  const origMessage = (toast as unknown as Record<string, ToastMethod>).message;

  const wrap = (
    key: "success" | "error" | "message" | "info" | "warning",
    kind: NotificationKind,
  ) => {
    const orig = (toast as unknown as Record<string, ToastMethod>)[key];
    if (typeof orig !== "function") return;
    (toast as unknown as Record<string, ToastMethod>)[key] = (
      message: unknown,
      data?: Record<string, unknown>,
    ) => {
      const explicitId = data?.id as string | number | undefined;
      recordNotification(kind, message, {
        id: explicitId,
        description: data?.description,
      });

      const safeTitle =
        typeof message === "string" || typeof message === "number" || typeof message === "boolean" || message instanceof Error
          ? sanitizeToastText(message, MAX_TOAST_TITLE_LEN)
          : message;
      const safeData: Record<string, unknown> | undefined = data
        ? {
            ...data,
            ...(typeof data.description === "string" ||
            typeof data.description === "number" ||
            typeof data.description === "boolean" ||
            data.description instanceof Error
              ? { description: sanitizeToastText(data.description, MAX_TOAST_DESC_LEN) }
              : {}),
          }
        : undefined;

      const guardKey = `${kind}|${sanitizeToastText(message, 120)}|${sanitizeToastText(data?.description, 120)}`;
      const decision = evaluateToastGuard(toastGuard, Date.now(), guardKey, explicitId);

      try {
        if (decision.action === "coalesce") {
          const fn = typeof origMessage === "function" ? origMessage : orig;
          return fn(decision.summaryTitle, {
            id: decision.id,
            description: decision.summaryDescription,
            duration: 4_000,
          });
        }
        const nextData =
          decision.id !== undefined
            ? { ...(safeData ?? {}), id: decision.id }
            : safeData;
        return orig(safeTitle, nextData);
      } catch {
        return explicitId ?? "";
      }
    };
  };

  wrap("success", "success");
  wrap("error", "error");
  wrap("message", "info");
  wrap("info", "info");
  wrap("warning", "warning");
}
