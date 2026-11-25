import { useDeferredValue, useEffect, useMemo, useRef, useState, type UIEvent } from "react";
import { ChevronsUpDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cssFamilyStack, loadFont } from "@/lib/fonts/loader";
import { loadGlyphAtlas } from "@/lib/fonts/glyph-map";
import { allFonts, findFont, tagsFor, useFontStore } from "@/lib/fonts/store";
import { isGoogleCatalog } from "@/lib/fonts/catalog";
import type { FontRecord } from "@/lib/fonts/types";
import { CATEGORY_LABEL } from "@/lib/fonts/types";
import { cn } from "@/lib/utils";

export function FontPicker({
  value,
  onChange,
  label,
  prefetchAtlas = false,
  pageSize = 60,
}: {
  value: string;
  onChange: (id: string) => void;
  label: string;
  prefetchAtlas?: boolean;
  /** Number of items rendered per page (defaults to 60 so huge local libraries open without lag). */
  pageSize?: number;
}) {
  const localFonts = useFontStore((s) => s.localFonts);
  const googleFonts = useFontStore((s) => s.googleFonts);
  const systemFonts = useFontStore((s) => s.systemFonts);
  const activatedSet = useFontStore((s) => s.activatedSet);
  const customTags = useFontStore((s) => s.customTags);
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const deferredQ = useDeferredValue(q);
  const [shown, setShown] = useState(pageSize > 0 ? pageSize : 60);
  const hoverTimerRef = useRef<number | null>(null);

  const fonts = useMemo(
    () => (open ? [...systemFonts, ...allFonts(localFonts, googleFonts)] : []),
    [open, systemFonts, localFonts, googleFonts],
  );
  const selected = findFont(value, localFonts, googleFonts);

  const topGoogleIds = useMemo(() => {
    if (!open) return new Set<string>();
    const sorted = googleFonts.slice().sort((a, b) => (a.popularity || 9999) - (b.popularity || 9999));
    return new Set(sorted.slice(0, 80).map((f) => f.id));
  }, [open, googleFonts]);

  const filtered = useMemo(() => {
    if (!open) return [];
    const query = deferredQ.trim().toLowerCase();
    const match = (f: FontRecord) => {
      if (!query && f.source === "google" && !activatedSet.has(f.id) && !topGoogleIds.has(f.id)) return false;
      if (!query) return true;
      if (f.family.toLowerCase().includes(query)) return true;
      if (f.fullName?.toLowerCase().includes(query)) return true;
      if (f.category.includes(query)) return true;
      if (CATEGORY_LABEL[f.category]?.toLowerCase().includes(query)) return true;
      return tagsFor(f, customTags).some((tag) => tag.includes(query));
    };
    const hits = fonts.filter(match);
    hits.sort((a, b) => {
      const rank = (font: FontRecord) =>
        font.source === "local" ? 0 : activatedSet.has(font.id) ? 1 : font.source === "system" ? 2 : 3;
      const diff = rank(a) - rank(b);
      if (diff !== 0) return diff;
      return (a.fullName || a.family).localeCompare(b.fullName || b.family);
    });
    return hits;
  }, [open, fonts, deferredQ, activatedSet, topGoogleIds, customTags]);

  const visible = useMemo(() => {
    if (!open) return [];
    const cap = pageSize > 0 ? shown : filtered.length;
    const slice = filtered.slice(0, Math.min(filtered.length, cap));
    if (selected && !slice.some((f) => f.id === selected.id)) {
      return [selected, ...slice];
    }
    return slice;
  }, [open, filtered, shown, pageSize, selected]);

  const groups = useMemo(() => {
    const windows = visible.filter((f) => f.source === "system");
    const uploaded = visible.filter((f) => f.source === "local");
    const gfonts = visible.filter(isGoogleCatalog);
    return [
      { label: "Google Fonts", items: gfonts },
      { label: "Local Files", items: uploaded },
      { label: "System", items: windows },
    ].filter((g) => g.items.length);
  }, [visible]);

  useEffect(() => {
    setShown(pageSize > 0 ? pageSize : Number.POSITIVE_INFINITY);
  }, [q, open, pageSize]);

  useEffect(() => {
    return () => {
      if (hoverTimerRef.current !== null) {
        window.clearTimeout(hoverTimerRef.current);
      }
    };
  }, []);

  function warm(font: FontRecord) {
    void loadFont(font);
    if (prefetchAtlas) {
      void loadGlyphAtlas(font);
    }
  }

  function scheduleWarm(font: FontRecord) {
    if (hoverTimerRef.current !== null) {
      window.clearTimeout(hoverTimerRef.current);
    }
    hoverTimerRef.current = window.setTimeout(() => {
      hoverTimerRef.current = null;
      warm(font);
    }, 80);
  }

  function cancelWarm() {
    if (hoverTimerRef.current !== null) {
      window.clearTimeout(hoverTimerRef.current);
      hoverTimerRef.current = null;
    }
  }

  function pick(font: FontRecord) {
    cancelWarm();
    onChange(font.id);
    setOpen(false);
    warm(font);
  }

  function onListScroll(event: UIEvent<HTMLDivElement>) {
    if (pageSize <= 0) return;
    const t = event.target as HTMLElement;
    if (!t || t.scrollHeight - t.scrollTop - t.clientHeight > 64) return;
    setShown((n) => Math.min(filtered.length, (Number.isFinite(n) ? n : pageSize) + pageSize));
  }

  const remaining = Math.max(0, filtered.length - shown);

  return (
    <Popover open={open} onOpenChange={setOpen} modal={false}>
      <PopoverTrigger asChild>
        <Button variant="outline" className="h-11 w-full justify-between font-normal">
          <span className="truncate" style={{ fontFamily: selected ? cssFamilyStack(selected) : undefined }}>
            {selected?.fullName || selected?.family || label}
          </span>
          <ChevronsUpDown className="size-4 text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[min(92vw,360px)] p-2" align="start">
        <Input
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Search typefaces…"
          className="mb-2 h-9"
        />
        <ScrollArea className="h-72" onScrollCapture={onListScroll}>
          <div className="flex flex-col gap-2">
            {groups.length === 0 && (
              <p className="px-2 py-6 text-center text-sm text-muted-foreground">
                {q.trim() ? "No matches" : "Activate a catalog face in the library. System fonts list here in the desktop app."}
              </p>
            )}
            {groups.map((group) => (
              <div key={group.label}>
                <p className="px-2 pb-1 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
                  {group.label}
                </p>
                {group.items.map((font, idx) => (
                  <button
                    key={font.id}
                    type="button"
                    onMouseEnter={() => scheduleWarm(font)}
                    onMouseLeave={cancelWarm}
                    onClick={() => pick(font)}
                    className={cn(
                      "flex w-full items-baseline justify-between gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent",
                      font.id === value && "bg-accent",
                    )}
                  >
                    <span
                      className="truncate"
                      style={
                        idx < 30 || font.id === value
                          ? { fontFamily: cssFamilyStack(font) }
                          : undefined
                      }
                    >
                      {font.fullName || font.family}
                    </span>
                    <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                      {font.source === "local" ? "file" : font.source === "system" ? "System" : CATEGORY_LABEL[font.category]}
                    </span>
                  </button>
                ))}
              </div>
            ))}
            {pageSize > 0 && remaining > 0 && (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="w-full text-xs text-muted-foreground"
                onClick={() => setShown((n) => Math.min(filtered.length, (Number.isFinite(n) ? n : pageSize) + pageSize))}
              >
                Show more ({remaining} remaining)
              </Button>
            )}
          </div>
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}