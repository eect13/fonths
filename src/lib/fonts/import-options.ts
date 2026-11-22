/**
 * Smart local font import analysis & filtering:
 * - Detects Variable vs. Static overlaps (both inside a dropped batch AND against existing local fonts).
 * - Classifies files by kind (variable, static upright, italic) and format (ttf, otf, ttc, woff2, woff).
 * - Provides smart defaults (e.g. Variable preferred over duplicate static files; web-only WOFF/WOFF2
 *   unchecked when desktop TTF/OTF files are present) while staying zero-friction for small clean drops.
 */

export type FontFileFormat = "ttf" | "otf" | "ttc" | "woff2" | "woff";
export type VarStaticMode = "variable" | "static" | "both";
export type ImportSourceMode = "upload" | "watch";

export interface SavedImportRule {
  varStaticMode: VarStaticMode;
  includeVariable: boolean;
  includeStatic: boolean;
  includeItalic: boolean;
  formats: Record<FontFileFormat, boolean>;
  autoActivate: boolean;
}

const SAVED_RULE_KEY = "fonths:import-smart-prefs-v1";

export interface MinimalFileLike {
  name: string;
  size?: number;
  webkitRelativePath?: string;
}

export interface MinimalLocalFont {
  id: string;
  family: string;
  variable?: boolean;
  italic?: boolean;
  fileName?: string;
}

export interface ClassifiedFontFile {
  index: number;
  name: string;
  size: number;
  relativePath: string;
  ext: FontFileFormat;
  familyKey: string;
  familyLabel: string;
  isVariable: boolean;
  isItalic: boolean;
  isWebOnly: boolean;
}

export interface VarStaticConflict {
  familyKey: string;
  familyLabel: string;
  batchVariableCount: number;
  batchStaticCount: number;
  existingVariableIds: string[];
  existingStaticIds: string[];
}

export interface ImportBatchAnalysis {
  totalFiles: number;
  totalBytes: number;
  files: ClassifiedFontFile[];
  familyCount: number;
  varStaticConflicts: VarStaticConflict[];
  variableCount: number;
  staticCount: number;
  staticUprightCount: number;
  italicCount: number;
  formats: Record<FontFileFormat, number>;
  presentFormats: FontFileFormat[];
  hasDesktop: boolean;
  hasWebOnly: boolean;
}

export interface ImportOptionsSelection {
  varStaticMode: VarStaticMode;
  replaceExistingOpposite: boolean;
  includeVariable: boolean;
  includeStatic: boolean;
  includeItalic: boolean;
  formats: Record<FontFileFormat, boolean>;
  autoActivate: boolean;
  saveAsDefault?: boolean;
}

export interface ImportOptionsRequest {
  analysis: ImportBatchAnalysis;
  mode: ImportSourceMode;
  folderLabel?: string;
}

const KNOWN_FORMATS = new Set<string>(["ttf", "otf", "ttc", "otc", "woff2", "woff"]);

const STYLE_TOKENS = new Set([
  "thin",
  "hairline",
  "extralight",
  "ultralight",
  "light",
  "regular",
  "normal",
  "book",
  "roman",
  "medium",
  "semibold",
  "demibold",
  "demi",
  "bold",
  "extrabold",
  "ultrabold",
  "black",
  "heavy",
  "extrablack",
  "ultrablack",
  "italic",
  "ital",
  "it",
  "oblique",
  "obl",
  "slanted",
  "variable",
  "variablefont",
  "varfont",
  "var",
  "vf",
  "static",
  "wght",
  "opsz",
  "wdth",
  "slnt",
  "grad",
  "soft",
  "wonk",
  "crsv",
  "casl",
  "mono",
]);

