import { FONT_BY_ID, GOOGLE_FONTS, isGoogleCatalog } from "../catalog";
import { fontLicense } from "../license";
import { fontMatchesSearch, parseSearchQuery } from "../metrics";
import { filterFontsWithIndex } from "../search-index";
import type {
  Collection,
  FontLicense,
  FontRecord,
  LibraryFacet,
  LibraryScope,
  LibrarySort,
} from "../types";
import { isFacetScope } from "../types";
import { collectFolderFontIds } from "./folder-helpers";

let systemFontProvider: () => FontRecord[] = () => [];

export function setSystemFontProvider(provider: () => FontRecord[]) {
  systemFontProvider = provider;
}

export function allFonts(
  localFonts: FontRecord[],
  googleFonts: FontRecord[] = GOOGLE_FONTS,
): FontRecord[] {
  if (!localFonts.length) return googleFonts;
  if (!googleFonts.length) return localFonts;
  return [...localFonts, ...googleFonts];
}

/** Narrow the array *before* filter/sort so 20k locals are not copied when viewing Google Fonts / Activated. */
export function poolForScope(
  scope: LibraryScope,
  localFonts: FontRecord[],
  googleFonts: FontRecord[],
  systemFonts: FontRecord[] = [],
  liveIds: readonly string[] = [],
): FontRecord[] {
  if (scope === "system") return systemFonts;
  if (scope === "uploaded") return localFonts;
  if (scope === "gfonts" || scope === "google") return googleFonts;
  if (scope === "recent") return allFonts(localFonts, googleFonts).concat(systemFonts);
  // Activated drawer/facet: O(live), never allFonts(~22k) on every GDI tick.
  if (scope === "activated") {
    if (!liveIds.length) return [];
    const googleOnly = liveIds.every((id) => id.startsWith("g:"));
    const localById = googleOnly ? null : new Map(localFonts.map((f) => [f.id, f]));
    const googleById = new Map(googleFonts.map((f) => [f.id, f]));
    const systemById = systemFonts.length ? new Map(systemFonts.map((f) => [f.id, f])) : null;
    const out: FontRecord[] = [];
    const seen = new Set<string>();
    for (const id of liveIds) {
      if (!id || seen.has(id)) continue;
      seen.add(id);
      // Prefer store googleFonts (disk VF honesty) over static FONT_BY_ID (variable:false).
      const font =
        localById?.get(id) ??
        googleById.get(id) ??
        systemById?.get(id) ??
        (id.startsWith("g:") ? FONT_BY_ID.get(id) : undefined) ??
        FONT_BY_ID.get(id);
      if (font) out.push(font);
    }
    return out;
  }
  return allFonts(localFonts, googleFonts);
}

export function findFont(
  id: string,
  localFonts: FontRecord[],
  googleFonts: FontRecord[] = GOOGLE_FONTS,
): FontRecord | undefined {
  if (id.startsWith("g:")) {
    return FONT_BY_ID.get(id) ?? googleFonts.find((f) => f.id === id);
  }
  if (id.startsWith("s:")) {
    return systemFontProvider().find((f) => f.id === id);
  }
  return (
    localFonts.find((f) => f.id === id) ??
    googleFonts.find((f) => f.id === id) ??
    FONT_BY_ID.get(id)
  );
}

export function tagsFor(font: FontRecord, customTags: Record<string, string[]>): string[] {
  const extra = customTags[font.id] ?? [];
  return Array.from(new Set([...font.tags, ...extra]));
}

export function matchesQuery(
  font: FontRecord,
  query: string,
  customTags: Record<string, string[]>,
): boolean {
  return fontMatchesSearch(font, parseSearchQuery(query), customTags);
}

export function filterLibrary(
  fonts: FontRecord[],
  scope: LibraryScope,
  query: string,
  favorites: string[],
  activated: string[],
  collections: Collection[],
  customTags: Record<string, string[]>,
  facet: LibraryFacet | string = "",
  recentIds: readonly string[] = [],
): FontRecord[] {
  let list = fonts;
  const where = isFacetScope(scope) ? "all" : scope;
  if (where === "activated") {
    const on = new Set(activated);
    list = list.filter((f) => on.has(f.id));
  } else if (where === "favorites") {
    const fav = new Set(favorites);
    list = list.filter((f) => fav.has(f.id));
  } else if (where === "uploaded") list = list.filter((f) => f.source === "local");
  else if (where === "recent") {
    const byId = new Map(list.map((f) => [f.id, f]));
    list = recentIds.map((id) => byId.get(id)).filter((f): f is FontRecord => Boolean(f));
  }
  else if (where === "google" || where === "gfonts") list = list.filter(isGoogleCatalog);
  else if (where === "system") {
    list = list.filter((f) => f.source === "system");
  }
  else if (where.startsWith("collection:")) {
    const ids = collectFolderFontIds(collections, where.slice(11));
    list = list.filter((f) => ids.has(f.id));
  }
  const cut = facet || (isFacetScope(scope) ? scope : "");
  if (cut.startsWith("category:")) {
    const cat = cut.slice(9);
    list = list.filter((f) => f.category === cat);
  } else if (cut.startsWith("tag:")) {
    const tag = cut.slice(4).toLowerCase();
    list = list.filter((f) => tagsFor(f, customTags).some((t) => t.toLowerCase() === tag));
  } else if (cut.startsWith("license:")) {
    const license = cut.slice(8) as FontLicense;
    list = list.filter((f) => fontLicense(f) === license);
  }
  if (query.trim()) list = filterFontsWithIndex(list, query, customTags);
  return list;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base" });

/**
 * Fast library sort: uses Intl.Collator for standard libraries (<=5k) and switches
 * to a pre-lowercased Schwartzian key sort for massive 10k-100k libraries (~10x faster).
 */
export function sortLibrary(fonts: FontRecord[], sort: LibrarySort = "name-asc"): FontRecord[] {
  if (fonts.length <= 1) return [...fonts];
  if (fonts.length > 5000) {
    const keyed = fonts.map((font) => ({
      font,
      fam: font.family.toLowerCase(),
      id: font.id,
      pop: font.popularity,
      added: font.addedAt ?? 0,
    }));
    keyed.sort((a, b) => {
      switch (sort) {
        case "name-desc":
          return (b.fam < a.fam ? -1 : b.fam > a.fam ? 1 : 0) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
        case "popular":
          return a.pop - b.pop || (a.fam < b.fam ? -1 : a.fam > b.fam ? 1 : 0);
        case "recent":
          return b.added - a.added || (a.fam < b.fam ? -1 : a.fam > b.fam ? 1 : 0);
        case "name-asc":
        default:
          return (a.fam < b.fam ? -1 : a.fam > b.fam ? 1 : 0) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
      }
    });
    return keyed.map((k) => k.font);
  }
  const copy = [...fonts];
  copy.sort((a, b) => {
    switch (sort) {
      case "name-desc":
        return collator.compare(b.family, a.family) || a.id.localeCompare(b.id);
      case "popular":
        return a.popularity - b.popularity || collator.compare(a.family, b.family);
      case "recent":
        return (b.addedAt ?? 0) - (a.addedAt ?? 0) || collator.compare(a.family, b.family);
      case "name-asc":
      default:
        return collator.compare(a.family, b.family) || a.id.localeCompare(b.id);
    }
  });
  return copy;
}
