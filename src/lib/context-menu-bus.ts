import type { ReactNode } from "react";

export type ContextMenuEntry =
  | { kind: "separator" }
  | { kind: "label"; text: string }
  | {
      kind: "item";
      label: string;
      icon?: ReactNode;
      shortcut?: string;
      checked?: boolean;
      disabled?: boolean;
      destructive?: boolean;
      onSelect: () => void;
    };

export interface ContextMenuState {
  x: number;
  y: number;
  items: ContextMenuEntry[];
}

type Listener = () => void;
let currentMenu: ContextMenuState | null = null;
const listeners = new Set<Listener>();

function emit() {
  for (const fn of listeners) fn();
}

export function getContextMenuState(): ContextMenuState | null {
  return currentMenu;
}

export function subscribeContextMenu(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function openContextMenu(x: number, y: number, items: ContextMenuEntry[]) {
  if (!items.length) return;
  currentMenu = { x, y, items };
  emit();
}

export function closeContextMenu() {
  if (!currentMenu) return;
  currentMenu = null;
  emit();
}

/**
 * Decide whether the native browser context menu should stay enabled for a target
 * (text inputs, textareas, contentEditable regions, or active non-empty text selection).
 */
export function shouldAllowNativeContextMenu(target: EventTarget | null, selectionText = ""): boolean {
  if (selectionText.trim().length > 0) return true;
  if (!target || typeof target !== "object") return false;
  const el = target as { isContentEditable?: boolean; tagName?: string; closest?: (sel: string) => unknown };
  if (el.isContentEditable) return true;
  const tag = (el.tagName ?? "").toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA") return true;
  return Boolean(typeof el.closest === "function" && el.closest("input, textarea, [contenteditable='true']"));
}

export function clampMenuPosition(
  x: number,
  y: number,
  menuWidth: number,
  menuHeight: number,
  viewportW: number,
  viewportH: number,
  margin = 8,
): { left: number; top: number } {
  const maxLeft = Math.max(margin, viewportW - menuWidth - margin);
  const maxTop = Math.max(margin, viewportH - menuHeight - margin);
  return {
    left: Math.min(Math.max(margin, x), maxLeft),
    top: Math.min(Math.max(margin, y), maxTop),
  };
}
