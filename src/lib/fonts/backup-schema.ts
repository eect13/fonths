import { coerceDesktopPrefs, DEFAULT_DESKTOP_PREFS, type DesktopPrefs } from "../desktop/prefs.ts";
import type { SavedImportRule } from "./import-options.ts";

export const FONTHS_BACKUP_VERSION = 1;

export interface BackupFontSet {
  id: string;
  name: string;
  families: string[];
  updatedAt: number;
}

export interface BackupCollection {
  id: string;
  name: string;
  fontIds: string[];
  createdAt: number;
  parentId: string | null;
  watchPath?: string;
  autoActivate?: boolean;
  importFilter?: SavedImportRule;
}

export interface LibraryBackup {
  version: number;
  app: "Fonths";
  exportedAt: string;
  favorites: string[];
  collections: BackupCollection[];
  fontSets?: BackupFontSet[];
  customTags: Record<string, string[]>;
  featurePrefs: Record<string, Record<string, boolean>>;
  previewAxes?: Record<string, Record<string, number>>;
  autoHideDuplicates: boolean;
  desktopPrefs: DesktopPrefs;
}

export interface BackupSourceState {
  favorites: string[];
  collections: BackupCollection[];
  fontSets?: BackupFontSet[];
  customTags: Record<string, string[]>;
  featurePrefs: Record<string, Record<string, boolean>>;
  previewAxes?: Record<string, Record<string, number>>;
  autoHideDuplicates: boolean;
  desktopPrefs: DesktopPrefs;
}

function sanitizeStringArray(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  return Array.from(new Set(raw.filter((v): v is string => typeof v === "string" && v.length > 0)));
}

function sanitizeCustomTags(raw: unknown): Record<string, string[]> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const out: Record<string, string[]> = {};
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    const clean = sanitizeStringArray(v);
    if (k && clean.length) out[k] = clean;
  }
  return out;
}

function sanitizeFeaturePrefs(raw: unknown): Record<string, Record<string, boolean>> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const out: Record<string, Record<string, boolean>> = {};
  for (const [fontId, map] of Object.entries(raw as Record<string, unknown>)) {
    if (!fontId || !map || typeof map !== "object" || Array.isArray(map)) continue;
    const entry: Record<string, boolean> = {};
    for (const [tag, val] of Object.entries(map as Record<string, unknown>)) {
      if (tag && typeof val === "boolean") entry[tag] = val;
    }
    if (Object.keys(entry).length) out[fontId] = entry;
  }
  return out;
}

function sanitizePreviewAxes(raw: unknown): Record<string, Record<string, number>> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const out: Record<string, Record<string, number>> = {};
  for (const [fontId, map] of Object.entries(raw as Record<string, unknown>)) {
    if (!fontId || !map || typeof map !== "object" || Array.isArray(map)) continue;
    const entry: Record<string, number> = {};
    for (const [axis, val] of Object.entries(map as Record<string, unknown>)) {
      if (axis && typeof val === "number" && Number.isFinite(val)) entry[axis] = val;
    }
    if (Object.keys(entry).length) out[fontId] = entry;
  }
  return out;
}

function sanitizeCollections(raw: unknown): BackupCollection[] {
  if (!Array.isArray(raw)) return [];
  const out: BackupCollection[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object" || Array.isArray(item)) continue;
    const c = item as Record<string, unknown>;
    if (typeof c.id !== "string" || !c.id.trim() || typeof c.name !== "string" || !c.name.trim()) {
      continue;
    }
    const clean: BackupCollection = {
      id: c.id,
      name: c.name,
      fontIds: sanitizeStringArray(c.fontIds),
      createdAt: typeof c.createdAt === "number" && Number.isFinite(c.createdAt) ? c.createdAt : Date.now(),
      parentId: typeof c.parentId === "string" && c.parentId.length > 0 ? c.parentId : null,
    };
    if (typeof c.watchPath === "string" && c.watchPath.trim()) {
      clean.watchPath = c.watchPath;
    }
    if (typeof c.autoActivate === "boolean") {
      clean.autoActivate = c.autoActivate;
    }
    if (c.importFilter && typeof c.importFilter === "object" && !Array.isArray(c.importFilter)) {
      clean.importFilter = c.importFilter as SavedImportRule;
    }
    out.push(clean);
  }
  return out;
}

function sanitizeFontSets(raw: unknown): BackupFontSet[] {
  if (!Array.isArray(raw)) return [];
  const out: BackupFontSet[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object" || Array.isArray(item)) continue;
    const fsItem = item as Record<string, unknown>;
    if (typeof fsItem.id !== "string" || !fsItem.id.trim() || typeof fsItem.name !== "string" || !fsItem.name.trim()) {
      continue;
    }
    out.push({
      id: fsItem.id,
      name: fsItem.name,
      families: sanitizeStringArray(fsItem.families),
      updatedAt:
        typeof fsItem.updatedAt === "number" && Number.isFinite(fsItem.updatedAt)
          ? fsItem.updatedAt
          : Date.now(),
    });
  }
  return out;
}

