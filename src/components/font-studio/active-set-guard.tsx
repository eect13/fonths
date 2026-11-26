import { Gauge, PowerOff, Sparkles, X } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { deactivateSet } from "./activate-toggle";
import { useFontStore } from "@/lib/fonts/store";

const HIGH_ACTIVE_THRESHOLD = 300;
const GUARD_DISMISSED_KEY = "fm.activeSetGuardDismissed";

function readDismissedFlag(): boolean {
  try {
    return typeof localStorage !== "undefined" && localStorage.getItem(GUARD_DISMISSED_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * Active-Set Performance Guard:
 * Zero-RAM Instant mode keeps fonts active with 0 MB Fonths RAM, while hundreds
 * of active fonts still add startup/menu overhead in third-party apps (Word/Adobe/Figma).
 * Offers one-click cleanup to keep only Uploads & Favorites active without deleting any files.
 */
export function ActiveSetGuard() {
  const activated = useFontStore((s) => s.activated);
  const favorites = useFontStore((s) => s.favorites);
  const scope = useFontStore((s) => s.scope);
  const [dismissed, setDismissed] = useState(readDismissedFlag);

  if (dismissed || activated.length <= HIGH_ACTIVE_THRESHOLD) return null;
  if (scope !== "activated" && scope !== "all") return null;

  const favSet = new Set(favorites);
  const bulkGoogleIds = activated.filter((id) => id.startsWith("g:") && !favSet.has(id));
  if (bulkGoogleIds.length === 0) return null;

  const keepCount = activated.length - bulkGoogleIds.length;

  return (
    <div
      data-testid="active-set-guard"
      className="flex flex-wrap items-center justify-between gap-2 border-b border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs md:px-4"
    >
      <div className="flex min-w-0 items-center gap-2">
        <Gauge className="size-4 shrink-0 text-amber-500" />
        <p className="min-w-0 text-foreground">
          <span className="font-medium tabular-nums">
            {activated.length.toLocaleString()} fonts active
          </span>{" "}
          <span className="text-muted-foreground">
            — Zero-RAM mode holds these with 0 MB background RAM, though huge active sets still slow Word/Adobe/Figma font menus. Downloaded files stay on disk when deactivated.
          </span>
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button
          type="button"
          size="sm"
          variant="secondary"
          className="h-7 px-2.5 text-xs"
          data-testid="active-guard-keep-essentials"
          onClick={() => {
            deactivateSet(bulkGoogleIds, "non-favorite Google fonts");
            toast.message(`Deactivating ${bulkGoogleIds.length.toLocaleString()} Google fonts`, {
              id: "fm-active-guard",
              description: `Keeping ${keepCount.toLocaleString()} uploads & favorites active. Files stay on disk.`,
            });
          }}
        >
          <Sparkles className="size-3" />
          Keep Uploads &amp; Favorites ({keepCount.toLocaleString()})
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7 px-2.5 text-xs"
          data-testid="active-guard-deactivate-all"
          onClick={() => {
            deactivateSet(activated.slice(), "all active fonts");
          }}
        >
          <PowerOff className="size-3" />
          Deactivate All
        </Button>
        <button
          type="button"
          aria-label="Dismiss active fonts tip"
          onClick={() => {
            setDismissed(true);
            try {
              localStorage.setItem(GUARD_DISMISSED_KEY, "1");
            } catch {
              /* ignore */
            }
          }}
          className="inline-flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-secondary hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
