import { useEffect } from "react";
import { toast } from "sonner";
import { GOOGLE_FONTS } from "./catalog";
import { loadCachedCatalog, scheduleCatalogSync } from "./google-api";
import { idbGet, persistStorageOnGesture, requestPersistentStorage } from "./idb";
import { refineLicense } from "./license";
import { isKnownGdiSessionIncapable, KNOWN_GDI_SESSION_INCAPABLE } from "./gdi-incapable";
import {
  blockPersistWrites,
  findFont,
  filterLibrary,
  poolForScope,
  setBootRestoringIds,
  sortLibrary,
  useFontStore,
} from "./store";
import { loadFont, noteDiskFamilies, primeGooglePreview } from "./loader";
import { indexDownloadedMetadata } from "./native-parse";
import { inferLocalStyle } from "./style-tags";
import { visibleFamilyNames } from "./visible-families";
import { orderPreferKeys, PREFER_FIRST_PAGE } from "./prefer-order.mjs";
import {
  restoreSessionFromDisk,
  rememberSessionFamilies,
  waitSessionBoot,
  syncManagedDocumentsRoot,
  bindDownloadEvents,
  installFontOnSystem,
} from "./os-activate";
import { inDesktopShell } from "@/lib/desktop/open-fonts";
import { applyDesktopPrefs } from "@/lib/desktop/prefs";
import { startWatchPolling } from "./watch-folder";
import { loadSystemFonts } from "./system-fonts";
import { hydrateLiveAxes } from "./live-axes";
import { hydratePersistedStore } from "./hydrate-steps";
import { loadLocalFontsMeta, pickLocalFontsPersist, saveLocalFontsMeta } from "./persist-local";
import type { FontRecord } from "./types";

async function reclassifyStoredLocalFonts(cancelled: () => boolean) {
  const { localFonts, collections } = useFontStore.getState();
  const targets = localFonts.filter((font) => !font.licenseUserSet);
  if (!targets.length) return;

  const namesFor = (id: string) =>
    collections.filter((collection) => collection.fontIds.includes(id)).map((collection) => collection.name);

  const next = localFonts.slice();
  const indexOf = new Map(next.map((font, index) => [font.id, index]));

  let cursor = 0;
  async function worker() {
    while (cursor < targets.length) {
      const index = cursor;
      cursor += 1;
      const font = targets[index]!;
      if (cancelled()) return;
      let refined = refineLicense(
        {
          license: font.license ?? "unknown",
          licenseName: font.licenseName,
          fileName: font.fileName,
        },
        { collectionNames: namesFor(font.id), fileName: font.fileName },
      );
      let family = font.family;
      let cssFamily = font.cssFamily;
      let kerningKey = font.kerningKey;
      let glyphCount = font.glyphCount;
      let category = font.category;
      let tags = font.tags;
      let metrics = font.metrics;
      let axes = font.axes;
      let otFeatures = font.otFeatures;
      const thinTags = !tags.length || (tags.length === 1 && tags[0] === "uploaded");
      const missingMeta = metrics && metrics.sFamilyClass === undefined && metrics.isFixedPitch === undefined;
      const needsMeta =
        !font.kerningKey || font.license === "unknown" || thinTags || Boolean(missingMeta);
      const needsParse =
        needsMeta && (!font.metrics || !font.kerningKey || font.license === "unknown");
      if (!needsMeta) continue;
      if (needsParse) {
        const blob = await idbGet(font.id);
        if (blob) {
          try {
            const { parseFontFile } = await import("./parse-font");
            const parsed = await parseFontFile(new File([blob], font.fileName || "font.ttf"));
            refined = refineLicense(parsed, {
              fileName: font.fileName,
              collectionNames: namesFor(font.id),
            });
            const fallback = (font.fileName ?? "").replace(/\.[^.]+$/, "").replace(/[-_]/g, " ");
            if (parsed.family && font.family === fallback) {
              family = parsed.family;
              cssFamily = parsed.family;
            }
            if (parsed.kerningKey) kerningKey = parsed.kerningKey;
            if (parsed.glyphCount) glyphCount = parsed.glyphCount;
            if (parsed.category) category = parsed.category;
            if (parsed.tags.length) tags = parsed.tags;
            if (parsed.metrics) metrics = parsed.metrics;
            if (parsed.axes.length) axes = parsed.axes;
            if (parsed.otFeatures.length) otFeatures = parsed.otFeatures;
          } catch {
            /* keep hint-based result */
          }
        }
      }
      const inferred = inferLocalStyle({
        family,
        fullName: font.fullName,
        fileName: `${font.fileName ?? ""} ${namesFor(font.id).join(" ")}`,
        sFamilyClass: metrics?.sFamilyClass,
        panose: metrics?.panose,
        widthClass: metrics?.widthClass,
        isFixedPitch: metrics?.isFixedPitch,
        hasLigatures: otFeatures?.includes("liga") || otFeatures?.includes("rlig"),
        axes,
      });
      if (inferred.fromName || (!font.metrics && metrics)) category = inferred.category;
      if (inferred.tags.length) {
        tags = Array.from(new Set([...inferred.tags, ...tags.filter((t) => t !== "uploaded")]));
      }
      const at = indexOf.get(font.id);
      if (at === undefined) continue;
      const current = next[at] as FontRecord;
      next[at] = {
        ...current,
        license: refined.license,
        licenseName: refined.licenseName || current.licenseName,
        family,
        cssFamily,
        kerningKey,
        glyphCount,
        category,
        tags,
        metrics,
        axes,
        otFeatures,
      };
    }
  }

  await Promise.all(Array.from({ length: Math.min(4, targets.length) }, () => worker()));
  if (cancelled()) return;
  useFontStore.setState({ localFonts: next });
}