export function buildLibraryBackup(state: BackupSourceState): LibraryBackup {
  return {
    version: FONTHS_BACKUP_VERSION,
    app: "Fonths",
    exportedAt: new Date().toISOString(),
    favorites: sanitizeStringArray(state.favorites),
    collections: sanitizeCollections(state.collections),
    fontSets: sanitizeFontSets(state.fontSets ?? []),
    customTags: sanitizeCustomTags(state.customTags),
    featurePrefs: sanitizeFeaturePrefs(state.featurePrefs),
    previewAxes: sanitizePreviewAxes(state.previewAxes ?? {}),
    autoHideDuplicates: Boolean(state.autoHideDuplicates),
    desktopPrefs: coerceDesktopPrefs(state.desktopPrefs ?? DEFAULT_DESKTOP_PREFS),
  };
}

export function parseLibraryBackup(raw: string): LibraryBackup {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("Backup file is not valid JSON.");
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Not a valid Fonths backup file.");
  }
  const o = parsed as Record<string, unknown>;
  if (o.app !== "Fonths") {
    throw new Error("Not a Fonths library backup file.");
  }
  if (o.version !== FONTHS_BACKUP_VERSION) {
    throw new Error(`Unsupported backup version: ${String(o.version ?? "unknown")}.`);
  }
  if (!Array.isArray(o.favorites) || !Array.isArray(o.collections)) {
    throw new Error("Backup file is missing favorites or collections data.");
  }
  return {
    version: FONTHS_BACKUP_VERSION,
    app: "Fonths",
    exportedAt: typeof o.exportedAt === "string" ? o.exportedAt : new Date().toISOString(),
    favorites: sanitizeStringArray(o.favorites),
    collections: sanitizeCollections(o.collections),
    fontSets: sanitizeFontSets(o.fontSets),
    customTags: sanitizeCustomTags(o.customTags),
    featurePrefs: sanitizeFeaturePrefs(o.featurePrefs),
    previewAxes: sanitizePreviewAxes(o.previewAxes),
    autoHideDuplicates: Boolean(o.autoHideDuplicates),
    desktopPrefs: coerceDesktopPrefs(o.desktopPrefs),
  };
}

export function mergeLibraryBackupState<T extends BackupSourceState>(
  current: T,
  backup: LibraryBackup,
): {
  favorites: string[];
  collections: BackupCollection[];
  fontSets: BackupFontSet[];
  customTags: Record<string, string[]>;
  featurePrefs: Record<string, Record<string, boolean>>;
  previewAxes: Record<string, Record<string, number>>;
  autoHideDuplicates: boolean;
  desktopPrefs: DesktopPrefs;
} {
  const favorites = Array.from(new Set([...sanitizeStringArray(current.favorites), ...backup.favorites]));

  const colMap = new Map<string, BackupCollection>();
  for (const col of current.collections ?? []) {
    colMap.set(col.id, { ...col, fontIds: [...col.fontIds] });
  }
  for (const col of backup.collections) {
    const existing = colMap.get(col.id);
    if (existing) {
      colMap.set(col.id, {
        ...col,
        fontIds: Array.from(new Set([...existing.fontIds, ...col.fontIds])),
        watchPath: existing.watchPath ?? col.watchPath,
      });
    } else {
      colMap.set(col.id, { ...col, fontIds: [...col.fontIds] });
    }
  }

  const fontSetMap = new Map<string, BackupFontSet>();
  for (const fsItem of current.fontSets ?? []) {
    fontSetMap.set(fsItem.id, fsItem);
  }
  for (const fsItem of backup.fontSets ?? []) {
    const existing = fontSetMap.get(fsItem.id);
    if (existing) {
      fontSetMap.set(fsItem.id, {
        ...fsItem,
        families: Array.from(new Set([...existing.families, ...fsItem.families])),
      });
    } else {
      fontSetMap.set(fsItem.id, fsItem);
    }
  }

  const customTags: Record<string, string[]> = {};
  for (const [id, list] of Object.entries(current.customTags ?? {})) {
    customTags[id] = [...list];
  }
  for (const [id, list] of Object.entries(backup.customTags)) {
    customTags[id] = Array.from(new Set([...(customTags[id] ?? []), ...list]));
  }

  const featurePrefs: Record<string, Record<string, boolean>> = {};
  for (const [id, map] of Object.entries(current.featurePrefs ?? {})) {
    featurePrefs[id] = { ...map };
  }
  for (const [id, map] of Object.entries(backup.featurePrefs)) {
    featurePrefs[id] = { ...(featurePrefs[id] ?? {}), ...map };
  }

  const previewAxes: Record<string, Record<string, number>> = {};
  for (const [id, map] of Object.entries(current.previewAxes ?? {})) {
    previewAxes[id] = { ...map };
  }
  for (const [id, map] of Object.entries(backup.previewAxes ?? {})) {
    previewAxes[id] = { ...(previewAxes[id] ?? {}), ...map };
  }

  const desktopPrefs = coerceDesktopPrefs({
    ...(current.desktopPrefs ?? DEFAULT_DESKTOP_PREFS),
    ...backup.desktopPrefs,
  });

  return {
    favorites,
    collections: Array.from(colMap.values()),
    fontSets: Array.from(fontSetMap.values()),
    customTags,
    featurePrefs,
    previewAxes,
    autoHideDuplicates: Boolean(backup.autoHideDuplicates),
    desktopPrefs,
  };
}
