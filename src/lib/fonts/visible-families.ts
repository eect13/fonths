/** Viewport-visible family names for Activate visible / Activate All / restore prefer-first. */
const visible = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;
let notifyRaf = 0;
let domListenersBound = false;

function key(name: string): string {
  return name.trim().toLowerCase();
}

function scheduleNotify(): void {
  version = (version + 1) | 0;
  if ( !listeners.size) return;
  if (typeof window !== "undefined" && typeof window.requestAnimationFrame === "function") {
    if (notifyRaf) return;
    notifyRaf = window.requestAnimationFrame(() => {
      notifyRaf = 0;
      for (const fn of listeners) fn();
    });
    return;
  }
  for (const fn of listeners) fn();
}

function bindViewportScrollNotify(): void {
  if (domListenersBound || typeof window === "undefined" || typeof document === "undefined") return;
  domListenersBound = true;
  const onViewportChange = () => scheduleNotify();
  window.addEventListener("scroll", onViewportChange, { capture: true, passive: true });
  window.addEventListener("resize", onViewportChange, { passive: true });
}

export function subscribeVisibleFamilies(fn: () => void): () => void {
  bindViewportScrollNotify();
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function visibleFamiliesVersion(): number {
  return version;
}

export function noteFamilyVisible(family: string, isVisible: boolean): void {
  const k = key(family);
  if (!k) return;
  const had = visible.has(k);
  if (isVisible) {
    if (!had) {
      visible.add(k);
      scheduleNotify();
    }
  } else if (had) {
    visible.delete(k);
    scheduleNotify();
  }
}

/** Measure strictly what the user can currently see inside `[data-library-scroll]`. */
function measureOnScreenFamilies(): Set<string> | null {
  if (typeof document === "undefined") return null;
  const scroller = document.querySelector("[data-library-scroll]") as HTMLElement | null;
  if (!scroller) return null;
  const cards = scroller.querySelectorAll<HTMLElement>("[data-font-card-family]");
  if (!cards.length) return null;
  const sr = scroller.getBoundingClientRect();
  if (sr.height <= 0 || sr.width <= 0) return null;
  const winH = typeof window !== "undefined" ? window.innerHeight || sr.bottom : sr.bottom;
  const winW = typeof window !== "undefined" ? window.innerWidth || sr.right : sr.right;
  const viewTop = Math.max(0, sr.top);
  const viewBottom = Math.min(winH, sr.bottom);
  const viewLeft = Math.max(0, sr.left);
  const viewRight = Math.min(winW, sr.right);
  if (viewBottom <= viewTop || viewRight <= viewLeft) return new Set();

  const onScreen = new Set<string>();
  for (const card of cards) {
    const fam = card.getAttribute("data-font-card-family");
    if (!fam) continue;
    const k = key(fam);
    if (!k) continue;
    const r = card.getBoundingClientRect();
    if (r.height <= 0 || r.width <= 0) continue;
    const overlapY = Math.min(r.bottom, viewBottom) - Math.max(r.top, viewTop);
    const overlapX = Math.min(r.right, viewRight) - Math.max(r.left, viewLeft);
    const minVisibleY = Math.min(Math.max(24, r.height * 0.35), viewBottom - viewTop);
    if (overlapY >= minVisibleY && overlapX > 16) {
      onScreen.add(k);
    }
  }
  return onScreen;
}

export function visibleFamilyNames(): string[] {
  return Array.from(visibleFamilySet());
}

export function visibleFamilySet(): Set<string> {
  const measured = measureOnScreenFamilies();
  if (measured) {
    visible.clear();
    for (const k of measured) visible.add(k);
    return measured;
  }
  return new Set(visible);
}
