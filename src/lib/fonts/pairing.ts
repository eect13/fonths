import { CATEGORY_LABEL, type FontCategory, type FontRecord } from "./types.ts";

export interface PairingPreset {
  id: string;
  label: string;
  headingFamily: string;
  bodyFamily: string;
  headingSize: number;
  bodySize: number;
  headingWeight: number;
  bodyWeight: number;
}

export const PAIRING_PRESETS: PairingPreset[] = [
  {
    id: "editorial",
    label: "Editorial Classic",
    headingFamily: "Playfair Display",
    bodyFamily: "Source Sans 3",
    headingSize: 48,
    bodySize: 18,
    headingWeight: 700,
    bodyWeight: 400,
  },
  {
    id: "geometric",
    label: "Geometric Modern",
    headingFamily: "Space Grotesk",
    bodyFamily: "Inter",
    headingSize: 44,
    bodySize: 17,
    headingWeight: 700,
    bodyWeight: 400,
  },
  {
    id: "technical",
    label: "Tech + Mono",
    headingFamily: "Plus Jakarta Sans",
    bodyFamily: "JetBrains Mono",
    headingSize: 42,
    bodySize: 16,
    headingWeight: 700,
    bodyWeight: 400,
  },
  {
    id: "humanist",
    label: "Warm Humanist",
    headingFamily: "Fraunces",
    bodyFamily: "Work Sans",
    headingSize: 48,
    bodySize: 18,
    headingWeight: 600,
    bodyWeight: 400,
  },
  {
    id: "swiss",
    label: "Display + Grotesque",
    headingFamily: "Syne",
    bodyFamily: "DM Sans",
    headingSize: 46,
    bodySize: 18,
    headingWeight: 700,
    bodyWeight: 400,
  },
];

const CLASSIC_PAIR_GRAPH: Record<string, string[]> = {
  "playfair display": ["source sans 3", "inter", "lato", "work sans"],
  "source sans 3": ["playfair display", "lora", "merriweather", "newsreader"],
  "space grotesk": ["inter", "ibm plex mono", "dm sans", "jetbrains mono"],
  inter: ["playfair display", "newsreader", "lora", "fraunces", "jetbrains mono"],
  "plus jakarta sans": ["jetbrains mono", "newsreader", "lora", "ibm plex sans"],
  "jetbrains mono": ["plus jakarta sans", "inter", "figtree", "space grotesk"],
  fraunces: ["work sans", "plus jakarta sans", "inter", "dm sans"],
  "work sans": ["fraunces", "playfair display", "lora", "crimson pro"],
  syne: ["dm sans", "inter", "space mono"],
  "dm sans": ["syne", "newsreader", "playfair display", "fraunces"],
  figtree: ["newsreader", "lora", "jetbrains mono", "merriweather"],
  newsreader: ["figtree", "inter", "plus jakarta sans", "dm sans"],
  lora: ["inter", "figtree", "dm sans", "work sans"],
  merriweather: ["source sans 3", "figtree", "inter", "open sans"],
  "ibm plex mono": ["space grotesk", "ibm plex sans", "inter", "lora"],
};

export function estimateStrokeContrast(font?: FontRecord): { value: number; label: string } {
  if (!font) return { value: 0.25, label: "Neutral" };
  if (typeof font.metrics?.contrast === "number") {
    const c = font.metrics.contrast;
    return {
      value: c,
      label: c >= 0.6 ? "High contrast" : c <= 0.28 ? "Low / monoline" : "Medium contrast",
    };
  }
  const tags = new Set(font.tags ?? []);
  if (tags.has("didone")) return { value: 0.85, label: "High (Didone)" };
  if (tags.has("editorial") || font.category === "serif") return { value: 0.62, label: "Medium-high (Serif)" };
  if (tags.has("slab")) return { value: 0.22, label: "Low (Slab)" };
  if (tags.has("geometric") || font.category === "mono") return { value: 0.15, label: "Low / even stroke" };
  if (tags.has("humanist")) return { value: 0.35, label: "Moderate (Humanist)" };
  return { value: 0.2, label: "Low (Sans)" };
}

export function estimateXHeight(font?: FontRecord): { value: number; measured: boolean } {
  if (typeof font?.metrics?.xh === "number" && font.metrics.xh > 0.2 && font.metrics.xh < 0.85) {
    return { value: font.metrics.xh, measured: true };
  }
  if (!font) return { value: 0.5, measured: false };
  if (font.category === "sans" || font.category === "mono") return { value: 0.53, measured: false };
  if (font.category === "serif") return { value: 0.48, measured: false };
  if (font.category === "display") return { value: 0.5, measured: false };
  return { value: 0.46, measured: false };
}

