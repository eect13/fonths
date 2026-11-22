import { toast } from "sonner";
import {
  FONT_BY_ID,
  GOOGLE_DIRECTORY,
  GOOGLE_FONTS,
  type SnapshotRow,
  familyKey,
  googleFontId,
  isGoogleCatalog,
  replaceGoogleCatalog,
  replaceGoogleDirectory,
  snapshotToRecord,
} from "./catalog";
import { CATALOG_CACHE_VERSION, healCachedCatalogFont } from "./heal-catalog";
import { idbGet, idbPut } from "./idb";
import type { FontRecord } from "./types";
import { APP_VERSION } from "../../version";

const CATALOG_CACHE_ID = "catalog:live";
const BUNDLED_COUNT = GOOGLE_FONTS.length;
const STALE_MS = 24 * 60 * 60 * 1000;
const IDLE_MS = 4_000;
const IDLE_CAP_MS = 8_000;

export type CatalogSyncResult = {
  count: number;
  added: number;
  skipped?: boolean;
  cdnChecked?: number;
  cdnQueued?: number;
  cdnOutdated?: number;
};

type CatalogCache = {
  v: 1 | typeof CATALOG_CACHE_VERSION;
  savedAt: number;
  fonts: FontRecord[];
  directory?: string[];
};

type CatalogSyncSnap = {
  busy: boolean;
  syncedAt: number;
};

let busy = false;
let syncedAt = 0;
let snap: CatalogSyncSnap = { busy: false, syncedAt: 0 };
const listeners = new Set<() => void>();
let inflight: Promise<CatalogSyncResult | null> | null = null;

function emitSync() {
  snap = { busy, syncedAt };
  for (const fn of listeners) fn();
}

