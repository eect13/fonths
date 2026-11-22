import type { FontCategory } from "./types.ts";
import { TAG_ORDER } from "./types.ts";
import { readPanose } from "./panose.ts";

function push(tags: string[], tag: string) {
  if (!tags.includes(tag)) tags.push(tag);
}

/** Free/shareware hosts that often ship dummy or template OS/2 PANOSE. */
export const UNTRUSTED_FONT_SOURCES = [
  "Dafont",
  "1001 Fonts",
  "FontSpace",
  "UrbanFonts",
  "Abstract Fonts",
  "Befonts",
  "Fonts2u",
  "FontZone",
  "FFonts",
  "FontPalace",
  "FontGarden",
  "CoolText",
  "SearchFreeFonts",
  "GetFreeFonts",
  "FontMeme",
  "FontTr",
  "Free-Fonts",
] as const;

function marketplaceHay(hay: string) {
  return /dafont|1001fonts|urbanfonts|abstractfonts|befonts|fontspace|fontsquirrel|fonts2u|fontzone|ffonts|fontpalace|fontgarden|cooltext|searchfreefonts|getfreefonts|fontmeme|fonttr|free-?fonts|freefonts?/i.test(
    hay,
  );
}

type NameHit = { category: FontCategory; tags: string[]; strong: boolean };

function categoryFromName(hay: string): NameHit | null {
  const tags: string[] = [];
  if (/\b(material symbols|material icons|font awesome|icon font|codicon|\bicons?\b)\b/i.test(hay)) {
    return { category: "icons", tags: ["symbols"], strong: true };
  }
  if (/\b(dingbat|dingbats|symbol|ornament|initials|icon[- ]?set|barcode)\b/i.test(hay)) {
    return { category: "display", tags: ["symbols"], strong: true };
  }
  if (
    /\b(script|handwriting|hand[- ]?letter(?:ing)?|calligraph(?:y|ic)?|signature|cursive)\b/i.test(hay) ||
    /\bbrush script\b/i.test(hay)
  ) {
    return { category: "handwriting", tags: ["script"], strong: true };
  }
  if (/\b(mono|monospace|typewriter|console|fixed[- ]?width)\b/i.test(hay) || /\bcode\b/i.test(hay)) {
    return { category: "mono", tags: ["coding"], strong: true };
  }
  if (
    /\b(display|poster|blackletter|graffiti|comic|decorative|titling|headline|stencil|inline|outline|vintage|western)\b/i.test(
      hay,
    )
  ) {
    return { category: "display", tags, strong: true };
  }
  if (/\bsans([- ]serif)?\b/i.test(hay) || /\b(gothic|grotesk|grotesque)\b/i.test(hay)) {
    return { category: "sans", tags, strong: true };
  }
  if (/\bserif\b/i.test(hay)) {
    return { category: "serif", tags, strong: true };
  }
  if (/\b(slab|clarendon|rockwell|egyptienne)\b/i.test(hay)) {
    return { category: "serif", tags: ["slab"], strong: true };
  }
  if (/\b(garamond|caslon|baskerville|bodoni|didot|times|georgia|palatino)\b/i.test(hay)) {
    return { category: "serif", tags: ["editorial"], strong: false };
  }
  return null;
}

const EXTRA_TAGS: [RegExp, string][] = [
  [/\b(geometric|futura|avant garde|century gothic|poppins|montserrat|gilroy|circular)\b/i, "geometric"],
  [/\b(humanist|gill sans|myriad|frutiger|verdana|tahoma|source sans|optima)\b/i, "humanist"],
  [/\b(neo[- ]?grotesque|helvetica|arial|roboto|inter|neue|geist|univers)\b/i, "neo-grotesque"],
  [/\b(grotesque|grotesk|akzidenz|franklin gothic)\b/i, "grotesque"],
  [/\b(editorial|garamond|caslon|baskerville|merriweather|eb garamond|crimson|newsreader|lora)\b/i, "editorial"],
  [/\b(condensed|narrow|compressed)\b/i, "condensed"],
  [/\b(slab|rockwell|clarendon|arvo|zilla)\b/i, "slab"],
  [/\b(didone|bodoni|didot|playfair)\b/i, "didone"],
  [/\b(rounded|nunito|comfortaa|quicksand|varela round)\b/i, "rounded"],
  [/\b(atkinson|lexend|hyperlegible|dyslex\w*)\b/i, "accessible"],
  [/\bnoto\b/i, "noto"],
  [/\b(colrv?1|emoji)\b/i, "color"],
];

