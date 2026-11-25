import { Cpu, Download, Eraser, FolderOpen, RefreshCw, Settings, ShieldCheck, Trash2, Upload, Zap } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { applyDesktopPrefs, type DeleteMode } from "@/lib/desktop/prefs";
import { syncGoogleCdnFiles } from "@/lib/fonts/google-api";
import {
  cleanupTempStageFiles,
  getDWriteEngineInfo,
  purgeNonGoogleFonts,
  type DWriteEngineInfo,
  type PurgeNonGoogleResult,
} from "@/lib/fonts/native-parse";
import {
  armDocsVfSyncOwnership,
  clearDocsVfSyncCancelPending,
  didDocsVfSyncCancelToast,
  getGdiSessionState,
  openActivatedFolder,
  peekDocsVfSyncCancelPending,
  syncDocumentsVfPolicy,
  syncManagedDocumentsRoot,
  type GdiSessionState,
} from "@/lib/fonts/os-activate";
import { docsCancelToastAction } from "@/lib/fonts/docs-cancel-toast-action";
import { exportLibraryBackup, importLibraryBackup } from "@/lib/fonts/backup";
import { useFontStore } from "@/lib/fonts/store";
import { toast } from "sonner";
import { useUiDensity, type UiDensity } from "@/lib/ui-density";

