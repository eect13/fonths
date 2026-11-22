import { useState } from "react";
import { Loader2, RotateCcw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useFontStore } from "@/lib/fonts/store";

export function UploadsResetDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [busy, setBusy] = useState<null | "clear" | "reset">(null);
  const count = useFontStore((s) => s.localFonts.length);
  const deleteMode = useFontStore((s) => s.desktopPrefs.deleteMode ?? "recycle");
  const clearLocalFonts = useFontStore((s) => s.clearLocalFonts);
  const resetLibrary = useFontStore((s) => s.resetLibrary);
  const permanent = deleteMode === "permanent";

  async function handleReset() {
    if (busy) return;
    setBusy("reset");
    try {
      const n = await resetLibrary();
      onOpenChange(false);
      toast.success(
        n
          ? `Library reset. ${n.toLocaleString()} upload${n === 1 ? "" : "s"} removed.`
          : "Library reset.",
        { id: "uploads-reset-done" },
      );
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err || "Reset failed");
      toast.error("Could not reset library", {
        id: "uploads-reset-err",
        description: msg,
      });
    } finally {
      setBusy(null);
    }
  }

  async function handleClear() {
    if (busy || !count) return;
    setBusy("clear");
    try {
      const n = await clearLocalFonts();
      onOpenChange(false);
      toast.success(
        `Removed ${n.toLocaleString()} uploaded typeface${n === 1 ? "" : "s"}`,
        { id: "uploads-clear-done" },
      );
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err || "Clear failed");
      toast.error("Could not clear uploads", {
        id: "uploads-clear-err",
        description: msg,
      });
    } finally {
      setBusy(null);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (busy) return;
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Uploaded fonts</DialogTitle>
          <DialogDescription>
            {count
              ? `${count.toLocaleString()} uploaded typeface${count === 1 ? "" : "s"} in this library. Active delete mode: ${permanent ? "Permanent Delete (cannot be undone)" : "Move to Recycle Bin"}. Google Fonts are not affected.`
              : "No uploaded typefaces. Reset still restores default activations."}
          </DialogDescription>
        </DialogHeader>
        <div className="fm-dialog-footer mt-5">
          <Button
            type="button"
            variant="ghost"
            disabled={busy !== null}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="secondary"
            disabled={busy !== null}
            onClick={() => void handleReset()}
          >
            {busy === "reset" ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RotateCcw className="size-3.5" />
            )}
            {busy === "reset" ? "Resetting…" : "Reset library"}
          </Button>
          <Button
            type="button"
            variant="destructive"
            disabled={!count || busy !== null}
            onClick={() => void handleClear()}
          >
            {busy === "clear" ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <Trash2 className="size-3.5" />
            )}
            {busy === "clear" ? "Clearing…" : "Clear uploads"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