export function subscribeCatalogSync(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getCatalogSyncState(): CatalogSyncSnap {
  return snap;
}

function setBusy(value: boolean) {
  if (busy === value) return;
  busy = value;
  emitSync();
}

function markSynced(at: number) {
  if (syncedAt === at) return;
  syncedAt = at;
  emitSync();
}

function catalogOf(family: string, dir: Set<string> = GOOGLE_DIRECTORY): "google" | "other" {
  return dir.has(familyKey(family)) ? "google" : "other";
}

export { CATALOG_CACHE_VERSION, healCachedCatalogFont } from "./heal-catalog";

function validCachedFonts(fonts: unknown): fonts is FontRecord[] {
  if (!Array.isArray(fonts) || fonts.length < BUNDLED_COUNT) return false;
  return fonts.every((font) => {
    if (!font || typeof font !== "object") return false;
    const rec = font as FontRecord;
    return (
      typeof rec.family === "string" &&
      rec.family.length > 0 &&
      typeof rec.id === "string" &&
      rec.id.startsWith("g:") &&
      typeof rec.category === "string" &&
      Array.isArray(rec.weights)
    );
  });
}

function withOfficialLanes(fonts: FontRecord[], dir: Set<string> = GOOGLE_DIRECTORY): FontRecord[] {
  return fonts.map((font) => ({
    ...font,
    catalog: catalogOf(font.family, dir),
  }));
}

function applyLiveCatalog(fonts: FontRecord[], dir: Set<string> = GOOGLE_DIRECTORY) {
  const official = withOfficialLanes(fonts, dir).filter(isGoogleCatalog);
  replaceGoogleCatalog(official.length ? official : GOOGLE_FONTS.filter(isGoogleCatalog));
  void import("./store").then(({ useFontStore }) => {
    useFontStore.getState().setGoogleFonts(GOOGLE_FONTS.slice());
    useFontStore.getState().setCatalogLive(true);
  });
}

export async function loadCachedCatalog(): Promise<boolean> {
  try {
    const blob = await idbGet(CATALOG_CACHE_ID);
    if (!blob || typeof blob.text !== "function") return false;
    const parsed = JSON.parse(await blob.text()) as Partial<CatalogCache>;
    if (parsed.v !== 1 && parsed.v !== CATALOG_CACHE_VERSION) return false;
    if (!validCachedFonts(parsed.fonts)) return false;
    if (Array.isArray(parsed.directory) && parsed.directory.length >= BUNDLED_COUNT) {
      replaceGoogleDirectory(parsed.directory.filter((s): s is string => typeof s === "string"));
    }
    const bundledVar = new Map(GOOGLE_FONTS.map((font) => [font.family.toLowerCase(), font] as const));
    const fonts = parsed.fonts
      .map((font) => {
        const bundled = bundledVar.get(font.family.toLowerCase());
        const healed = healCachedCatalogFont(font, bundled);
        return {
          ...font,
          source: "google" as const,
          id: font.id.startsWith("g:") ? font.id : googleFontId(font.family),
          catalog: catalogOf(font.family),
          weights: font.weights?.length ? font.weights : [400],
          catalogVariable: healed.catalogVariable,
          variable: false,
        };
      })
      .filter(isGoogleCatalog);
    applyLiveCatalog(fonts);
    if (typeof parsed.savedAt === "number" && parsed.savedAt > 0) markSynced(parsed.savedAt);
    return true;
  } catch {
    return false;
  }
}

function isValidSnapshotRows(raw: unknown): raw is SnapshotRow[] {
  if (!Array.isArray(raw) || raw.length < 1000) return false;
  return raw.every(
    (row) =>
      Array.isArray(row) &&
      row.length >= 7 &&
      typeof row[0] === "string" &&
      row[0].trim().length > 0 &&
      typeof row[1] === "string" &&
      Array.isArray(row[2]),
  );
}

/** Google Fonts is the only remote catalog. In desktop mode, fetches live rows via Rust (no browser CORS block). */
export async function refreshGoogleCatalog(force = false): Promise<CatalogSyncResult | null> {
  if (!force && syncedAt > 0 && Date.now() - syncedAt < STALE_MS) {
    return { count: GOOGLE_FONTS.length, added: 0, skipped: true };
  }
  try {
    const core = await import("@tauri-apps/api/core");
    if (typeof core.isTauri === "function" && core.isTauri()) {
      const rows = await core.invoke<SnapshotRow[]>("fetch_live_google_catalog");
      if (isValidSnapshotRows(rows)) {
        const prevKeys = new Set(GOOGLE_FONTS.map((f) => familyKey(f.family)));
        const dirList = rows.map((r) => r[0]);
        replaceGoogleDirectory(dirList);
        const records = rows.map(snapshotToRecord);
        const liveDir = new Set(dirList.map(familyKey));
        applyLiveCatalog(records, liveDir);
        const added = GOOGLE_FONTS.filter((f) => !prevKeys.has(familyKey(f.family))).length;
        const now = Date.now();
        markSynced(now);
        const payload: CatalogCache = {
          v: CATALOG_CACHE_VERSION,
          savedAt: now,
          fonts: GOOGLE_FONTS.slice(),
          directory: dirList,
        };
        await idbPut(
          CATALOG_CACHE_ID,
          new Blob([JSON.stringify(payload)], { type: "application/json" }),
        ).catch(() => undefined);
        return { count: GOOGLE_FONTS.length, added };
      }
      if (force) return null;
    }
  } catch {
    if (force) return null;
  }
  const official = GOOGLE_FONTS.filter(isGoogleCatalog);
  applyLiveCatalog(official);
  markSynced(Date.now());
  return { count: official.length, added: 0 };
}

function notifyCatalog(result: CatalogSyncResult, force: boolean) {
  if (result.skipped) return;
  const summary = `Google Fonts ${result.count.toLocaleString()}`;
  const cdnNote =
    result.cdnQueued && result.cdnQueued > 0
      ? `Downloading ${result.cdnQueued.toLocaleString()} updated Google famil${result.cdnQueued === 1 ? "y" : "ies"} from CDN.`
      : result.cdnChecked && result.cdnChecked > 0
        ? `All ${result.cdnChecked.toLocaleString()} downloaded Google families are current.`
        : "No downloaded Google families needed updates.";
  if (result.added > 0) {
    toast.success(
      `Catalog updated — ${result.added.toLocaleString()} new typeface${result.added === 1 ? "" : "s"}`,
      {
        id: "fm-google-sync",
        description: `${summary}. ${cdnNote}`,
      },
    );
    return;
  }
  if (force || (result.cdnQueued && result.cdnQueued > 0)) {
    toast.message(
      result.cdnQueued && result.cdnQueued > 0
        ? `Updating ${result.cdnQueued.toLocaleString()} Google font famil${result.cdnQueued === 1 ? "y" : "ies"}`
        : "Google Fonts catalog & files are current",
      {
        id: "fm-google-sync",
        description: `${summary}. ${cdnNote}`,
      },
    );
  }
}

/** Explicitly check and re-download outdated or all downloaded Google Fonts from CDN. */
export async function syncGoogleCdnFiles(force = false) {
  const { syncGoogleCdnUpdates } = await import("./native-parse");
  const cdn = await syncGoogleCdnUpdates(force);
  if (!cdn) {
    toast.error("Google CDN file sync requires the Windows desktop app.", { id: "fm-google-cdn" });
    return null;
  }
  if (cdn.queued > 0) {
    toast.success(
      `Queued ${cdn.queued.toLocaleString()} Google famil${cdn.queued === 1 ? "y" : "ies"} for CDN download`,
      {
        id: "fm-google-cdn",
        description: `${cdn.outdated} outdated · ${cdn.incomplete} incomplete out of ${cdn.checked} downloaded.`,
      },
    );
  } else {
    toast.message("Downloaded Google Fonts are up to date", {
      id: "fm-google-cdn",
      description: `Checked ${cdn.checked.toLocaleString()} downloaded Google famil${cdn.checked === 1 ? "y" : "ies"}.`,
    });
  }
  return cdn;
}

export function syncFontCatalog(opts?: {
  force?: boolean;
  notify?: boolean;
}): Promise<CatalogSyncResult | null> {
  const force = Boolean(opts?.force);
  const notify = Boolean(opts?.notify);
  if (inflight) return inflight;
  setBusy(true);
  inflight = (async () => {
    const result = await refreshGoogleCatalog(force);
    if (!result) {
      if (notify) {
        toast.error("Could not refresh Google Fonts", {
          id: "fm-google-sync",
          description: "Using the shipped list. Try again when you are online.",
        });
      }
      return null;
    }
    // Automatically check downloaded Google Fonts against live CDN revisions and queue updates.
    if (!result.skipped) {
      try {
        const { syncGoogleCdnUpdates } = await import("./native-parse");
        const cdn = await syncGoogleCdnUpdates(false);
        if (cdn) {
          result.cdnChecked = cdn.checked;
          result.cdnQueued = cdn.queued;
          result.cdnOutdated = cdn.outdated;
        }
      } catch {
        /* web preview or offline */
      }
    }
    if (result.added > 0 || (result.cdnQueued && result.cdnQueued > 0) || notify) {
      notifyCatalog(result, notify);
    }
    if (notify) {
      // Always probe latest release on Refresh; only toast when behind (avoid double noise).
      const rel = await checkLatestGitHubRelease();
      if (rel.updateAvailable && rel.latest) {
        toast.message(`App update available — ${rel.latest}`, {
          description: `Installed ${rel.installed}. ${rel.releaseUrl ?? "See GitHub releases"}.`,
        });
      }
    }
    return result;
  })()
    .catch(() => null)
    .finally(() => {
      inflight = null;
      setBusy(false);
    });
  return inflight;
}

export function scheduleCatalogSync(): () => void {
  let cancelled = false;
  let idleId = 0;
  let timeoutId = 0;
  const start = () => {
    if (cancelled) return;
    if (syncedAt && Date.now() - syncedAt < STALE_MS) return;
    void syncFontCatalog({ force: false, notify: false });
  };
  timeoutId = window.setTimeout(() => {
    if (cancelled) return;
    if (typeof requestIdleCallback === "function") {
      idleId = requestIdleCallback(start, { timeout: IDLE_CAP_MS });
    } else {
      start();
    }
  }, IDLE_MS);
  return () => {
    cancelled = true;
    if (idleId && typeof cancelIdleCallback === "function") cancelIdleCallback(idleId);
    if (timeoutId) clearTimeout(timeoutId);
  };
}

export function googleFontLookup(id: string): FontRecord | undefined {
  return FONT_BY_ID.get(id);
}

export type AppReleaseCheck = {
  installed: string;
  latest: string | null;
  updateAvailable: boolean;
  releaseUrl: string | null;
};

/** Compare installed app version to GitHub latest release tag (v1.0.x). */
export async function checkLatestGitHubRelease(): Promise<AppReleaseCheck> {
  const installed = APP_VERSION;
  const releaseUrl = "https://github.com/eect13/fonths/releases/latest";
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 12_000);
    const res = await fetch("https://api.github.com/repos/eect13/fonths/releases/latest", {
      signal: ctrl.signal,
      headers: { Accept: "application/vnd.github+json" },
      cache: "no-store",
    });
    clearTimeout(timer);
    if (!res.ok) {
      return { installed, latest: null, updateAvailable: false, releaseUrl };
    }
    const data = (await res.json()) as { tag_name?: string; html_url?: string };
    const tag = (data.tag_name || "").replace(/^v/i, "").trim();
    const latest = tag || null;
    const updateAvailable = Boolean(latest && latest !== installed && latest.localeCompare(installed, undefined, { numeric: true, sensitivity: "base" }) > 0);
    return {
      installed,
      latest,
      updateAvailable,
      releaseUrl: data.html_url || releaseUrl,
    };
  } catch {
    return { installed, latest: null, updateAvailable: false, releaseUrl };
  }
}
