import { useCallback, useEffect, useMemo, useState } from "react";
import { FolderSearch, Power, RotateCcw, Sliders, Sparkles } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import {
  activateFontPath,
  deactivateFontPath,
  openTypeFeatureLabel,
  pickAndScanFontDirectory,
  scanFontDirectory,
  type SkiaFontMetadata,
} from "@/lib/fonts/skia-engine";
import { inDesktopShell } from "@/lib/desktop/open-fonts";
import { useFontStore } from "@/lib/fonts/store";
import { cn } from "@/lib/utils";
import { SkiaFontCanvas } from "./skia-font-canvas";

const DEFAULT_SAMPLE = "Sphinx of black quartz, judge my vow — 0123456789 fi fl";

function resolveCanvasInkHex(): string {
  if (typeof document === "undefined") return "#111111";
  return document.documentElement.classList.contains("dark") ? "#f4f4f5" : "#111111";
}

/**
 * Skia + Fontations Native Font Studio:
 * - Zero-copy directory scanning via `read-fonts` + `memmap2` (`scan_font_directory`).
 * - Raw RGBA8888 `<canvas>` specimen blitting via `render_font_preview` (FreeType + HarfBuzz).
 * - System-wide temporary Windows font activation (`activate_font` / `deactivate_font`).
 * - Live Inspector for every detected OpenType GSUB/GPOS feature tag and variable axis.
 */
