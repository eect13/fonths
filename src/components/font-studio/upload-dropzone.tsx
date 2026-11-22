import { useEffect, useRef, useState, type DragEvent, type ReactNode } from "react";
import { Pause, Play, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { filesFromDataTransfer } from "@/lib/fonts/fs-drop";
import { useFontStore } from "@/lib/fonts/store";
import { cn } from "@/lib/utils";
import { runFontImport } from "./import-fonts";

function isOsFileDrag(e: DragEvent) {
  const dt = e.dataTransfer;
  if (!dt) return false;
  const types = Array.from(dt.types);
  if (types.includes("application/x-font-id") || types.includes("application/x-folder-id")) return false;
  return types.includes("Files");
}

export function UploadDropzone({ children }: { children: ReactNode }) {
  const uploadBusy = useFontStore((s) => s.uploadBusy);
  const uploadJob = useFontStore((s) => s.uploadJob);
  const pauseUpload = useFontStore((s) => s.pauseUpload);
  const resumeUpload = useFontStore((s) => s.resumeUpload);
  const cancelUpload = useFontStore((s) => s.cancelUpload);
  const [over, setOver] = useState(false);
  const depth = useRef(0);
  const pct = uploadJob.total > 0 ? Math.min(100, Math.round((uploadJob.done / uploadJob.total) * 100)) : 0;

  useEffect(() => {
    function clear() {
      depth.current = 0;
      setOver(false);
    }
    window.addEventListener("dragend", clear);
    window.addEventListener("drop", clear);
    return () => {
      window.removeEventListener("dragend", clear);
      window.removeEventListener("drop", clear);
    };
  }, []);

  async function onDrop(e: DragEvent) {
    e.preventDefault();
    depth.current = 0;
    setOver(false);
    if (!isOsFileDrag(e) || !e.dataTransfer) return;
    const { files, folderName } = await filesFromDataTransfer(e.dataTransfer);
    if (files.length) void runFontImport(files, { collectionName: folderName });
  }

  return (
    <div
      className="relative flex min-h-0 flex-1 flex-col"
      onDragEnter={(e) => {
        if (!isOsFileDrag(e)) return;
        e.preventDefault();
        depth.current += 1;
        setOver(true);
      }}
      onDragOver={(e) => {
        if (!isOsFileDrag(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(e) => {
        if (!isOsFileDrag(e)) return;
        depth.current = Math.max(0, depth.current - 1);
        if (depth.current === 0) setOver(false);
      }}
      onDrop={(e) => void onDrop(e)}
    >
      {children}
      {(over || uploadBusy) && (
        <div
          className={cn(
            "absolute inset-2 z-20 flex items-center justify-center rounded-xl border border-dashed bg-background/85 backdrop-blur-xs",
            uploadBusy ? "pointer-events-auto border-border" : "pointer-events-none",
            over && !uploadBusy ? "border-primary" : "border-border",
          )}
        >
          {uploadBusy ? (
            <div
              data-testid="upload-progress-card"
              className="w-full max-w-md space-y-3 rounded-lg border border-border bg-card p-5 shadow-lg"
            >
              <div className="flex items-center justify-between gap-3">
                <p className="font-heading text-lg font-medium">
                  {uploadJob.cancelled
                    ? "Stopping import…"
                    : uploadJob.paused
                      ? "Import paused"
                      : "Reading & importing fonts…"}
                </p>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {uploadJob.done.toLocaleString()} / {uploadJob.total.toLocaleString()} ({pct}%)
                </span>
              </div>
              <div className="h-2 w-full overflow-hidden rounded-full bg-secondary">
                <div
                  className="h-full bg-primary transition-[width] duration-150"
                  style={{ width: `${pct}%` }}
                />
              </div>
              <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                <span className="truncate">{uploadJob.current || "Preparing files…"}</span>
                <span className="shrink-0 tabular-nums">
                  +{uploadJob.added.toLocaleString()} added
                  {uploadJob.skipped ? ` · ${uploadJob.skipped.toLocaleString()} skipped` : ""}
                  {uploadJob.failed ? ` · ${uploadJob.failed.toLocaleString()} failed` : ""}
                </span>
              </div>
              <div className="flex items-center justify-end gap-2 pt-1">
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  data-testid="upload-pause-resume"
                  disabled={uploadJob.cancelled}
                  onClick={() => (uploadJob.paused ? resumeUpload() : pauseUpload())}
                >
                  {uploadJob.paused ? (
                    <>
                      <Play className="size-3.5 mr-1" />
                      Resume
                    </>
                  ) : (
                    <>
                      <Pause className="size-3.5 mr-1" />
                      Pause
                    </>
                  )}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="destructive"
                  data-testid="upload-cancel"
                  disabled={uploadJob.cancelled}
                  onClick={() => cancelUpload()}
                >
                  <X className="size-3.5 mr-1" />
                  Cancel
                </Button>
              </div>
            </div>
          ) : (
            <p className="font-heading text-2xl">Drop files or a folder</p>
          )}
        </div>
      )}
    </div>
  );
}
