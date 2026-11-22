import type { FontRecord } from "./types";
import { cssFamilyStack, googleCssUrls } from "./loader";
import { formatLocalFontFace, tailwindFontVarKey } from "./css-format";

export { fontFormatForFile, formatInspectorCss, formatLocalFontFace, tailwindFontVarKey } from "./css-format";

export function exportGoogleImport(fonts: FontRecord[]): string {
  return googleCssUrls(fonts)
    .map((url) => `@import url('${url}');`)
    .join("\n");
}

export function exportLinkTag(fonts: FontRecord[]): string {
  return googleCssUrls(fonts)
    .map((url) => `<link rel="stylesheet" href="${url}" />`)
    .join("\n");
}

export function exportFontFamilies(fonts: FontRecord[]): string {
  return fonts
    .map((font) => {
      const stack = cssFamilyStack(font);
      return `/* ${font.family} */\nfont-family: ${stack};`;
    })
    .join("\n\n");
}

export function exportLocalFaces(fonts: FontRecord[]): string {
  const locals = fonts.filter((f) => f.source === "local");
  if (!locals.length) return "";
  return locals.map((font) => formatLocalFontFace(font)).join("\n\n");
}

export function exportTailwind(fonts: FontRecord[]): string {
  const seen = new Set<string>();
  const entries = fonts.map((font, idx) => {
    const base = tailwindFontVarKey(font.family, idx);
    let key = base;
    let n = 2;
    while (seen.has(key)) {
      key = `${base}-${n}`;
      n += 1;
    }
    seen.add(key);
    return `  --font-${key}: ${cssFamilyStack(font)};`;
  });
  return `@theme {\n${entries.join("\n")}\n}`;
}

export function exportBundle(fonts: FontRecord[]): string {
  const parts = [
    "/* Fonths export */",
    exportGoogleImport(fonts),
    exportLocalFaces(fonts),
    exportFontFamilies(fonts),
  ].filter(Boolean);
  return parts.join("\n\n");
}