export function normalizeImportFamilyKey(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function splitCamel(str: string): string {
  return str
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .trim();
}

function extractFormat(fileName: string): FontFileFormat {
  const m = /\.([a-z0-9]+)$/i.exec(fileName.trim());
  const raw = (m?.[1] ?? "ttf").toLowerCase();
  if (raw === "otc") return "ttc";
  if (KNOWN_FORMATS.has(raw)) return raw as FontFileFormat;
  return "ttf";
}

function stripTrailingStyleTokens(stem: string): string {
  // Remove bracketed axis declarations like [wght] or [opsz,wght].
  let clean = stem.replace(/\[[^\]]+\]/g, " ");
  // Remove VariableFont_... suffixes (e.g. Inter-VariableFont_opsz,wght).
  clean = clean.replace(/[-_\s]*variablefont(?:[-_,\s]+[a-z0-9]+)*/gi, " ");
  // Remove optical size suffixes like _18pt, _24pt, -18pt.
  clean = clean.replace(/[-_\s]+\d+pt\b/gi, " ");
  // Split into tokens on separators.
  const tokens = clean
    .replace(/[-_]+/g, " ")
    .trim()
    .split(/\s+/)
    .filter(Boolean);

  // Strip trailing compound weight/style tokens (e.g. BoldItalic, ExtraLight, SemiBold, VF).
  while (tokens.length > 1) {
    const last = tokens[tokens.length - 1]!.toLowerCase();
    const withoutItalic = last.replace(/(italic|oblique|ital)$/i, "");
    if (
      STYLE_TOKENS.has(last) ||
      (withoutItalic && withoutItalic !== last && STYLE_TOKENS.has(withoutItalic))
    ) {
      tokens.pop();
    } else {
      break;
    }
  }
  return tokens.join(" ").trim() || stem;
}

export function classifyFontFile(file: MinimalFileLike, index: number): ClassifiedFontFile {
  const name = file.name || `font-${index}.ttf`;
  const relativePath = (file.webkitRelativePath || "").replace(/\\/g, "/");
  const ext = extractFormat(name);
  const stem = name.replace(/\.[^.]+$/, "");
  const lowerStem = stem.toLowerCase();
  const lowerPath = relativePath.toLowerCase();

  const inStaticDir = /(?:^|\/)static\//.test(lowerPath);
  const hasVarMarker =
    /\[[^\]]*(?:wght|opsz|wdth|slnt|ital|grad|soft|wonk|casl|crsv)[^\]]*\]/i.test(stem) ||
    /variablefont/i.test(stem) ||
    /(?:^|[-_\s])(variable|varfont|vf)(?:$|[-_\s,])/i.test(stem) ||
    /(?:^|[-_])(?:opsz[-_,])?wght(?:$|[-_,])/i.test(stem);

  const isVariable = hasVarMarker && !inStaticDir;
  const isItalic =
    /(?:italic|oblique)/i.test(lowerStem) ||
    /(?:^|[-_])(?:it|ital)(?:$|[-_])/i.test(lowerStem);
  const isWebOnly = ext === "woff" || ext === "woff2";

  const rawLabel = stripTrailingStyleTokens(stem);
  const familyLabel = splitCamel(rawLabel) || rawLabel || stem;
  const familyKey = normalizeImportFamilyKey(familyLabel) || normalizeImportFamilyKey(stem);

  return {
    index,
    name,
    size: typeof file.size === "number" ? file.size : 0,
    relativePath,
    ext,
    familyKey,
    familyLabel,
    isVariable,
    isItalic,
    isWebOnly,
  };
}