export function categoryRelationLabel(leftCat?: FontCategory, rightCat?: FontCategory): {
  label: string;
  tier: "complementary" | "harmonic" | "warn";
} {
  if (!leftCat || !rightCat) return { label: "Select two typefaces", tier: "harmonic" };
  if (leftCat === rightCat) {
    return {
      label: `${CATEGORY_LABEL[leftCat]} × ${CATEGORY_LABEL[rightCat]} (Superfamily / Scale pair)`,
      tier: "harmonic",
    };
  }
  if (
    (leftCat === "serif" && rightCat === "sans") ||
    (leftCat === "sans" && rightCat === "serif") ||
    (leftCat === "display" && (rightCat === "sans" || rightCat === "serif")) ||
    ((leftCat === "sans" || leftCat === "serif") && rightCat === "mono")
  ) {
    return {
      label: `${CATEGORY_LABEL[leftCat]} × ${CATEGORY_LABEL[rightCat]} (Complementary contrast)`,
      tier: "complementary",
    };
  }
  if (rightCat === "display" || rightCat === "handwriting") {
    return {
      label: `${CATEGORY_LABEL[leftCat]} × ${CATEGORY_LABEL[rightCat]} (Body may be hard to read at small sizes)`,
      tier: "warn",
    };
  }
  return {
    label: `${CATEGORY_LABEL[leftCat]} × ${CATEGORY_LABEL[rightCat]}`,
    tier: "complementary",
  };
}

export function computePairingMatrix(opts: {
  left?: FontRecord;
  right?: FontRecord;
  leftSize: number;
  rightSize: number;
  leftWeight: number;
  rightWeight: number;
}) {
  const { left, right, leftSize, rightSize, leftWeight, rightWeight } = opts;
  const ratio = rightSize > 0 ? leftSize / rightSize : 1;
  const ratioTier =
    ratio >= 2.2
      ? "Editorial scale"
      : ratio >= 1.5
        ? "Golden / balanced"
        : ratio >= 1.25
          ? "Compact subhead"
          : "Flat hierarchy";
  const weightDelta = Math.abs(Math.round(leftWeight) - Math.round(rightWeight));
  const weightTier =
    weightDelta >= 250
      ? "Strong weight separation"
      : weightDelta >= 100
        ? "Moderate weight step"
        : "Uniform weight";
  const catRel = categoryRelationLabel(left?.category, right?.category);
  const leftCtr = estimateStrokeContrast(left);
  const rightCtr = estimateStrokeContrast(right);
  const ctrDelta = Math.abs(leftCtr.value - rightCtr.value);
  const leftXh = estimateXHeight(left);
  const rightXh = estimateXHeight(right);
  const xhDiff = Math.abs(leftXh.value - rightXh.value);

  let score = 60;
  if (ratio >= 1.5 && ratio <= 3.6) score += 14;
  else if (ratio < 1.2) score -= 12;
  if (weightDelta >= 200) score += 12;
  else if (weightDelta >= 100) score += 6;
  else if (left?.id === right?.id) score -= 10;
  if (catRel.tier === "complementary") score += 12;
  else if (catRel.tier === "warn") score -= 14;
  if (ctrDelta >= 0.25) score += 6;
  if (xhDiff <= 0.06) score += 6;
  score = Math.max(25, Math.min(99, Math.round(score)));

  const verdict =
    score >= 86
      ? "High Contrast & Harmony"
      : score >= 72
        ? "Balanced Pairing"
        : "Low Visual Separation";

  return {
    ratio,
    ratioTier,
    weightDelta,
    weightTier,
    catRel,
    leftCtr,
    rightCtr,
    leftXh,
    rightXh,
    xhDiff,
    score,
    verdict,
  };
}

export interface CompanionCandidate {
  font: FontRecord;
  score: number;
  reason: string;
  matrix: ReturnType<typeof computePairingMatrix>;
}

function hasReadableBodyWeight(font: FontRecord): boolean {
  const wght = font.axes?.find((a) => a.tag === "wght");
  if (wght && wght.min <= 450 && wght.max >= 350) return true;
  if (!font.weights?.length) return true;
  return font.weights.some((w) => w >= 300 && w <= 500);
}

function subStyleBonus(left: FontRecord, cand: FontRecord): { bonus: number; note?: string } {
  const lt = new Set(left.tags ?? []);
  const ct = new Set(cand.tags ?? []);
  if ((lt.has("editorial") || lt.has("didone")) && (ct.has("geometric") || ct.has("humanist") || ct.has("neo-grotesque"))) {
    return { bonus: 6, note: "editorial × clean sans" };
  }
  if ((ct.has("editorial") || ct.has("didone")) && (lt.has("geometric") || lt.has("humanist") || lt.has("neo-grotesque"))) {
    return { bonus: 5, note: "sans × editorial contrast" };
  }
  if (lt.has("geometric") && (ct.has("editorial") || ct.has("slab"))) {
    return { bonus: 5, note: "geometric × warm serif" };
  }
  if (lt.has("slab") && (ct.has("humanist") || ct.has("grotesque") || ct.has("neo-grotesque"))) {
    return { bonus: 5, note: "slab × neutral sans" };
  }
  if (left.category === cand.category && lt.has("didone") !== ct.has("didone")) {
    return { bonus: 4 };
  }
  return { bonus: 0 };
}