export function SkiaStudio() {
  const watchedCollections = useFontStore((s) => s.collections);
  const [desktop, setDesktop] = useState(false);
  const [dirPath, setDirPath] = useState("");
  const [fonts, setFonts] = useState<SkiaFontMetadata[]>([]);
  const [scanning, setScanning] = useState(false);
  const [sampleText, setSampleText] = useState(DEFAULT_SAMPLE);
  const [fontSize, setFontSize] = useState(32);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [activePaths, setActivePaths] = useState<Record<string, boolean>>({});
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const [featuresByPath, setFeaturesByPath] = useState<Record<string, Record<string, number>>>({});
  const [variationsByPath, setVariationsByPath] = useState<Record<string, Record<string, number>>>({});
  const [inkHex, setInkHex] = useState(resolveCanvasInkHex);

  useEffect(() => {
    void inDesktopShell().then(setDesktop);
    const updateInk = () => setInkHex(resolveCanvasInkHex());
    updateInk();
    const obs = new MutationObserver(updateInk);
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => obs.disconnect();
  }, []);

  const watchedFolders = useMemo(
    () => watchedCollections.filter((c) => Boolean(c.watchPath)),
    [watchedCollections],
  );

  const selectedFont = useMemo(
    () => fonts.find((f) => f.path === selectedPath) ?? fonts[0] ?? null,
    [fonts, selectedPath],
  );

  const loadDirectory = useCallback(async (targetDir: string) => {
    setScanning(true);
    try {
      const list = await scanFontDirectory(targetDir);
      setDirPath(targetDir);
      setFonts(list);
      setSelectedPath(list[0]?.path ?? null);
      toast.success(`Scanned ${list.length.toLocaleString()} typeface${list.length === 1 ? "" : "s"}`, {
        id: "skia-scan",
        description: targetDir,
      });
    } catch (err) {
      toast.error("Failed to scan directory", {
        description: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setScanning(false);
    }
  }, []);

  const handlePickFolder = useCallback(async () => {
    if (!desktop) {
      toast.message("Skia Studio requires the desktop app", {
        description: "Run Fonths desktop to scan local font directories with Rust + Skia.",
      });
      return;
    }
    setScanning(true);
    try {
      const picked = await pickAndScanFontDirectory();
      if (!picked) return;
      setDirPath(picked.dirPath);
      setFonts(picked.fonts);
      setSelectedPath(picked.fonts[0]?.path ?? null);
      toast.success(
        `Scanned ${picked.fonts.length.toLocaleString()} typeface${picked.fonts.length === 1 ? "" : "s"}`,
        {
          id: "skia-scan",
          description: picked.dirPath,
        },
      );
    } catch (err) {
      toast.error("Folder scan failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setScanning(false);
    }
  }, [desktop]);

  const toggleActivate = useCallback(
    async (font: SkiaFontMetadata) => {
      const currentlyOn = Boolean(activePaths[font.path]);
      setBusyPath(font.path);
      try {
        if (currentlyOn) {
          await deactivateFontPath(font.path);
          setActivePaths((prev) => ({ ...prev, [font.path]: false }));
          toast.message(`Deactivated ${font.family_name}`, { id: `skia-act-${font.path}` });
        } else {
          await activateFontPath(font.path);
          setActivePaths((prev) => ({ ...prev, [font.path]: true }));
          toast.success(`Activated ${font.family_name} system-wide`, {
            id: `skia-act-${font.path}`,
            description: "Visible to Photoshop, Illustrator, Figma & Word until Fonths closes.",
          });
        }
      } catch (err) {
        toast.error(currentlyOn ? "Deactivate failed" : "Activate failed", {
          description: err instanceof Error ? err.message : String(err),
        });
      } finally {
        setBusyPath(null);
      }
    },
    [activePaths],
  );

  const selectedFeatures = useMemo(
    () => (selectedFont ? (featuresByPath[selectedFont.path] ?? {}) : {}),
    [selectedFont, featuresByPath],
  );

  const selectedVariations = useMemo(() => {
    if (!selectedFont) return {};
    const existing = variationsByPath[selectedFont.path];
    if (existing) return existing;
    const defaults: Record<string, number> = {};
    for (const axis of selectedFont.axes) {
      defaults[axis.tag] = axis.default;
    }
    return defaults;
  }, [selectedFont, variationsByPath]);

  const toggleFeature = useCallback((fontPath: string, tag: string, on: boolean) => {
    setFeaturesByPath((prev) => ({
      ...prev,
      [fontPath]: {
        ...(prev[fontPath] ?? {}),
        [tag]: on ? 1 : 0,
      },
    }));
  }, []);

  const setAxisValue = useCallback((fontPath: string, tag: string, value: number) => {
    setVariationsByPath((prev) => ({
      ...prev,
      [fontPath]: {
        ...(prev[fontPath] ?? {}),
        [tag]: value,
      },
    }));
  }, []);

  const resetInspectorControls = useCallback((font: SkiaFontMetadata) => {
    const defaults: Record<string, number> = {};
    for (const axis of font.axes) defaults[axis.tag] = axis.default;
    setVariationsByPath((prev) => ({ ...prev, [font.path]: defaults }));
    setFeaturesByPath((prev) => ({ ...prev, [font.path]: {} }));
  }, []);

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden bg-background">
      {/* Top Toolbar: Folder Picker, Sample Text Input, Font Size Slider */}
      <div className="flex shrink-0 flex-wrap items-center gap-3 border-b border-border bg-card/60 px-4 py-2.5 md:px-6">
        <Button
          type="button"
          size="sm"
          disabled={scanning}
          data-testid="skia-scan-folder-btn"
          onClick={() => void handlePickFolder()}
        >
          <FolderSearch className="size-4" />
          {scanning ? "Scanning…" : "Scan Font Folder"}
        </Button>

        {watchedFolders.length > 0 && !dirPath ? (
          <div className="flex flex-wrap items-center gap-1.5">
            {watchedFolders.slice(0, 3).map((wf) => (
              <Button
                key={wf.id}
                type="button"
                size="sm"
                variant="outline"
                className="h-8 text-xs"
                onClick={() => wf.watchPath && void loadDirectory(wf.watchPath)}
              >
                {wf.name}
              </Button>
            ))}
          </div>
        ) : null}

        <div className="min-w-[220px] flex-1">
          <Input
            value={sampleText}
            onChange={(e) => setSampleText(e.target.value)}
            placeholder="Type sample text to shape with HarfBuzz + FreeType…"
            aria-label="Global Skia preview sample text"
            data-testid="skia-sample-input"
            className="h-8 bg-background text-sm"
          />
        </div>

        <div className="flex items-center gap-2" role="group" aria-label="Skia font size">
          <span className="text-xs text-muted-foreground">Size</span>
          <div className="w-28">
            <Slider
              min={14}
              max={96}
              step={1}
              value={[fontSize]}
              aria-label="Skia preview font size"
              data-testid="skia-size-slider"
              onValueChange={([v]) => {
                if (typeof v === "number") setFontSize(v);
              }}
            />
          </div>
          <span className="w-9 text-right font-mono text-xs tabular-nums text-muted-foreground">
            {fontSize}px
          </span>
        </div>
      </div>

      {/* Main Split: Scrollable Font Cards (Left) + Live OpenType/Axis Inspector (Right) */}
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div className="fm-scroll flex-1 overflow-y-auto p-4 md:p-6">
          {dirPath ? (
            <div className="mb-3 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <span className="truncate font-mono">{dirPath}</span>
              <span className="tabular-nums">
                {fonts.length.toLocaleString()} typeface{fonts.length === 1 ? "" : "s"} · Skia
                FreeType + HarfBuzz
              </span>
            </div>
          ) : null}

          {fonts.length === 0 ? (
            <div className="mx-auto mt-12 max-w-lg rounded-xl border border-dashed border-border bg-card/40 p-8 text-center">
              <Sparkles className="mx-auto size-8 text-muted-foreground" />
              <h2 className="mt-3 font-heading text-xl">Skia Native Font Studio</h2>
              <p className="mt-2 text-sm text-muted-foreground">
                Pick any local folder containing <code className="font-mono">.ttf</code>,{" "}
                <code className="font-mono">.otf</code>, <code className="font-mono">.ttc</code>,
                or <code className="font-mono">.woff2</code> files. Fonths parses OpenType tables
                zero-copy with Fontations, shapes previews on a dedicated Skia (FreeType +
                HarfBuzz) canvas thread, and temporarily activates fonts system-wide without copying
                to <code className="font-mono">C:\Windows\Fonts</code>.
              </p>
              <Button
                type="button"
                className="mt-5"
                onClick={() => void handlePickFolder()}
                disabled={scanning}
              >
                <FolderSearch className="size-4" />
                Choose Font Folder
              </Button>
            </div>
          ) : (
            <div className="space-y-3">
              {fonts.map((font) => {
                const isSelected = selectedFont?.path === font.path;
                const isActive = Boolean(activePaths[font.path]);
                const cardFeatures = featuresByPath[font.path] ?? {};
                const cardVariations = variationsByPath[font.path] ?? {};

                return (
                  <article
                    key={`${font.path}::${font.postscript_name}`}
                    data-testid="skia-font-card"
                    onClick={() => setSelectedPath(font.path)}
                    className={cn(
                      "cursor-pointer rounded-lg border bg-card p-3.5 transition-colors",
                      isSelected
                        ? "border-primary ring-1 ring-primary/40"
                        : "border-border hover:border-muted-foreground/40",
                    )}
                  >
                    <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                      <div className="flex min-w-0 items-center gap-2">
                        <h3 className="truncate text-sm font-semibold">{font.family_name}</h3>
                        <span className="truncate text-xs text-muted-foreground">
                          {font.subfamily_name}
                        </span>
                        {font.is_variable ? (
                          <Badge variant="default" className="h-5 text-[10px]">
                            Variable ({font.axes.length})
                          </Badge>
                        ) : null}
                        {font.opentype_features.length > 0 ? (
                          <Badge variant="outline" className="h-5 text-[10px]">
                            {font.opentype_features.length} OT features
                          </Badge>
                        ) : null}
                      </div>

                      <div
                        className="flex items-center gap-2"
                        onClick={(e) => e.stopPropagation()}
                      >
                        <Button
                          type="button"
                          size="sm"
                          variant={isActive ? "default" : "outline"}
                          className="h-7 px-2.5 text-xs"
                          disabled={busyPath === font.path}
                          data-testid="skia-activate-toggle"
                          aria-pressed={isActive}
                          onClick={() => void toggleActivate(font)}
                        >
                          <Power className="size-3" />
                          {isActive ? "Active" : "Activate"}
                        </Button>
                      </div>
                    </div>

                    <SkiaFontCanvas
                      fontPath={font.path}
                      text={sampleText}
                      fontSize={fontSize}
                      height={Math.max(64, Math.round(fontSize * 1.9))}
                      colorHex={inkHex}
                      features={cardFeatures}
                      variations={cardVariations}
                    />
                  </article>
                );
              })}
            </div>
          )}
        </div>

        {/* Live Inspector Panel: Variable Axes & OpenType GSUB/GPOS Feature Toggles */}
        {selectedFont ? (
          <aside
            data-testid="skia-inspector"
            className="fm-scroll hidden w-80 shrink-0 flex-col gap-4 overflow-y-auto border-l border-border bg-card/50 p-4 lg:flex xl:w-96"
          >
            <div className="flex items-start justify-between gap-2">
              <div className="min-w-0">
                <h2 className="truncate font-heading text-lg">{selectedFont.family_name}</h2>
                <p className="truncate text-xs text-muted-foreground">
                  {selectedFont.subfamily_name} ·{" "}
                  <span className="font-mono">{selectedFont.postscript_name}</span>
                </p>
              </div>
              <Button
                type="button"
                size="icon-sm"
                variant="ghost"
                title="Reset axes and OpenType features"
                aria-label="Reset Skia inspector controls"
                onClick={() => resetInspectorControls(selectedFont)}
              >
                <RotateCcw className="size-3.5" />
              </Button>
            </div>

            <div className="rounded-lg border border-border bg-background p-3">
              <SkiaFontCanvas
                fontPath={selectedFont.path}
                text={sampleText}
                fontSize={Math.min(64, Math.max(24, fontSize + 6))}
                height={110}
                colorHex={inkHex}
                features={selectedFeatures}
                variations={selectedVariations}
              />
            </div>

            <section className="space-y-2.5">
              <div className="flex items-center gap-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                <Sliders className="size-3.5" />
                <span>Variable Axes ({selectedFont.axes.length})</span>
              </div>
              {selectedFont.axes.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  Static typeface — no <code className="font-mono">fvar</code> variation axes.
                </p>
              ) : (
                <div className="space-y-3 rounded-lg border border-border bg-background/60 p-3">
                  {selectedFont.axes.map((axis) => {
                    const currentVal = selectedVariations[axis.tag] ?? axis.default;
                    const step = axis.max - axis.min <= 20 ? 0.1 : 1;
                    return (
                      <div key={axis.tag} className="space-y-1.5">
                        <div className="flex items-center justify-between text-xs">
                          <span className="font-mono font-medium">{axis.tag}</span>
                          <span className="font-mono tabular-nums text-muted-foreground">
                            {Number(currentVal).toFixed(step < 1 ? 1 : 0)} ({axis.min}–{axis.max})
                          </span>
                        </div>
                        <Slider
                          min={axis.min}
                          max={axis.max}
                          step={step}
                          value={[currentVal]}
                          aria-label={`Variable axis ${axis.tag}`}
                          data-testid={`skia-axis-${axis.tag}`}
                          onValueChange={([v]) => {
                            if (typeof v === "number") setAxisValue(selectedFont.path, axis.tag, v);
                          }}
                        />
                      </div>
                    );
                  })}
                </div>
              )}
            </section>

            <section className="space-y-2.5">
              <div className="flex items-center justify-between text-xs font-medium uppercase tracking-wider text-muted-foreground">
                <span>OpenType Features ({selectedFont.opentype_features.length})</span>
              </div>
              {selectedFont.opentype_features.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  No GSUB or GPOS OpenType feature tags detected.
                </p>
              ) : (
                <div className="space-y-1.5 rounded-lg border border-border bg-background/60 p-2.5">
                  {selectedFont.opentype_features.map((tag) => {
                    const checked = selectedFeatures[tag] === 1;
                    return (
                      <label
                        key={tag}
                        className="flex cursor-pointer items-center justify-between gap-2 rounded px-1.5 py-1 text-xs hover:bg-accent/60"
                      >
                        <span className="min-w-0 truncate">
                          <span className="mr-1.5 rounded bg-secondary px-1 py-0.5 font-mono text-[10px] font-medium">
                            {tag}
                          </span>
                          <span className="text-muted-foreground">
                            {openTypeFeatureLabel(tag)}
                          </span>
                        </span>
                        <Switch
                          checked={checked}
                          data-testid={`skia-ot-${tag}`}
                          onCheckedChange={(on) => toggleFeature(selectedFont.path, tag, on)}
                        />
                      </label>
                    );
                  })}
                </div>
              )}
            </section>

            <p className="mt-auto truncate border-t border-border pt-2 font-mono text-[10px] text-muted-foreground">
              {selectedFont.path}
            </p>
          </aside>
        ) : null}
      </div>
    </div>
  );
}
