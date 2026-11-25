import type { Collection, FontRecord } from "../types";

export function collectionIsWatched(collections: Collection[], id: string): boolean {
  const byId = new Map(collections.map((c) => [c.id, c]));
  let current: string | null = id;
  while (current) {
    const node = byId.get(current);
    if (!node) return false;
    if (node.watchPath) return true;
    current = node.parentId;
  }
  return false;
}

export function collectFolderTreeIds(collections: Collection[], id: string): string[] {
  const ids: string[] = [];
  const walk = (folderId: string) => {
    ids.push(folderId);
    for (const child of collections) {
      if (child.parentId === folderId) walk(child.id);
    }
  };
  walk(id);
  return ids;
}

export function folderDeleteImpact(
  collections: Collection[],
  localFonts: FontRecord[],
  id: string,
): { folderIds: string[]; localFontIds: string[]; folderName: string } {
  const folderIds = collections.some((c) => c.id === id) ? collectFolderTreeIds(collections, id) : [];
  const folderIdSet = new Set(folderIds);
  const remainingIds = new Set(
    collections.filter((c) => !folderIdSet.has(c.id)).flatMap((c) => c.fontIds),
  );
  const inDeleted = new Set<string>();
  const deletedWatchRoots: string[] = [];
  for (const c of collections) {
    if (folderIdSet.has(c.id)) {
      c.fontIds.forEach((fontId) => inDeleted.add(fontId));
      if (c.watchPath) {
        deletedWatchRoots.push(c.watchPath.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase());
      }
    }
  }
  const underDeletedWatch = (originPath: string | undefined): boolean => {
    if (!originPath || !deletedWatchRoots.length) return false;
    const normOrigin = originPath.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    return deletedWatchRoots.some((root) => normOrigin === root || normOrigin.startsWith(`${root}/`));
  };
  const localSet = new Set(localFonts.map((f) => f.id));
  const localFontIds = new Set<string>(
    [...inDeleted].filter((fontId) => localSet.has(fontId) && !remainingIds.has(fontId)),
  );
  // Any font linked from a stopped watched folder must be removed completely from Fonths.
  if (deletedWatchRoots.length) {
    for (const f of localFonts) {
      if (underDeletedWatch(f.originPath) || (inDeleted.has(f.id) && deletedWatchRoots.length > 0)) {
        localFontIds.add(f.id);
      }
    }
  }
  return {
    folderIds,
    localFontIds: [...localFontIds],
    folderName: collections.find((c) => c.id === id)?.name ?? "Folder",
  };
}

export function collectFolderFontIds(collections: Collection[], id: string): Set<string> {
  return new Set(folderFontStats(collections).get(id)?.ids ?? []);
}

/** Apply auto-hide duplicate ids to folder/collection badge stats (shared). */
export function folderStatsWithAutoHide(
  collections: Collection[],
  hideDupIds: string[],
): Map<string, { count: number; ids: string[] }> {
  const raw = folderFontStats(collections);
  if (!hideDupIds.length) return raw;
  const hide = new Set(hideDupIds);
  const next = new Map<string, { count: number; ids: string[] }>();
  for (const [id, stat] of raw) {
    const ids = stat.ids.filter((fid) => !hide.has(fid));
    next.set(id, { count: ids.length, ids });
  }
  return next;
}

export function folderFontStats(
  collections: Collection[],
): Map<string, { count: number; ids: string[] }> {
  const byParent = new Map<string | null, Collection[]>();
  const byId = new Map<string, Collection>();
  for (const c of collections) {
    byId.set(c.id, c);
    const parent = c.parentId ?? null;
    const list = byParent.get(parent) ?? [];
    list.push(c);
    byParent.set(parent, list);
  }
  const map = new Map<string, { count: number; ids: string[] }>();
  const walk = (folderId: string): string[] => {
    const cached = map.get(folderId);
    if (cached) return cached.ids;
    const col = byId.get(folderId);
    if (!col) return [];
    const ids = new Set(col.fontIds);
    for (const child of byParent.get(folderId) ?? []) {
      for (const fontId of walk(child.id)) ids.add(fontId);
    }
    const list = [...ids];
    map.set(folderId, { count: list.length, ids: list });
    return list;
  };
  const known = new Set(collections.map((c) => c.id));
  for (const c of collections) {
    if (!c.parentId || !known.has(c.parentId)) walk(c.id);
  }
  return map;
}

export function folderTree(collections: Collection[]): { folder: Collection; depth: number }[] {
  const byParent = new Map<string | null, Collection[]>();
  for (const c of collections) {
    const parent = c.parentId ?? null;
    const list = byParent.get(parent) ?? [];
    list.push(c);
    byParent.set(parent, list);
  }
  for (const list of byParent.values()) {
    list.sort((a, b) => a.name.localeCompare(b.name));
  }
  const ids = new Set(collections.map((c) => c.id));
  const rows: { folder: Collection; depth: number }[] = [];
  const walk = (parentId: string | null, depth: number) => {
    for (const folder of byParent.get(parentId) ?? []) {
      rows.push({ folder, depth });
      walk(folder.id, depth + 1);
    }
  };
  walk(null, 0);
  for (const c of collections) {
    if (c.parentId && !ids.has(c.parentId) && !rows.some((r) => r.folder.id === c.id)) {
      rows.push({ folder: { ...c, parentId: null }, depth: 0 });
    }
  }
  return rows;
}

export function wouldCreateCycle(collections: Collection[], id: string, parentId: string): boolean {
  let current: string | null = parentId;
  const seen = new Set<string>();
  while (current) {
    if (current === id) return true;
    if (seen.has(current)) return true;
    seen.add(current);
    current = collections.find((c) => c.id === current)?.parentId ?? null;
  }
  return false;
}
