/** SuperSearch metrics from OS/2 + head — never glyf raster (20k-safe). */

import { previewWghtAxis } from "./axes.ts";
import { isWoff2OnlyVariableFamily } from "./heal-catalog.ts";
import { fontLicense, licenseSearchHay } from "./license.ts";
import { scriptKindOf } from "./scripts.ts";
import type { FontMetrics, FontRecord } from "./types.ts";

export type { FontMetrics };

export type SearchClause =
  | { kind: "hay"; value: string; negated?: boolean }
  | {
      kind: "range";
      field: "xh" | "contrast" | "weight" | "width";
      min: number;
      max: number;
      minExclusive?: boolean;
      maxExclusive?: boolean;
      negated?: boolean;
    }
  | {
      kind: "axis";
      tag: string;
      min: number;
      max: number;
      minExclusive?: boolean;
      maxExclusive?: boolean;
      negated?: boolean;
    }
  | { kind: "flag"; flag: "variable" | "italic"; negated?: boolean }
  | { kind: "tag"; value: string; values?: string[]; negated?: boolean }
  | { kind: "license"; value: string; values?: string[]; negated?: boolean }
  | { kind: "name"; value: string; negated?: boolean }
  | { kind: "category"; values: string[]; negated?: boolean }
  | { kind: "source"; values: string[]; negated?: boolean }
  | { kind: "script"; values: string[]; negated?: boolean };

export const SEARCH_PRESETS: { id: string; label: string; token: string; hint: string; group?: string }[] = [
  { id: "xh-high", label: "High x-height", token: "xh:0.52-1", hint: "OS/2 sxHeight / UPM", group: "xh" },
  { id: "xh-low", label: "Low x-height", token: "xh:0-0.45", hint: "Smaller lowercase", group: "xh" },
  { id: "ctr-high", label: "High contrast", token: "contrast:high", hint: "PANOSE, not a stem raster", group: "contrast" },
  { id: "ctr-low", label: "Low contrast", token: "contrast:low", hint: "Even stroke", group: "contrast" },
  { id: "heavy", label: "Heavy", token: "weight:700-1000", hint: "Weight class or wght axis", group: "weight" },
  { id: "light", label: "Light", token: "weight:100-350", hint: "Thin–light", group: "weight" },
  { id: "condensed", label: "Condensed", token: "width:condensed", hint: "Width class 1–4 or wdth", group: "width" },
  { id: "expanded", label: "Expanded", token: "width:expanded", hint: "Width class 6–9", group: "width" },
  { id: "opsz-text", label: "Text opsz", token: "opsz:6-18", hint: "Caption–text optical size", group: "opsz" },
  { id: "opsz-display", label: "Display opsz", token: "opsz:36-144", hint: "Display optical size", group: "opsz" },
  { id: "variable", label: "Variable", token: "variable", hint: "Catalog VF or on-disk fvar" },
  { id: "italic", label: "Italic", token: "italic", hint: "Italic faces" },
];

const WDTH_STOPS = [50, 62.5, 75, 87.5, 100, 112.5, 125, 150, 200];

function clamp01(n: number) {
  if (!Number.isFinite(n)) return 0;
  return Math.min(1, Math.max(0, n));
}

function panoseContrast(panose?: number[]): number | undefined {
  if (!panose?.length) return undefined;
  const family = (panose[0] ?? 0) & 0xff;
  const contrast = (panose[4] ?? 0) & 0xff;
  if (family !== 2) return undefined;
  if (contrast <= 1) return undefined;
  return clamp01((contrast - 2) / 7);
}

/** Map fvar `wdth` percent onto OS/2 usWidthClass 1–9. */
export function wdthToClass(n: number): number {
  if (!Number.isFinite(n)) return 5;
  let best = 5;
  let dist = Infinity;
  for (let i = 0; i < WDTH_STOPS.length; i += 1) {
    const d = Math.abs(n - WDTH_STOPS[i]!);
    if (d < dist) {
      dist = d;
      best = i + 1;
    }
  }
  return best;
}