export function analyzeImportBatch(
  files: MinimalFileLike[],
  existingLocalFonts: MinimalLocalFont[] = [],
): ImportBatchAnalysis {
  const classified = files.map((f, i) => classifyFontFile(f, i));
  const formats: Record<FontFileFormat, number> = {
    ttf: 0,
    otf: 0,
    ttc: 0,
    woff2: 0,
    woff: 0,
  };

  let totalBytes = 0;
  let variableCount = 0;
  let staticCount = 0;
  let staticUprightCount = 0;
  let italicCount = 0;

  const byFamily = new Map<
    string,
    { label: string; varCount: number; staticCount: number }
  >();

  for (const item of classified) {
    totalBytes += item.size;
    formats[item.ext] += 1;
    if (item.isVariable) variableCount += 1;
    else {
      staticCount += 1;
      if (!item.isItalic) staticUprightCount += 1;
    }
    if (item.isItalic) italicCount += 1;

    const slot = byFamily.get(item.familyKey) ?? {
      label: item.familyLabel,
      varCount: 0,
      staticCount: 0,
    };
    if (item.isVariable) slot.varCount += 1;
    else slot.staticCount += 1;
    byFamily.set(item.familyKey, slot);
  }

  // Group existing local fonts by normalized family key.
  const existingByFamily = new Map<
    string,
    { label: string; variableIds: string[]; staticIds: string[] }
  >();
  for (const font of existingLocalFonts) {
    const key = normalizeImportFamilyKey(font.family);
    if (!key) continue;
    const slot = existingByFamily.get(key) ?? {
      label: font.family,
      variableIds: [],
      staticIds: [],
    };
    if (font.variable) slot.variableIds.push(font.id);
    else slot.staticIds.push(font.id);
    existingByFamily.set(key, slot);
  }

  const varStaticConflicts: VarStaticConflict[] = [];
  for (const [familyKey, slot] of byFamily) {
    const existing = existingByFamily.get(familyKey);
    const existingVar = existing?.variableIds ?? [];
    const existingStatic = existing?.staticIds ?? [];

    const hasVarAnywhere = slot.varCount > 0 || existingVar.length > 0;
    const hasStaticAnywhere = slot.staticCount > 0 || existingStatic.length > 0;
    const batchHasBoth = slot.varCount > 0 && slot.staticCount > 0;
    const crossLibraryConflict =
      (slot.varCount > 0 && existingStatic.length > 0) ||
      (slot.staticCount > 0 && existingVar.length > 0);

    if (hasVarAnywhere && hasStaticAnywhere && (batchHasBoth || crossLibraryConflict)) {
      varStaticConflicts.push({
        familyKey,
        familyLabel: existing?.label || slot.label,
        batchVariableCount: slot.varCount,
        batchStaticCount: slot.staticCount,
        existingVariableIds: existingVar,
        existingStaticIds: existingStatic,
      });
    }
  }

  const presentFormats = (["ttf", "otf", "ttc", "woff2", "woff"] as FontFileFormat[]).filter(
    (fmt) => formats[fmt] > 0,
  );
  const hasDesktop = formats.ttf + formats.otf + formats.ttc > 0;
  const hasWebOnly = formats.woff + formats.woff2 > 0;

  return {
    totalFiles: classified.length,
    totalBytes,
    files: classified,
    familyCount: byFamily.size,
    varStaticConflicts,
    variableCount,
    staticCount,
    staticUprightCount,
    italicCount,
    formats,
    presentFormats,
    hasDesktop,
    hasWebOnly,
  };
}

/** Smart defaults: prefer Variable when Variable+Static overlap exists; skip WOFF/WOFF2 if desktop TTF/OTF exists. */
export function defaultImportSelection(analysis: ImportBatchAnalysis): ImportOptionsSelection {
  const hasConflicts = analysis.varStaticConflicts.length > 0;
  const anyBatchHasVariable = analysis.varStaticConflicts.some((c) => c.batchVariableCount > 0);
  const defaultVarStaticMode: VarStaticMode = !hasConflicts
    ? "both"
    : anyBatchHasVariable
      ? "variable"
      : "static";
  const hasCrossLibrary = analysis.varStaticConflicts.some(
    (c) => c.existingVariableIds.length > 0 || c.existingStaticIds.length > 0,
  );
  return {
    varStaticMode: defaultVarStaticMode,
    replaceExistingOpposite: hasCrossLibrary,
    includeVariable: true,
    includeStatic: true,
    includeItalic: true,
    formats: {
      ttf: true,
      otf: true,
      ttc: true,
      woff2: !analysis.hasDesktop,
      woff: !analysis.hasDesktop,
    },
    autoActivate: true,
  };
}

/**
 * Only prompt when smart options add real value:
 * - Variable + Static overlap (inside batch or with existing local library), OR
 * - Mixed desktop + web-only formats (e.g. TTF + WOFF2), OR
 * - Multiple distinct file formats (e.g. TTF + OTF), OR
 * - Multi-file bundle (>= 6 files).
 * Keeps small clean drops (1-5 files, 1 format, no Var/Static overlap) 1-click instant.
 */
export function shouldPromptImportOptions(analysis: ImportBatchAnalysis): boolean {
  if (analysis.totalFiles === 0) return false;
  if (analysis.varStaticConflicts.length > 0) return true;
  if (analysis.hasDesktop && analysis.hasWebOnly) return true;
  if (analysis.presentFormats.length >= 2) return true;
  if (analysis.totalFiles >= 6) return true;
  return false;
}

