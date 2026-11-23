/**
 * Thin IPC bridge to the Rust Skia (FreeType + HarfBuzz) render worker (`src-tauri/src/renderer.rs`).
 */

export interface RenderFontSpecimenOptions {
  /** One Inspector canvas = one slot; newer requests for a slot supersede queued older ones. */
  slot: string;
  /** Every on-disk file of the family; Rust picks the face matching `weight` / `italic`. */
  fontPaths: string[];
  text: string;
  fontSize: number;
  width: number;
  maxHeight: number;
  colorHex: string;
  features?: Record<string, number>;
  variations?: Record<string, number>;
  weight?: number;
  italic?: boolean;
  align?: "left" | "center" | "right";
  lineHeight?: number;
}

export interface SkiaSpecimenFrame {
  width: number;
  height: number;
  pixels: Uint8ClampedArray;
}

/** Error text Rust returns when a newer request for the same slot replaced this one. */
export const SKIA_SUPERSEDED = "superseded";

/**
 * Render an auto-height specimen. Response layout: `[u32 LE width][u32 LE height]` + RGBA8888.
 */
export async function renderFontSpecimen(opts: RenderFontSpecimenOptions): Promise<SkiaSpecimenFrame> {
  const { invoke } = await import("@tauri-apps/api/core");
  const buffer = await invoke<ArrayBuffer>("render_font_specimen", {
    slot: opts.slot,
    fontPaths: opts.fontPaths,
    text: opts.text,
    fontSize: opts.fontSize,
    width: Math.max(1, Math.round(opts.width)),
    maxHeight: Math.max(1, Math.round(opts.maxHeight)),
    colorHex: opts.colorHex,
    features: opts.features ?? {},
    variations: opts.variations ?? {},
    weight: opts.weight ?? null,
    italic: Boolean(opts.italic),
    align: opts.align ?? "left",
    lineHeight: opts.lineHeight ?? null,
  });
  if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 8) {
    throw new Error("Empty Skia frame");
  }
  const view = new DataView(buffer);
  const width = view.getUint32(0, true);
  const height = view.getUint32(4, true);
  const expected = width * height * 4;
  if (buffer.byteLength - 8 !== expected) {
    throw new Error(`Unexpected RGBA buffer (${buffer.byteLength - 8} != ${expected})`);
  }
  return { width, height, pixels: new Uint8ClampedArray(buffer, 8, expected) };
}

const familyFilesCache = new Map<string, Promise<string[]>>();

/** All intact on-disk files for a family (cached per session; cleared on library changes). */
export function familyFontFiles(family: string): Promise<string[]> {
  const key = family.trim().toLowerCase();
  let hit = familyFilesCache.get(key);
  if (!hit) {
    hit = import("@tauri-apps/api/core")
      .then(({ invoke }) => invoke<string[]>("list_family_font_files", { family }))
      .then((files) => (Array.isArray(files) ? files : []))
      .catch(() => [] as string[]);
    familyFilesCache.set(key, hit);
    // Never cache an empty answer: the family may still be downloading.
    void hit.then((files) => {
      if (!files.length) familyFilesCache.delete(key);
    });
  }
  return hit;
}

export function forgetFamilyFontFiles(family?: string) {
  if (family) familyFilesCache.delete(family.trim().toLowerCase());
  else familyFilesCache.clear();
}

/** Convert a computed CSS colour (`rgb(...)` / `rgba(...)` / `#hex`) to `#rrggbb[aa]`. */
export function cssColorToHex(css: string, fallback = "#111111"): string {
  const s = css.trim();
  if (/^#[0-9a-f]{3,8}$/i.test(s)) return s;
  const m = /^rgba?\(([^)]+)\)$/i.exec(s);
  if (!m) return fallback;
  const parts = m[1]!.split(/[\s,/]+/).filter(Boolean).map(Number);
  if (parts.length < 3 || parts.slice(0, 3).some((n) => !Number.isFinite(n))) return fallback;
  const hex = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");
  const alpha = parts[3];
  const base = `#${hex(parts[0]!)}${hex(parts[1]!)}${hex(parts[2]!)}`;
  return typeof alpha === "number" && Number.isFinite(alpha) && alpha < 1 ? `${base}${hex(alpha * 255)}` : base;
}
