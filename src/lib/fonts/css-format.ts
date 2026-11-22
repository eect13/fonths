import { axesForFont, formatFvar, vfStyleDescriptor } from "./axes.ts";
import type { FontRecord } from "./types.ts";

export function fontFormatForFile(fileName: string): string {
  const ext = fileName.trim().toLowerCase().split(".").pop() ?? "";
  switch (ext) {
    case "woff2":
      return "woff2";
    case "woff":
      return "woff";
    case "otf":
      return "opentype";
    case "ttf":
    case "ttc":
    default:
      return "truetype";
  }
}

export function tailwindFontVarKey(family: string, index = 0): string {
  const ascii = family
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return ascii || `font-${index + 1}`;
}

export function formatLocalFontFace(font: FontRecord): string {
  const family = font.cssFamily || font.family;
  const axes = axesForFont(font);
  const wghtAxis = axes.find((a) => a.tag === "wght");
  const wdthAxis = axes.find((a) => a.tag === "wdth");
  const fallbackWeight = font.weights[0] ?? 400;
  const minW = font.weights.length ? Math.min(...font.weights) : 100;
  const maxW = font.weights.length ? Math.max(...font.weights) : 900;
  const weightDesc = font.variable
    ? wghtAxis
      ? `${formatFvar(wghtAxis.min)} ${formatFvar(wghtAxis.max)}`
      : `${minW} ${maxW}`
    : `${fallbackWeight}`;
  const styleDesc = font.variable ? vfStyleDescriptor(font) : font.italic ? "italic" : "normal";
  const file = font.fileName || `${family}.ttf`;
  const lines = [
    "@font-face {",
    `  font-family: ${JSON.stringify(family)};`,
    `  src: url("/fonts/${encodeURIComponent(file)}") format("${fontFormatForFile(file)}");`,
    `  font-weight: ${weightDesc};`,
    `  font-style: ${styleDesc};`,
  ];
  if (font.variable && wdthAxis && wdthAxis.max > wdthAxis.min) {
    lines.push(`  font-stretch: ${formatFvar(wdthAxis.min)}% ${formatFvar(wdthAxis.max)}%;`);
  }
  lines.push("  font-display: swap;", "}");
  return lines.join("\n");
}

export interface InspectorCssInput {
  family: string;
  category: FontRecord["category"];
  weight: number | string;
  fontStyle?: string;
  fontStretch?: string;
  fontVariationSettings?: string;
  fontOpticalSizing?: string;
  fontFeatureSettings?: string;
}

export function formatInspectorCss(input: InspectorCssInput): string {
  const fallback =
    input.category === "serif"
      ? "serif"
      : input.category === "mono"
        ? "monospace"
        : input.category === "handwriting"
          ? "cursive"
          : "sans-serif";
  const lines = [
    `font-family: ${JSON.stringify(input.family)}, ${fallback};`,
    `font-weight: ${input.weight};`,
  ];
  if (input.fontStyle && input.fontStyle !== "normal") {
    lines.push(`font-style: ${input.fontStyle};`);
  }
  if (input.fontStretch) {
    lines.push(`font-stretch: ${input.fontStretch};`);
  }
  if (input.fontVariationSettings && input.fontVariationSettings !== "normal") {
    lines.push(`font-variation-settings: ${input.fontVariationSettings};`);
  }
  if (input.fontOpticalSizing === "none") {
    lines.push("font-optical-sizing: none;");
  }
  lines.push(`font-feature-settings: ${input.fontFeatureSettings ?? "normal"};`);
  return lines.join("\n");
}
