import { AlertCircle, AlertTriangle, Bell, Check, CheckCircle2, Copy, Info, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  clearNotifications,
  markAllNotificationsRead,
  removeNotification,
  useNotifications,
  type NotificationKind,
} from "@/lib/notifications";
import { cn } from "@/lib/utils";

function formatRelativeTime(at: number): string {
  const sec = Math.max(0, Math.round((Date.now() - at) / 1000));
  if (sec < 30) return "just now";
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

function KindIcon({ kind }: { kind: NotificationKind }) {
  switch (kind) {
    case "success":
      return <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-emerald-500" />;
    case "error":
      return <AlertCircle className="mt-0.5 size-3.5 shrink-0 text-destructive" />;
    case "warning":
      return <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-amber-500" />;
    default:
      return <Info className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />;
  }
}

export function NotificationCenter() {
  const items = useNotifications();
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState<"all" | "issues">("all");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [, setTimeTick] = useState(0);

  useEffect(() => {
    if (!open) return;
    setTimeTick((n) => n + 1);
    const timer = window.setInterval(() => {
      setTimeTick((n) => n + 1);
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [open]);
  const unread = items.filter((n) => !n.read).length;
  const issueCount = items.filter((n) => n.kind === "error" || n.kind === "warning").length;
  const shown = filter === "issues" ? items.filter((n) => n.kind === "error" || n.kind === "warning") : items;

  function copyItem(id: string, title: string, description?: string) {
    const text = description ? `${title}\n${description}` : title;
    void navigator.clipboard?.writeText(text).then(() => {
      setCopiedId(id);
      window.setTimeout(() => setCopiedId((cur) => (cur === id ? null : cur)), 1400);
    });
  }

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) markAllNotificationsRead();
      }}
    >
      <PopoverTrigger asChild>
        <Button
          size="icon-sm"
          variant="ghost"
          className="relative"
          aria-label={unread > 0 ? `Notifications (${unread} unread)` : "Notifications"}
          title="Notification history"
          data-testid="notifications-open"
        >
          <Bell className="size-4" />
          {unread > 0 ? (
            <span
              data-testid="notifications-badge"
              className="absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none text-primary-foreground tabular-nums"
            >
              {unread > 99 ? "99+" : unread}
            </span>
          ) : null}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="flex max-h-[min(28rem,calc(100vh-5rem))] w-80 flex-col overflow-hidden p-0 sm:w-96"
        data-testid="notifications-popover"
      >
        <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-2">
          <div className="flex items-center gap-2">
            <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Notifications
            </p>
            {items.length > 0 ? (
              <span className="rounded bg-secondary px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-muted-foreground">
                {items.length}
              </span>
            ) : null}
          </div>
          <div className="flex items-center gap-1">
            {issueCount > 0 ? (
              <div className="flex items-center rounded-md border border-border bg-secondary/40 p-0.5 text-[10px]">
                <button
                  type="button"
                  onClick={() => setFilter("all")}
                  className={cn(
                    "rounded px-1.5 py-0.5 transition-colors",
                    filter === "all" ? "bg-background font-medium text-foreground shadow-xs" : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  All
                </button>
                <button
                  type="button"
                  onClick={() => setFilter("issues")}
                  className={cn(
                    "rounded px-1.5 py-0.5 transition-colors",
                    filter === "issues" ? "bg-background font-medium text-foreground shadow-xs" : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  Issues ({issueCount})
                </button>
              </div>
            ) : null}
            {items.length > 0 ? (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-6 px-2 text-[11px] text-muted-foreground hover:text-foreground"
                data-testid="notifications-clear"
                onClick={() => {
                  clearNotifications();
                  setFilter("all");
                }}
              >
                <Trash2 className="mr-1 size-3" />
                Clear all
              </Button>
            ) : null}
          </div>
        </div>
        {shown.length === 0 ? (
          <div className="px-4 py-8 text-center text-xs text-muted-foreground">
            {items.length === 0
              ? "No notifications yet. Activity from downloads, activations, syncs, and errors will appear here."
              : "No errors or warnings in recent notifications."}
          </div>
        ) : (
          <div
            data-testid="notifications-list"
            className="fm-scroll min-h-0 flex-1 overflow-y-auto overscroll-contain"
          >
            <div className="divide-y divide-border/60">
              {shown.map((item) => (
                <div
                  key={item.id}
                  className={cn(
                    "group flex items-start gap-2.5 px-3 py-2.5 text-xs transition-colors hover:bg-secondary/40",
                    !item.read && "bg-secondary/25",
                  )}
                >
                  <KindIcon kind={item.kind} />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="flex items-start justify-between gap-2">
                      <p className="break-words font-medium leading-snug text-foreground">{item.title}</p>
                      <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                        {formatRelativeTime(item.at)}
                      </span>
                    </div>
                    {item.description ? (
                      <p className="break-words text-[11px] leading-snug text-muted-foreground">
                        {item.description}
                      </p>
                    ) : null}
                  </div>
                  <div className="mt-0.5 flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      type="button"
                      aria-label="Copy notification"
                      title="Copy message"
                      onClick={() => copyItem(item.id, item.title, item.description)}
                      className="inline-flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-secondary hover:text-foreground"
                    >
                      {copiedId === item.id ? <Check className="size-3 text-emerald-500" /> : <Copy className="size-3" />}
                    </button>
                    <button
                      type="button"
                      aria-label="Dismiss notification"
                      title="Dismiss"
                      onClick={() => removeNotification(item.id)}
                      className="inline-flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-secondary hover:text-foreground"
                    >
                      <X className="size-3" />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}