export function useHydrateFonts() {
  useEffect(() => {
    let cancelled = false;
    let stopCatalog = () => {};
    let idleHandle = 0;
    let idleTimeout = 0;
    void bindDownloadEvents();
    void (async () => {
      // Launch persisted store hydration, local IndexedDB font metadata, and Tauri daemon check in parallel.
      const localMetaPromise = loadLocalFontsMeta().catch(() => null);
      const desktopProbePromise = inDesktopShell()
        .then(async (isDesktop) => {
          if (!isDesktop) return { desktop: false, daemonLive: false };
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            const daemonLive = Boolean(await invoke<boolean>("daemon_status"));
            return { desktop: true, daemonLive };
          } catch {
            return { desktop: true, daemonLive: false };
          }
        })
        .catch(() => ({ desktop: false, daemonLive: false }));

      const status = await hydratePersistedStore({
        loadCatalog: loadCachedCatalog,
        rehydrate: () => useFontStore.persist.rehydrate(),
        hasHydrated: () => useFontStore.persist.hasHydrated(),
        cancelled: () => cancelled,
        report: (step, err) => {
          console.error(`[fonths] startup ${step} load failed`, err);
          if (step !== "library") return;
          // Keep writes off: saving now would put defaults over the saved library.
          blockPersistWrites();
          toast.error("Couldn't load your saved library", {
            id: "hydrate-library-error",
            description:
              "Changes in this session won't be saved, so your saved favorites, collections and activations are not overwritten. Reload to try again.",
            duration: 20_000,
          });
        },
      });
      if (status === "cancelled" || cancelled) return;
      try {
        const fromLs = useFontStore.getState().localFonts;
        const fromIdb = await localMetaPromise;
        if (cancelled) return;
        const locals = pickLocalFontsPersist(fromIdb ?? [], fromLs);
        if (locals !== fromLs) useFontStore.setState({ localFonts: locals });
        if (locals.length) void saveLocalFontsMeta(locals);
        hydrateLiveAxes(useFontStore.getState().previewAxes);
        void applyDesktopPrefs(useFontStore.getState().desktopPrefs);
        const { collections, scope } = useFontStore.getState();
        if (
          typeof scope === "string" &&
          ((scope as string) === "disk" ||
            (scope.startsWith("collection:") &&
              !collections.some((c) => c.id === scope.slice("collection:".length))))
        ) {
          useFontStore.getState().setScope("all");
        }
      } catch (err) {
        // A restore step threw: still finish hydrate so the library works and saves.
        console.error("[fonths] startup restore step failed", err);
        toast.error("Some saved settings didn't load", {
          id: "hydrate-restore-error",
          description: "Your library still saves. Reload to try the rest again.",
        });
      }
      const { localFonts, setHydrated, googleFonts, selectedId } = useFontStore.getState();
      const localById = new Map(localFonts.map((f) => [f.id, f]));
      if (selectedId && selectedId.startsWith("l-") && !localById.has(selectedId)) {
        useFontStore.setState({ selectedId: null, inspectorOpen: false });
      }
      setHydrated(true);
      // 1.0.206b: seed GDI-incapable allowlist into settled on UI boot (mirror Rust seed).
      useFontStore.getState().addSettledFamilies(
        KNOWN_GDI_SESSION_INCAPABLE.map((e) => e.family),
      );
      useFontStore.getState().clearPendingActivate();
      void requestPersistentStorage();
      persistStorageOnGesture();

      const google = googleFonts.length ? googleFonts : GOOGLE_FONTS;
      void primeGooglePreview(google.slice(0, 24));
      const { favorites, recentIds } = useFontStore.getState();
      const favSet = new Set(favorites);
      const recentRank = new Map(recentIds.map((id, idx) => [id, idx]));
      // Priority-first session restore: selected, favorites, recents, and local uploads restore
      // first (< 1s) so active working fonts appear immediately before large background sets.
      const wantIds = Array.from(new Set(useFontStore.getState().activated))
        .filter((id) => !id.startsWith("l-") || localById.has(id))
        .sort((a, b) => {
          const score = (id: string) => {
            if (id === selectedId) return 0;
            if (favSet.has(id)) return 1;
            if (recentRank.has(id)) return 2;
            if (id.startsWith("l-")) return 3;
            return 4;
          };
          const sa = score(a);
          const sb = score(b);
          if (sa !== sb) return sa - sb;
          if (sa === 2) return (recentRank.get(a) ?? 0) - (recentRank.get(b) ?? 0);
          return 0;
        });
      const { desktop, daemonLive } = await desktopProbePromise;
      if (cancelled) return;
      if (desktop) {
        // Keep persisted IDs safe across localStorage writes while startup restore is running.
        setBootRestoringIds(wantIds);
        // Honest startup state: if the headless daemon is already holding the live Win32 Session
        // Table, show remembered active fonts immediately; on a cold boot, mark them pending until
        // Win32 Session Table restore actually confirms each family live.
        if (daemonLive) {
          useFontStore.getState().restoreActivation(wantIds, []);
        } else {
          useFontStore.getState().restoreActivation([], wantIds);
        }
        const persistNames: string[] = [];
        const seenPersist = new Set<string>();
        for (const id of wantIds) {
          const font = findFont(id, localFonts, google);
          if (!font || font.source === "system") continue;
          const famKey = font.family.trim().toLowerCase();
          if (famKey && !seenPersist.has(famKey)) {
            seenPersist.add(famKey);
            persistNames.push(font.family.trim());
          }
        }
        const flushRememberedSessionOnClose = () => {
          const st = useFontStore.getState();
          const activeIds = st.activated.length ? st.activated : wantIds;
          const names: string[] = [];
          const seen = new Set<string>();
          for (const id of activeIds) {
            const f = findFont(id, st.localFonts, st.googleFonts.length ? st.googleFonts : google);
            if (!f || f.source === "system") continue;
            const k = f.family.trim().toLowerCase();
            if (k && !seen.has(k)) {
              seen.add(k);
              names.push(f.family.trim());
            }
          }
          void rememberSessionFamilies(names);
        };
        window.addEventListener("beforeunload", flushRememberedSessionOnClose);
        window.addEventListener("pagehide", flushRememberedSessionOnClose);
        if (persistNames.length) {
          void rememberSessionFamilies(persistNames);
        }
        const liveIdsFromReadyNames = (readyNames: string[]): string[] => {
          const allow = new Set(readyNames.map((n) => n.trim().toLowerCase()).filter(Boolean));
          const live: string[] = [];
          const seen = new Set<string>();
          const seenFamilies = new Set<string>();
          const consider = (id: string) => {
            if (seen.has(id)) return;
            const font = findFont(id, localFonts, google);
            // Strict count honesty: never synthesize phantom `g:` IDs for families outside the
            // library — prevents Activated / Session GDI count from ever exceeding font count.
            if (!font || font.source === "system") return;
            const famKey = font.family.trim().toLowerCase();
            if (allow.has(famKey) && !seenFamilies.has(famKey)) {
              seen.add(id);
              seenFamilies.add(famKey);
              live.push(id);
            }
          };
          for (const id of wantIds) consider(id);
          const byFamily = new Map<string, string>();
          for (const font of [...google, ...localFonts]) {
            const key = font.family.trim().toLowerCase();
            if (key && !byFamily.has(key)) byFamily.set(key, font.id);
          }
          for (const name of readyNames) {
            const key = name.trim().toLowerCase();
            if (!allow.has(key) || seenFamilies.has(key)) continue;
            const matchedId = byFamily.get(key);
            if (matchedId) consider(matchedId);
          }
          return live;
        };
        // 1.0.190 progressive Live: flush session_boot.ready as Adds succeed —
        // do not wait for boot.done (~2099) before restoreActivation.
        // Never resurrect an ID the user explicitly deactivated during startup.
        const boot = await waitSessionBoot(180_000, (readyChunk, _done) => {
          if (cancelled) return;
          const stNow = useFontStore.getState();
          const bootLive = liveIdsFromReadyNames(readyChunk).filter(
            (id) => !stNow.pendingDeactivateSet.has(id),
          );
          if (daemonLive) {
            const stillWanted = wantIds.filter(
              (id) => stNow.activatedSet.has(id) && !stNow.pendingDeactivateSet.has(id),
            );
            const live = Array.from(new Set([...stillWanted, ...bootLive]));
            stNow.restoreActivation(live, []);
          } else {
            const liveSet = new Set(bootLive);
            const remainingPending = wantIds.filter(
              (id) => !liveSet.has(id) && !stNow.pendingDeactivateSet.has(id),
            );
            const live = bootLive;
            stNow.restoreActivation(live, remainingPending);
          }
        });
        const bootReady = boot.ready.slice();
        const sessionNames = bootReady;
        const bootSet = new Set(sessionNames.map((n) => n.trim().toLowerCase()));
        const settledNow = useFontStore.getState().settledFamilySet;
        const wantNames: string[] = [];
        const seenWant = new Set<string>();
        for (const n of [...persistNames, ...sessionNames]) {
          const t = n.trim();
          const k = t.toLowerCase();
          if (t && !seenWant.has(k)) {
            seenWant.add(k);
            wantNames.push(t);
          }
        }
        // already-Live this session (boot.ready / loaded) or known GDI-incapable Settled → skip re-walk.
        let needRegister = wantNames.filter((n) => {
          const k = n.trim().toLowerCase();
          return !bootSet.has(k) && !settledNow.has(k) && !isKnownGdiSessionIncapable(n);
        });
        // 1.0.206l prefer waves: selected → favorites → viewport → first-page (scope) → recent → remainder.
        if (needRegister.length > 1) {
          const state = useFontStore.getState();
          const familyOf = (id: string | null | undefined): string | null => {
            if (!id) return null;
            const font = findFont(id, localFonts, google);
            if (font) return font.family.trim().toLowerCase();
            if (id.startsWith("g:")) return id.slice(2).trim().toLowerCase();
            return null;
          };
          const favoriteNames = state.favorites
            .map((id) => familyOf(id))
            .filter((n): n is string => Boolean(n));
          const recentNames = state.recentIds
            .slice(0, 24)
            .map((id) => familyOf(id))
            .filter((n): n is string => Boolean(n));
          // First page of current library scope (not only mounted viewport).
          const liveIds = state.activated;
          const localPool =
            state.scope === "gfonts" || state.scope === "google" || state.scope === "system"
              ? []
              : localFonts;
          const pool = poolForScope(
            state.scope,
            localPool,
            google,
            state.systemFonts,
            liveIds,
          );
          const filtered = filterLibrary(
            pool,
            state.scope,
            state.query,
            state.favorites,
            liveIds,
            state.collections,
            state.customTags,
            state.facet,
            state.recentIds,
          );
          const sortMode =
            state.scope === "system" && (state.preview.sort ?? "name-asc") === "popular"
              ? "name-asc"
              : (state.preview.sort ?? "name-asc");
          const scoped =
            state.scope === "recent" ? filtered : sortLibrary(filtered, sortMode);
          const firstPageNames = scoped
            .slice(0, PREFER_FIRST_PAGE)
            .map((f) => f.family.trim().toLowerCase())
            .filter(Boolean);
          needRegister = orderPreferKeys(needRegister, {
            selected: familyOf(state.selectedId),
            favorites: favoriteNames,
            visible: visibleFamilyNames(),
            firstPage: firstPageNames,
            recent: recentNames,
            caseFold: true,
          });
        }
        const restore = needRegister.length
          ? restoreSessionFromDisk(needRegister)
          : Promise.resolve({ ready: [] as string[], missing: [] as string[], onDisk: [] as string[] });
        void restore
          .then(async (result) => {
            setBootRestoringIds(null);
            if (cancelled) return;
            const diskNames = result.onDisk;
            if (diskNames.length) {
              noteDiskFamilies(diskNames);
              useFontStore.getState().setDiskFamilies(diskNames);
            }
            const stBefore = useFontStore.getState();
            const allowNames = [...result.ready, ...sessionNames];
            const live = liveIdsFromReadyNames(allowNames).filter(
              (id) => !stBefore.pendingDeactivateSet.has(id),
            );
            // Re-activate persisted local / watched-folder fonts that weren't found under Documents/.
            // State honesty: await OS activation confirmation before marking Live (never guess).
            for (const id of wantIds) {
              if (cancelled) return;
              const cur = useFontStore.getState();
              if (cur.pendingDeactivateSet.has(id) || !cur.activatedSet.has(id)) continue;
              const localFont = localById.get(id);
              if (localFont && !live.includes(id)) {
                const ok = await installFontOnSystem(localFont).catch(() => false);
                if (ok && !useFontStore.getState().pendingDeactivateSet.has(id)) {
                  live.push(id);
                }
              }
            }
            if (cancelled) return;
            const stAfter = useFontStore.getState();
            const finalLive = live.filter((id) => !stAfter.pendingDeactivateSet.has(id));
            stAfter.restoreActivation(finalLive, []);
            void rememberSessionFamilies(
              finalLive
                .map((id) => findFont(id, localFonts, google)?.family ?? (id.startsWith("g:") ? id.slice(2) : ""))
                .filter((name): name is string => Boolean(name)),
            );
            // Card 36: no automatic prune at start. Folders not in the catalog stay
            // (uploads, renamed or delisted families); Scan → "Remove extras" moves them to
            // the Recycle Bin only when the user asks.
            if (!cancelled) void syncManagedDocumentsRoot();
          })
          .catch(() => {
            setBootRestoringIds(null);
            if (!cancelled) void syncManagedDocumentsRoot();
          });
      }
      const liveIds = useFontStore.getState().activated;
      const toLoad = liveIds.slice(0, 12).map((id) => findFont(id, localFonts, google));
      void Promise.all(toLoad.map((font) => (font ? loadFont(font) : Promise.resolve())));

      const scheduleIdle = () => {
        if (cancelled) return;
        void reclassifyStoredLocalFonts(() => cancelled);
        const curLocals = useFontStore.getState().localFonts;
        const watchedExtras: [string, string][] = [];
        const seenFam = new Set<string>();
        for (const f of curLocals) {
          if (!f.originPath) continue;
          const famKey = f.family.trim().toLowerCase();
          if (!famKey || seenFam.has(famKey)) continue;
          if (f.metrics?.xh !== undefined && f.otFeatures !== undefined) continue;
          seenFam.add(famKey);
          watchedExtras.push([f.family, f.originPath]);
        }
        void indexDownloadedMetadata(watchedExtras).then((rows) => {
          if (!cancelled && rows?.length) {
            useFontStore.getState().applyBulkMetadataIndex(rows);
          }
        });
      };
      if (typeof requestIdleCallback === "function") {
        idleHandle = requestIdleCallback(scheduleIdle, { timeout: 2_500 });
      } else {
        idleTimeout = window.setTimeout(scheduleIdle, 1_500);
      }

      startWatchPolling();
      window.setTimeout(() => {
        if (!cancelled) void loadSystemFonts();
      }, 500);

      if (!cancelled) stopCatalog = scheduleCatalogSync();
    })();
    return () => {
      cancelled = true;
      stopCatalog();
      if (idleHandle && typeof cancelIdleCallback === "function") cancelIdleCallback(idleHandle);
      if (idleTimeout) clearTimeout(idleTimeout);
    };
  }, []);
}
