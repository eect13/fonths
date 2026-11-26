import { bytesNearlySame } from "../binary-diff";
import { idbGet } from "../idb";
import { diffFontPaths, nativeHashFontPaths } from "../native-parse";
import type { DuplicateGroup, FontRecord } from "../types";
import {
  familyDuplicateHideIds,
  hideIdsFromDuplicateGroups,
  pickKeepFont,
} from "./duplicate-pure";

export { familyDuplicateHideIds, hideIdsFromDuplicateGroups, pickKeepFont };

function sourceRank(source: FontRecord["source"]) {
  if (source === "system") return 0;
  if (source === "google") return 1;
  return 2;
}

export async function findDuplicates(
  localFonts: FontRecord[],
  googleFonts: FontRecord[] = [],
  systemFonts: FontRecord[] = [],
  onChecksumsComputed?: (updates: Record<string, string>) => void,
): Promise<DuplicateGroup[]> {
  const missingOriginPaths = localFonts
    .filter((f) => !f.checksum && Boolean(f.originPath))
    .map((f) => f.originPath!);
  let hashedLocalFonts = localFonts;
  if (missingOriginPaths.length > 0) {
    const rustHashes = await nativeHashFontPaths(missingOriginPaths).catch(() => null);
    if (rustHashes) {
      const updates: Record<string, string> = {};
      hashedLocalFonts = localFonts.map((f) => {
        if (!f.checksum && f.originPath && rustHashes[f.originPath]) {
          updates[f.id] = rustHashes[f.originPath]!;
          return { ...f, checksum: rustHashes[f.originPath] };
        }
        return f;
      });
      if (Object.keys(updates).length > 0) {
        onChecksumsComputed?.(updates);
      }
    }
  }

  const groups: DuplicateGroup[] = [];
  const grouped = new Set<string>();

  const byHash = new Map<string, FontRecord[]>();
  for (const font of hashedLocalFonts) {
    if (!font.checksum) continue;
    const list = byHash.get(font.checksum) ?? [];
    list.push(font);
    byHash.set(font.checksum, list);
  }
  for (const [hash, fonts] of byHash) {
    if (fonts.length < 2) continue;
    groups.push({ key: `hash:${hash}`, reason: "checksum", fonts, diffBytes: 0 });
    fonts.forEach((f) => grouped.add(f.id));
  }

  const rest = hashedLocalFonts.filter((f) => !grouped.has(f.id));
  const bySize = new Map<number, FontRecord[]>();
  for (const font of rest) {
    const size = font.fileSize ?? 0;
    if (size <= 0) continue;
    const list = bySize.get(size) ?? [];
    list.push(font);
    bySize.set(size, list);
  }

  const toLoad = [...bySize.values()].filter((list) => list.length >= 2).flat();
  const buffers = new Map<string, Uint8Array>();
  await Promise.all(
    toLoad.map(async (font) => {
      const blob = await idbGet(font.id).catch(() => undefined);
      if (!blob) return;
      const buf = new Uint8Array(await blob.arrayBuffer());
      if (buf.byteLength) buffers.set(font.id, buf);
    }),
  );

  let cmp = 0;
  // Cap per-size-bucket pairwise byte diffs so 100k libraries with identical stub
  // sizes cannot go O(B^2); checksum grouping above already catches exact matches in O(N).
  const MAX_BUCKET_PAIRWISE = 32;
  for (const [size, fonts] of bySize) {
    if (fonts.length < 2 || fonts.length > MAX_BUCKET_PAIRWISE) continue;
    const parent = fonts.map((_, i) => i);
    const diffs = new Map<string, number>();
    const find = (i: number): number => {
      let p = i;
      while (parent[p] !== p) p = parent[p]!;
      let c = i;
      while (parent[c] !== p) {
        const n = parent[c]!;
        parent[c] = p;
        c = n;
      }
      return p;
    };
    const unite = (i: number, j: number) => {
      const a = find(i);
      const b = find(j);
      if (a !== b) parent[a] = b;
    };

    for (let i = 0; i < fonts.length; i += 1) {
      const leftFont = fonts[i]!;
      const left = buffers.get(leftFont.id);
      if (!left && !leftFont.originPath) continue;
      for (let j = i + 1; j < fonts.length; j += 1) {
        const rightFont = fonts[j]!;
        const right = buffers.get(rightFont.id);
        cmp += 1;
        if (cmp % 8 === 0) await new Promise<void>((r) => setTimeout(r, 0));
        let res: { near: boolean; diffs: number } | null = null;
        if (left && right) {
          res = bytesNearlySame(left, right);
        } else if (leftFont.originPath && rightFont.originPath) {
          res = await diffFontPaths(leftFont.originPath, rightFont.originPath).catch(() => null);
        }
        if (!res?.near) continue;
        unite(i, j);
        const edge = `${Math.min(i, j)}:${Math.max(i, j)}`;
        diffs.set(edge, res.diffs);
      }
    }
    const clusters = new Map<number, FontRecord[]>();
    for (let i = 0; i < fonts.length; i += 1) {
      const root = find(i);
      const list = clusters.get(root) ?? [];
      list.push(fonts[i]!);
      clusters.set(root, list);
    }
    for (const [root, list] of clusters) {
      if (list.length < 2) continue;
      let worst = 0;
      for (let i = 0; i < fonts.length; i += 1) {
        if (find(i) !== root) continue;
        for (let j = i + 1; j < fonts.length; j += 1) {
          if (find(j) !== root) continue;
          worst = Math.max(worst, diffs.get(`${i}:${j}`) ?? 0);
        }
      }
      groups.push({
        key: `bin:${size}:${list.map((f) => f.id).join(",")}`,
        reason: worst === 0 ? "checksum" : "binary",
        fonts: list,
        diffBytes: worst,
      });
      list.forEach((f) => grouped.add(f.id));
    }
  }

  // O(N) family index (replaces prior O(N^2) localFonts.filter inside names loop).
  const googleByFamily = new Map<string, FontRecord>();
  for (const font of googleFonts) {
    const key = font.family.trim().toLowerCase();
    if (key && !googleByFamily.has(key)) googleByFamily.set(key, font);
  }
  const systemByFamily = new Map<string, FontRecord>();
  for (const font of systemFonts) {
    const key = font.family.trim().toLowerCase();
    if (key && !systemByFamily.has(key)) systemByFamily.set(key, font);
  }
  const localByFamily = new Map<string, FontRecord[]>();
  for (const font of hashedLocalFonts) {
    const key = font.family.trim().toLowerCase();
    if (!key) continue;
    let bucket = localByFamily.get(key);
    if (!bucket) {
      bucket = [font];
      localByFamily.set(key, bucket);
    } else {
      bucket.push(font);
    }
  }

  const names = new Set<string>([
    ...googleByFamily.keys(),
    ...systemByFamily.keys(),
    ...localByFamily.keys(),
  ]);
  for (const name of names) {
    const sys = systemByFamily.get(name);
    const google = googleByFamily.get(name);
    const locals = localByFamily.get(name) ?? [];
    const fonts = [sys, google, ...locals].filter((f): f is FontRecord => Boolean(f));
    const sources = new Set(fonts.map((f) => f.source));
    if (fonts.length >= 2 && sources.size >= 2) {
      fonts.sort((a, b) => sourceRank(a.source) - sourceRank(b.source) || a.id.localeCompare(b.id));
      groups.push({ key: `fam:${name}`, reason: "family-weight", fonts });
      continue;
    }
    // Also detect same-family + same-weight + same-italic duplicate local uploads
    // (e.g. two copies/versions of a custom font in different folders) without
    // falsely grouping distinct weights like Regular (400) + Bold (700).
    if (locals.length >= 2) {
      const byStyle = new Map<string, FontRecord[]>();
      for (const f of locals) {
        if (grouped.has(f.id)) continue;
        const styleKey = `${f.weights?.[0] ?? 400}:${f.italic ? "i" : "r"}`;
        const list = byStyle.get(styleKey) ?? [];
        list.push(f);
        byStyle.set(styleKey, list);
      }
      for (const [styleKey, sameStyle] of byStyle) {
        if (sameStyle.length < 2) continue;
        sameStyle.sort((a, b) => (b.fileSize ?? 0) - (a.fileSize ?? 0) || a.id.localeCompare(b.id));
        groups.push({ key: `local-fam:${name}:${styleKey}`, reason: "family-weight", fonts: sameStyle });
      }
    }
  }

  return groups;
}
