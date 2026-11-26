import type { DuplicateGroup, FontRecord } from "../types.ts";

export function pickKeepFont(group: DuplicateGroup): FontRecord | undefined {
  return (
    group.fonts.find((f) => f.source === "system") ??
    group.fonts.find((f) => f.source === "google") ??
    [...group.fonts].sort((a, b) => (b.fileSize ?? 0) - (a.fileSize ?? 0) || a.id.localeCompare(b.id))[0]
  );
}

export function familyDuplicateHideIds(
  localFonts: FontRecord[],
  googleFonts: FontRecord[],
  systemFonts: FontRecord[] = [],
): string[] {
  const systemSet = new Set(systemFonts.map((f) => f.family.trim().toLowerCase()).filter(Boolean));
  const googleSet = new Set(googleFonts.map((f) => f.family.trim().toLowerCase()).filter(Boolean));
  const catalog = new Set([...googleSet, ...systemSet]);
  const seenStyle = new Set<string>();
  const seenHash = new Set<string>();
  const hide = new Set<string>();
  for (const gf of googleFonts) {
    const key = gf.family.trim().toLowerCase();
    if (key && systemSet.has(key)) hide.add(gf.id);
  }
  for (const font of localFonts) {
    const key = font.family.trim().toLowerCase();
    const styleKey = key ? `${key}:${font.weights?.[0] ?? 400}:${font.italic ? "i" : "r"}` : "";
    const hash = font.checksum?.trim();
    const isHashDup = Boolean(hash && seenHash.has(hash));
    if (hash) seenHash.add(hash);
    if (isHashDup || (key && catalog.has(key)) || (styleKey && seenStyle.has(styleKey))) {
      hide.add(font.id);
    } else if (styleKey) {
      seenStyle.add(styleKey);
    }
  }
  return [...hide];
}

export function hideIdsFromDuplicateGroups(groups: DuplicateGroup[]): string[] {
  const hide = new Set<string>();
  for (const group of groups) {
    const keep = pickKeepFont(group);
    if (!keep) continue;
    for (const font of group.fonts) {
      if (font.id !== keep.id && font.source !== "system") hide.add(font.id);
    }
  }
  return [...hide];
}