export function metricsFromTables(input: {
  upem?: number;
  weightClass?: number;
  widthClass?: number;
  xHeight?: number;
  capHeight?: number;
  panose?: number[];
  sFamilyClass?: number;
  isFixedPitch?: boolean;
}): FontMetrics {
  const upem = input.upem && input.upem > 0 ? input.upem : 1000;
  const weightClass =
    input.weightClass && input.weightClass >= 1 && input.weightClass <= 1000 ? Math.round(input.weightClass) : 400;
  const widthClass =
    input.widthClass && input.widthClass >= 1 && input.widthClass <= 9 ? Math.round(input.widthClass) : 5;
  const xHeight = input.xHeight && input.xHeight > 0 ? input.xHeight : undefined;
  const capHeight = input.capHeight && input.capHeight > 0 ? input.capHeight : undefined;
  const xh = xHeight ? clamp01(xHeight / upem) : undefined;
  const panose =
    input.panose && input.panose.length >= 10
      ? Array.from(input.panose.slice(0, 10), (n) => Number(n) & 0xff)
      : undefined;
  const contrast = panoseContrast(panose ?? input.panose);
  const sFamilyClass =
    typeof input.sFamilyClass === "number" && Number.isFinite(input.sFamilyClass) && input.sFamilyClass > 0
      ? input.sFamilyClass
      : undefined;
  const isFixedPitch = typeof input.isFixedPitch === "boolean" ? input.isFixedPitch : undefined;
  return {
    upem,
    weightClass,
    widthClass,
    xHeight,
    capHeight,
    contrast,
    xh,
    ...(panose ? { panose } : {}),
    ...(sFamilyClass !== undefined ? { sFamilyClass } : {}),
    ...(isFixedPitch !== undefined ? { isFixedPitch } : {}),
  };
}

export function mergeMetrics(prev: FontMetrics | undefined, next: FontMetrics): FontMetrics {
  return {
    upem: next.upem || prev?.upem || 1000,
    weightClass: next.weightClass || prev?.weightClass || 400,
    widthClass: next.widthClass || prev?.widthClass || 5,
    xHeight: next.xHeight ?? prev?.xHeight,
    capHeight: next.capHeight ?? prev?.capHeight,
    contrast: next.contrast ?? prev?.contrast,
    xh: next.xh ?? prev?.xh,
    panose: next.panose ?? prev?.panose,
    sFamilyClass: next.sFamilyClass ?? prev?.sFamilyClass,
    isFixedPitch: next.isFixedPitch ?? prev?.isFixedPitch,
  };
}

function isVariableFamily(font: FontRecord) {
  if (isWoff2OnlyVariableFamily(font.family)) return false;
  return Boolean(font.variable || font.catalogVariable);
}

function tagsOf(font: FontRecord, customTags: Record<string, string[]>): string[] {
  const extra = customTags[font.id] ?? [];
  return Array.from(new Set([...(font.tags ?? []), ...extra]));
}

function hayFor(font: FontRecord, tags: string[]): string {
  return [
    font.family,
    font.fullName ?? "",
    font.source,
    font.category,
    ...tags,
    font.variable || font.catalogVariable ? "variable" : "",
    font.italic ? "italic" : "",
    font.fileName ?? "",
    licenseSearchHay(font),
    ...(font.axes ?? []).map((a) => a.tag),
  ]
    .join(" ")
    .toLowerCase();
}

export function metricsFor(font: FontRecord): FontMetrics {
  if (font.metrics) return font.metrics;
  const tags = font.tags ?? [];
  let widthClass = 5;
  if (tags.includes("condensed")) widthClass = 3;
  else if (tags.includes("expanded") || tags.includes("wide")) widthClass = 7;
  const ws = font.weights?.filter((n) => Number.isFinite(n) && n > 0) ?? [];
  const weightClass = ws.includes(400) ? 400 : ws[0] ?? 400;
  return { upem: 1000, weightClass, widthClass };
}