export function DesktopSettings() {
  const prefs = useFontStore((s) => s.desktopPrefs);
  const setDesktopPrefs = useFontStore((s) => s.setDesktopPrefs);
  const liveCount = useFontStore((s) => s.activated.length);
  const googleCount = useFontStore((s) => s.googleFonts.length);
  const uploadCount = useFontStore((s) => s.localFonts.length);
  const diskCount = useFontStore((s) => s.diskFamilies.length);
  const { density, setDensity } = useUiDensity();
  const [open, setOpen] = useState(false);
  const [engineInfo, setEngineInfo] = useState<DWriteEngineInfo | null>(null);
  const [gdiState, setGdiState] = useState<GdiSessionState | null>(null);
  const [cleaningTemp, setCleaningTemp] = useState(false);
  const [syncingCdn, setSyncingCdn] = useState(false);
  const [purgingNonGoogle, setPurgingNonGoogle] = useState(false);
  const [purgePreview, setPurgePreview] = useState<PurgeNonGoogleResult | null>(null);

  const {
    closeToTray,
    startWithWindows,
    startMinimized = false,
    preferStaticFaces,
    fastSessionResume,
    deleteMode = "recycle",
    hkcuRegistrySync = true,
  } = prefs;

  useEffect(() => {
    void applyDesktopPrefs({
      closeToTray,
      startWithWindows,
      startMinimized,
      preferStaticFaces,
      fastSessionResume,
      deleteMode,
      hkcuRegistrySync,
    });
  }, [closeToTray, startWithWindows, startMinimized, preferStaticFaces, fastSessionResume, deleteMode, hkcuRegistrySync]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      const [info, gdi] = await Promise.all([getDWriteEngineInfo(), getGdiSessionState()]);
      if (!cancelled) {
        if (info) setEngineInfo(info);
        if (gdi) setGdiState(gdi);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open]);

  const densityOpts: { id: UiDensity; label: string; hint: string }[] = [
    { id: "comfortable", label: "Comfortable", hint: "Roomy cards and rows" },
    { id: "compact", label: "Compact", hint: "Tighter density across library and dialogs" },
  ];

  const deleteModeOpts: { id: DeleteMode; label: string; hint: string }[] = [
    { id: "recycle", label: "Recycle Bin", hint: "Move deleted library fonts to Windows Recycle Bin" },
    { id: "permanent", label: "Permanent Delete", hint: "Permanently remove deleted library fonts immediately" },
  ];

  const handleCleanTempCache = async () => {
    setCleaningTemp(true);
    try {
      const res = await cleanupTempStageFiles();
      if (!res) {
        toast.error("Temp cache cleanup is only available in the Windows desktop app.");
        return;
      }
      if (res.scanned === 0) {
        toast.success("Temp cache is already clean.", { id: "fm-temp-clean" });
      } else if (res.failed > 0) {
        toast.error(`Permanently removed ${res.deleted} of ${res.scanned} temp files (${res.failed} locked)`, {
          id: "fm-temp-clean",
        });
      } else {
        toast.success(`Permanently removed ${res.deleted} temporary font file${res.deleted === 1 ? "" : "s"}.`, {
          id: "fm-temp-clean",
        });
      }
    } finally {
      setCleaningTemp(false);
    }
  };

  const handleSyncGoogleCdn = async () => {
    setSyncingCdn(true);
    try {
      await syncGoogleCdnFiles(false);
    } finally {
      setSyncingCdn(false);
    }
  };

  const handleScanNonGoogle = async () => {
    setPurgingNonGoogle(true);
    try {
      const keep = useFontStore.getState().googleFonts.map((f) => f.family);
      const preview = await purgeNonGoogleFonts(keep, true);
      if (!preview) {
        toast.error("Purge non-Google is only available in the Windows desktop app.");
        return;
      }
      if (preview.candidates.length === 0) {
        toast.success("Providers → Google is clean — no non-Google folders found.", {
          id: "fm-purge-nongoogle",
          description: "Your local & watched folders are kept separate and untouched.",
        });
        return;
      }
      setPurgePreview(preview);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Could not scan for non-Google folders.");
    } finally {
      setPurgingNonGoogle(false);
    }
  };

  const handleConfirmPurgeNonGoogle = async () => {
    setPurgingNonGoogle(true);
    setPurgePreview(null);
    try {
      const keep = useFontStore.getState().googleFonts.map((f) => f.family);
      const res = await purgeNonGoogleFonts(keep, false);
      if (!res) return;
      if (res.removed > 0) {
        await syncManagedDocumentsRoot().catch(() => undefined);
      }
      if (res.failed > 0) {
        toast.error(
          `Purged ${res.removed} non-Google folder${res.removed === 1 ? "" : "s"} (${res.failed} locked)`,
          { id: "fm-purge-nongoogle" },
        );
      } else {
        toast.success(
          res.permanent
            ? `Permanently purged ${res.removed} non-Google folder${res.removed === 1 ? "" : "s"}.`
            : `Moved ${res.removed} non-Google folder${res.removed === 1 ? "" : "s"} to Recycle Bin.`,
          {
            id: "fm-purge-nongoogle",
            description: "Local and watched folders were left untouched.",
          },
        );
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Purge non-Google failed.");
    } finally {
      setPurgingNonGoogle(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="icon-sm" variant="ghost" aria-label="Settings" data-testid="settings-open">
          <Settings />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-h-[85vh] overflow-y-auto pb-8">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Appearance, startup, deletion behavior, Google CDN updates, and Fontations + HarfRust engine status.
          </DialogDescription>
        </DialogHeader>

        {/* Group 1: Appearance */}
        <div className="fm-settings-stack fm-settings-row rounded-md bg-secondary">
          <div className="min-w-0">
            <p className="text-sm font-medium">Density</p>
            <p className="text-xs text-muted-foreground">
              Spacing for cards, rows, and dialogs. Saved on this device.
            </p>
          </div>
          <div className="fm-settings-opts flex flex-wrap" role="group" aria-label="UI density">
            {densityOpts.map((opt) => (
              <Button
                key={opt.id}
                type="button"
                size="sm"
                variant={density === opt.id ? "default" : "outline"}
                aria-label={opt.label}
                aria-pressed={density === opt.id}
                title={opt.hint}
                data-testid={`density-${opt.id}`}
                onClick={() => setDensity(opt.id)}
              >
                {opt.label}
              </Button>
            ))}
          </div>
        </div>

        {/* Group 2: Background & Startup (separate from GDI restore) */}
        <div className="space-y-2" data-testid="background-mode-card">
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium">Run in background (Headless daemon)</p>
              <p className="text-xs text-muted-foreground">
                Closing the window hands active fonts off to the lightweight pure-Rust tray daemon (~6 MB RAM) and exits WebView2 — keeping fonts live in Word, Adobe, and Figma with zero idle browser memory.
              </p>
            </div>
            <Switch
              data-testid="run-in-background-switch"
              checked={prefs.closeToTray}
              onCheckedChange={(on) => setDesktopPrefs({ closeToTray: on })}
            />
          </label>
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium">Start with Windows</p>
              <p className="text-xs text-muted-foreground">
                Launch Fonths automatically when you sign in (uses Task Scheduler when elevated).
              </p>
            </div>
            <Switch
              checked={prefs.startWithWindows}
              onCheckedChange={(on) => setDesktopPrefs({ startWithWindows: on })}
            />
          </label>
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium">Start minimized in background</p>
              <p className="text-xs text-muted-foreground">
                Launch the pure-Rust headless daemon (`--daemon`) at Windows login to activate your saved fonts immediately without opening WebView2.
              </p>
            </div>
            <Switch
              data-testid="start-minimized-switch"
              checked={Boolean(prefs.startMinimized)}
              onCheckedChange={(on) => setDesktopPrefs({ startMinimized: on })}
            />
          </label>
        </div>

        {/* Group 3: Win32 Session Table & Font Format Policy */}
        <div className="space-y-2" data-testid="gdi-restore-card">
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium flex items-center gap-1.5">
                <Zap className="size-3.5" />
                Fast Win32 Session Table restore
              </p>
              <p className="text-xs text-muted-foreground">
                Automatic on reopen — Hardlinked session stage stays ready so the next launch restores fast, skipping deep SFNT re-checks on verified folders.
              </p>
            </div>
            <Switch
              data-testid="fast-gdi-restore-switch"
              checked={prefs.fastSessionResume !== false}
              onCheckedChange={(on) => setDesktopPrefs({ fastSessionResume: on })}
            />
          </label>
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium">Prefer static fonts (compatibility mode)</p>
              <p className="text-xs text-muted-foreground">
                Download and register separate static weights instead of 1 Variable TTF per family. Turn ON if older apps struggle with variable fonts; leave OFF for 4–8× faster activation.
              </p>
            </div>
            <Switch
              data-testid="prefer-static-faces-switch"
              checked={Boolean(prefs.preferStaticFaces)}
              onCheckedChange={(on) => setDesktopPrefs({ preferStaticFaces: on })}
            />
          </label>
          <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary text-sm">
            <div>
              <p className="font-medium">Adobe / Office HKCU registry sync</p>
              <p className="text-xs text-muted-foreground">
                Mirror temporary session fonts under HKCU\...\Fonts so Photoshop, Illustrator, and Office detect them instantly. Turn OFF for maximum activation speed if your apps already read Win32 session tables.
              </p>
            </div>
            <Switch
              data-testid="hkcu-registry-sync-switch"
              checked={prefs.hkcuRegistrySync !== false}
              onCheckedChange={(on) => setDesktopPrefs({ hkcuRegistrySync: on })}
            />
          </label>
        </div>

        {/* Group 3: Smart Deletion & Temporary Fonts */}
        <div className="fm-settings-stack fm-settings-row rounded-md bg-secondary" data-testid="delete-mode-card">
          <div className="min-w-0">
            <p className="text-sm font-medium flex items-center gap-1.5">
              <Trash2 className="size-3.5" />
              Delete behavior
            </p>
            <p className="text-xs text-muted-foreground">
              Choose whether deleting library fonts moves them to the Recycle Bin or permanently removes them. Temporary/preview fonts are always permanently deleted automatically.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div className="fm-settings-opts flex flex-wrap" role="group" aria-label="Delete behavior">
              {deleteModeOpts.map((opt) => (
                <Button
                  key={opt.id}
                  type="button"
                  size="sm"
                  variant={deleteMode === opt.id ? (opt.id === "permanent" ? "destructive" : "default") : "outline"}
                  aria-pressed={deleteMode === opt.id}
                  title={opt.hint}
                  data-testid={`delete-mode-${opt.id}`}
                  onClick={() => setDesktopPrefs({ deleteMode: opt.id })}
                >
                  {opt.label}
                </Button>
              ))}
            </div>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={cleaningTemp}
              data-testid="clean-temp-cache-btn"
              onClick={() => void handleCleanTempCache()}
            >
              <Trash2 className="size-3.5 mr-1" />
              {cleaningTemp ? "Cleaning…" : "Clean temp cache"}
            </Button>
          </div>
        </div>

        {/* Group 4: Font Engine & Library Folders */}
        <div className="fm-settings-stack fm-settings-row mb-1 rounded-md bg-secondary" data-testid="dwrite-engine-card">
          <div className="min-w-0">
            <p className="text-sm font-medium flex items-center gap-1.5">
              <Cpu className="size-3.5" />
              Font Engine (Fontations + HarfRust)
              {engineInfo?.elevated ? (
                <span className="inline-flex items-center gap-1 rounded bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium text-primary">
                  <ShieldCheck className="size-3" />
                  Admin
                </span>
              ) : null}
            </p>
            <p className="text-xs text-muted-foreground">
              {engineInfo
                ? `${engineInfo.engine} · Win32 Session Table (${engineInfo.activationMode}) · System Fonts protected (${engineInfo.systemFontCount.toLocaleString()} installed in Windows\\Fonts)`
                : "Pure-Rust Fontations (read-fonts + skrifa + harfrust) + font:// protocol with Win32 Session Table activation. System Fonts remain protected."}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              Google: {googleCount.toLocaleString()} catalog · Watched: {uploadCount.toLocaleString()} · On disk: {diskCount.toLocaleString()} · Active: {liveCount.toLocaleString()}
              {gdiState
                ? ` · Win32 Session Table: ${gdiState.liveFaceCount.toLocaleString()} faces (${gdiState.gdiObjects.toLocaleString()} / ${gdiState.gdiQuota.toLocaleString()} handles)`
                : ""}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label="Open Library folder"
              onClick={() => void openActivatedFolder()}
            >
              <FolderOpen className="size-3.5 mr-1" />
              Fonths
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label="Open Google Fonts folder"
              data-testid="open-google-folder-btn"
              onClick={() => void openActivatedFolder("google")}
            >
              <FolderOpen className="size-3.5 mr-1" />
              Providers\Google
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={syncingCdn}
              data-testid="sync-google-cdn-btn"
              onClick={() => void handleSyncGoogleCdn()}
            >
              <RefreshCw className={`size-3.5 mr-1 ${syncingCdn ? "animate-spin" : ""}`} />
              {syncingCdn ? "Syncing CDN…" : "Update Google files"}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={purgingNonGoogle}
              data-testid="purge-non-google-btn"
              onClick={() => void handleScanNonGoogle()}
            >
              <Eraser className="size-3.5 mr-1" />
              {purgingNonGoogle ? "Scanning…" : "Purge non-Google"}
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label="Optimize Library (replace redundant static fonts with Variable TTF)"
              title="Replace redundant static weights with 1 Variable TTF per family to cut file count and speed up activation"
              data-testid="refresh-documents-settings"
              onClick={() => {
                void (async () => {
                  try {
                    armDocsVfSyncOwnership();
                    toast.message("Refreshing Documents folder…", {
                      id: "sync-docs-vf",
                      description: "Cancel from the progress bar if needed.",
                      duration: 8_000,
                      action: docsCancelToastAction(),
                    });
                    const result = await syncDocumentsVfPolicy();
                    if (!result) return;
                    if (result.cancelled) {
                      if (!didDocsVfSyncCancelToast()) {
                        toast.message("Documents refresh cancelled", {
                          id: "sync-docs-vf",
                          description: `Checked ${result.familiesSeen.toLocaleString()} folders · removed ${result.staticsDeleted.toLocaleString()} statics before cancel.`,
                        });
                      }
                      try {
                        await syncManagedDocumentsRoot();
                      } catch {
                        /* best-effort */
                      }
                      return;
                    }
                    if (result.cancelArrivedLate) {
                      didDocsVfSyncCancelToast();
                      const n = result.staticsDeleted;
                      toast.message(
                        n === 0
                          ? "Cancel arrived after Documents refresh finished — no redundant statics removed"
                          : `Cancel arrived after Documents refresh finished — ${n.toLocaleString()} redundant statics removed`,
                        {
                          id: "sync-docs-vf",
                          description: `${result.familiesSeen.toLocaleString()} folders checked before Cancel landed.`,
                        },
                      );
                      try {
                        await syncManagedDocumentsRoot();
                      } catch {
                        /* best-effort */
                      }
                      return;
                    }
                    try {
                      await syncManagedDocumentsRoot();
                    } catch {
                      toast.error("Documents refreshed but library rescan failed", {
                        id: "sync-docs-vf",
                      });
                      return;
                    }
                    if (peekDocsVfSyncCancelPending()) {
                      didDocsVfSyncCancelToast();
                      const n = result.staticsDeleted;
                      toast.message(
                        n === 0
                          ? "Cancel arrived after Documents refresh finished — no redundant statics removed"
                          : `Cancel arrived after Documents refresh finished — ${n.toLocaleString()} redundant statics removed`,
                        {
                          id: "sync-docs-vf",
                          description: `${result.familiesSeen.toLocaleString()} folders checked before Cancel landed.`,
                        },
                      );
                      return;
                    }
                    if (result.locked > 0) {
                      toast.error(
                        `${result.locked.toLocaleString()} face${result.locked === 1 ? "" : "s"} locked — deactivate fonts or quit Adobe/Word, then Repair`,
                        {
                          id: "sync-docs-vf-locked",
                          description: "Another app is locking static TTFs during Refresh.",
                          duration: 24_000,
                        },
                      );
                    }
                    toast.success(
                      `Documents refreshed — ${result.staticsDeleted.toLocaleString()} redundant statics removed`,
                      {
                        id: "sync-docs-vf",
                        description: `${result.familiesSeen.toLocaleString()} folders · ${result.familiesPurged.toLocaleString()} VF families updated.`,
                        duration: 12_000,
                      },
                    );
                  } finally {
                    clearDocsVfSyncCancelPending();
                  }
                })();
              }}
            >
              <Zap className="size-3.5 mr-1" />
              Optimize Library
            </Button>
          </div>
        </div>

        {/* Group 5: Backup & Restore */}
        <div className="fm-settings-stack fm-settings-row rounded-md bg-secondary" data-testid="backup-restore-card">
          <div className="min-w-0">
            <p className="text-sm font-medium">Backup & Restore</p>
            <p className="text-xs text-muted-foreground">
              Export or restore your Favorites, Collections, Custom Tags, OpenType feature choices, duplicate hiding, and desktop settings as a JSON file.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-testid="export-backup-btn"
              onClick={() => void exportLibraryBackup()}
            >
              <Download className="size-3.5 mr-1" />
              Export backup
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              data-testid="import-backup-btn"
              onClick={() => void importLibraryBackup()}
            >
              <Upload className="size-3.5 mr-1" />
              Import backup
            </Button>
          </div>
        </div>
      </DialogContent>

      <Dialog open={Boolean(purgePreview)} onOpenChange={(next) => !next && setPurgePreview(null)}>
        <DialogContent data-testid="purge-non-google-dialog">
          <DialogHeader>
            <DialogTitle>
              Purge {purgePreview?.candidates.length ?? 0} non-Google folder
              {(purgePreview?.candidates.length ?? 0) === 1 ? "" : "s"}?
            </DialogTitle>
            <DialogDescription>
              {purgePreview?.permanent
                ? "Permanent Delete mode is active. These non-Google folders under Documents\\Fonths will be permanently deleted. Your Uploads folder and C:\\Windows\\Fonts are never touched."
                : "Recycle Bin mode is active. These non-Google folders under Documents\\Fonths will move to the Recycle Bin. Your Uploads folder and C:\\Windows\\Fonts are never touched."}
            </DialogDescription>
          </DialogHeader>
          {purgePreview?.candidates.length ? (
            <div className="fm-scroll max-h-40 overflow-y-auto rounded-md border border-border bg-secondary/40 p-2 text-xs text-muted-foreground">
              {purgePreview.candidates.slice(0, 40).join(", ")}
              {purgePreview.candidates.length > 40
                ? ` +${purgePreview.candidates.length - 40} more`
                : ""}
            </div>
          ) : null}
          <div className="fm-dialog-footer">
            <Button type="button" variant="ghost" onClick={() => setPurgePreview(null)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              data-testid="purge-non-google-confirm"
              onClick={() => void handleConfirmPurgeNonGoogle()}
            >
              {purgePreview?.permanent ? "Permanently purge" : "Move to Recycle Bin"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </Dialog>
  );
}