export function filterImportFiles<T extends MinimalFileLike>(
  files: T[],
  analysis: ImportBatchAnalysis,
  selection: ImportOptionsSelection,
): { files: T[]; replaceExistingLocalIds: string[]; skippedCount: number } {
  const conflictMap = new Map(
    analysis.varStaticConflicts.map((c) => [c.familyKey, c]),
  );

  const selected: T[] = [];
  for (const item of analysis.files) {
    // 1. Format checkbox filter.
    if (!selection.formats[item.ext]) continue;

    // 2. Kind / style checkbox filter.
    if (item.isVariable && !selection.includeVariable) continue;
    if (!item.isVariable && !selection.includeStatic) continue;
    if (item.isItalic && !selection.includeItalic) continue;

    // 3. Variable vs. Static conflict rule for overlapping families.
    const conflict = conflictMap.get(item.familyKey);
    if (conflict) {
      if (selection.varStaticMode === "variable" && !item.isVariable) continue;
      if (selection.varStaticMode === "static" && item.isVariable) continue;
    }

    const original = files[item.index];
    if (original) selected.push(original);
  }

  // Determine existing opposite-kind local font IDs to replace when requested.
  const replaceIds = new Set<string>();
  if (selection.replaceExistingOpposite && selection.varStaticMode !== "both") {
    const selectedFamilies = new Set(
      selected.map((f, i) => {
        const idx = files.indexOf(f);
        return analysis.files[idx >= 0 ? idx : i]?.familyKey ?? "";
      }),
    );
    for (const c of analysis.varStaticConflicts) {
      if (!selectedFamilies.has(c.familyKey)) continue;
      if (selection.varStaticMode === "variable") {
        for (const id of c.existingStaticIds) replaceIds.add(id);
      } else if (selection.varStaticMode === "static") {
        for (const id of c.existingVariableIds) replaceIds.add(id);
      }
    }
  }

  return {
    files: selected,
    replaceExistingLocalIds: Array.from(replaceIds),
    skippedCount: Math.max(0, files.length - selected.length),
  };
}

export function toSavedImportRule(selection: ImportOptionsSelection): SavedImportRule {
  return {
    varStaticMode: selection.varStaticMode,
    includeVariable: selection.includeVariable,
    includeStatic: selection.includeStatic,
    includeItalic: selection.includeItalic,
    formats: { ...selection.formats },
    autoActivate: selection.autoActivate,
  };
}

export function loadSavedImportRule(): SavedImportRule | null {
  try {
    if (typeof window === "undefined") return null;
    const raw = window.localStorage.getItem(SAVED_RULE_KEY);
    if (!raw) return null;
    const o = JSON.parse(raw) as Partial<SavedImportRule>;
    if (!o || typeof o !== "object") return null;
    const mode: VarStaticMode =
      o.varStaticMode === "static" || o.varStaticMode === "both" ? o.varStaticMode : "variable";
    const fmt = (o.formats ?? {}) as Partial<Record<FontFileFormat, unknown>>;
    return {
      varStaticMode: mode,
      includeVariable: o.includeVariable !== false,
      includeStatic: o.includeStatic !== false,
      includeItalic: o.includeItalic !== false,
      formats: {
        ttf: fmt.ttf !== false,
        otf: fmt.otf !== false,
        ttc: fmt.ttc !== false,
        woff2: Boolean(fmt.woff2),
        woff: Boolean(fmt.woff),
      },
      autoActivate: o.autoActivate !== false,
    };
  } catch {
    return null;
  }
}

export function saveImportRule(rule: SavedImportRule): void {
  try {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(SAVED_RULE_KEY, JSON.stringify(rule));
  } catch {
    /* ignore storage quota */
  }
}

export function clearSavedImportRule(): void {
  try {
    if (typeof window === "undefined") return;
    window.localStorage.removeItem(SAVED_RULE_KEY);
  } catch {
    /* ignore */
  }
}

