/** Desktop Rust parser (ttf-parser + sha2). Web keeps sfnt.ts / SubtleCrypto.
 *  Pass Uint8Array — never Array.from (JSON-boxes every byte). */

import { metricsFromTables } from "./metrics";
import { snapAxes } from "./axes";
import type { FontMetrics } from "./types";
import { CMAP_GLYPH_CAP, shouldUseIpcCmap, shouldUseIpcLayout } from "./wasm-parse";

export type NativeGlyph = { cp: number; gid: number; name: string };
export type NativeAxis = { tag: string; name: string; min: number; max: number; def: number };
export type NativeMetrics = {
  upem: number;
  weightClass: number;
  widthClass: number;
  xHeight?: number;
  capHeight?: number;
  panose?: number[];
  familyClass?: number;
  isFixedPitch?: boolean;
};
export type NativeLayout = {
  axes: NativeAxis[];
  otFeatures: string[];
  variable: boolean;
  glyphCount: number;
  metrics?: NativeMetrics;
};

export function fontMetricsFromLayout(layout: NativeLayout): FontMetrics | undefined {
  if (!layout.metrics) return undefined;
  return metricsFromTables({
    upem: layout.metrics.upem,
    weightClass: layout.metrics.weightClass,
    widthClass: layout.metrics.widthClass,
    xHeight: layout.metrics.xHeight,
    capHeight: layout.metrics.capHeight,
    panose: layout.metrics.panose,
    sFamilyClass: layout.metrics.familyClass,
    isFixedPitch: layout.metrics.isFixedPitch,
  });
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  try {
    const api = await import("@tauri-apps/api/core");
    if (typeof api.isTauri === "function" && !api.isTauri()) return null;
    return await api.invoke<T>(cmd, args);
  } catch {
    return null;
  }
}

function bytesArg(buffer: ArrayBuffer | Uint8Array) {
  return buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
}

function snapLayout(layout: NativeLayout): NativeLayout {
  return { ...layout, axes: layout.axes?.length ? snapAxes(layout.axes) : layout.axes };
}

export async function nativeFamilyCmap(family: string): Promise<NativeGlyph[] | null> {
  const rows = await invoke<NativeGlyph[]>("parse_family_cmap", { family });
  return rows ? rows.slice(0, CMAP_GLYPH_CAP) : null;
}

export async function nativeFamilyLayout(family: string): Promise<NativeLayout | null> {
  const layout = await invoke<NativeLayout>("parse_family_layout", { family });
  return layout ? snapLayout(layout) : null;
}

export async function nativeLayoutFromBytes(buffer: ArrayBuffer): Promise<NativeLayout | null> {
  const faces = await nativeLayoutsFromBytes(buffer);
  if (faces?.length) {
    return faces.reduce((best, face) =>
      face.glyphCount > best.glyphCount || (face.glyphCount === best.glyphCount && face.axes.length > best.axes.length)
        ? face
        : best,
    );
  }
  if (!shouldUseIpcLayout(buffer.byteLength)) return null;
  const layout = await invoke<NativeLayout>("parse_font_layout", { bytes: bytesArg(buffer) });
  return layout ? snapLayout(layout) : null;
}

export async function nativeLayoutsFromBytes(buffer: ArrayBuffer): Promise<NativeLayout[] | null> {
  if (!shouldUseIpcLayout(buffer.byteLength)) return null;
  const faces = await invoke<NativeLayout[]>("parse_font_layouts", { bytes: bytesArg(buffer) });
  return faces?.map(snapLayout) ?? null;
}

export async function nativeCmapFromBytes(buffer: ArrayBuffer): Promise<NativeGlyph[] | null> {
  if (!shouldUseIpcCmap(buffer.byteLength)) return null;
  const rows = await invoke<NativeGlyph[]>("parse_font_cmap", { bytes: bytesArg(buffer) });
  return rows?.length ? rows.slice(0, CMAP_GLYPH_CAP) : null;
}

export async function nativeHashBytes(buffer: ArrayBuffer): Promise<string | null> {
  if (buffer.byteLength > 400_000) return null;
  return invoke<string>("hash_bytes", { bytes: bytesArg(buffer) });
}

export async function nativeHashPath(path: string): Promise<string | null> {
  return invoke<string>("hash_font_path", { path });
}

export type NativeIndexFace = {
  path: string;
  fileName: string;
  fileSize: number;
  family: string;
  fullName: string;
  weight: number;
  italic: boolean;
  variable: boolean;
  axes: NativeAxis[];
  metrics?: NativeMetrics;
  checksum: string;
  glyphCount: number;
  otFeatures: string[];
  /** Face in a TTC/OTC (0 for single fonts). */
  faceIndex?: number;
};

