import type { FontRecord } from "./types";

export type ImportCollisionChoice = "prefer-local" | "keep-both" | "skip-colliding" | "cancel";

export interface ImportCollisionItem {
  familyKey: string;
  familyName: string;
  googleFont: FontRecord;
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
  let cur = base;
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
): { collisions: ImportCollisionItem[]; collidingFileIndices: Set<number> } {
  const compactKey = (s: string) => s.trim().toLowerCase().replace(/[\s_-]+/g, "");
  const googleByKey = new Map<string, FontRecord>();
  for (const gf of googleFonts) {
    const key = gf.family.trim().toLowerCase();
    if (!googleByKey.has(key)) googleByKey.set(key, gf);
    const cKey = compactKey(gf.family);
    if (cKey && !googleByKey.has(cKey)) googleByKey.set(cKey, gf);
  }
  const localByKey = new Map<string, FontRecord>();
  for (const lf of existingLocalFonts) {
    const key = lf.family.trim().toLowerCase();
    if (!localByKey.has(key)) localByKey.set(key, lf);
    const cKey = compactKey(lf.family);
    if (cKey && !localByKey.has(cKey)) localByKey.set(cKey, lf);
  }

  const groups = new Map<string, { gf: FontRecord; indices: number[]; names: string[]; bytes: number }>();
  const collidingFileIndices = new Set<number>();

  files.forEach((file, idx) => {
    const guess = guessFamilyFromFileName(file.name).toLowerCase();
    if (!guess) return;
    const cGuess = compactKey(guess);
    const gf =
      googleByKey.get(guess) ??
      googleByKey.get(cGuess) ??
      localByKey.get(guess) ??
      localByKey.get(cGuess);
    if (!gf) return;
    const canonKey = gf.family.trim().toLowerCase();
    collidingFileIndices.add(idx);
    const existing = groups.get(canonKey) ?? { gf, indices: [], names: [], bytes: 0 };
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
      isGoogleActive: activatedSet.has(g.gf.id),
      isGoogleOnDisk: diskFamilySet.has(familyKey) || g.gf.source === "local",
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
