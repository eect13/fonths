import { X } from "lucide-react";
import { useDeferredValue, useMemo } from "react";
import { filterLibrary, poolForScope, useFontStore } from "@/lib/fonts/store";
import type { LibraryFacet } from "@/lib/fonts/types";
import { CATEGORY_LABEL, LICENSE_LABEL, isFacetScope } from "@/lib/fonts/types";
import { SEARCH_PRESETS, countSearchPresets, queryHasToken, toggleSearchPreset } from "@/lib/fonts/metrics";
import { cn } from "@/lib/utils";

function facetLabel(facet: LibraryFacet) {
  if (facet.startsWith("license:")) return LICENSE_LABEL[facet.slice(8) as keyof typeof LICENSE_LABEL] ?? facet;
  if (facet.startsWith("category:")) return CATEGORY_LABEL[facet.slice(9) as keyof typeof CATEGORY_LABEL] ?? facet;
  if (facet.startsWith("tag:")) return facet.slice(4);
  return facet;
}

const EMPTY_IDS: string[] = [];

/** SuperSearch chips: live counts in this drawer. 0-count chips hide (xh/contrast until OS/2). */
export function SearchChips() {
  const query = useFontStore((s) => s.query);
  const setQuery = useFontStore((s) => s.setQuery);
  const facet = useFontStore((s) => s.facet);
  const setFacet = useFontStore((s) => s.setFacet);
  const facetOn = isFacetScope(facet);
  const scope = useFontStore((s) => s.scope);
  const localFonts = useFontStore((s) => s.localFonts);
  const googleFonts = useFontStore((s) => s.googleFonts);
  const systemFonts = useFontStore((s) => s.systemFonts);
  const customTags = useFontStore((s) => s.customTags);
  const favorites = useFontStore((s) => s.favorites);
  const collections = useFontStore((s) => s.collections);
  const activated = useFontStore((s) => s.activated);
  const recentIds = useFontStore((s) => s.recentIds);
  const autoHideDuplicates = useFontStore((s) => s.autoHideDuplicates);
  const duplicateHideIds = useFontStore((s) => s.duplicateHideIds);
  const deferredLocal = useDeferredValue(localFonts);
  const deferredGoogle = useDeferredValue(googleFonts);

  const counts = useMemo(() => {
    const skipLocals = scope === "gfonts" || scope === "google" || scope === "system";
    const liveIds = scope === "activated" ? activated : EMPTY_IDS;
    const pool = poolForScope(scope, skipLocals ? [] : deferredLocal, deferredGoogle, systemFonts, liveIds);
    let baseList = filterLibrary(pool, scope, "", favorites, liveIds, collections, customTags, facet, recentIds);
    if (autoHideDuplicates && duplicateHideIds.length) {
      const hidden = new Set(duplicateHideIds);
      baseList = baseList.filter((f) => !hidden.has(f.id));
    }
    const trimmedQuery = query.trim();
    if (!trimmedQuery) {
      return countSearchPresets(baseList, customTags);
    }
    // When filters are already active, intersect counts against the active query so chip numbers
    // show exact remaining matches (removing each chip's own token when that chip is already ON).
    const out: Record<string, number> = {};
    for (const preset of SEARCH_PRESETS) {
      const active = queryHasToken(trimmedQuery, preset.token);
      const effectiveQuery = active ? toggleSearchPreset(trimmedQuery, preset.token).trim() : trimmedQuery;
      const scoped = effectiveQuery
        ? filterLibrary(baseList, scope, effectiveQuery, favorites, liveIds, collections, customTags, facet, recentIds)
        : baseList;
      out[preset.id] = countSearchPresets(scoped, customTags)[preset.id] ?? 0;
    }
    return out;
  }, [
    query,
    scope,
    facet,
    deferredLocal,
    deferredGoogle,
    systemFonts,
    customTags,
    favorites,
    collections,
    activated,
    recentIds,
    autoHideDuplicates,
    duplicateHideIds,
  ]);

  const chips = SEARCH_PRESETS.filter((chip) => queryHasToken(query, chip.token) || (counts[chip.id] ?? 0) > 0);
  if (!chips.length && !facetOn) return null;

  return (
    <div className="flex flex-wrap gap-1" role="group" aria-label="SuperSearch">
      {chips.map((chip) => {
        const on = queryHasToken(query, chip.token);
        const n = counts[chip.id] ?? 0;
        return (
          <button
            key={chip.id}
            type="button"
            aria-pressed={on}
            title={chip.hint}
            onClick={() => setQuery(toggleSearchPreset(query, chip.token))}
            className={cn(
              "inline-flex h-6 shrink-0 items-center gap-1 rounded-full px-2.5 text-[11px] font-medium transition-colors duration-150",
              on
                ? "bg-primary text-primary-foreground"
                : "bg-secondary text-muted-foreground hover:text-foreground",
            )}
          >
            {chip.label}
            {n > 0 ? (
              <span className={cn("tabular-nums", on ? "text-primary-foreground/85" : "text-foreground")}>
                {n.toLocaleString()}
              </span>
            ) : null}
          </button>
        );
      })}
      {facetOn ? (
        <button
          type="button"
          aria-pressed
          onClick={() => setFacet("")}
          className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full bg-primary px-2.5 text-[11px] font-medium text-primary-foreground"
        >
          {facetLabel(facet)}
          <X className="size-3" />
        </button>
      ) : null}
    </div>
  );
}