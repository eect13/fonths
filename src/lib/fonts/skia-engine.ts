import { inDesktopShell } from "@/lib/desktop/open-fonts";

export interface SkiaVariableAxis {
  tag: string;
  min: number;
  default: number;
  max: number;
}

export interface SkiaFontMetadata {
  path: string;
  family_name: string;
  subfamily_name: string;
  postscript_name: string;
  is_variable: boolean;
  axes: SkiaVariableAxis[];
  opentype_features: string[];
}

export interface RenderFontPreviewOptions {
  fontPath: string;
  text: string;
  fontSize: number;
  width: number;
  height: number;
  colorHex: string;
  features?: Record<string, number>;
  variations?: Record<string, number>;
}

/** Recursively scan a directory via Rust Fontations (`read-fonts` + `memmap2`) without OS install. */
export async function scanFontDirectory(dirPath: string): Promise<SkiaFontMetadata[]> {
  if (!(await inDesktopShell())) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<SkiaFontMetadata[]>("scan_font_directory", { dirPath });
}

/** Open a native OS directory picker via `@tauri-apps/plugin-dialog` and scan it with `scan_font_directory`. */
export async function pickAndScanFontDirectory(): Promise<{
  dirPath: string;
  fonts: SkiaFontMetadata[];
} | null> {
  if (!(await inDesktopShell())) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    directory: true,
    multiple: false,
    title: "Select a font directory to scan with Skia + Fontations",
  });
  const dirPath = Array.isArray(picked) ? picked[0] : picked;
  if (!dirPath || typeof dirPath !== "string") return null;
  const fonts = await scanFontDirectory(dirPath);
  return { dirPath, fonts };
}

/**
 * Render a font preview on the dedicated Rust Skia (FreeType + HarfBuzz) OS thread.
 * Returns raw RGBA8888 pixel bytes directly over Tauri IPC (zero PNG encoding overhead).
 */
export async function renderFontPreview(opts: RenderFontPreviewOptions): Promise<ArrayBuffer> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<ArrayBuffer>("render_font_preview", {
    fontPath: opts.fontPath,
    text: opts.text,
    fontSize: opts.fontSize,
    width: Math.max(1, Math.round(opts.width)),
    height: Math.max(1, Math.round(opts.height)),
    colorHex: opts.colorHex,
    features: opts.features ?? {},
    variations: opts.variations ?? {},
  });
}

/** Temporarily activate a font path system-wide on Windows (`AddFontResourceExW` + `WM_FONTCHANGE`). */
export async function activateFontPath(fontPath: string): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke<void>("activate_font", { fontPath });
}

/** Deactivate a temporarily activated font path on Windows (`RemoveFontResourceExW` + `WM_FONTCHANGE`). */
export async function deactivateFontPath(fontPath: string): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke<void>("deactivate_font", { fontPath });
}

/** Human-friendly label for common 4-character OpenType GSUB/GPOS feature tags. */
export const OPENTYPE_FEATURE_LABELS: Record<string, string> = {
  liga: "Standard Ligatures",
  dlig: "Discretionary Ligatures",
  clig: "Contextual Ligatures",
  calt: "Contextual Alternates",
  kern: "Kerning",
  smcp: "Small Capitals",
  c2sc: "Caps to Small Caps",
  onum: "Oldstyle Figures",
  lnum: "Lining Figures",
  tnum: "Tabular Figures",
  pnum: "Proportional Figures",
  zero: "Slashed Zero",
  frac: "Fractions",
  sups: "Superscript",
  subs: "Subscript",
  swsh: "Swash",
  salt: "Stylistic Alternates",
  case: "Case-Sensitive Forms",
  ordn: "Ordinals",
  aalt: "Access All Alternates",
};

export function openTypeFeatureLabel(tag: string): string {
  if (OPENTYPE_FEATURE_LABELS[tag]) return OPENTYPE_FEATURE_LABELS[tag]!;
  if (/^ss\d\d$/.test(tag)) return `Stylistic Set ${tag.slice(2)}`;
  if (/^cv\d\d$/.test(tag)) return `Character Variant ${tag.slice(2)}`;
  return tag.toUpperCase();
}
