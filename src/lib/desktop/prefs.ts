export type DeleteMode = "recycle" | "permanent";

export type DesktopPrefs = {
  closeToTray: boolean;
  startWithWindows: boolean;
  preferStaticFaces?: boolean;
  fastSessionResume?: boolean;
  deleteMode?: DeleteMode;
};

export const DEFAULT_DESKTOP_PREFS: DesktopPrefs = {
  closeToTray: false,
  startWithWindows: false,
  preferStaticFaces: false,
  fastSessionResume: true,
  deleteMode: "recycle",
};

export function coerceDesktopPrefs(raw: unknown): DesktopPrefs {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    closeToTray: Boolean(o.closeToTray),
    startWithWindows: Boolean(o.startWithWindows),
    preferStaticFaces: Boolean(o.preferStaticFaces),
    fastSessionResume: o.fastSessionResume === undefined ? true : Boolean(o.fastSessionResume),
    deleteMode: o.deleteMode === "permanent" ? "permanent" : "recycle",
  };
}

/** Push prefs to the desktop shell. No-op in the browser preview. */
export async function applyDesktopPrefs(prefs: DesktopPrefs): Promise<void> {
  try {
    const { invoke, isTauri } = await import("@tauri-apps/api/core");
    if (typeof isTauri === "function" && !isTauri()) return;
    await invoke("set_desktop_prefs", {
      closeToTray: prefs.closeToTray,
      startWithWindows: prefs.startWithWindows,
      preferStaticFaces: Boolean(prefs.preferStaticFaces),
      fastSessionResume: prefs.fastSessionResume !== false,
      deleteMode: prefs.deleteMode === "permanent" ? "permanent" : "recycle",
    });
  } catch {
    /* web / old installer */
  }
}
