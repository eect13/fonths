import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { applyDesktopPrefs } from "@/lib/desktop/prefs";
import {
  resolveActivateConfirm,
  resolveDeactivateConfirm,
  subscribeActivateConfirm,
  subscribeDeactivateConfirm,
  type ActivateConfirmRequest,
  type DeactivateConfirmRequest,
} from "@/lib/fonts/activate-confirm";
import { useFontStore } from "@/lib/fonts/store";

// eslint-disable-next-line react-refresh/only-export-components
export function toggleFastSessionResumePref(next: boolean) {
  const store = useFontStore.getState();
  store.setDesktopPrefs({ fastSessionResume: next });
  const updated = { ...store.desktopPrefs, fastSessionResume: next };
  void applyDesktopPrefs(updated);
}

function FastResumeOptionRow({ testId }: { testId: string }) {
  const fastResume = useFontStore((s) => Boolean(s.desktopPrefs.fastSessionResume));
  return (
    <label className="fm-settings-row flex items-center justify-between gap-3 rounded-md bg-secondary/70 px-3 py-2 text-xs">
      <div className="space-y-0.5">
        <div className="font-medium text-foreground">
          Bypass deep disk check on next session (Fast Resume)
        </div>
        <div className="text-muted-foreground">
          Trust already-complete folders and pre-staged Win32 Session Table files instead of re-verifying every font header.
        </div>
      </div>
      <Switch
        data-testid={testId}
        checked={fastResume}
        onCheckedChange={toggleFastSessionResumePref}
      />
    </label>
  );
}

/** In-app modal: nothing downloads or activates until confirmed. Esc/X/Cancel = Abort cleanly. */
export function ActivateConfirmDialog() {
  const [req, setReq] = useState<ActivateConfirmRequest | null>(null);
  useEffect(() => subscribeActivateConfirm(setReq), []);

  const open = Boolean(req);
  const sliceCount = req ? (req.preferCount > 0 ? req.preferCount : req.cancelCount) : 0;
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Esc / outside click / X always aborts cleanly — never starts downloads without user go.
        if (!next && req) resolveActivateConfirm("abort");
      }}
    >
      <DialogContent data-testid="activate-confirm-dialog">
        <DialogHeader>
          <DialogTitle>Activate {req?.total.toLocaleString() ?? ""} in {req?.label ?? ""}?</DialogTitle>
          <DialogDescription>
            Nothing starts downloading or activating until you confirm below.{" "}
            {req?.etaHint ?? ""} Word/Adobe stay honest (Add&gt;0 only). Priority order:{" "}
            selected/favorites/visible/first-page/recent first.
          </DialogDescription>
        </DialogHeader>
        <FastResumeOptionRow testId="activate-fast-resume-toggle" />
        <div className="fm-dialog-footer flex-wrap">
          <Button
            type="button"
            variant="ghost"
            data-testid="activate-confirm-cancel"
            onClick={() => resolveActivateConfirm("abort")}
          >
            Cancel
          </Button>
          {sliceCount > 0 ? (
            <Button
              type="button"
              variant="secondary"
              data-testid="activate-confirm-slice"
              onClick={() => resolveActivateConfirm("cancel")}
            >
              {req && req.visibleCount > 0
                ? `Visible only (${sliceCount.toLocaleString()})`
                : `First page only (${sliceCount.toLocaleString()})`}
            </Button>
          ) : null}
          <Button
            type="button"
            data-testid="activate-confirm-all"
            onClick={() => resolveActivateConfirm("ok")}
          >
            Activate all {req?.total.toLocaleString() ?? ""}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** In-app logic options popup when bulk-deactivating fonts (>50 active). */
export function DeactivateConfirmDialog() {
  const [req, setReq] = useState<DeactivateConfirmRequest | null>(null);
  useEffect(() => subscribeDeactivateConfirm(setReq), []);

  const open = Boolean(req);
  const canSplitGoogle = Boolean(req && req.googleCount > 0 && req.localCount > 0);
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && req) resolveDeactivateConfirm("cancel");
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            Deactivate {req?.total.toLocaleString() ?? ""} active in {req?.label ?? ""}?
          </DialogTitle>
          <DialogDescription>
            {canSplitGoogle
              ? `${req?.googleCount.toLocaleString()} Google Fonts and ${req?.localCount.toLocaleString()} imported/local fonts are active.`
              : "Unloading clears Win32 Session Table / Adobe registration while keeping downloaded & staged files ready on disk."}{" "}
            Or keep them active with Fast Resume enabled so the next launch starts quickly without re-checking every folder.
          </DialogDescription>
        </DialogHeader>
        <FastResumeOptionRow testId="deactivate-fast-resume-toggle" />
        <div className="fm-dialog-footer flex-wrap">
          <Button type="button" variant="ghost" onClick={() => resolveDeactivateConfirm("cancel")}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => resolveDeactivateConfirm("keep-fast")}
          >
            Keep active · Fast Resume
          </Button>
          {canSplitGoogle ? (
            <Button
              type="button"
              variant="secondary"
              onClick={() => resolveDeactivateConfirm("google")}
            >
              Google Fonts only ({req?.googleCount.toLocaleString() ?? 0})
            </Button>
          ) : null}
          <Button type="button" onClick={() => resolveDeactivateConfirm("all")}>
            Deactivate {canSplitGoogle ? "all " : ""}
            {req?.total.toLocaleString() ?? 0}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
