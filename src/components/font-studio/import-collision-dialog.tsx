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
            {items.length} upload{items.length === 1 ? "" : "s"} match Google Fonts
          </DialogTitle>
          <DialogDescription>
            Importing a font with the same family name as an active Google Font can shadow faces in Adobe/Office. Choose how to handle the collision:
          </DialogDescription>
        </DialogHeader>

        <div className="max-h-60 space-y-2 overflow-y-auto pr-1">
          {items.map((item) => (
            <div
              key={item.familyKey}
              className="grid grid-cols-2 gap-2 rounded-md border border-border bg-secondary/40 p-2.5 text-xs"
            >
              <div>
                <div className="font-semibold text-foreground">
                  Existing: {item.googleFont.family}
                </div>
                <div className="text-muted-foreground">
                  Google Fonts · {CATEGORY_LABEL[item.googleFont.category]} · {item.googleFont.weights.length} weights
                </div>
                <div className="mt-0.5 text-[11px] text-muted-foreground">
                  {item.isGoogleActive ? "Currently Active" : item.isGoogleOnDisk ? "On disk (Inactive)" : "Catalog only"}
                </div>
              </div>
              <div>
                <div className="font-semibold text-foreground">
                  New Upload ({item.uploadFileNames.length} file{item.uploadFileNames.length === 1 ? "" : "s"})
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
          ))}
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
            Prefer local (deactivate Google)
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