function curatedBonus(leftFamily: string, candFamily: string): boolean {
  const l = leftFamily.trim().toLowerCase();
  const c = candFamily.trim().toLowerCase();
  return Boolean(CLASSIC_PAIR_GRAPH[l]?.includes(c) || CLASSIC_PAIR_GRAPH[c]?.includes(l));
}

export function suggestCompanionCandidates(opts: {
  left: FontRecord;
  pool: FontRecord[];
  activatedIds?: Set<string>;
  leftSize: number;
  rightSize: number;
  leftWeight: number;
}): CompanionCandidate[] {
  const { left, pool, activatedIds, leftSize, rightSize, leftWeight } = opts;
  const leftNameKey = left.family.trim().toLowerCase();
  const nonLatinHeading = Boolean(left.script && left.script !== "latin");

  const base = pool.filter((f) => {
    if (f.id === left.id) return false;
    if (f.family.trim().toLowerCase() === leftNameKey) return false;
    if (f.category === "icons" || f.category === "handwriting") return false;
    if (f.colorKind && f.colorKind !== "none") return false;
    if ((f.tags ?? []).includes("emoji") || (f.tags ?? []).includes("symbols")) return false;
    if (nonLatinHeading && f.script && f.script !== left.script) return false;
    return true;
  });

  const readable = base.filter(hasReadableBodyWeight);
  const candidatesPool = readable.length > 0 ? readable : base;

  return candidatesPool
    .map((cand) => {
      const m = computePairingMatrix({
        left,
        right: cand,
        leftSize,
        rightSize,
        leftWeight,
        rightWeight: 400,
      });

      let rank = m.score;
      const reasons: string[] = [];

      if (
        (left.category === "serif" && cand.category === "sans") ||
        (left.category === "sans" && cand.category === "serif")
      ) {
        rank += 8;
        reasons.push(`${CATEGORY_LABEL[left.category]} × ${CATEGORY_LABEL[cand.category]}`);
      } else if (left.category === "display" && (cand.category === "sans" || cand.category === "serif")) {
        rank += 7;
        reasons.push(`Display × ${CATEGORY_LABEL[cand.category]}`);
      } else if ((left.category === "sans" || left.category === "serif") && cand.category === "mono") {
        rank += 4;
        reasons.push(`${CATEGORY_LABEL[left.category]} × Mono`);
      } else if (left.category === cand.category) {
        rank -= 3;
      }

      const style = subStyleBonus(left, cand);
      rank += style.bonus;
      if (style.note && reasons.length < 2) reasons.push(style.note);

      if (curatedBonus(left.family, cand.family)) {
        rank += 10;
        reasons.unshift("Classic pair");
      }

      if (activatedIds?.has(cand.id) || cand.source === "local") {
        rank += 4;
      }
      if (Number.isFinite(cand.popularity) && cand.popularity > 0 && cand.popularity < 600) {
        rank += Math.max(0, 5 - Math.floor(cand.popularity / 120));
      }

      const lWidth = left.metrics?.widthClass ?? 5;
      const rWidth = cand.metrics?.widthClass ?? 5;
      if (Math.abs(lWidth - rWidth) >= 4) rank -= 5;

      if (!nonLatinHeading && cand.script && cand.script !== "latin" && !cand.coversLatin) {
        rank -= 30;
      }

      const reason = reasons.length
        ? reasons.join(" · ")
        : `${m.catRel.label} · ${m.verdict}`;

      return {
        font: cand,
        rawRank: rank,
        score: Math.max(25, Math.min(99, Math.round(rank))),
        reason,
        matrix: m,
      };
    })
    .sort((a, b) => b.rawRank - a.rawRank || a.font.family.localeCompare(b.font.family))
    .map(({ rawRank: _rawRank, ...rest }) => rest);
}

/**
 * Diversified round-robin across top-scoring categories so clicking "Suggest pair"
 * cycles through distinct Sans, Serif, and Mono companions instead of clones.
 */
export function pickNextCompanion(
  candidates: CompanionCandidate[],
  index: number,
  currentRightId?: string,
): CompanionCandidate | null {
  if (!candidates.length) return null;
  const buckets = new Map<FontCategory, CompanionCandidate[]>();
  for (const cand of candidates.slice(0, 60)) {
    const arr = buckets.get(cand.font.category) ?? [];
    arr.push(cand);
    buckets.set(cand.font.category, arr);
  }
  const order = Array.from(buckets.entries()).sort(
    (a, b) => (b[1][0]?.score ?? 0) - (a[1][0]?.score ?? 0),
  );
  const diverse: CompanionCandidate[] = [];
  let depth = 0;
  while (diverse.length < 15) {
    let pushed = false;
    for (const [, list] of order) {
      if (list[depth]) {
        diverse.push(list[depth]!);
        pushed = true;
        if (diverse.length >= 15) break;
      }
    }
    if (!pushed) break;
    depth += 1;
  }
  const ring = diverse.length ? diverse : candidates.slice(0, 15);
  let pick = ring[index % ring.length]!;
  if (ring.length > 1 && currentRightId && pick.font.id === currentRightId) {
    pick = ring[(index + 1) % ring.length]!;
  }
  return pick;
}