/** Merge saved user defaults with smart batch safeguards so we never deadlock at 0 selected files. */
export function selectionFromSavedRule(
  analysis: ImportBatchAnalysis,
  saved: SavedImportRule | null,
): ImportOptionsSelection {
  const smart = defaultImportSelection(analysis);
  if (!saved) return smart;
  const anyBatchHasVariable = analysis.varStaticConflicts.some((c) => c.batchVariableCount > 0);
  const varStaticMode: VarStaticMode =
    analysis.varStaticConflicts.length === 0
      ? saved.varStaticMode
      : saved.varStaticMode === "variable" && !anyBatchHasVariable
        ? "static"
        : saved.varStaticMode;
  const formats: Record<FontFileFormat, boolean> = {
    ...saved.formats,
    // If the user ONLY dropped web formats (no desktop TTF/OTF), keep web formats enabled.
    woff2: !analysis.hasDesktop && analysis.formats.woff2 > 0 ? true : saved.formats.woff2,
    woff: !analysis.hasDesktop && analysis.formats.woff > 0 ? true : saved.formats.woff,
  };
  return {
    ...smart,
    varStaticMode,
    includeVariable: saved.includeVariable,
    includeStatic: saved.includeStatic,
    includeItalic: saved.includeItalic,
    formats,
    autoActivate: saved.autoActivate,
    saveAsDefault: true,
  };
}

/** Convert disk origin paths under `rootPath` into MinimalFileLike entries for smart analysis. */
export function originPathsToFileLikes(originPaths: string[], rootPath?: string): MinimalFileLike[] {
  const normRoot = (rootPath ?? "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return originPaths.map((fullPath) => {
    const slash = fullPath.replace(/\\/g, "/");
    const name = slash.split("/").pop() || fullPath;
    const lower = slash.toLowerCase();
    const rel =
      normRoot && lower.startsWith(`${normRoot}/`)
        ? slash.slice(normRoot.length + 1)
        : slash;
    return { name, webkitRelativePath: rel };
  });
}

/** Filter newly discovered watch-folder paths through a saved smart rule. */
export function filterOriginPathsByRule(
  originPaths: string[],
  rootPath: string,
  rule: SavedImportRule,
  existingLocalFonts: MinimalLocalFont[] = [],
): string[] {
  if (!originPaths.length) return [];
  const likes = originPathsToFileLikes(originPaths, rootPath);
  const analysis = analyzeImportBatch(likes, existingLocalFonts);
  const selection = selectionFromSavedRule(analysis, rule);
  const filtered = filterImportFiles(likes, analysis, selection);
  const keptIndices = new Set(
    filtered.files.map((item) => likes.indexOf(item)).filter((i) => i >= 0),
  );
  return originPaths.filter((_, i) => keptIndices.has(i));
}

// Pub/sub bridge between runFontImport() / addWatchedFolder() and <ImportOptionsDialog />.
type Listener = (req: ImportOptionsRequest | null) => void;
let currentRequest: ImportOptionsRequest | null = null;
let currentResolver: ((selection: ImportOptionsSelection | null) => void) | null = null;
const listeners = new Set<Listener>();

function emit() {
  for (const fn of listeners) fn(currentRequest);
}

export function subscribeImportOptions(fn: Listener): () => void {
  listeners.add(fn);
  fn(currentRequest);
  return () => {
    listeners.delete(fn);
  };
}

export function requestImportOptions(
  analysis: ImportBatchAnalysis,
  opts?: { mode?: ImportSourceMode; folderLabel?: string },
): Promise<ImportOptionsSelection | null> {
  if (currentResolver) {
    currentResolver(null);
    currentResolver = null;
  }
  return new Promise((resolve) => {
    currentRequest = {
      analysis,
      mode: opts?.mode ?? "upload",
      folderLabel: opts?.folderLabel,
    };
    currentResolver = resolve;
    emit();
  });
}

export function resolveImportOptions(selection: ImportOptionsSelection | null) {
  if (selection) {
    if (selection.saveAsDefault) {
      saveImportRule(toSavedImportRule(selection));
    } else if (selection.saveAsDefault === false) {
      clearSavedImportRule();
    }
  }
  const resolve = currentResolver;
  currentRequest = null;
  currentResolver = null;
  emit();
  resolve?.(selection);
}
