/**
 * Pre-built inverted trigram + token search index for 20k+ font libraries.
 *
 * Eliminates O(N) `parseSearchQuery()` + `hayFor()` string allocations on every keystroke.
 * Trigram posting lists preserve 100% substring fidelity (e.g. "boto" -> "Roboto") while
 * shrinking candidate scans dramatically for queries >= 3 characters.
 */

import { licenseSearchHay } from "./license.ts";
import {
  fontMatchesSearchWithCache,
  metricsFor,
  parseSearchQuery,
} from "./metrics.ts";
import type { FontMetrics, FontRecord } from "./types.ts";

export interface IndexedFontEntry {
  font: FontRecord;
  metrics: FontMetrics;
  tags: string[];
  hay: string;
}

export interface FontSearchIndex {
  entries: IndexedFontEntry[];
  /** 3-char lowercase trigrams -> sorted array of entry indices */
  trigramMap: Map<string, number[]>;
  /** Exact lowercase alphanumeric tokens -> sorted array of entry indices */
  tokenMap: Map<string, number[]>;
}

function buildTags(font: FontRecord, customTags: Record<string, string[]>): string[] {
  const extra = customTags[font.id];
  if (!extra?.length) return font.tags ?? [];
  return Array.from(new Set([...(font.tags ?? []), ...extra]));
}

function buildHay(font: FontRecord, tags: string[]): string {
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

function isAsciiAlnum(code: number): boolean {
  return (code >= 97 && code <= 122) || (code >= 48 && code <= 57);
}

export function buildFontSearchIndex(
  fonts: FontRecord[],
  customTags: Record<string, string[]>,
): FontSearchIndex {
  const entries: IndexedFontEntry[] = new Array(fonts.length);
  const trigramMap = new Map<string, number[]>();
  const tokenMap = new Map<string, number[]>();

  for (let i = 0; i < fonts.length; i += 1) {
    const font = fonts[i]!;
    const tags = buildTags(font, customTags);
    const hay = buildHay(font, tags);
    const metrics = metricsFor(font);
    entries[i] = { font, metrics, tags, hay };

    // Allocation-free scanner for exact tokens + 3-char trigrams across hay (scales cleanly to 100k fonts).
    const len = hay.length;
    let start = -1;
    for (let pos = 0; pos <= len; pos += 1) {
      const ok = pos < len && isAsciiAlnum(hay.charCodeAt(pos));
      if (ok) {
        if (start === -1) start = pos;
        continue;
      }
      if (start === -1) continue;
      const tokLen = pos - start;
      if (tokLen >= 2) {
        const tok = hay.slice(start, pos);
        let exact = tokenMap.get(tok);
        if (!exact) {
          exact = [i];
          tokenMap.set(tok, exact);
        } else if (exact[exact.length - 1] !== i) {
          exact.push(i);
        }

        if (tokLen >= 3) {
          const endTri = pos - 3;
          for (let k = start; k <= endTri; k += 1) {
            const tri = hay.slice(k, k + 3);
            let list = trigramMap.get(tri);
            if (!list) {
              list = [i];
              trigramMap.set(tri, list);
            } else if (list[list.length - 1] !== i) {
              list.push(i);
            }
          }
        }
      }
      start = -1;
    }
  }

  return { entries, trigramMap, tokenMap };
}

// Two-level WeakMap cache keyed by (fonts array reference, customTags reference).
const indexCache = new WeakMap<FontRecord[], WeakMap<Record<string, string[]>, FontSearchIndex>>();

export function getCachedSearchIndex(
  fonts: FontRecord[],
  customTags: Record<string, string[]>,
): FontSearchIndex {
  let byTags = indexCache.get(fonts);
  if (!byTags) {
    byTags = new WeakMap();
    indexCache.set(fonts, byTags);
  }
  let hit = byTags.get(customTags);
  if (!hit) {
    hit = buildFontSearchIndex(fonts, customTags);
    byTags.set(customTags, hit);
  }
  return hit;
}

function intersectSorted(a: number[], b: number[]): number[] {
  const out: number[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const av = a[i]!;
    const bv = b[j]!;
    if (av === bv) {
      out.push(av);
      i += 1;
      j += 1;
    } else if (av < bv) {
      i += 1;
    } else {
      j += 1;
    }
  }
  return out;
}

/**
 * High-speed library query filter:
 * 1. Parses the query ONCE (never O(N) re-parsing per font).
 * 2. Reuses memoized normalized haystacks, merged tags, and metrics.
 * 3. Narrows candidates via trigram + token posting list intersections for positive
 *    terms while preserving 100% substring fidelity (including mid-word matches).
 */
export function filterFontsWithIndex(
  fonts: FontRecord[],
  query: string,
  customTags: Record<string, string[]>,
): FontRecord[] {
  const trimmed = query.trim();
  if (!trimmed) return fonts;
  const clauses = parseSearchQuery(trimmed);
  if (!clauses.length) return fonts;

  const index = getCachedSearchIndex(fonts, customTags);

  // Collect selective posting lists and intersect smallest-first for minimal work.
  const postingLists: number[][] = [];
  for (const clause of clauses) {
    if (clause.negated) continue;
    if (clause.kind === "hay" || clause.kind === "name") {
      const parts = clause.value.split(/[^a-z0-9]+/);
      for (const part of parts) {
        if (part.length >= 3) {
          // Sample start, middle, and end trigrams of the token for tight selectivity.
          const tris = [
            part.slice(0, 3),
            part.slice(part.length - 3),
          ];
          if (part.length >= 5) {
            const mid = Math.floor((part.length - 3) / 2);
            tris.push(part.slice(mid, mid + 3));
          }
          for (const tri of tris) {
            const list = index.trigramMap.get(tri);
            if (!list || list.length === 0) return [];
            postingLists.push(list);
          }
        }
      }
    } else if (clause.kind === "category" && clause.values.length === 1) {
      const tok = clause.values[0]!;
      if (/^[a-z0-9]{2,}$/.test(tok)) {
        const list = index.tokenMap.get(tok);
        if (!list || list.length === 0) return [];
        postingLists.push(list);
      }
    } else if (clause.kind === "source" && clause.values.length === 1) {
      const tok = clause.values[0]!;
      if (/^[a-z0-9]{2,}$/.test(tok)) {
        const list = index.tokenMap.get(tok);
        if (!list || list.length === 0) return [];
        postingLists.push(list);
      }
    }
  }

  let candidateIndices: number[] | null = null;
  if (postingLists.length > 0) {
    postingLists.sort((a, b) => a.length - b.length);
    candidateIndices = postingLists[0]!;
    for (let i = 1; i < postingLists.length; i += 1) {
      candidateIndices = intersectSorted(candidateIndices, postingLists[i]!);
      if (candidateIndices.length === 0) return [];
    }
  }

  const out: FontRecord[] = [];
  if (candidateIndices) {
    for (let i = 0; i < candidateIndices.length; i += 1) {
      const entry = index.entries[candidateIndices[i]!]!;
      if (fontMatchesSearchWithCache(entry.font, clauses, entry.metrics, entry.tags, entry.hay)) {
        out.push(entry.font);
      }
    }
  } else {
    const entries = index.entries;
    for (let i = 0; i < entries.length; i += 1) {
      const entry = entries[i]!;
      if (fontMatchesSearchWithCache(entry.font, clauses, entry.metrics, entry.tags, entry.hay)) {
        out.push(entry.font);
      }
    }
  }
  return out;
}
