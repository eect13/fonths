import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  resolveImportCollision,
  subscribeImportCollision,
  type ImportCollisionRequest,
} from "@/lib/fonts/import-collision";
import { CATEGORY_LABEL } from "@/lib/fonts/types";

export function ImportCollisionDialog() {
  const [req, setReq] = useState<ImportCollisionRequest | null>(null);
  useEffect(() => subscribeImportCollision(setReq), []);

  const open = Boolean(req);
  const items = req?.collisions ?? [];
  const hasGoogle = items.some((i) => !i.matchSource || i.matchSource === "google");
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && req) resolveImportCollision("cancel");
      }}
    >
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {items.length} duplicate or matching font{items.length === 1 ? "" : "s"} found
          </DialogTitle>
          <DialogDescription>
            Importing a font with the same family name as an existing Google, library, or system font can shadow faces in Adobe/Office. Choose how to handle the overlap:
          </DialogDescription>
        </DialogHeader>

        <div className="max-h-60 space-y-2 overflow-y-auto pr-1">
          {items.map((item) => {
            const sourceLabel =
              item.matchSource === "system"
                ? "System font (Windows\\Fonts)"
                : item.matchSource === "local"
                  ? "Local / Watched library"
                  : "Google Fonts";
            const statusLabel =
              item.matchSource === "system"
                ? "Always active (read-only)"
                : item.isGoogleActive
                  ? "Currently Active"
                  : item.isGoogleOnDisk
                    ? "On disk (Inactive)"
                    : "Catalog only";
            return (
              <div
                key={item.familyKey}
                className="grid grid-cols-2 gap-2 rounded-md border border-border bg-secondary/40 p-2.5 text-xs"
              >
                <div>
                  <div className="font-semibold text-foreground">
                    Existing: {item.googleFont.family}
                  </div>
                  <div className="text-muted-foreground">
                    {sourceLabel} · {CATEGORY_LABEL[item.googleFont.category]} · {item.googleFont.weights.length} weights
                  </div>
                  <div className="mt-0.5 text-[11px] text-muted-foreground">
                    {statusLabel}
                  </div>
                </div>
                <div>
                  <div className="font-semibold text-foreground">
                    New Folder ({item.uploadFileNames.length} file{item.uploadFileNames.length === 1 ? "" : "s"})
                  </div>
                  <div className="truncate text-muted-foreground" title={item.uploadFileNames.join(", ")}>
                    {item.uploadFileNames.slice(0, 2).join(", ")}
                    {item.uploadFileNames.length > 2 ? ` +${item.uploadFileNames.length - 2}` : ""}
                  </div>
                  <div className="mt-0.5 text-[11px] text-muted-foreground">
                    {Math.max(1, Math.round(item.uploadBytes / 1024)).toLocaleString()} KB
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        <div className="fm-dialog-footer flex-wrap">
          <Button type="button" variant="ghost" onClick={() => resolveImportCollision("cancel")}>
            Cancel
          </Button>
          <Button type="button" variant="outline" onClick={() => resolveImportCollision("skip-colliding")}>
            Skip colliding
          </Button>
          <Button type="button" variant="secondary" onClick={() => resolveImportCollision("keep-both")}>
            Keep both
          </Button>
          <Button type="button" onClick={() => resolveImportCollision("prefer-local")}>
            {hasGoogle ? "Prefer local (deactivate Google)" : "Prefer new (deactivate existing)"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
