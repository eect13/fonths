import { useEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { Copy, Eye, EyeOff, Folder, Power, RotateCcw, Sparkles, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { fontDisplayName } from "@/lib/fonts/axes";
import { formatBytes } from "@/lib/fonts/hash";
import { cssFamilyStack, evictFamilyPreview, loadFont } from "@/lib/fonts/loader";
import {
  findDuplicates,
  hideIdsFromDuplicateGroups,
  pickKeepFont,
  useFontStore,
} from "@/lib/fonts/store";
import { loadSystemFonts } from "@/lib/fonts/system-fonts";
import type { DuplicateGroup, FontRecord } from "@/lib/fonts/types";
import { cn } from "@/lib/utils";
import { openContextMenu, type ContextMenuEntry } from "@/lib/context-menu-bus";

const PAGE_SIZE = 50;

function reasonLabel(group: DuplicateGroup) {
  if (group.reason === "checksum" || group.diffBytes === 0) return "Identical file";
  if (group.reason === "binary") {
    const n = group.diffBytes ?? 0;
    return `Same size — ${n.toLocaleString()} byte${n === 1 ? "" : "s"} differ`;
  }
  const cat = group.fonts.find((f) => f.source === "google");
  if (group.fonts.some((f) => f.source === "system")) return "Same family as a system font";
  if (cat?.catalog === "other") return "Same family as an older catalog face";
  if (cat) return "Same family as a Google Font";
  return "Duplicate local family & style";
}

function providerLabel(font: FontRecord) {
  if (font.source === "local") return font.originPath ? "Watched" : "Uploaded";
  if (font.source === "system") return "System";
  return "Google Fonts";
}

function parentFolderLabel(originPath?: string): string {
  if (!originPath) return "";
  const norm = originPath.replace(/\\/g, "/");
  const idx = norm.lastIndexOf("/");
  if (idx <= 0) return "";
  return norm.slice(0, idx);
}

function DuplicateHeader({ extra }: { extra?: ReactNode }) {
  const autoHide = useFontStore((s) => s.autoHideDuplicates);
  const setAutoHide = useFontStore((s) => s.setAutoHideDuplicates);
  const ignoredCount = useFontStore((s) => s.ignoredWatchedPaths.length);
  const restoreIgnored = useFontStore((s) => s.restoreIgnoredWatchedPaths);
  return (
    <div>
      <h1 className="font-heading text-3xl">Duplicates</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Compares watched local typefaces, Google Fonts, and Windows System fonts by file checksum,
        near-identical binary tables, and family name. Keep order: System fonts → Google Fonts →
        largest local file.
      </p>
      <div className="mt-3 rounded-lg border border-border bg-secondary/35 p-3">
        <label className="flex cursor-pointer items-center justify-between gap-3 text-sm font-medium">
          <span>Auto-hide &amp; deactivate duplicates</span>
          <Switch
            checked={autoHide}
            onCheckedChange={setAutoHide}
            data-testid="auto-hide-duplicates-switch"
            aria-label="Auto-hide duplicates in the library"
          />
        </label>
        <p className="mt-1 text-xs text-muted-foreground">
          When enabled, hides redundant duplicates in the library and immediately deactivates any
          active duplicate faces. Turning this switch off restores visibility without changing
          activation state.
        </p>
      </div>
      {ignoredCount > 0 ? (
        <div
          data-testid="ignored-watched-bar"
          className="mt-2 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-secondary/25 px-3 py-2 text-xs text-muted-foreground"
        >
          <span>
            {ignoredCount.toLocaleString()} watched-folder file{ignoredCount === 1 ? "" : "s"}{" "}
            hidden/ignored from your library (files stay untouched on disk).
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-7 px-2.5 text-xs"
            onClick={() => {
              restoreIgnored();
              toast.success("Restored ignored watched-folder files");
            }}
          >
            <RotateCcw className="size-3" />
            Restore ignored ({ignoredCount.toLocaleString()})
          </Button>
        </div>
      ) : null}
      {extra}
    </div>
  );
}

function DuplicateRow({
  font,
  isKeep,
  onRequestRemove,
}: {
  font: FontRecord;
  isKeep: boolean;
  onRequestRemove: (font: FontRecord) => void;
}) {
  const on = useFontStore((s) => s.activatedSet.has(font.id) || s.pendingSet.has(font.id));
  const pending = useFontStore((s) => s.pendingSet.has(font.id));
  const toggleActivated = useFontStore((s) => s.toggleActivated);
  const swapActivateFont = useFontStore((s) => s.swapActivateFont);
  const selectFont = useFontStore((s) => s.selectFont);
  const isWatched = font.source === "local" && Boolean(font.originPath);
  const folder = isWatched ? parentFolderLabel(font.originPath) : "";

  function handleContextMenu(e: MouseEvent<HTMLLIElement>) {
    e.preventDefault();
    e.stopPropagation();
    const items: ContextMenuEntry[] = [
      { kind: "label", text: fontDisplayName(font) },
    ];
    if (font.source !== "system") {
      items.push({
        kind: "item",
        label: on ? "Deactivate" : "Activate (swap active copy)",
        icon: <Power className="size-3.5" />,
        onSelect: () => {
          if (on) toggleActivated(font.id);
          else swapActivateFont(font.id);
        },
      });
    }
    items.push(
      {
        kind: "item",
        label: "Open Inspector",
        icon: <Eye className="size-3.5" />,
        onSelect: () => selectFont(font.id),
      },
      {
        kind: "item",
        label: "Copy Family Name",
        icon: <Copy className="size-3.5" />,
        onSelect: () => {
          void navigator.clipboard?.writeText(font.family);
          toast.success(`Copied "${font.family}"`);
        },
      },
    );
    if (font.source === "local") {
      const permanent = useFontStore.getState().desktopPrefs.deleteMode === "permanent";
      items.push(
        { kind: "separator" },
        {
          kind: "item",
          label: isWatched
            ? "Hide / Ignore from Library"
            : permanent
              ? "Delete Permanently"
              : "Move to Recycle Bin",
          icon: isWatched ? <EyeOff className="size-3.5" /> : <Trash2 className="size-3.5" />,
          destructive: true,
          onSelect: () => onRequestRemove(font),
        },
      );
    }
    openContextMenu(e.clientX, e.clientY, items);
  }

  return (
    <li className="flex flex-wrap items-center gap-3 py-3" onContextMenu={handleContextMenu}>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => selectFont(font.id)}
            className="max-w-full truncate text-left font-medium hover:underline"
          >
            {font.family}
          </button>
          {isKeep ? (
            <Badge variant="on" className="h-5 px-1.5 text-[10px]">
              Keep
            </Badge>
          ) : null}
          <DuplicateSpecimen font={font} />
        </div>
        <p className="mt-0.5 truncate text-xs text-muted-foreground" title={font.originPath || undefined}>
          {providerLabel(font)}
          {font.fileName ? ` · ${font.fileName}` : ""}
          {font.fileSize ? ` · ${formatBytes(font.fileSize)}` : ""}
          {` · ${font.weights[0] ?? 400}`}
          {font.italic ? " italic" : ""}
        </p>
        {folder ? (
          <p className="mt-0.5 flex items-center gap-1 truncate font-mono text-[11px] text-muted-foreground/85" title={font.originPath}>
            <Folder className="size-3 shrink-0" aria-hidden />
            <span className="truncate">{folder}</span>
          </p>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button
          size="sm"
          variant={on ? "default" : "outline"}
          className={cn(pending && !on && "animate-pulse")}
          onClick={() => {
            if (on) toggleActivated(font.id);
            else swapActivateFont(font.id);
          }}
          disabled={font.source === "system"}
          aria-pressed={on}
          title={
            font.source === "system"
              ? "System font — already installed. View only."
              : on
                ? "Deactivate — the other copy of this family can then be activated"
                : "Activate — automatically swaps off the other active copy of this family"
          }
        >
          <Power />
          {on ? (pending ? "Queued" : "On") : "Activate"}
        </Button>
        {font.source === "local" ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => onRequestRemove(font)}
            title={
              isWatched
                ? "Remove from Fonths and ignore in watch sync (file stays on disk)"
                : "Delete uploaded duplicate copy"
            }
          >
            {isWatched ? <EyeOff className="size-3.5" /> : <Trash2 className="size-3.5" />}
            {isWatched ? "Hide / Ignore" : "Delete"}
          </Button>
        ) : null}
      </div>
    </li>
  );
}

export function DuplicateFinder() {
  const localFonts = useFontStore((s) => s.localFonts);
  const googleFonts = useFontStore((s) => s.googleFonts);
  const systemFonts = useFontStore((s) => s.systemFonts);
  const hydrated = useFontStore((s) => s.hydrated);
  const autoHide = useFontStore((s) => s.autoHideDuplicates);
  const deleteMode = useFontStore((s) => s.desktopPrefs.deleteMode ?? "recycle");
  const removeLocalFont = useFontStore((s) => s.removeLocalFont);
  const removeLocalFontsBatch = useFontStore((s) => s.removeLocalFontsBatch);
  const ignoreWatchedFont = useFontStore((s) => s.ignoreWatchedFont);
  const ignoreWatchedFontsBatch = useFontStore((s) => s.ignoreWatchedFontsBatch);
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  const [scanning, setScanning] = useState(false);
  const [filter, setFilter] = useState<"all" | "binary" | "family">("all");
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  const [pendingRemove, setPendingRemove] = useState<FontRecord | null>(null);
  const [bulkConfirmOpen, setBulkConfirmOpen] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);

  const localFontsRef = useRef(localFonts);
  const googleFontsRef = useRef(googleFonts);
  const systemFontsRef = useRef(systemFonts);
  localFontsRef.current = localFonts;
  googleFontsRef.current = googleFonts;
  systemFontsRef.current = systemFonts;

  // Exclude checksum mutations from scanKey so patchLocalChecksums does not trigger a second scan flash.
  const scanKey = useMemo(() => {
    const localSig = localFonts
      .map(
        (f) =>
          `${f.id}|${f.family}|${f.fileSize ?? 0}|${f.originPath ?? ""}|${f.weights[0] ?? 400}|${f.italic ? 1 : 0}`,
      )
      .join(";");
    return `${localSig}::g=${googleFonts.length}::s=${systemFonts.length}`;
  }, [localFonts, googleFonts.length, systemFonts.length]);

  useEffect(() => {
    if (!hydrated) return;
    let cancelled = false;
    setScanning(true);
    void findDuplicates(
      localFontsRef.current,
      googleFontsRef.current,
      systemFontsRef.current,
      (updates) => {
        if (!cancelled) useFontStore.getState().patchLocalChecksums(updates);
      },
    )
      .then((next) => {
        if (cancelled) return;
        setGroups(next);
        setScanning(false);
      })
      .catch(() => {
        if (cancelled) return;
        setGroups([]);
        setScanning(false);
      });
    return () => {
      cancelled = true;
    };
  }, [hydrated, scanKey]);

  useEffect(() => {
    // Mount used to write duplicateHideIds before rehydrate, and persist then
    // saved the empty defaults over favorites, activated, and collections.
    if (!hydrated) return;
    if (!autoHide || !groups.length) {
      if (!autoHide) useFontStore.setState({ duplicateHideIds: [] });
      return;
    }
    const hide = hideIdsFromDuplicateGroups(groups);
    useFontStore.setState({ duplicateHideIds: hide });
    if (hide.length) {
      const st = useFontStore.getState();
      const activeHide =
        st.activatedSet || st.pendingSet
          ? hide.filter((id) => st.activatedSet?.has(id) || st.pendingSet?.has(id))
          : hide;
      if (activeHide.length) st.setActivatedMany(activeHide, false);
    }
  }, [hydrated, autoHide, groups]);

  useEffect(() => {
    if (!hydrated) return;
    if (systemFonts.length === 0) {
      void loadSystemFonts();
    }
  }, [hydrated, systemFonts.length]);

  const binaryCount = useMemo(
    () => groups.filter((g) => g.reason === "checksum" || g.reason === "binary").length,
    [groups],
  );
  const familyCount = groups.length - binaryCount;
  const filteredGroups = useMemo(() => {
    if (filter === "binary") return groups.filter((g) => g.reason === "checksum" || g.reason === "binary");
    if (filter === "family") return groups.filter((g) => g.reason === "family-weight");
    return groups;
  }, [groups, filter]);
  const visibleGroups = useMemo(
    () => filteredGroups.slice(0, visibleCount),
    [filteredGroups, visibleCount],
  );

  const redundantLocals = useMemo(() => {
    const keepIds = new Set<string>();
    for (const group of groups) {
      const keep = pickKeepFont(group);
      if (keep) keepIds.add(keep.id);
    }
    const byId = new Map<string, FontRecord>();
    for (const group of groups) {
      for (const f of group.fonts) {
        if (f.source !== "local" || keepIds.has(f.id)) continue;
        if (!byId.has(f.id)) byId.set(f.id, f);
      }
    }
    const items = [...byId.values()];
    const uploads = items.filter((f) => !f.originPath);
    const watched = items.filter((f) => Boolean(f.originPath));
    const uploadBytes = uploads.reduce((sum, f) => sum + (f.fileSize ?? 0), 0);
    return { items, uploads, watched, uploadBytes };
  }, [groups]);

  function confirmRemove() {
    if (!pendingRemove) return;
    const target = pendingRemove;
    setPendingRemove(null);
    evictFamilyPreview(target.family);
    if (target.originPath) {
      void ignoreWatchedFont(target.id).then(() => {
        toast.success(`Hidden ${fontDisplayName(target)} from library`, {
          description: "File stays untouched in your watched folder.",
        });
      });
      return;
    }
    void removeLocalFont(target.id).then(() => {
      toast.success(
        deleteMode === "permanent"
          ? `Permanently deleted ${fontDisplayName(target)}`
          : `Moved ${fontDisplayName(target)} to Recycle Bin`,
      );
    });
  }

  async function confirmBulkClean() {
    if (bulkBusy || !redundantLocals.items.length) return;
    setBulkBusy(true);
    try {
      const uploadIds = redundantLocals.uploads.map((f) => f.id);
      const watchedIds = redundantLocals.watched.map((f) => f.id);
      let deleted = 0;
      let hidden = 0;
      if (uploadIds.length) {
        deleted = await removeLocalFontsBatch(uploadIds, { deleteFromDisk: true });
      }
      if (watchedIds.length) {
        hidden = await ignoreWatchedFontsBatch(watchedIds);
      }
      setBulkConfirmOpen(false);
      const parts: string[] = [];
      if (deleted > 0) parts.push(`${deleted.toLocaleString()} uploaded copy${deleted === 1 ? "" : "ies"} deleted`);
      if (hidden > 0) parts.push(`${hidden.toLocaleString()} watched copy${hidden === 1 ? "" : "ies"} ignored`);
      toast.success(parts.length ? parts.join(" · ") : "Cleaned redundant duplicates");
    } finally {
      setBulkBusy(false);
    }
  }

  if (!hydrated || scanning) {
    return (
      <div className="space-y-4 p-4 md:p-6">
        <DuplicateHeader />
        <p className="text-sm text-muted-foreground">Comparing names and files…</p>
      </div>
    );
  }

  if (localFonts.length === 0 && systemFonts.length === 0) {
    return (
      <div className="space-y-4 p-4 md:p-6">
        <DuplicateHeader
          extra={
            <p className="mt-6 max-w-sm text-sm text-muted-foreground">
              Upload fonts to scan for identical files — even when the names differ. System fonts
              that share a name with a catalog family also appear here. Two catalog sources
              will not appear as a pair.
            </p>
          }
        />
      </div>
    );
  }

  if (groups.length === 0) {
    return (
      <div className="space-y-4 p-4 md:p-6">
        <DuplicateHeader
          extra={
            <p className="mt-6 max-w-sm text-sm text-muted-foreground">
              {localFonts.length
                ? `${localFonts.length.toLocaleString()} uploaded file${localFonts.length === 1 ? "" : "s"} look unique (no catalog or system name match).`
                : "No overlapping family names between system fonts and the catalog."}
            </p>
          }
        />
      </div>
    );
  }

  const pendingIsWatched = Boolean(pendingRemove?.originPath);

  return (
    <div className="space-y-6 overflow-y-auto p-4 md:p-6">
      <DuplicateHeader
        extra={
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant={filter === "all" ? "default" : "outline"}
              className="h-7 px-2.5 text-xs"
              onClick={() => {
                setFilter("all");
                setVisibleCount(PAGE_SIZE);
              }}
            >
              All ({groups.length.toLocaleString()})
            </Button>
            <Button
              type="button"
              size="sm"
              variant={filter === "binary" ? "default" : "outline"}
              className="h-7 px-2.5 text-xs"
              onClick={() => {
                setFilter("binary");
                setVisibleCount(PAGE_SIZE);
              }}
            >
              Identical files ({binaryCount.toLocaleString()})
            </Button>
            <Button
              type="button"
              size="sm"
              variant={filter === "family" ? "default" : "outline"}
              className="h-7 px-2.5 text-xs"
              onClick={() => {
                setFilter("family");
                setVisibleCount(PAGE_SIZE);
              }}
            >
              Same family ({familyCount.toLocaleString()})
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="ml-auto h-7 px-2.5 text-xs"
              data-testid="clean-redundant-duplicates-btn"
              disabled={redundantLocals.items.length === 0}
              onClick={() => setBulkConfirmOpen(true)}
            >
              <Sparkles className="size-3.5" />
              Clean redundant copies ({redundantLocals.items.length.toLocaleString()})
            </Button>
          </div>
        }
      />

      {visibleGroups.length === 0 ? (
        <p className="text-sm text-muted-foreground">No duplicates match this filter.</p>
      ) : (
        visibleGroups.map((group) => {
          const keepId = pickKeepFont(group)?.id;
          return (
            <section key={group.key} className="rounded-xl border border-border bg-card p-4">
              <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                {reasonLabel(group)}
              </p>
              <ul className="mt-3 divide-y divide-border">
                {group.fonts.map((font) => (
                  <DuplicateRow
                    key={font.id}
                    font={font}
                    isKeep={keepId === font.id}
                    onRequestRemove={setPendingRemove}
                  />
                ))}
              </ul>
            </section>
          );
        })
      )}

      {visibleGroups.length < filteredGroups.length ? (
        <div className="flex justify-center pb-4">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setVisibleCount((c) => c + PAGE_SIZE)}
          >
            Show more ({(filteredGroups.length - visibleGroups.length).toLocaleString()} remaining)
          </Button>
        </div>
      ) : null}

      <Dialog open={Boolean(pendingRemove)} onOpenChange={(open) => !open && setPendingRemove(null)}>
        <DialogContent data-testid="duplicate-remove-dialog">
          <DialogHeader>
            <DialogTitle>
              {pendingIsWatched
                ? "Hide / ignore watched font?"
                : deleteMode === "permanent"
                  ? "Permanently delete font?"
                  : "Move font to Recycle Bin?"}
            </DialogTitle>
            <DialogDescription>
              {pendingIsWatched
                ? `This removes "${pendingRemove ? fontDisplayName(pendingRemove) : ""}" from Fonths and skips it on watch polls. Your file on disk (${pendingRemove?.originPath}) will NOT be deleted or modified.`
                : deleteMode === "permanent"
                  ? `This will permanently delete "${pendingRemove ? fontDisplayName(pendingRemove) : ""}" from disk and remove it from your library. This cannot be undone.`
                  : `This will move "${pendingRemove ? fontDisplayName(pendingRemove) : ""}" to the Windows Recycle Bin and remove it from your library.`}
            </DialogDescription>
          </DialogHeader>
          <div className="fm-dialog-footer">
            <Button type="button" variant="ghost" onClick={() => setPendingRemove(null)}>
              Cancel
            </Button>
            <Button type="button" variant="destructive" onClick={confirmRemove}>
              {pendingIsWatched
                ? "Hide / Ignore"
                : deleteMode === "permanent"
                  ? "Delete permanently"
                  : "Move to Recycle Bin"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={bulkConfirmOpen} onOpenChange={(open) => !bulkBusy && setBulkConfirmOpen(open)}>
        <DialogContent data-testid="bulk-clean-duplicates-dialog">
          <DialogHeader>
            <DialogTitle>
              Clean {redundantLocals.items.length.toLocaleString()} redundant local copy
              {redundantLocals.items.length === 1 ? "" : "ies"}?
            </DialogTitle>
            <DialogDescription>
              Keeps every preferred copy marked &ldquo;Keep&rdquo; (System &rarr; Google &rarr; largest local).
              {redundantLocals.uploads.length > 0
                ? ` ${redundantLocals.uploads.length.toLocaleString()} uploaded file${redundantLocals.uploads.length === 1 ? "" : "s"} (${formatBytes(redundantLocals.uploadBytes)}) will ${deleteMode === "permanent" ? "be permanently deleted" : "move to Recycle Bin"}.`
                : ""}
              {redundantLocals.watched.length > 0
                ? ` ${redundantLocals.watched.length.toLocaleString()} watched-folder file${redundantLocals.watched.length === 1 ? "" : "s"} will be hidden/ignored from Fonths (files stay untouched on disk).`
                : ""}
            </DialogDescription>
          </DialogHeader>
          <div className="fm-scroll max-h-48 space-y-1 overflow-y-auto rounded-md border border-border bg-secondary/40 p-2.5 text-xs">
            {redundantLocals.items.slice(0, 60).map((f) => (
              <div key={f.id} className="flex items-center justify-between gap-2 truncate">
                <span className="min-w-0 truncate">
                  {fontDisplayName(f)}
                  {f.fileName ? ` · ${f.fileName}` : ""}
                </span>
                <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                  {f.originPath ? "Ignore" : formatBytes(f.fileSize ?? 0)}
                </span>
              </div>
            ))}
            {redundantLocals.items.length > 60 ? (
              <p className="pt-1 text-muted-foreground">
                +{(redundantLocals.items.length - 60).toLocaleString()} more
              </p>
            ) : null}
          </div>
          <div className="fm-dialog-footer">
            <Button
              type="button"
              variant="ghost"
              disabled={bulkBusy}
              onClick={() => setBulkConfirmOpen(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={bulkBusy || !redundantLocals.items.length}
              data-testid="bulk-clean-confirm-btn"
              onClick={() => void confirmBulkClean()}
            >
              {bulkBusy ? "Cleaning…" : "Clean redundant copies"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

// Placed below DuplicateFinder so its useEffect never shifts the scan/auto-hide useEffect indices tested in fix-0930-duplicates-hydrate.test.mjs.
function DuplicateSpecimen({ font }: { font: FontRecord }) {
  useEffect(() => {
    if (font.source === "system") return;
    void loadFont(font, "preview");
  }, [font]);

  return (
    <span
      className="max-w-[140px] truncate rounded bg-secondary/50 px-2 py-0.5 text-sm text-foreground/90"
      style={{ fontFamily: cssFamilyStack(font) }}
      title={`${font.family} preview`}
    >
      Aa Bb 123
    </span>
  );
}