/** Desktop watch/20k: ttf-parser on disk. No File buffers in JS. WOFF2 omitted. */
export async function indexFontPaths(paths: string[]): Promise<NativeIndexFace[]> {
  if (!paths.length) return [];
  const rows = await invoke<NativeIndexFace[]>("index_font_paths", { paths });
  if (!rows?.length) return [];
  return rows.map((row) => ({
    ...row,
    axes: row.axes?.length ? snapAxes(row.axes) : [],
  }));
}

export async function nativeDiffBytes(
  left: Uint8Array,
  right: Uint8Array,
): Promise<{ near: boolean; diffs: number } | null> {
  if (left.byteLength > 8_000_000 || right.byteLength > 8_000_000) return null;
  return invoke<{ near: boolean; diffs: number }>("diff_font_bytes", {
    left: bytesArg(left),
    right: bytesArg(right),
  });
}

export type NativeFamilyPreviewInfo = {
  path: string;
  layout?: NativeLayout | null;
};

const previewInfoCache = new Map<string, NativeFamilyPreviewInfo>();

export async function nativeFamilyPreviewInfo(
  family: string,
  italic = false,
): Promise<NativeFamilyPreviewInfo | null> {
  const key = `${family.trim().toLowerCase()}|${italic ? 1 : 0}`;
  const cached = previewInfoCache.get(key);
  if (cached) return cached;
  const info = await invoke<NativeFamilyPreviewInfo>("read_family_preview_info", {
    family,
    italic,
  });
  if (!info?.path) return null;
  const normalized: NativeFamilyPreviewInfo = {
    path: info.path,
    layout: info.layout ? snapLayout(info.layout) : null,
  };
  previewInfoCache.set(key, normalized);
  return normalized;
}

export type DWriteEngineInfo = {
  engine: string;
  available: boolean;
  activationMode: string;
  systemFontsReadOnly: boolean;
  systemFontCount: number;
  platform: string;
  elevated?: boolean;
};

export type DWriteInspectResult = {
  supported: boolean;
  faceCount: number;
  fileType: string;
  faceType: string;
  isVariable: boolean;
  typographicFamily?: string | null;
  win32Family?: string | null;
  fullName?: string | null;
  weight: number;
  italic: boolean;
  hresultCode?: number | null;
  errorReason?: string | null;
};

export async function getDWriteEngineInfo(): Promise<DWriteEngineInfo | null> {
  return invoke<DWriteEngineInfo>("dwrite_engine_info");
}

export async function inspectFontDWrite(path: string): Promise<DWriteInspectResult | null> {
  return invoke<DWriteInspectResult>("dwrite_inspect_font", { path });
}

export type LeftoverStageFile = {
  path: string;
  fileName: string;
  bytes: number;
  modifiedMs: number;
  family?: string | null;
  reason: string;
};

export type RecycleStageFailure = {
  path: string;
  error: string;
};

export type RecycleLeftoversResult = {
  requested: number;
  recycled: number;
  failed: RecycleStageFailure[];
};

export async function listLeftoverStageFiles(): Promise<LeftoverStageFile[] | null> {
  return invoke<LeftoverStageFile[]>("list_leftover_stage_files");
}

export async function recycleLeftoverStageFiles(
  paths: string[],
): Promise<RecycleLeftoversResult | null> {
  return invoke<RecycleLeftoversResult>("recycle_leftover_stage_files", { paths });
}

export type TempStageCleanupResult = {
  scanned: number;
  deleted: number;
  failed: number;
};

export async function cleanupTempStageFiles(): Promise<TempStageCleanupResult | null> {
  return invoke<TempStageCleanupResult>("cleanup_temp_stage_files");
}

export type GoogleCdnSyncResult = {
  checked: number;
  outdated: number;
  incomplete: number;
  queued: number;
  upToDate: number;
  familiesQueued: string[];
};

export async function syncGoogleCdnUpdates(force = false): Promise<GoogleCdnSyncResult | null> {
  return invoke<GoogleCdnSyncResult>("sync_google_cdn_updates", { force });
}

export type PurgeNonGoogleResult = {
  candidates: string[];
  removed: number;
  failed: number;
  permanent: boolean;
};

export async function purgeNonGoogleFonts(
  keep: string[],
  dryRun = false,
): Promise<PurgeNonGoogleResult | null> {
  return invoke<PurgeNonGoogleResult>("purge_non_google_fonts", { keep, dryRun });
}
