import type { FontRecord } from "./types";

export type ImportCollisionChoice = "prefer-local" | "keep-both" | "skip-colliding" | "cancel";

export interface ImportCollisionItem {
  familyKey: string;
  familyName: string;
  googleFont: FontRecord;
  matchSource?: "google" | "local" | "system";
  isGoogleActive: boolean;
  isGoogleOnDisk: boolean;
  uploadFileNames: string[];
  uploadBytes: number;
}

export interface ImportCollisionRequest {
  collisions: ImportCollisionItem[];
  totalFiles: number;
  resolve: (choice: ImportCollisionChoice) => void;
}

const STYLE_SUFFIX_RE =
  /[-_ ](?:thin|extralight|ultralight|light|regular|normal|book|medium|semibold|demibold|bold|extrabold|ultrabold|black|heavy|italic|oblique|variable(?:font)?(?:_[a-z,]+)?|\d{3})(?:[-_ ]?(?:italic|oblique))?$/i;

/** Best-effort family name from an uploaded font filename before full SFNT parse. */
export function guessFamilyFromFileName(fileName: string): string {
  const base = fileName.replace(/\.[^.]+$/, "").trim();
  // Strip camelCase `Variable` marker (e.g. `InterVariable` / `InterVariable-Italic` -> `Inter`).
  let cur = base.replace(/([a-z0-9])Variable(?=$|[-_ ]|Italic|Oblique)/g, "$1");
  for (let i = 0; i < 4; i++) {
    const next = cur.replace(STYLE_SUFFIX_RE, "").trim();
    if (next === cur || !next) break;
    cur = next;
  }
  return cur.replace(/[_-]+/g, " ").replace(/\s+/g, " ").trim();
}

export function findImportGoogleCollisions(
  files: Array<{ name: string; size?: number }>,
  googleFonts: FontRecord[],
  activatedSet: Set<string>,
  diskFamilySet: Set<string>,
  existingLocalFonts: FontRecord[] = [],
  existingSystemFonts: FontRecord[] = [],
): { collisions: ImportCollisionItem[]; collidingFileIndices: Set<number> } {
  const compactKey = (s: string) => s.trim().toLowerCase().replace(/[\s_-]+/g, "");
  const indexByFamily = (list: FontRecord[]) => {
    const map = new Map<string, FontRecord>();
    for (const font of list) {
      const key = font.family.trim().toLowerCase();
      if (key && !map.has(key)) map.set(key, font);
      const cKey = compactKey(font.family);
      if (cKey && !map.has(cKey)) map.set(cKey, font);
    }
    return map;
  };
  const googleByKey = indexByFamily(googleFonts);
  const localByKey = indexByFamily(existingLocalFonts);
  const systemByKey = indexByFamily(existingSystemFonts);

  const groups = new Map<
    string,
    {
      gf: FontRecord;
      matchSource: "google" | "local" | "system";
      indices: number[];
      names: string[];
      bytes: number;
    }
  >();
  const collidingFileIndices = new Set<number>();

  files.forEach((file, idx) => {
    const guess = guessFamilyFromFileName(file.name).toLowerCase();
    if (!guess) return;
    const cGuess = compactKey(guess);
    const googleMatch = googleByKey.get(guess) ?? googleByKey.get(cGuess);
    const localMatch = localByKey.get(guess) ?? localByKey.get(cGuess);
    const systemMatch = systemByKey.get(guess) ?? systemByKey.get(cGuess);

    let gf: FontRecord | undefined;
    let matchSource: "google" | "local" | "system" = "google";
    if (
      googleMatch &&
      (activatedSet.has(googleMatch.id) ||
        diskFamilySet.has(googleMatch.family.trim().toLowerCase()))
    ) {
      gf = googleMatch;
      matchSource = "google";
    } else if (localMatch) {
      gf = localMatch;
      matchSource = "local";
    } else if (googleMatch) {
      gf = googleMatch;
      matchSource = "google";
    } else if (systemMatch) {
      gf = systemMatch;
      matchSource = "system";
    }
    if (!gf) return;
    const canonKey = gf.family.trim().toLowerCase();
    collidingFileIndices.add(idx);
    const existing = groups.get(canonKey) ?? {
      gf,
      matchSource,
      indices: [],
      names: [],
      bytes: 0,
    };
    existing.indices.push(idx);
    existing.names.push(file.name);
    existing.bytes += file.size || 0;
    groups.set(canonKey, existing);
  });

  const collisions: ImportCollisionItem[] = [];
  for (const [familyKey, g] of groups.entries()) {
    collisions.push({
      familyKey,
      familyName: g.gf.family,
      googleFont: g.gf,
      matchSource: g.matchSource,
      isGoogleActive: activatedSet.has(g.gf.id),
      isGoogleOnDisk: diskFamilySet.has(familyKey) || g.gf.source === "local" || g.matchSource === "system",
      uploadFileNames: g.names,
      uploadBytes: g.bytes,
    });
  }
  return { collisions, collidingFileIndices };
}

let currentReq: ImportCollisionRequest | null = null;
const listeners = new Set<(req: ImportCollisionRequest | null) => void>();

function emit() {
  for (const l of listeners) l(currentReq);
}

export function subscribeImportCollision(
  listener: (req: ImportCollisionRequest | null) => void,
): () => void {
  listeners.add(listener);
  listener(currentReq);
  return () => {
    listeners.delete(listener);
  };
}

export function requestImportCollisionChoice(
  collisions: ImportCollisionItem[],
  totalFiles: number,
): Promise<ImportCollisionChoice> {
  return new Promise((resolve) => {
    if (currentReq) currentReq.resolve("cancel");
    currentReq = { collisions, totalFiles, resolve };
    emit();
  });
}

export function resolveImportCollision(choice: ImportCollisionChoice) {
  const req = currentReq;
  currentReq = null;
  emit();
  req?.resolve(choice);
}
