import { AlignCenter, AlignLeft, AlignRight, Check, ChevronsUpDown, Italic, LayoutGrid, List, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { HelpTip } from "./help-tip";
import { SearchChips } from "./search-chips";
import { ALIGN_LABEL, SAMPLE_PRESETS, SORT_LABEL, type LibrarySort, type PreviewAlign, type PreviewTheme } from "@/lib/fonts/types";
import { useFontStore } from "@/lib/fonts/store";
import { cn } from "@/lib/utils";

const PRIMARY_THEMES: { id: PreviewTheme; label: string; hint: string }[] = [
  { id: "paper", label: "Paper", hint: "Light paper specimen" },
  { id: "ink", label: "Ink", hint: "Inverted ink on black" },
];

const EXTRA_THEMES: { id: PreviewTheme; label: string }[] = [
  { id: "paper", label: "Paper (Light)" },
  { id: "ink", label: "Ink (Dark)" },
  { id: "newsprint", label: "Newsprint proof" },
  { id: "blueprint", label: "Blueprint proof" },
];

const SORTS: LibrarySort[] = ["name-asc", "name-desc", "popular", "recent"];

const ALIGNS: { id: PreviewAlign; icon: typeof AlignLeft }[] = [
  { id: "left", icon: AlignLeft },
  { id: "center", icon: AlignCenter },
  { id: "right", icon: AlignRight },
];

export function PreviewToolbar() {
  const preview = useFontStore((s) => s.preview);
  const setPreview = useFontStore((s) => s.setPreview);
  const sort = preview.sort ?? "name-asc";
  const align = preview.align ?? "left";

  return (
    <div className="fm-preview-toolbar flex flex-col border-b border-border bg-background/90 backdrop-blur-sm">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <div className="flex min-w-[180px] flex-1 items-center gap-1">
          <Input
            value={preview.sampleText}
            onChange={(e) => setPreview({ sampleText: e.target.value })}
            placeholder="Type to preview…"
            className="h-8 bg-card"
            aria-label="Preview text"
          />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon-sm" variant="ghost" className="size-7 shrink-0" aria-label="Sample presets">
                <ChevronsUpDown className="size-3.5" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="max-w-80">
              <DropdownMenuLabel>Sample</DropdownMenuLabel>
              {SAMPLE_PRESETS.map((preset) => (
                <DropdownMenuItem key={preset} onSelect={() => setPreview({ sampleText: preset })}>
                  <span className="min-w-0 flex-1 truncate font-serif">{preset}</span>
                  {preview.sampleText === preset ? <Check className="size-3.5" /> : null}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* Gestalt proximity group: live specimen typography controls (size, tracking, italic) */}
        <div className="flex shrink-0 items-center gap-2 rounded-md bg-secondary px-2 py-0.5">
          <div className="flex w-28 items-center gap-1.5" onPointerDown={(e) => e.stopPropagation()}>
            <span className="w-6 shrink-0 text-right text-xs tabular-nums text-muted-foreground" title="Preview size (px)">
              {preview.fontSize}
            </span>
            <Slider
              min={14}
              max={96}
              step={1}
              value={[preview.fontSize]}
              onValueChange={([fontSize]) => {
                if (typeof fontSize === "number") setPreview({ fontSize });
              }}
              aria-label="Font size"
            />
          </div>
          <div className="hidden w-24 items-center gap-1.5 lg:flex" onPointerDown={(e) => e.stopPropagation()}>
            <span className="w-8 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground" title="Letter spacing (em)">
              {preview.letterSpacing.toFixed(2)}
            </span>
            <Slider
              min={-0.06}
              max={0.25}
              step={0.005}
              value={[preview.letterSpacing]}
              onValueChange={([letterSpacing]) => {
                if (typeof letterSpacing === "number") setPreview({ letterSpacing });
              }}
              aria-label="Letter spacing"
            />
          </div>
          <Button
            type="button"
            size="icon-sm"
            variant={preview.italic ? "default" : "ghost"}
            className="size-7"
            aria-pressed={Boolean(preview.italic)}
            aria-label="Preview italic"
            title="Italic — only families with a real italic face"
            onClick={() => setPreview({ italic: !preview.italic })}
          >
            <Italic />
          </Button>
        </div>

        {/* Streamlined Light/Dark surface switch (proof themes folded into More menu) */}
        <div className="flex shrink-0 items-center gap-0.5 rounded-md bg-secondary p-0.5">
          {PRIMARY_THEMES.map((theme) => (
            <HelpTip key={theme.id} label={theme.hint}>
              <button
                type="button"
                onClick={() => setPreview({ theme: theme.id })}
                className={cn(
                  "h-7 rounded-full px-2.5 text-xs font-medium transition-colors duration-150",
                  preview.theme === theme.id
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {theme.label}
              </button>
            </HelpTip>
          ))}
        </div>

        <div className="ml-auto flex shrink-0 items-center gap-0.5">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon-sm" variant="ghost" className="size-7" aria-label="More specimen options">
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-56">
              <DropdownMenuLabel className="flex items-center justify-between">
                <span>Line height</span>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {preview.lineHeight.toFixed(2)}
                </span>
              </DropdownMenuLabel>
              <div className="px-2 py-1.5" onPointerDown={(e) => e.stopPropagation()}>
                <Slider
                  min={0.9}
                  max={2}
                  step={0.05}
                  value={[preview.lineHeight]}
                  onValueChange={([lineHeight]) => {
                    if (typeof lineHeight === "number") setPreview({ lineHeight });
                  }}
                  aria-label="Line height"
                />
              </div>
              <DropdownMenuLabel className="flex items-center justify-between">
                <span>Letter spacing</span>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {preview.letterSpacing.toFixed(2)}em
                </span>
              </DropdownMenuLabel>
              <div className="px-2 py-1.5" onPointerDown={(e) => e.stopPropagation()}>
                <Slider
                  min={-0.06}
                  max={0.25}
                  step={0.005}
                  value={[preview.letterSpacing]}
                  onValueChange={([letterSpacing]) => {
                    if (typeof letterSpacing === "number") setPreview({ letterSpacing });
                  }}
                  aria-label="Letter spacing"
                />
              </div>
              {preview.fontSize !== 36 || preview.lineHeight !== 1.3 || preview.letterSpacing !== 0 ? (
                <div className="px-2 pb-1">
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    className="h-7 w-full text-xs"
                    onClick={() => setPreview({ fontSize: 36, lineHeight: 1.3, letterSpacing: 0 })}
                  >
                    Reset size &amp; spacing
                  </Button>
                </div>
              ) : null}
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Surface proof</DropdownMenuLabel>
              {EXTRA_THEMES.map((t) => (
                <DropdownMenuItem key={t.id} onSelect={() => setPreview({ theme: t.id })}>
                  <span className="flex-1">{t.label}</span>
                  {preview.theme === t.id && <Check className="size-3.5" />}
                </DropdownMenuItem>
              ))}
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Align</DropdownMenuLabel>
              <div className="flex gap-0.5 px-2 pb-1.5">
                {ALIGNS.map((item) => {
                  const Icon = item.icon;
                  return (
                    <Button
                      key={item.id}
                      size="icon-sm"
                      variant={align === item.id ? "default" : "ghost"}
                      className="size-7"
                      aria-label={ALIGN_LABEL[item.id]}
                      onClick={() => setPreview({ align: item.id })}
                    >
                      <Icon />
                    </Button>
                  );
                })}
              </div>
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Sort</DropdownMenuLabel>
              {SORTS.map((id) => (
                <DropdownMenuItem key={id} onSelect={() => setPreview({ sort: id })}>
                  <span className="flex-1">{SORT_LABEL[id]}</span>
                  {sort === id && <Check className="size-3.5" />}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          <HelpTip label="Grid view">
            <Button
              size="icon-sm"
              variant={preview.view === "grid" ? "secondary" : "ghost"}
              className="size-7"
              aria-label="Grid view"
              onClick={() => setPreview({ view: "grid" })}
            >
              <LayoutGrid />
            </Button>
          </HelpTip>
          <HelpTip label="List view">
            <Button
              size="icon-sm"
              variant={preview.view === "list" ? "secondary" : "ghost"}
              className="size-7"
              aria-label="List view"
              onClick={() => setPreview({ view: "list" })}
            >
              <List />
            </Button>
          </HelpTip>
        </div>
      </div>
      <SearchChips />
    </div>
  );
}
