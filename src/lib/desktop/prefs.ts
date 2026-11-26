export type DeleteMode = "recycle" | "permanent";
export type ActivationMode = "instant-zero-ram" | "daemon-tray" | "session-only";
export type CacheCleanScope = "minimal" | "standard" | "deep";

export type DesktopPrefs = {
  closeToTray: boolean;
  startWithWindows: boolean;
  startMinimized?: boolean;
  preferStaticFaces?: boolean;
  fastSessionResume?: boolean;
  deleteMode?: DeleteMode;
  hkcuRegistrySync?: boolean;
  activationMode?: ActivationMode;
  cacheCleanScope?: CacheCleanScope;
};

export const DEFAULT_DESKTOP_PREFS: DesktopPrefs = {
  closeToTray: true,
  startWithWindows: false,
  startMinimized: false,
  preferStaticFaces: false,
  fastSessionResume: true,
  deleteMode: "recycle",
  hkcuRegistrySync: true,
  activationMode: "instant-zero-ram",
  cacheCleanScope: "standard",
};

function coerceActivationMode(raw: unknown): ActivationMode {
  if (raw === "daemon-tray" || raw === "session-only" || raw === "instant-zero-ram") {
    return raw;
  }
  return "instant-zero-ram";
}

function coerceCacheCleanScope(raw: unknown): CacheCleanScope {
  if (raw === "minimal" || raw === "standard" || raw === "deep") {
    return raw;
  }
  return "standard";
}

export function coerceDesktopPrefs(raw: unknown): DesktopPrefs {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const activationMode = coerceActivationMode(o.activationMode);
  const closeToTray =
    o.closeToTray === undefined
      ? activationMode !== "session-only"
      : Boolean(o.closeToTray);
  return {
    closeToTray,
    startWithWindows: Boolean(o.startWithWindows),
    startMinimized: Boolean(o.startMinimized),
    preferStaticFaces: Boolean(o.preferStaticFaces),
    fastSessionResume: o.fastSessionResume === undefined ? true : Boolean(o.fastSessionResume),
    deleteMode: o.deleteMode === "permanent" ? "permanent" : "recycle",
    hkcuRegistrySync:
      activationMode === "instant-zero-ram"
        ? true
        : o.hkcuRegistrySync === undefined
          ? true
          : Boolean(o.hkcuRegistrySync),
    activationMode,
    cacheCleanScope: coerceCacheCleanScope(o.cacheCleanScope),
  };
}

/** Push prefs to the desktop shell. No-op in the browser preview. */
export async function applyDesktopPrefs(prefs: DesktopPrefs): Promise<void> {
  try {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (typeof isTauri === "function" && !isTauri()) return;
    const mode = coerceActivationMode(prefs.activationMode);
    await invoke("set_desktop_prefs", {
      closeToTray: prefs.closeToTray,
      startWithWindows: prefs.startWithWindows,
      startMinimized: Boolean(prefs.startMinimized),
      preferStaticFaces: Boolean(prefs.preferStaticFaces),
      fastSessionResume: prefs.fastSessionResume !== false,
      deleteMode: prefs.deleteMode === "permanent" ? "permanent" : "recycle",
      hkcuRegistrySync: mode === "instant-zero-ram" ? true : prefs.hkcuRegistrySync !== false,
      activationMode: mode,
    });
  } catch {
    /* web / old installer */
  }
}
