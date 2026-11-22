import { useEffect, useMemo, useState, type MouseEvent, type ReactNode } from "react";
import { Copy, Eye, Power, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { formatBytes } from "@/lib/fonts/hash";
import { evictFamilyPreview } from "@/lib/fonts/loader";
import { findDuplicates, hideIdsFromDuplicateGroups, useFontStore } from "@/lib/fonts/store";
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
  if (font.source === "local") return "Local file";
  if (font.source === "system") return "System";
  return "Google Fonts";
}

function DuplicateHeader({ extra }: { extra?: ReactNode }) {
  const autoHide = useFontStore((s) => s.autoHideDuplicates);
  const setAutoHide = useFontStore((s) => s.setAutoHideDuplicates);
  return (
    <div>
      <h1 className="font-heading text-3xl">Duplicates</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Google Fonts is the catalog. This page lists an upload next to that catalog family, or two
        uploads of the same file. Same size is byte-compared; a tiny patch still counts. Keep order:
        system fonts, then Google Fonts, then uploads. System fonts cannot be deleted or deactivated.
      </p>
      <label className="mt-3 flex items-center gap-2 text-sm">
        <Switch checked={autoHide} onCheckedChange={setAutoHide} />
        Auto-hide duplicates in the library
      </label>
      <p className="mt-1 text-xs text-muted-foreground">
        Keeps the system font when present, else the catalog family (or the larger
        file), hides extra uploads, and deactivates them. Activate on a catalog face
        unloads a local of that name, and the other way around. Files stay on disk.
      </p>
      {extra}
    </div>
  );
}

function DuplicateRow({
  font,
  onRequestDelete,
}: {
  font: FontRecord;
  onRequestDelete: (font: FontRecord) => void;
}) {
  const on = useFontStore((s) => s.activatedSet.has(font.id) || s.pendingSet.has(font.id));
  const pending = useFontStore((s) => s.pendingSet.has(font.id));
  const toggleActivated = useFontStore((s) => s.toggleActivated);
  const selectFont = useFontStore((s) => s.selectFont);

  function handleContextMenu(e: MouseEvent<HTMLLIElement>) {
    e.preventDefault();
    e.stopPropagation();
    const items: ContextMenuEntry[] = [
      { kind: "label", text: font.fullName || font.family },
    ];
    if (font.source !== "system") {
      items.push({
        kind: "item",
        label: on ? "Deactivate" : "Activate",
        icon: <Power className="size-3.5" />,
        onSelect: () => toggleActivated(font.id),
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
          label: permanent ? "Delete Permanently" : "Move to Recycle Bin",
          icon: <Trash2 className="size-3.5" />,
          destructive: true,
          onSelect: () => onRequestDelete(font),
        },
      );
    }
    openContextMenu(e.clientX, e.clientY, items);
  }

  return (
    <li className="flex items-center gap-3 py-3" onContextMenu={handleContextMenu}>
      <div className="min-w-0 flex-1">
        <button
          type="button"
          onClick={() => selectFont(font.id)}
          className="block max-w-full truncate text-left font-medium hover:underline"
        >
          {font.family}
        </button>
        <p className="truncate text-xs text-muted-foreground">
          {providerLabel(font)}
          {font.fileName ? ` · ${font.fileName}` : ""}
          {font.fileSize ? ` · ${formatBytes(font.fileSize)}` : ""}
          {` · ${font.weights[0] ?? 400}`}
          {font.italic ? " italic" : ""}
        </p>
      </div>
      <Button
        size="sm"
        variant={on ? "default" : "outline"}
        className={cn(pending && !on && "animate-pulse")}
        onClick={() => toggleActivated(font.id)}
        disabled={font.source === "system"}
        aria-pressed={on}
        title={
          font.source === "system"
            ? "System font — already installed. View only."
            : on
              ? "Deactivate — the other copy of this family can then be activated"
              : "Activate — unloads the other copy of this family name"
        }
      >
        <Power />
        {on ? (pending ? "Queued" : "On") : "Activate"}
      </Button>
      {font.source === "local" ? (
        <Button size="sm" variant="ghost" onClick={() => onRequestDelete(font)}>
          <Trash2 />
          Delete
        </Button>
      ) : null}
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
  const [groups, setGroups] = useState<DuplicateGroup[]>([]);
  const [scanning, setScanning] = useState(false);
  const [filter, setFilter] = useState<"all" | "binary" | "family">("all");
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  const [pendingDelete, setPendingDelete] = useState<FontRecord | null>(null);

  useEffect(() => {
    if (!hydrated) return;
    let cancelled = false;
    setScanning(true);
    void findDuplicates(localFonts, googleFonts, systemFonts)
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
  }, [hydrated, localFonts, googleFonts, systemFonts]);

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
    if (hide.length) useFontStore.getState().setActivatedMany(hide, false);
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

  function confirmDelete() {
    if (!pendingDelete) return;
    const target = pendingDelete;
    setPendingDelete(null);
    evictFamilyPreview(target.family);
    void removeLocalFont(target.id).then(() => {
      toast.success(
        deleteMode === "permanent"
          ? `Permanently deleted ${target.fullName || target.family}`
          : `Moved ${target.fullName || target.family} to Recycle Bin`,
      );
    });
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
          </div>
        }
      />

      {visibleGroups.length === 0 ? (
        <p className="text-sm text-muted-foreground">No duplicates match this filter.</p>
      ) : (
        visibleGroups.map((group) => (
          <section key={group.key} className="rounded-xl border border-border bg-card p-4">
            <p className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              {reasonLabel(group)}
            </p>
            <ul className="mt-3 divide-y divide-border">
              {group.fonts.map((font) => (
                <DuplicateRow key={font.id} font={font} onRequestDelete={setPendingDelete} />
              ))}
            </ul>
          </section>
        ))
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

      <Dialog open={Boolean(pendingDelete)} onOpenChange={(open) => !open && setPendingDelete(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {deleteMode === "permanent" ? "Permanently delete font?" : "Move font to Recycle Bin?"}
            </DialogTitle>
            <DialogDescription>
              {deleteMode === "permanent"
                ? `This will permanently delete "${pendingDelete?.fullName || pendingDelete?.family}" from disk and remove it from your library. This cannot be undone.`
                : `This will move "${pendingDelete?.fullName || pendingDelete?.family}" to the Windows Recycle Bin and remove it from your library.`}
            </DialogDescription>
          </DialogHeader>
          <div className="fm-dialog-footer">
            <Button type="button" variant="ghost" onClick={() => setPendingDelete(null)}>
              Cancel
            </Button>
            <Button type="button" variant="destructive" onClick={confirmDelete}>
              {deleteMode === "permanent" ? "Delete permanently" : "Move to Recycle Bin"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