const WEIGHT_NAME: Record<string, [number, number]> = {
  thin: [100, 200],
  hairline: [100, 200],
  extralight: [200, 300],
  ultralight: [200, 300],
  light: [200, 350],
  regular: [350, 500],
  book: [350, 500],
  medium: [500, 600],
  semibold: [600, 750],
  demibold: [600, 750],
  bold: [600, 850],
  extrabold: [800, 1000],
  ultrabold: [800, 1000],
  black: [800, 1000],
  heavy: [800, 1000],
};

const WIDTH_NAME: Record<string, [number, number]> = {
  condensed: [1, 4],
  compact: [1, 4],
  narrow: [1, 4],
  normal: [5, 5],
  regular: [5, 5],
  expanded: [6, 9],
  extended: [6, 9],
  wide: [6, 9],
};

const CONTRAST_NAME: Record<string, [number, number]> = {
  none: [0, 0.2],
  low: [0, 0.4],
  medium: [0.35, 0.7],
  high: [0.65, 1],
};

const AXIS_TAG = /^(wght|wdth|opsz|slnt|ital|grad|soft|casl|wonk|crsv|mono|fill|xopq|yopq|xtra|ytuc|ytlc|ytas|ytde)$/i;

interface ParsedBounds {
  min: number;
  max: number;
  minExclusive?: boolean;
  maxExclusive?: boolean;
}

export function tokenizeSearchQuery(query: string): string[] {
  const tokens: string[] = [];
  let cur = "";
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < query.length; i += 1) {
    const ch = query[i]!;
    if (quote) {
      cur += ch;
      if (ch === quote && query[i - 1] !== "\\") quote = null;
      continue;
    }
    if ((ch === '"' || ch === "'") && query[i - 1] !== "\\") {
      quote = ch;
      cur += ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (cur) {
        tokens.push(cur);
        cur = "";
      }
      continue;
    }
    cur += ch;
  }
  if (cur) tokens.push(cur);
  return tokens;
}

