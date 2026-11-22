import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  clampMenuPosition,
  closeContextMenu,
  getContextMenuState,
  subscribeContextMenu,
  type ContextMenuState,
} from "@/lib/context-menu-bus";
import { cn } from "@/lib/utils";

export function AppContextMenu() {
  const [menu, setMenu] = useState<ContextMenuState | null>(getContextMenuState);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>({ left: 0, top: 0 });

  useEffect(() => subscribeContextMenu(() => setMenu(getContextMenuState())), []);

  useLayoutEffect(() => {
    if (!menu) return;
    const el = ref.current;
    const w = el?.offsetWidth ?? 220;
    const h = el?.offsetHeight ?? 260;
    setPos(clampMenuPosition(menu.x, menu.y, w, h, window.innerWidth, window.innerHeight));
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const onPointerDown = (e: PointerEvent) => {
      if (ref.current && e.target instanceof Node && ref.current.contains(e.target)) return;
      closeContextMenu();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeContextMenu();
      }
    };
    const onDismiss = () => closeContextMenu();
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("scroll", onDismiss, true);
    window.addEventListener("resize", onDismiss);
    window.addEventListener("blur", onDismiss);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("scroll", onDismiss, true);
      window.removeEventListener("resize", onDismiss);
      window.removeEventListener("blur", onDismiss);
    };
  }, [menu]);

  if (!menu) return null;

  return (
    <div
      ref={ref}
      role="menu"
      data-testid="app-context-menu"
      style={{ left: pos.left, top: pos.top }}
      onContextMenu={(e) => e.preventDefault()}
      className="fixed z-50 min-w-52 max-w-72 overflow-hidden rounded-lg border border-border bg-popover p-1 text-popover-foreground shadow-lg"
    >
      {menu.items.map((entry, idx) => {
        if (entry.kind === "separator") {
          return <div key={`sep-${idx}`} role="separator" className="-mx-1 my-1 h-px bg-border" />;
        }
        if (entry.kind === "label") {
          return (
            <div
              key={`lbl-${idx}`}
              className="truncate px-2 py-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground"
            >
              {entry.text}
            </div>
          );
        }
        return (
          <button
            key={`item-${idx}-${entry.label}`}
            type="button"
            role="menuitem"
            disabled={entry.disabled}
            onClick={(e) => {
              e.stopPropagation();
              if (entry.disabled) return;
              closeContextMenu();
              entry.onSelect();
            }}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
              entry.disabled
                ? "cursor-not-allowed opacity-45"
                : entry.destructive
                  ? "text-destructive hover:bg-destructive/10 focus:bg-destructive/10"
                  : "text-popover-foreground hover:bg-accent hover:text-accent-foreground focus:bg-accent",
            )}
          >
            {entry.icon ? <span className="flex size-3.5 shrink-0 items-center justify-center">{entry.icon}</span> : null}
            <span className="min-w-0 flex-1 truncate">{entry.label}</span>
            {entry.checked ? <span className="text-[11px] font-semibold text-primary">✓</span> : null}
            {entry.shortcut ? (
              <span className="ml-2 shrink-0 font-mono text-[10px] text-muted-foreground">{entry.shortcut}</span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