export function normalizeCustomTag(raw: string): string {
  return raw
    .trim()
    .replace(/^#+/, "")
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

export function parseTagInput(raw: string): string[] {
  const out: string[] = [];
  for (const part of raw.split(/[,;]+/)) {
    const tag = normalizeCustomTag(part);
    if (tag && !out.includes(tag)) out.push(tag);
  }
  return out;
}

function ibmClass(sFamilyClass?: number) {
  if (sFamilyClass == null) return 0;
  return (sFamilyClass >> 8) & 0xff;
}

function ibmSubclass(sFamilyClass?: number) {
  if (sFamilyClass == null) return 0;
  return sFamilyClass & 0xff;
}

function fromIbm(cls: number, sub: number): { category: FontCategory; tags: string[] } | null {
  const tags: string[] = [];
  if (cls === 0) return null;
  if (cls === 10) return { category: "handwriting", tags: ["script"] };
  if (cls === 11) return { category: "display", tags: ["symbols"] };
  if (cls === 9) return { category: "display", tags };
  if (cls === 5) return { category: "serif", tags: ["slab"] };
  if (cls === 3) return { category: "serif", tags: ["didone"] };
  if (cls === 1 || cls === 2 || cls === 4) return { category: "serif", tags: ["editorial"] };
  if (cls >= 1 && cls <= 7) return { category: "serif", tags };
  if (cls === 8) {
    if (sub === 9) return { category: "mono", tags: ["coding"] };
    if (sub === 2) tags.push("humanist");
    else if (sub === 1 || sub === 5 || sub === 6) tags.push("neo-grotesque");
    else if (sub === 3 || sub === 4) tags.push("geometric");
    else if (sub === 10) tags.push("rounded");
    return { category: "sans", tags };
  }
  return null;
}

export interface LocalStyleInput {
  family: string;
  subfamily?: string;
  fullName?: string;
  fileName?: string;
  sFamilyClass?: number;
  panose?: number[];
  widthClass?: number;
  isFixedPitch?: boolean;
  hasLigatures?: boolean;
  axes?: { tag: string; min?: number; max?: number }[];
}

/**
 * Uploads (especially Dafont): file name wins. PANOSE/IBM/width/post tables refine when
 * filled in and not overridden by marketplace noise.
 */
export function inferLocalStyle(input: LocalStyleInput): {
  category: FontCategory;
  tags: string[];
  fromName: boolean;
} {
  const hay = [input.family, input.subfamily, input.fullName, input.fileName].filter(Boolean).join(" ");
  const tags: string[] = [];
  const named = categoryFromName(hay);
  const panose = readPanose(input.panose);
  const untrusted = marketplaceHay(hay);
  const ibm = fromIbm(ibmClass(input.sFamilyClass), ibmSubclass(input.sFamilyClass));

  let category: FontCategory = "sans";
  if (named?.strong) {
    category = named.category;
    for (const t of named.tags) push(tags, t);
  } else if (input.isFixedPitch === true) {
    category = "mono";
    push(tags, "coding");
  } else if (!untrusted && panose.confident && panose.category) {
    category = panose.category;
  } else if (!untrusted && ibm) {
    category = ibm.category;
    for (const t of ibm.tags) push(tags, t);
  } else if (named) {
    category = named.category;
    for (const t of named.tags) push(tags, t);
  } else if (panose.category && panose.confident) {
    category = panose.category;
  }

  if (named && !named.strong) {
    for (const t of named.tags) push(tags, t);
  }
  if (!untrusted) {
    for (const t of panose.tags) push(tags, t);
    if (ibm) for (const t of ibm.tags) push(tags, t);
  }
  if (input.isFixedPitch === true && category !== "icons") {
    if (!named?.strong) category = "mono";
    push(tags, "coding");
  }
  if (
    (input.widthClass !== undefined && input.widthClass >= 1 && input.widthClass <= 4) ||
    input.axes?.some((a) => a.tag === "wdth" && typeof a.min === "number" && a.min <= 85)
  ) {
    push(tags, "condensed");
  }
  if (input.axes?.some((a) => a.tag === "opsz" && typeof a.max === "number" && a.max >= 36)) {
    push(tags, "editorial");
  }
  for (const [re, tag] of EXTRA_TAGS) {
    if (re.test(hay)) push(tags, tag);
  }
  if (input.hasLigatures) push(tags, "ligatures");
  if (category === "handwriting") push(tags, "script");
  if (category === "mono") push(tags, "coding");

  return { category, tags: canonicalTags(tags), fromName: Boolean(named?.strong) };
}

const CATEGORY_AS_TAG: Record<FontCategory, Set<string>> = {
  sans: new Set(["sans", "sans-serif"]),
  serif: new Set(["serif"]),
  display: new Set(["display"]),
  handwriting: new Set(["handwriting", "script"]),
  mono: new Set(["monospace", "mono"]),
  other: new Set(["other"]),
  icons: new Set(["icons", "icon"]),
};

/** Style categories are not tags. Seed leftovers and CSS table names map or drop. */
const TAG_ALIAS: Record<string, string | null> = {
  svg: "color",
  colrv0: "color",
  colrv1: "color",
  cbdt: "color",
  sbix: "color",
  symbol: "symbols",
  dingbat: "symbols",
  dingbats: "symbols",
  "display-sans": null,
  ui: null,
  friendly: null,
  elegant: null,
  neutral: null,
  handwriting: null,
  display: null,
  sans: null,
  "sans-serif": null,
  serif: null,
  mono: null,
  monospace: null,
  other: null,
  icons: null,
  icon: null,
  uploaded: null,
};

const CANONICAL_TAG = new Set<string>(TAG_ORDER as readonly string[]);

/** One vocabulary for sidebar chips, search, and inspector. */
export function canonicalTags(tags: Iterable<string>): string[] {
  const out: string[] = [];
  for (const raw of tags) {
    const key = raw.trim().toLowerCase();
    if (!key) continue;
    const mapped = Object.prototype.hasOwnProperty.call(TAG_ALIAS, key) ? TAG_ALIAS[key] : key;
    if (!mapped || !CANONICAL_TAG.has(mapped)) continue;
    push(out, mapped);
  }
  return out;
}

export function tagsForGoogleFamily(
  family: string,
  category: FontCategory,
  extra: string[] = [],
): string[] {
  const inferred = inferLocalStyle({ family, fileName: family });
  const skip = CATEGORY_AS_TAG[category] ?? new Set();
  const raw: string[] = [];
  for (const tag of [...extra, ...inferred.tags]) {
    const t = tag.trim().toLowerCase();
    if (!t || skip.has(t)) continue;
    raw.push(t);
  }
  return canonicalTags(raw);
}