function stripQuotes(raw: string): string {
  if (
    raw.length >= 2 &&
    ((raw.startsWith('"') && raw.endsWith('"')) || (raw.startsWith("'") && raw.endsWith("'")))
  ) {
    return raw.slice(1, -1);
  }
  return raw.replace(/^["']|["']$/g, "");
}

function splitSearchValues(raw: string): string[] {
  return raw
    .split(",")
    .map((part) => stripQuotes(part.trim()).trim())
    .filter(Boolean);
}

function normalizeCategoryToken(v: string): string {
  switch (v) {
    case "sans":
    case "sansserif":
    case "sans-serif":
      return "sans";
    case "serif":
    case "slab":
      return "serif";
    case "display":
    case "decorative":
      return "display";
    case "hand":
    case "handwriting":
    case "script":
    case "calligraphy":
      return "handwriting";
    case "mono":
    case "monospace":
    case "code":
    case "fixed":
      return "mono";
    case "icon":
    case "icons":
      return "icons";
    default:
      return v;
  }
}

function normalizeSourceToken(v: string): string {
  switch (v) {
    case "google":
    case "catalog":
    case "web":
      return "google";
    case "local":
    case "upload":
    case "uploads":
    case "file":
    case "files":
    case "watched":
      return "local";
    case "system":
    case "windows":
    case "os":
      return "system";
    default:
      return v;
  }
}

function normalizeScriptTokens(v: string): string[] {
  switch (v) {
    case "arab":
    case "arabic":
      return ["arabic"];
    case "heb":
    case "hebrew":
      return ["hebrew"];
    case "ja":
    case "jp":
    case "japanese":
      return ["jp"];
    case "ko":
    case "kr":
    case "korean":
      return ["kr"];
    case "sc":
    case "zh-cn":
    case "simplified":
    case "chinese-simplified":
      return ["sc"];
    case "tc":
    case "zh-tw":
    case "traditional":
    case "chinese-traditional":
      return ["tc"];
    case "hk":
    case "zh-hk":
    case "chinese-hongkong":
      return ["hk"];
    case "zh":
    case "chinese":
      return ["sc", "tc", "hk"];
    case "cjk":
      return ["jp", "kr", "sc", "tc", "hk"];
    case "cyr":
    case "cyrillic":
    case "ru":
      return ["cyrillic"];
    case "el":
    case "greek":
      return ["greek"];
    default:
      return [v];
  }
}

function parseBoundPair(
  raw: string,
  names: Record<string, [number, number]> | undefined,
  scaleIfUnit: boolean,
): ParsedBounds | null {
  if (names) {
    const named = names[raw];
    if (named) return { min: named[0], max: named[1] };
  }
  const cmp = raw.match(/^(>=|<=|>|<)(-?\d+(?:\.\d+)?)$/);
  if (cmp) {
    let n = Number(cmp[2]);
    if (!Number.isFinite(n)) return null;
    if (scaleIfUnit && n > 0 && n <= 1) n *= 1000;
    if (cmp[1] === ">=") return { min: n, max: Infinity };
    if (cmp[1] === ">") return { min: n, max: Infinity, minExclusive: true };
    if (cmp[1] === "<=") return { min: -Infinity, max: n };
    return { min: -Infinity, max: n, maxExclusive: true };
  }
  const plus = raw.match(/^(-?\d+(?:\.\d+)?)\+$/);
  if (plus) {
    let n = Number(plus[1]);
    if (!Number.isFinite(n)) return null;
    if (scaleIfUnit && n > 0 && n <= 1) n *= 1000;
    return { min: n, max: Infinity };
  }
  const dotRange = raw.match(/^(-?\d+(?:\.\d+)?)\.\.(-?\d+(?:\.\d+)?)$/);
  const dashRange = !dotRange ? raw.match(/^(-?\d+(?:\.\d+)?)(?:-(-?\d+(?:\.\d+)?))?$/) : null;
  const m = dotRange ?? dashRange;
  if (!m) return null;
  const a = Number(m[1]);
  const b = m[2] != null ? Number(m[2]) : a;
  if (!Number.isFinite(a) || !Number.isFinite(b)) return null;
  let min = Math.min(a, b);
  let max = Math.max(a, b);
  if (scaleIfUnit && min >= 0 && max > 0 && max <= 1) {
    min *= 1000;
    max *= 1000;
  }
  return { min, max };
}

export function parseSearchQuery(query: string): SearchClause[] {
  const q = query.trim();
  if (!q) return [];
  const tokens = tokenizeSearchQuery(q);
  const out: SearchClause[] = [];
  for (const token of tokens) {
    const rawLower = token.toLowerCase();
    const negated = (rawLower.startsWith("-") || rawLower.startsWith("!")) && rawLower.length > 1 && !/^-\d/.test(rawLower);
    const lower = negated ? rawLower.slice(1) : rawLower;
    const negProp = negated ? { negated: true as const } : {};
    const colon = lower.indexOf(":");
    const quotedWhole = lower.startsWith('"') || lower.startsWith("'");
    if (colon <= 0 || quotedWhole) {
      const clean = stripQuotes(lower);
      if (!clean) continue;
      if (clean === "variable") out.push({ kind: "flag", flag: "variable", ...negProp });
      else if (clean === "italic") out.push({ kind: "flag", flag: "italic", ...negProp });
      else out.push({ kind: "hay", value: clean, ...negProp });
      continue;
    }
    const key = lower.slice(0, colon);
    const rawVal = lower.slice(colon + 1);
    const val = stripQuotes(rawVal);
    if (!val) {
      out.push({ kind: "hay", value: stripQuotes(lower), ...negProp });
      continue;
    }
    if (key === "tag") {
      const values = splitSearchValues(rawVal);
      out.push({ kind: "tag", value: values[0] ?? val, values, ...negProp });
      continue;
    }
    if (key === "license") {
      const values = splitSearchValues(rawVal);
      out.push({ kind: "license", value: values[0] ?? val, values, ...negProp });
      continue;
    }
    if (key === "name") {
      out.push({ kind: "name", value: val, ...negProp });
      continue;
    }
    if (key === "category" || key === "cat" || key === "style") {
      const values = splitSearchValues(rawVal).map(normalizeCategoryToken);
      out.push({ kind: "category", values, ...negProp });
      continue;
    }
    if (key === "source" || key === "from") {
      const values = splitSearchValues(rawVal).map(normalizeSourceToken);
      out.push({ kind: "source", values, ...negProp });
      continue;
    }
    if (key === "script" || key === "lang") {
      const values = splitSearchValues(rawVal).flatMap(normalizeScriptTokens);
      out.push({ kind: "script", values, ...negProp });
      continue;
    }
    if (key === "variable") {
      if (val === "1" || val === "true" || val === "yes") {
        out.push({ kind: "flag", flag: "variable", ...negProp });
        continue;
      }
      if (val === "0" || val === "false" || val === "no") {
        out.push({ kind: "flag", flag: "variable", ...(!negated ? { negated: true } : {}) });
        continue;
      }
    }
    if (key === "italic") {
      if (val === "1" || val === "true" || val === "yes") {
        out.push({ kind: "flag", flag: "italic", ...negProp });
        continue;
      }
      if (val === "0" || val === "false" || val === "no") {
        out.push({ kind: "flag", flag: "italic", ...(!negated ? { negated: true } : {}) });
        continue;
      }
    }
    if (key === "has") {
      if (val === "variable" || val === "var" || val === "vf") {
        out.push({ kind: "flag", flag: "variable", ...negProp });
        continue;
      }
      if (val === "italic" || val === "ital") {
        out.push({ kind: "flag", flag: "italic", ...negProp });
        continue;
      }
      const tag = val === "slant" ? "slnt" : val;
      if (AXIS_TAG.test(tag) || /^[a-z]{4}$/.test(tag)) {
        out.push({ kind: "axis", tag, min: -Infinity, max: Infinity, ...negProp });
        continue;
      }
    }
    if (key === "xh" || key === "xheight" || key === "x-height") {
      const pair = parseBoundPair(val, undefined, false);
      out.push(pair ? { kind: "range", field: "xh", ...pair, ...negProp } : { kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (key === "contrast" || key === "ctr") {
      const pair = parseBoundPair(val, CONTRAST_NAME, false);
      out.push(pair ? { kind: "range", field: "contrast", ...pair, ...negProp } : { kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (key === "width") {
      const pair = parseBoundPair(val, WIDTH_NAME, false);
      out.push(pair ? { kind: "range", field: "width", ...pair, ...negProp } : { kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (key === "weight") {
      const pair = parseBoundPair(val, WEIGHT_NAME, true);
      out.push(pair ? { kind: "range", field: "weight", ...pair, ...negProp } : { kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (key === "slant") {
      if (val === "any" || val === "yes" || val === "true" || val === "1") {
        out.push({ kind: "axis", tag: "slnt", min: -Infinity, max: Infinity, ...negProp });
        continue;
      }
      const pair = parseBoundPair(val, undefined, false);
      out.push(pair ? { kind: "axis", tag: "slnt", ...pair, ...negProp } : { kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (key === "axis") {
      const eq = val.indexOf("=");
      const tag = (eq > 0 ? val.slice(0, eq) : val).toLowerCase();
      if (!/^[a-z]{4}$/.test(tag)) {
        out.push({ kind: "hay", value: lower, ...negProp });
        continue;
      }
      if (eq <= 0) {
        out.push({ kind: "axis", tag, min: -Infinity, max: Infinity, ...negProp });
        continue;
      }
      const rest = val.slice(eq + 1);
      const pair = parseBoundPair(rest, tag === "wght" ? WEIGHT_NAME : undefined, tag === "wght");
      if (pair) out.push({ kind: "axis", tag, ...pair, ...negProp });
      else out.push({ kind: "hay", value: lower, ...negProp });
      continue;
    }
    if (AXIS_TAG.test(key) || (key.length === 4 && /^[a-z]{4}$/.test(key))) {
      const pair = parseBoundPair(val, key === "wght" ? WEIGHT_NAME : undefined, key === "wght");
      if (pair) out.push({ kind: "axis", tag: key, ...pair, ...negProp });
      else out.push({ kind: "hay", value: lower, ...negProp });
      continue;
    }
    out.push({ kind: "hay", value: lower, ...negProp });
  }
  return out;
}

function overlaps(
  aMin: number,
  aMax: number,
  bMin: number,
  bMax: number,
  minExclusive = false,
  maxExclusive = false,
) {
  const loOk = minExclusive ? aMax > bMin : aMax + 1e-4 >= bMin;
  const hiOk = maxExclusive ? bMax > aMin : bMax + 1e-4 >= aMin;
  return loOk && hiOk;
}

function weightSpan(font: FontRecord, metrics: FontMetrics): [number, number] {
  const wght = previewWghtAxis(font);
  if (wght) return [wght.min, wght.max];
  const ws = font.weights?.length ? font.weights : [metrics.weightClass];
  return [Math.min(...ws), Math.max(...ws)];
}

function widthSpan(font: FontRecord, metrics: FontMetrics): [number, number] {
  const wdth = (font.axes ?? []).find((a) => a.tag === "wdth");
  if (wdth && wdth.max > wdth.min) {
    return [wdthToClass(wdth.min), wdthToClass(wdth.max)].sort((a, b) => a - b) as [number, number];
  }
  return [metrics.widthClass, metrics.widthClass];
}

export function fontMatchesSearch(
  font: FontRecord,
  clauses: SearchClause[],
  customTags: Record<string, string[]>,
): boolean {
  if (!clauses.length) return true;
  const metrics = metricsFor(font);
  const tags = tagsOf(font, customTags);
  const needsHay = clauses.some((c) => c.kind === "hay");
  return matchClauses(font, clauses, metrics, tags, needsHay ? hayFor(font, tags) : "");
}

/** Fast path for pre-indexed searches: reuses cached metrics, tags, and lowercase haystack. */
export function fontMatchesSearchWithCache(
  font: FontRecord,
  clauses: SearchClause[],
  metrics: FontMetrics,
  tags: string[],
  hay: string,
): boolean {
  if (!clauses.length) return true;
  return matchClauses(font, clauses, metrics, tags, hay);
}

function matchClause(
  font: FontRecord,
  clause: SearchClause,
  metrics: FontMetrics,
  tags: string[],
  hay: string,
): boolean {
  switch (clause.kind) {
    case "hay":
      return hay.includes(clause.value);
    case "name":
      return (
        font.family.toLowerCase().includes(clause.value) ||
        (font.fullName ?? "").toLowerCase().includes(clause.value)
      );
    case "tag": {
      const vals = clause.values?.length ? clause.values : [clause.value];
      return vals.some((v) => tags.some((t) => t.toLowerCase() === v));
    }
    case "license": {
      const vals = clause.values?.length ? clause.values : [clause.value];
      return vals.includes(fontLicense(font));
    }
    case "category":
      return clause.values.includes(font.category.toLowerCase());
    case "source":
      return clause.values.includes(font.source.toLowerCase());
    case "script": {
      const kind = scriptKindOf(font).toLowerCase();
      return clause.values.some((v) => v === kind || (v === "latin" && font.coversLatin));
    }
    case "flag":
      return clause.flag === "variable" ? isVariableFamily(font) : Boolean(font.italic);
    case "range": {
      if (clause.field === "xh") {
        if (metrics.xh == null) return false;
        const lo = clause.minExclusive ? metrics.xh > clause.min : metrics.xh + 1e-4 >= clause.min;
        const hi = clause.maxExclusive ? metrics.xh < clause.max : metrics.xh - 1e-4 <= clause.max;
        return lo && hi;
      }
      if (clause.field === "contrast") {
        if (metrics.contrast == null) return false;
        const lo = clause.minExclusive ? metrics.contrast > clause.min : metrics.contrast + 1e-4 >= clause.min;
        const hi = clause.maxExclusive ? metrics.contrast < clause.max : metrics.contrast - 1e-4 <= clause.max;
        return lo && hi;
      }
      if (clause.field === "weight") {
        const [min, max] = weightSpan(font, metrics);
        return overlaps(min, max, clause.min, clause.max, clause.minExclusive, clause.maxExclusive);
      }
      const [min, max] = widthSpan(font, metrics);
      return overlaps(min, max, clause.min, clause.max, clause.minExclusive, clause.maxExclusive);
    }
    case "axis": {
      const real = (font.axes ?? []).find((a) => a.tag.toLowerCase() === clause.tag);
      if (real) {
        return overlaps(real.min, real.max, clause.min, clause.max, clause.minExclusive, clause.maxExclusive);
      }
      if (clause.tag === "wght") {
        const inferred = previewWghtAxis(font);
        if (inferred) {
          return overlaps(inferred.min, inferred.max, clause.min, clause.max, clause.minExclusive, clause.maxExclusive);
        }
        const [min, max] = weightSpan(font, metrics);
        return overlaps(min, max, clause.min, clause.max, clause.minExclusive, clause.maxExclusive);
      }
      return false;
    }
    default:
      return true;
  }
}

function matchClauses(
  font: FontRecord,
  clauses: SearchClause[],
  metrics: FontMetrics,
  tags: string[],
  hay: string,
): boolean {
  return clauses.every((clause) => {
    const ok = matchClause(font, clause, metrics, tags, hay);
    return clause.negated ? !ok : ok;
  });
}

export function contrastLabel(contrast?: number): string | null {
  if (contrast == null) return null;
  if (contrast >= 0.65) return "high";
  if (contrast >= 0.35) return "medium";
  return "low";
}

export function formatXh(xh?: number): string | null {
  if (xh == null) return null;
  return xh.toFixed(2);
}

export function widthClassLabel(n?: number): string | null {
  if (!n || n < 1 || n > 9) return null;
  return (
    [
      "Ultra condensed",
      "Extra condensed",
      "Condensed",
      "Semi condensed",
      "Normal",
      "Semi expanded",
      "Expanded",
      "Extra expanded",
      "Ultra expanded",
    ][n - 1] ?? null
  );
}

export function toggleSearchToken(query: string, token: string): string {
  const parts = tokenizeSearchQuery(query.trim());
  const hit = parts.findIndex((p) => p.toLowerCase() === token.toLowerCase());
  if (hit >= 0) parts.splice(hit, 1);
  else parts.push(token);
  return parts.join(" ");
}

/** Toggle a SuperSearch preset; sibling chips in the same group replace each other. */
export function toggleSearchPreset(query: string, token: string): string {
  const preset = SEARCH_PRESETS.find((p) => p.token.toLowerCase() === token.toLowerCase());
  const siblings = SEARCH_PRESETS.filter((p) => p.group && p.group === preset?.group).map((p) => p.token.toLowerCase());
  const parts = tokenizeSearchQuery(query.trim());
  const key = token.toLowerCase();
  const had = parts.some((p) => p.toLowerCase() === key);
  const next = parts.filter((p) => p.toLowerCase() !== key && !siblings.includes(p.toLowerCase()));
  if (!had) next.push(token);
  return next.join(" ");
}

export function queryHasToken(query: string, token: string): boolean {
  return tokenizeSearchQuery(query.trim()).some((p) => p.toLowerCase() === token.toLowerCase());
}

export function queryTokenCount(query: string): number {
  return parseSearchQuery(query).length;
}

/** Counts per SuperSearch preset in this drawer (no hay). Hide 0-count chips.
 *  Metrics + tags once per font — 12× fontMatchesSearch would hitch 20k. */
export function countSearchPresets(
  fonts: FontRecord[],
  customTags: Record<string, string[]>,
): Record<string, number> {
  const parsed = SEARCH_PRESETS.map((p) => {
    const clauses = parseSearchQuery(p.token);
    return { id: p.id, clauses, needsHay: clauses.some((c) => c.kind === "hay") };
  });
  const counts: Record<string, number> = {};
  for (const p of parsed) counts[p.id] = 0;
  const anyHay = parsed.some((p) => p.needsHay);
  for (const font of fonts) {
    const metrics = metricsFor(font);
    const tags = tagsOf(font, customTags);
    const hay = anyHay ? hayFor(font, tags) : "";
    for (const p of parsed) {
      if (matchClauses(font, p.clauses, metrics, tags, p.needsHay ? hay : "")) {
        counts[p.id] += 1;
      }
    }
  }
  return counts;
}
