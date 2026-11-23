import { toast } from "sonner";
import {
  forbiddenWatchReason,
  inDesktopShell,
  isManagedFontManagerRoot,
  listWatchFolder,
  pickWatchFolder,
} from "@/lib/desktop/open-fonts";
import { managedDocumentsRoot, syncManagedDocumentsRoot } from "./os-activate";
import { isOwnMarkerOnlyEvent } from "./watch-event-filter.ts";
import {
  findImportGoogleCollisions,
  requestImportCollisionChoice,
} from "./import-collision";
import {
  analyzeImportBatch,
  defaultImportSelection,
  filterImportFiles,
  filterOriginPathsByRule,
  originPathsToFileLikes,
  requestImportOptions,
  shouldPromptImportOptions,
  toSavedImportRule,
  type SavedImportRule,
} from "./import-options";
import { useFontStore } from "./store";

function norm(path: string) {
  return path.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

function underWatch(origin: string, root: string) {
  const a = norm(origin);
  const b = norm(root);
  return a === b || a.startsWith(`${b}/`);
}

let pollTimer = 0;
let lastSig = "";
let inflight = false;
let queued = false;
let started = false;
const unwatchers = new Map<string, () => void>();
let managedUnwatch: (() => void) | null = null;
let managedRootKey = "";
let managedDebounce = 0;
let managedPoll = 0;
let managedVisBound = false;

function signature(paths: string[], sizes: number[], mtimes: number[]) {
  return paths
    .map((path, i) => `${norm(path)}:${sizes[i] ?? 0}:${mtimes[i] ?? 0}`)
    .sort()
    .join("|");
}

export async function addWatchedFolder(): Promise<void> {
  const picked = await pickWatchFolder();
  if (picked === "web") {
    toast.message("Watch folders need the desktop app", {
      description: "Build with deploy.bat. In the browser, use Folder to import a copy instead.",
    });
    return;
  }
  if (!picked) return;
  if (picked.blocked) {
    toast.message("That folder cannot be watched", { description: picked.blocked });
    return;
  }
  const blocked = forbiddenWatchReason(picked.path);
  if (blocked) {
    toast.message("That folder cannot be watched", { description: blocked });
    return;
  }
  const store = useFontStore.getState();
  const existing = store.collections.find((c) => c.watchPath && norm(c.watchPath) === norm(picked.path));
  let originPaths = picked.originPaths.slice();
  let autoActivate = existing?.autoActivate ?? true;
  let importFilter: SavedImportRule | undefined = existing?.importFilter;
  let skippedByRule = 0;

  if (originPaths.length) {
    const likes = originPathsToFileLikes(originPaths, picked.path);
    const analysis = analyzeImportBatch(likes, store.localFonts);
    if (shouldPromptImportOptions(analysis)) {
      const selection = await requestImportOptions(analysis, {
        mode: "watch",
        folderLabel: picked.name,
      });
      if (!selection) return;
      const filtered = filterImportFiles(likes, analysis, selection);
      const keptIdx = new Set(filtered.files.map((item) => likes.indexOf(item)).filter((i) => i >= 0));
      originPaths = originPaths.filter((_, i) => keptIdx.has(i));
      skippedByRule = filtered.skippedCount;
      autoActivate = selection.autoActivate;
      importFilter = toSavedImportRule(selection);
    }
    if (originPaths.length) {
      const remainingLikes = originPathsToFileLikes(originPaths, picked.path);
      const latest = useFontStore.getState();
      const { collisions, collidingFileIndices } = findImportGoogleCollisions(
        remainingLikes,
        latest.googleFonts,
        latest.activatedSet,
        latest.diskFamilySet,
        latest.localFonts,
      );
      if (collisions.length > 0) {
        const choice = await requestImportCollisionChoice(collisions, remainingLikes.length);
        if (choice === "cancel") return;
        if (choice === "skip-colliding") {
          originPaths = originPaths.filter((_, idx) => !collidingFileIndices.has(idx));
          skippedByRule += collidingFileIndices.size;
          const excludedKeys = collisions.map((c) => c.familyKey);
          const baseRule = importFilter ?? toSavedImportRule(defaultImportSelection(analysis));
          importFilter = {
            ...baseRule,
            excludeFamilies: Array.from(new Set([...(baseRule.excludeFamilies ?? []), ...excludedKeys])),
          };
          if (!originPaths.length) {
            toast.message("All colliding fonts skipped", {
              description: "Skipped watched-folder fonts that matched existing Google or library families.",
            });
            return;
          }
        } else if (choice === "prefer-local") {
          const activeCollidingIds = collisions
            .filter((c) => c.isGoogleActive)
            .map((c) => c.googleFont.id);
          if (activeCollidingIds.length > 0) {
            useFontStore.getState().setActivatedMany(activeCollidingIds, false);
          }
        }
      }
    }
  }

  const id = existing?.id ?? store.addCollection(picked.name);
  store.setCollectionWatch(id, picked.path, autoActivate);
  if (importFilter) {
    store.setCollectionImportFilter(id, importFilter);
  }
  store.setScope(`collection:${id}`);
  if (originPaths.length) {
    const result = await store.importOriginPaths(originPaths, {
      collectionId: id,
      collectionName: picked.name,
    });
    const skippedNote = skippedByRule > 0 ? ` (${skippedByRule.toLocaleString()} filtered out by smart rules)` : "";
    toast.success(`Watching ${picked.name}`, {
      description: `${result.added.toLocaleString()} typeface${result.added === 1 ? "" : "s"} linked in place${skippedNote}. Files stay in that folder; auto-activate is ${autoActivate ? "on" : "off"}.`,
    });
  } else {
    toast.message(`Watching ${picked.name}`, {
      description: "No matching font files yet. New fonts dropped into that folder will appear automatically.",
    });
  }
  lastSig = "";
  void bindNativeWatch();
  void refreshWatchedFolders();
}

export async function refreshWatchedFolders(): Promise<void> {
  if (inflight) {
    queued = true;
    return;
  }
  if (!(await inDesktopShell())) return;
  inflight = true;
  try {
    const { collections, localFonts, importOriginPaths, removeLocalFont } = useFontStore.getState();
    const watched = collections.filter((c) => c.watchPath);
    if (!watched.length) return;
    const allOrigins: string[] = [];
    const allSizes: number[] = [];
    const allMtimes: number[] = [];
    for (const folder of watched) {
      // If user stopped watching this folder while a refresh was running, skip it immediately.
      if (!useFontStore.getState().collections.some((c) => c.id === folder.id && c.watchPath)) {
        continue;
      }
      const root = folder.watchPath!;
      if (isManagedFontManagerRoot(root)) {
        // Never treat managed store as a watch folder — sync library from Documents instead.
        await syncManagedDocumentsRoot();
        continue;
      }
      const blocked = forbiddenWatchReason(root);
      if (blocked) continue;
      const listed = await listWatchFolder(root);
      if (!useFontStore.getState().collections.some((c) => c.id === folder.id && c.watchPath)) {
        continue;
      }
      allOrigins.push(...listed.paths);
      allSizes.push(...listed.sizes);
      allMtimes.push(...listed.mtimes);
      const known = localFonts.filter((f) => f.originPath && underWatch(f.originPath, root));
      // A TTC/OTC path holds one record per face.
      const knownByPath = new Map<string, typeof known>();
      for (const f of known) {
        const key = norm(f.originPath!);
        knownByPath.set(key, [...(knownByPath.get(key) ?? []), f]);
      }
      const freshPaths: string[] = [];
      for (let i = 0; i < listed.paths.length; i += 1) {
        const origin = listed.paths[i]!;
        const prev = knownByPath.get(norm(origin)) ?? [];
        if (prev.length && (prev[0]!.fileSize ?? 0) === listed.sizes[i]) continue;
        for (const face of prev) await removeLocalFont(face.id);
        freshPaths.push(origin);
      }
      const filteredFresh =
        freshPaths.length && folder.importFilter
          ? filterOriginPathsByRule(freshPaths, root, folder.importFilter, localFonts)
          : freshPaths;
      if (
        filteredFresh.length &&
        useFontStore.getState().collections.some((c) => c.id === folder.id && c.watchPath)
      ) {
        await importOriginPaths(filteredFresh, {
          collectionId: folder.id,
          collectionName: folder.name,
        });
      }
    }
    const sig = signature(allOrigins, allSizes, allMtimes);
    if (sig === lastSig) return;
    lastSig = sig;
    const live = new Set(allOrigins.map(norm));
    const stale = useFontStore
      .getState()
      .localFonts.filter(
        (f) =>
          f.originPath &&
          watched.some((w) => underWatch(f.originPath!, w.watchPath!)) &&
          !live.has(norm(f.originPath)),
      );
    for (const font of stale) {
      await removeLocalFont(font.id);
    }
  } finally {
    inflight = false;
    if (queued) {
      queued = false;
      void refreshWatchedFolders();
    }
  }
}

async function bindNativeWatch() {
  if (!(await inDesktopShell())) return;
  const watched = useFontStore
    .getState()
    .collections.filter((c) => c.watchPath && !forbiddenWatchReason(c.watchPath));
  const live = new Set(watched.map((c) => norm(c.watchPath!)));
  for (const [key, stop] of unwatchers) {
    if (live.has(key)) continue;
    try {
      stop();
    } catch {
      /* ignore */
    }
    unwatchers.delete(key);
  }
  if (!watched.length) return;
  try {
    const { watch } = await import("@tauri-apps/plugin-fs");
    let debounce = 0;
    const kick = () => {
      window.clearTimeout(debounce);
      debounce = window.setTimeout(() => void refreshWatchedFolders(), 280);
    };
    for (const folder of watched) {
      const key = norm(folder.watchPath!);
      if (unwatchers.has(key)) continue;
      try {
        const stop = await watch(folder.watchPath!, () => kick(), {
          delayMs: 280,
          recursive: true,
        });
        unwatchers.set(key, stop);
      } catch {
        /* installer without fs watch — poll instead */
      }
    }
  } catch {
    /* website bundle */
  }
}


function kickManagedSync() {
  window.clearTimeout(managedDebounce);
  managedDebounce = window.setTimeout(() => {
    void syncManagedDocumentsRoot();
  }, 320);
}

/** App-owned live sync for Documents\Font Manager. Not a user watch folder —
 * forbiddenWatch still blocks picking this root / Windows Fonts. */
async function bindManagedRootWatch() {
  if (!(await inDesktopShell())) return;
  const root = await managedDocumentsRoot();
  if (!root) return;
  // Only the managed Documents store — never Windows Fonts.
  const n = root.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  if (n.includes("/windows/fonts")) return;
  if (
    !isManagedFontManagerRoot(root) &&
    !n.endsWith("/documents/fonths") &&
    !n.endsWith("/documents/font manager") &&
    !n.endsWith("/documents/font%20manager")
  ) {
    return;
  }
  const key = norm(root);
  if (managedUnwatch && managedRootKey === key) return;
  if (managedUnwatch) {
    try {
      managedUnwatch();
    } catch {
      /* ignore */
    }
    managedUnwatch = null;
  }
  managedRootKey = key;
  try {
    const { watch } = await import("@tauri-apps/plugin-fs");
    managedUnwatch = await watch(root, (ev) => {
      if (isOwnMarkerOnlyEvent(ev)) return;
      kickManagedSync();
    }, {
      delayMs: 320,
      recursive: true,
    });
  } catch {
    /* installer without fs watch — poll below */
  }
  const tickManagedPoll = () => {
    const hidden = typeof document !== "undefined" && document.visibilityState === "hidden";
    const native = Boolean(managedUnwatch);
    const ms = hidden ? 45_000 : native ? 25_000 : 10_000;
    window.clearInterval(managedPoll);
    managedPoll = window.setInterval(() => void syncManagedDocumentsRoot(), ms);
  };
  tickManagedPoll();
  if (typeof document !== "undefined" && !managedVisBound) {
    managedVisBound = true;
    document.addEventListener("visibilitychange", () => {
      tickManagedPoll();
      if (document.visibilityState === "visible") void syncManagedDocumentsRoot();
    });
  }
}

export function startWatchPolling() {
  if (started || typeof window === "undefined") return;
  started = true;
  window.setTimeout(() => {
    void bindManagedRootWatch();
    void syncManagedDocumentsRoot();
    void bindNativeWatch();
    void refreshWatchedFolders();
    const tick = () => {
      const hidden = typeof document !== "undefined" && document.visibilityState === "hidden";
      const native = unwatchers.size > 0;
      const ms = hidden ? 30_000 : native ? 20_000 : 8_000;
      window.clearInterval(pollTimer);
      pollTimer = window.setInterval(() => void refreshWatchedFolders(), ms);
    };
    tick();
    document.addEventListener("visibilitychange", () => {
      tick();
      if (document.visibilityState === "visible") void refreshWatchedFolders();
    });
  }, 8000);
}
