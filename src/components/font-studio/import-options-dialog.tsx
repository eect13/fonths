import { useEffect, useMemo, useState } from "react";
import { FolderSearch, Layers, Sparkles, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  defaultImportSelection,
  filterImportFiles,
  loadSavedImportRule,
  resolveImportOptions,
  selectionFromSavedRule,
  subscribeImportOptions,
  type FontFileFormat,
  type ImportOptionsRequest,
  type ImportOptionsSelection,
  type VarStaticMode,
} from "@/lib/fonts/import-options";
import { cn } from "@/lib/utils";

const FORMAT_LABELS: Record<FontFileFormat, { label: string; badge?: string }> = {
  ttf: { label: "TTF" },
  otf: { label: "OTF" },
  ttc: { label: "TTC / OTC" },
  woff2: { label: "WOFF2", badge: "web" },
  woff: { label: "WOFF", badge: "web" },
};

export function ImportOptionsDialog() {
  const [req, setReq] = useState<ImportOptionsRequest | null>(null);
  const [selection, setSelection] = useState<ImportOptionsSelection | null>(null);

  useEffect(() => {
    return subscribeImportOptions((next) => {
      setReq(next);
      setSelection(
        next ? selectionFromSavedRule(next.analysis, loadSavedImportRule()) : null,
      );
    });
  }, []);

  const analysis = req?.analysis ?? null;

  const preview = useMemo(() => {
    if (!analysis || !selection) {
      return { selectedCount: 0, skippedCount: 0, replaceCount: 0 };
    }
    const stubFiles = analysis.files.map((f) => ({
      name: f.name,
      size: f.size,
      webkitRelativePath: f.relativePath,
    }));
    const res = filterImportFiles(stubFiles, analysis, selection);
    return {
      selectedCount: res.files.length,
      skippedCount: res.skippedCount,
      replaceCount: res.replaceExistingLocalIds.length,
    };
  }, [analysis, selection]);

  const open = Boolean(req && analysis && selection);
  if (!open || !req || !analysis || !selection) {
    return (
      <Dialog open={false} onOpenChange={() => undefined}>
        <DialogContent />
      </Dialog>
    );
  }

  const isWatch = req.mode === "watch";
  const hasConflicts = analysis.varStaticConflicts.length > 0;
  const hasCrossLibrary = analysis.varStaticConflicts.some(
    (c) => c.existingVariableIds.length > 0 || c.existingStaticIds.length > 0,
  );
  const anyBatchHasVariable = analysis.varStaticConflicts.some((c) => c.batchVariableCount > 0);
  const sizeKb = Math.max(1, Math.round(analysis.totalBytes / 1024));

  const setVarStaticMode = (mode: VarStaticMode) => {
    setSelection((prev) => (prev ? { ...prev, varStaticMode: mode } : prev));
  };

  const toggleFormat = (fmt: FontFileFormat) => {
    setSelection((prev) =>
      prev
        ? { ...prev, formats: { ...prev.formats, [fmt]: !prev.formats[fmt] } }
        : prev,
    );
  };

  const applySmartRecommended = () => {
    setSelection((prev) => {
      const smart = defaultImportSelection(analysis);
      return prev ? { ...smart, saveAsDefault: prev.saveAsDefault } : smart;
    });
  };

  const applyDesktopOnly = () => {
    setSelection((prev) =>
      prev
        ? {
            ...prev,
            includeVariable: true,
            includeStatic: true,
            includeItalic: true,
            formats: {
              ttf: true,
              otf: true,
              ttc: true,
              woff2: false,
              woff: false,
            },
          }
        : prev,
    );
  };

  const applySelectAll = () => {
    setSelection((prev) =>
      prev
        ? {
            ...prev,
            varStaticMode: "both",
            includeVariable: true,
            includeStatic: true,
            includeItalic: true,
            formats: {
              ttf: true,
              otf: true,
              ttc: true,
              woff2: true,
              woff: true,
            },
          }
        : prev,
    );
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) resolveImportOptions(null);
      }}
    >
      <DialogContent className="max-w-lg" data-testid="import-options-dialog">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Sparkles className="size-4 text-primary" />
            {isWatch ? "Watch folder import rules" : "Import local fonts"}
          </DialogTitle>
          <DialogDescription>
            {analysis.totalFiles.toLocaleString()} font file
            {analysis.totalFiles === 1 ? "" : "s"} across {analysis.familyCount.toLocaleString()}{" "}
            {analysis.familyCount === 1 ? "family" : "families"}
            {analysis.totalBytes > 0 ? ` (${sizeKb.toLocaleString()} KB)` : ""}.
            {req.folderLabel ? ` Source: ${req.folderLabel}.` : ""}
          </DialogDescription>
        </DialogHeader>

        {/* Clear differentiation between Watch Folder (live link in place) and Upload (copy to library) */}
        <div
          data-testid="import-mode-banner"
          className="flex items-start gap-2.5 rounded-lg border border-border bg-secondary/50 px-3 py-2 text-xs"
        >
          {isWatch ? (
            <FolderSearch className="mt-0.5 size-4 shrink-0 text-primary" />
          ) : (
            <Upload className="mt-0.5 size-4 shrink-0 text-primary" />
          )}
          <div className="min-w-0">
            <div className="font-semibold text-foreground">
              {isWatch
                ? "Watch Folder (Live Link — files stay in place)"
                : "Upload Copy → Documents → Fonths → Uploads"}
            </div>
            <div className="text-[11px] text-muted-foreground">
              {isWatch
                ? "Nothing is copied. Future fonts added to this folder will automatically follow these rules."
                : "Copies selected files into your Fonths library. Does not watch the source folder for future changes."}
            </div>
          </div>
        </div>

        <div className="max-h-[54vh] space-y-3.5 overflow-y-auto pr-1">
          {hasConflicts ? (
            <section className="space-y-2 rounded-lg border border-border bg-secondary/40 p-3">
              <div className="flex items-center gap-1.5 text-xs font-semibold text-foreground">
                <Layers className="size-3.5 text-primary" />
                Variable &amp; Static files detected for the same family
              </div>
              <div className="max-h-24 space-y-1 overflow-y-auto text-[11px] text-muted-foreground">
                {analysis.varStaticConflicts.slice(0, 8).map((c) => {
                  const parts: string[] = [];
                  if (c.batchVariableCount) parts.push(`${c.batchVariableCount} variable`);
                  if (c.batchStaticCount) parts.push(`${c.batchStaticCount} static`);
                  if (c.existingVariableIds.length) {
                    parts.push(`${c.existingVariableIds.length} variable already in library`);
                  }
                  if (c.existingStaticIds.length) {
                    parts.push(`${c.existingStaticIds.length} static already in library`);
                  }
                  return (
                    <div key={c.familyKey} className="flex items-baseline justify-between gap-2">
                      <span className="truncate font-medium text-foreground">{c.familyLabel}</span>
                      <span className="shrink-0">{parts.join(" · ")}</span>
                    </div>
                  );
                })}
                {analysis.varStaticConflicts.length > 8 ? (
                  <div>+{analysis.varStaticConflicts.length - 8} more families</div>
                ) : null}
              </div>

              <div className="grid grid-cols-1 gap-1.5 pt-1 sm:grid-cols-3">
                {(
                  [
                    {
                      mode: "variable" as const,
                      title: anyBatchHasVariable ? "Variable only" : "Keep Variable",
                      hint: anyBatchHasVariable
                        ? "Recommended · 1 flexible file"
                        : "Skip incoming static",
                    },
                    {
                      mode: "static" as const,
                      title: "Static only",
                      hint: "Fixed-weight files only",
                    },
                    {
                      mode: "both" as const,
                      title: "Both",
                      hint: "Load variable & static",
                    },
                  ]
                ).map((opt) => {
                  const active = selection.varStaticMode === opt.mode;
                  return (
                    <button
                      key={opt.mode}
                      type="button"
                      data-testid={`import-var-mode-${opt.mode}`}
                      onClick={() => setVarStaticMode(opt.mode)}
                      className={cn(
                        "rounded-md border px-2.5 py-2 text-left transition-colors",
                        active
                          ? "border-primary bg-primary/10 text-foreground"
                          : "border-border bg-background/60 text-muted-foreground hover:border-foreground/30 hover:text-foreground",
                      )}
                    >
                      <div className="text-xs font-semibold leading-tight">{opt.title}</div>
                      <div className="mt-0.5 text-[10px] leading-tight opacity-80">{opt.hint}</div>
                    </button>
                  );
                })}
              </div>

              {hasCrossLibrary && !isWatch && selection.varStaticMode !== "both" ? (
                <label className="flex cursor-pointer items-center gap-2 pt-1 text-xs text-foreground">
                  <input
                    type="checkbox"
                    className="size-3.5 accent-primary"
                    checked={selection.replaceExistingOpposite}
                    onChange={(e) =>
                      setSelection((prev) =>
                        prev ? { ...prev, replaceExistingOpposite: e.target.checked } : prev,
                      )
                    }
                  />
                  <span>
                    Replace existing{" "}
                    {selection.varStaticMode === "variable" ? "static" : "variable"} faces in
                    library
                    {preview.replaceCount > 0 ? ` (${preview.replaceCount})` : ""}
                  </span>
                </label>
              ) : null}
            </section>
          ) : null}

          <section className="space-y-2.5 rounded-lg border border-border p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-semibold text-foreground">What to import</span>
              <div className="flex items-center gap-2.5 text-[11px]">
                <button
                  type="button"
                  className="text-primary underline-offset-2 hover:underline"
                  onClick={applySmartRecommended}
                >
                  Smart default
                </button>
                {analysis.hasDesktop && analysis.hasWebOnly ? (
                  <button
                    type="button"
                    className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                    onClick={applyDesktopOnly}
                  >
                    Desktop only
                  </button>
                ) : null}
                <button
                  type="button"
                  className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                  onClick={applySelectAll}
                >
                  Select all
                </button>
              </div>
            </div>

            <div className="space-y-1.5">
              <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                Font kinds
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-1.5 text-xs">
                {analysis.variableCount > 0 ? (
                  <label className="flex cursor-pointer items-center gap-1.5 text-foreground">
                    <input
                      type="checkbox"
                      data-testid="import-kind-variable"
                      className="size-3.5 accent-primary"
                      checked={selection.includeVariable}
                      onChange={(e) =>
                        setSelection((prev) =>
                          prev ? { ...prev, includeVariable: e.target.checked } : prev,
                        )
                      }
                    />
                    <span>Variable ({analysis.variableCount})</span>
                  </label>
                ) : null}
                {analysis.staticCount > 0 ? (
                  <label className="flex cursor-pointer items-center gap-1.5 text-foreground">
                    <input
                      type="checkbox"
                      data-testid="import-kind-static"
                      className="size-3.5 accent-primary"
                      checked={selection.includeStatic}
                      onChange={(e) =>
                        setSelection((prev) =>
                          prev ? { ...prev, includeStatic: e.target.checked } : prev,
                        )
                      }
                    />
                    <span>Static ({analysis.staticCount})</span>
                  </label>
                ) : null}
                {analysis.italicCount > 0 ? (
                  <label className="flex cursor-pointer items-center gap-1.5 text-foreground">
                    <input
                      type="checkbox"
                      data-testid="import-kind-italic"
                      className="size-3.5 accent-primary"
                      checked={selection.includeItalic}
                      onChange={(e) =>
                        setSelection((prev) =>
                          prev ? { ...prev, includeItalic: e.target.checked } : prev,
                        )
                      }
                    />
                    <span>Italic / Oblique ({analysis.italicCount})</span>
                  </label>
                ) : null}
              </div>
            </div>

            <div className="space-y-1.5 border-t border-border/60 pt-2">
              <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                File formats
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-1.5 text-xs">
                {analysis.presentFormats.map((fmt) => {
                  const meta = FORMAT_LABELS[fmt];
                  return (
                    <label
                      key={fmt}
                      className="flex cursor-pointer items-center gap-1.5 text-foreground"
                    >
                      <input
                        type="checkbox"
                        data-testid={`import-fmt-${fmt}`}
                        className="size-3.5 accent-primary"
                        checked={selection.formats[fmt]}
                        onChange={() => toggleFormat(fmt)}
                      />
                      <span>
                        {meta.label} ({analysis.formats[fmt]})
                      </span>
                      {meta.badge ? (
                        <span className="rounded bg-secondary px-1 py-0.5 text-[10px] text-muted-foreground">
                          {meta.badge}
                        </span>
                      ) : null}
                    </label>
                  );
                })}
              </div>
              {analysis.hasDesktop && analysis.hasWebOnly ? (
                <p className="text-[11px] text-muted-foreground">
                  Web-only WOFF/WOFF2 formats are unchecked by default because desktop TTF/OTF files
                  are present.
                </p>
              ) : null}
            </div>

            {preview.skippedCount > 0 ? (
              <div className="rounded bg-secondary/60 px-2.5 py-1.5 text-[11px] text-muted-foreground">
                Keeping <strong className="text-foreground">{preview.selectedCount.toLocaleString()}</strong>{" "}
                · Skipping <strong className="text-foreground">{preview.skippedCount.toLocaleString()}</strong>{" "}
                file{preview.skippedCount === 1 ? "" : "s"} based on your filters.
              </div>
            ) : null}
          </section>

          <div className="space-y-1.5 px-1 text-xs">
            <label className="flex cursor-pointer items-center gap-2 text-foreground">
              <input
                type="checkbox"
                data-testid="import-auto-activate"
                className="size-3.5 accent-primary"
                checked={selection.autoActivate}
                onChange={(e) =>
                  setSelection((prev) =>
                    prev ? { ...prev, autoActivate: e.target.checked } : prev,
                  )
                }
              />
              <span>
                {isWatch
                  ? "Auto-activate fonts in this watched folder"
                  : "Auto-activate imported fonts right away"}
              </span>
            </label>
            <label className="flex cursor-pointer items-center gap-2 text-muted-foreground">
              <input
                type="checkbox"
                data-testid="import-save-default"
                className="size-3.5 accent-primary"
                checked={Boolean(selection.saveAsDefault)}
                onChange={(e) =>
                  setSelection((prev) =>
                    prev ? { ...prev, saveAsDefault: e.target.checked } : prev,
                  )
                }
              />
              <span>Remember these smart choices as my default</span>
            </label>
          </div>
        </div>

        <div className="fm-dialog-footer mt-4">
          <Button type="button" variant="ghost" onClick={() => resolveImportOptions(null)}>
            Cancel
          </Button>
          <Button
            type="button"
            data-testid="import-options-confirm"
            disabled={preview.selectedCount === 0}
            onClick={() => resolveImportOptions(selection)}
          >
            {isWatch ? "Watch & link" : "Import"} {preview.selectedCount.toLocaleString()} of{" "}
            {analysis.totalFiles.toLocaleString()} file
            {analysis.totalFiles === 1 ? "" : "s"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
