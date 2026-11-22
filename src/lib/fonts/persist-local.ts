import type { FontRecord } from "./types";
import { idbGet, idbPut } from "./idb";

/** IndexedDB key for upload catalog metadata. File blobs stay on their own keys. */
export const LOCAL_FONTS_META_ID = "meta:local-fonts";

export function slimLocalFont(font: FontRecord): FontRecord {
  const out: FontRecord = {
    id: font.id,
    family: font.family,
    source: "local",
    category: font.category,
    weights: font.weights?.length ? font.weights : [400],
    italic: Boolean(font.italic),
    variable: Boolean(font.variable),
    tags: font.tags ?? [],
    popularity: font.popularity ?? 9999,
    license: font.license,
  };
  if (font.fullName && font.fullName !== font.family) out.fullName = font.fullName;
  if (font.catalogVariable) out.catalogVariable = font.catalogVariable;
  if (font.axes?.length) out.axes = font.axes;
  if (font.instances?.length) out.instances = font.instances;
  if (font.licenseName) out.licenseName = font.licenseName;
  if (font.licenseUserSet) out.licenseUserSet = font.licenseUserSet;
  if (font.fileName) out.fileName = font.fileName;
  if (font.fileSize) out.fileSize = font.fileSize;
  if (font.checksum) out.checksum = font.checksum;
  if (font.originPath) out.originPath = font.originPath;
  if (typeof font.faceIndex === "number" && font.faceIndex !== 0) out.faceIndex = font.faceIndex;
  if (font.script) out.script = font.script;
  if (typeof font.coversLatin === "boolean") out.coversLatin = font.coversLatin;
  if (font.cssFamily && font.cssFamily !== font.family) out.cssFamily = font.cssFamily;
  if (font.colorKind && font.colorKind !== "none") out.colorKind = font.colorKind;
  if (font.metrics) out.metrics = font.metrics;
  return out;
}

export async function saveLocalFontsMeta(fonts: FontRecord[]): Promise<void> {
  // Chunked JSON serialization avoids massive single-string allocation spikes at 100k fonts.
  const parts: string[] = ["["];
  const CHUNK = 4000;
  for (let i = 0; i < fonts.length; i += CHUNK) {
    const slice = fonts.slice(i, i + CHUNK).map(slimLocalFont);
    const json = JSON.stringify(slice);
    if (i > 0) parts.push(",");
    parts.push(json.slice(1, -1));
  }
  parts.push("]");
  await idbPut(LOCAL_FONTS_META_ID, new Blob(parts, { type: "application/json" }));
}

export async function loadLocalFontsMeta(): Promise<FontRecord[] | null> {
  try {
    const blob = await idbGet(LOCAL_FONTS_META_ID);
    if (!blob || typeof blob.text !== "function") return null;
    const parsed = JSON.parse(await blob.text()) as unknown;
    if (!Array.isArray(parsed)) return null;
    const fonts = parsed.filter(
      (row): row is FontRecord =>
        Boolean(row) &&
        typeof row === "object" &&
        typeof (row as FontRecord).id === "string" &&
        typeof (row as FontRecord).family === "string",
    );
    return fonts;
  } catch {
    return null;
  }
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
let pending: FontRecord[] | null = null;

export function scheduleSaveLocalFontsMeta(fonts: FontRecord[]) {
  pending = fonts;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = undefined;
    const next = pending;
    pending = null;
    if (!next) return;
    void saveLocalFontsMeta(next).catch(() => undefined);
  }, 400);
}

export function pickLocalFontsPersist(idb: FontRecord[] | null, fromLs: FontRecord[]): FontRecord[] {
  const a = idb ?? [];
  if (a.length >= fromLs.length) return a.length ? a : fromLs;
  return fromLs;
}
