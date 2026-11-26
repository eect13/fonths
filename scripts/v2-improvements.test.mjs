import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";
import { GOOGLE_SUBDIR, UPLOADS_SUBDIR, documentsFamilyFolder, documentsFamilySubpath } from "../src/lib/fonts/family-folder.ts";
import { axisStep, previewAxisValues, variationStyle, vfStyleDescriptor } from "../src/lib/fonts/axes.ts";
import { inferLocalStyle, parseTagInput, tagsForGoogleFamily } from "../src/lib/fonts/style-tags.ts";
import {
  computePairingMatrix,
  pickNextCompanion,
  suggestCompanionCandidates,
  suggestHeadingCandidates,
  suggestRandomIdealPair,
} from "../src/lib/fonts/pairing.ts";
import { findImportGoogleCollisions } from "../src/lib/fonts/import-collision.ts";
import { dedupeWatchedBatch } from "../src/lib/fonts/import-options.ts";
import { fontMatchesSearch, metricsFromTables, parseSearchQuery, tokenizeSearchQuery, toggleSearchToken } from "../src/lib/fonts/metrics.ts";
import { fontFormatForFile, formatInspectorCss, formatLocalFontFace, tailwindFontVarKey } from "../src/lib/fonts/css-format.ts";
import { FONTHS_BACKUP_VERSION, buildLibraryBackup, mergeLibraryBackupState, parseLibraryBackup } from "../src/lib/fonts/backup-schema.ts";
import { coerceDesktopPrefs, DEFAULT_DESKTOP_PREFS } from "../src/lib/desktop/prefs.ts";
import { readRustModule, readTsModule } from "./lib/read-source.mjs";

function makeFont(overrides = {}) {
  return {
    id: "font-1",
    family: "Sample Sans",
    source: "google",
    category: "sans",
    weights: [400, 700],
    script: "latin",
    italic: false,
    variable: false,
    ...overrides,
  };
}

test("1. Folder separation helpers separate Google and Uploads cleanly", () => {
  assert.equal(GOOGLE_SUBDIR, "Google");
  assert.equal(UPLOADS_SUBDIR, "Uploads");
  assert.deepEqual(documentsFamilySubpath("Open Sans", "google"), ["Google", "Open Sans"]);
  assert.deepEqual(documentsFamilySubpath("My Custom:Font*", "local"), [
    "Uploads",
    documentsFamilyFolder("My Custom:Font*"),
  ]);
});

test("2. Slant axis preview step, descriptor, and variationStyle work without snapping to 0deg", () => {
  const slnt = { tag: "slnt", name: "Slant", min: -12, max: 0, def: 0 };
  const wght = { tag: "wght", name: "Weight", min: 100, max: 900, def: 400 };
  assert.equal(axisStep(slnt), 0.1);

  const font = makeFont({
    id: "inter-slnt",
    family: "Inter Slant",
    variable: true,
    italic: false,
    axes: [wght, slnt],
  });

  assert.equal(vfStyleDescriptor(font), "oblique 0deg 12deg");
  assert.equal(variationStyle({ wght: 500, slnt: -10 }, [wght, slnt]).fontStyle, "oblique 10deg");
  assert.equal(variationStyle({ wght: 500, slnt: 0 }, [wght, slnt]).fontStyle, "normal");

  const stored = previewAxisValues(font, { slnt: -6.5 }, 400, true);
  assert.equal(stored.slnt, -6.5);
  const zeroed = previewAxisValues(font, { slnt: 0 }, 400, false);
  assert.equal(zeroed.slnt, 0);
});

test("3. Style tagging avoids substring false positives and parses comma-delimited tags", () => {
  assert.equal(tagsForGoogleFamily("Monoton", "display").includes("noto"), false);
  assert.equal(tagsForGoogleFamily("Winter Sans", "sans").includes("neo-grotesque"), false);
  assert.equal(tagsForGoogleFamily("Background Serif", "serif").includes("rounded"), false);

  const iconInferred = inferLocalStyle({ family: "Material Symbols Outlined" });
  assert.equal(iconInferred.category, "icons");
  assert.equal(iconInferred.tags.includes("symbols"), true);

  assert.deepEqual(parseTagInput(" Brand , #editorial ,  brand , Display  Type "), [
    "brand",
    "editorial",
    "display-type",
  ]);
});

test("4. Suggest pair diversifies categories, respects scripts/weights, and avoids immediate repeats", () => {
  const heading = makeFont({
    id: "h1",
    family: "Playfair Display",
    category: "serif",
    weights: [400, 700],
    script: "latin",
  });
  const pool = [
    heading,
    makeFont({ id: "c1", family: "Inter", category: "sans", weights: [400, 500], script: "latin" }),
    makeFont({ id: "c2", family: "JetBrains Mono", category: "mono", weights: [400, 700], script: "latin" }),
    makeFont({ id: "c3", family: "Cormorant", category: "serif", weights: [400, 600], script: "latin" }),
    makeFont({ id: "c4", family: "Caveat", category: "handwriting", weights: [400, 700], script: "latin" }),
    makeFont({ id: "c5", family: "Oswald", category: "display", weights: [400, 700], script: "latin" }),
    makeFont({ id: "c6", family: "Cairo", category: "sans", weights: [400], script: "arabic" }),
  ];

  const candidates = suggestCompanionCandidates({
    left: heading,
    pool,
    activatedIds: new Set(["c1"]),
    leftSize: 52,
    rightSize: 16,
    leftWeight: 700,
  });
  assert.ok(candidates.length >= 4);
  assert.ok(candidates[0].font.id !== "h1");
  // Inter is classic paired with Playfair Display + activated sans-serif body
  assert.equal(candidates[0].font.id, "c1");
  // Arabic-only font cannot lead Latin candidates over Latin sans-serif
  const c6Score = candidates.find((c) => c.font.id === "c6")?.score ?? 0;
  assert.ok(candidates[0].score > c6Score);

  const pick0 = pickNextCompanion(candidates, 0, "c1");
  assert.ok(pick0 && pick0.font.id !== "c1");
  const pick1 = pickNextCompanion(candidates, 1, pick0.font.id);
  assert.ok(pick1 && pick1.font.id !== pick0.font.id);

  const matrix = computePairingMatrix({
    left: heading,
    right: candidates[0].font,
    leftSize: 52,
    rightSize: 16,
    leftWeight: 700,
    rightWeight: 400,
  });
  assert.ok(matrix.score > 60);
});

test("5. Upgraded SuperSearch parses quotes, negation, comparators, negative slnt bounds, category/source/script", () => {
  assert.deepEqual(tokenizeSearchQuery('"Space Grotesk" -italic weight:>=700'), [
    '"Space Grotesk"',
    "-italic",
    "weight:>=700",
  ]);

  const clauses = parseSearchQuery('"Space Grotesk" -italic weight:>=700 slnt:-12..0 axis:opsz source:local category:sans,mono script:latin');
  assert.equal(clauses[0].kind, "hay");
  assert.equal(clauses[0].value, "space grotesk");
  assert.equal(clauses[1].kind, "flag");
  assert.equal(clauses[1].negated, true);
  assert.equal(clauses[2].kind, "range");
  assert.equal(clauses[2].min, 700);
  assert.equal(clauses[2].max, Infinity);
  assert.equal(clauses[3].kind, "axis");
  assert.equal(clauses[3].min, -12);
  assert.equal(clauses[3].max, 0);
  assert.equal(clauses[4].kind, "axis");
  assert.equal(clauses[4].tag, "opsz");

  const matchFont = makeFont({
    id: "sg",
    family: "Space Grotesk",
    source: "local",
    category: "sans",
    weights: [400, 700],
    variable: true,
    italic: false,
    axes: [
      { tag: "wght", name: "Weight", min: 300, max: 700, def: 400 },
      { tag: "slnt", name: "Slant", min: -10, max: 0, def: 0 },
      { tag: "opsz", name: "Optical size", min: 8, max: 72, def: 14 },
    ],
  });
  assert.equal(fontMatchesSearch(matchFont, clauses, {}), true);
  assert.equal(fontMatchesSearch(makeFont({ family: "Space Mono", source: "google" }), clauses, {}), false);

  assert.equal(fontMatchesSearch(matchFont, parseSearchQuery("weight:>700"), {}), false);
  assert.equal(fontMatchesSearch(matchFont, parseSearchQuery("weight:>=700"), {}), true);
  assert.equal(fontMatchesSearch(matchFont, parseSearchQuery("variable:false"), {}), false);
  assert.equal(fontMatchesSearch(makeFont({ variable: false }), parseSearchQuery("has:variable"), {}), false);
  assert.equal(fontMatchesSearch(matchFont, parseSearchQuery("has:slnt"), {}), true);

  assert.equal(toggleSearchToken('weight:>=700 "Space Grotesk"', '"Space Grotesk"'), "weight:>=700");
});

test("6. CSS format helpers emit accurate formats, VF descriptors, encoded URLs, and safe Tailwind tokens", () => {
  assert.equal(fontFormatForFile("Inter.ttf"), "truetype");
  assert.equal(fontFormatForFile("FiraCode.otf"), "opentype");
  assert.equal(fontFormatForFile("Newsreader.woff2"), "woff2");
  assert.equal(fontFormatForFile("Legacy.woff"), "woff");

  assert.equal(tailwindFontVarKey("Space Grotesk"), "space-grotesk");
  assert.equal(tailwindFontVarKey("Noto Sans JP", 2), "noto-sans-jp");
  assert.equal(tailwindFontVarKey("思源黑體", 2), "font-3");

  const vf = makeFont({
    id: "local-vf",
    family: "Recursive VF",
    source: "local",
    fileName: "Recursive Variable.ttf",
    variable: true,
    axes: [
      { tag: "wght", name: "Weight", min: 300, max: 1000, def: 400 },
      { tag: "slnt", name: "Slant", min: -15, max: 0, def: 0 },
      { tag: "wdth", name: "Width", min: 75, max: 125, def: 100 },
    ],
  });
  const faceCss = formatLocalFontFace(vf);
  assert.match(faceCss, /src: url\("\/fonts\/Recursive%20Variable\.ttf"\) format\("truetype"\);/);
  assert.match(faceCss, /font-weight: 300 1000;/);
  assert.match(faceCss, /font-style: oblique 0deg 15deg;/);
  assert.match(faceCss, /font-stretch: 75% 125%;/);

  const specCss = formatInspectorCss({
    family: "Recursive VF",
    category: "sans",
    weight: 600,
    fontStyle: "oblique 12deg",
    fontStretch: "90%",
    fontVariationSettings: '"opsz" 24',
    fontOpticalSizing: "none",
    fontFeatureSettings: '"ss01" 1',
  });
  assert.match(specCss, /font-family: "Recursive VF", sans-serif;/);
  assert.match(specCss, /font-weight: 600;/);
  assert.match(specCss, /font-style: oblique 12deg;/);
  assert.match(specCss, /font-stretch: 90%;/);
  assert.match(specCss, /font-variation-settings: "opsz" 24;/);
  assert.match(specCss, /font-optical-sizing: none;/);
  assert.match(specCss, /font-feature-settings: "ss01" 1;/);
});

test("7. Specimen CSS classes do not use font-style !important that would override live slnt oblique styles", () => {
  const css = readFileSync(join(process.cwd(), "src", "styles.css"), "utf8");
  const romanBlock = css.match(/\.fm-spec\.fm-spec-roman\s*\{[^}]*\}/)?.[0] ?? "";
  const italicBlock = css.match(/\.fm-spec\.fm-spec-italic\s*\{[^}]*\}/)?.[0] ?? "";
  assert.ok(romanBlock.length > 0 && italicBlock.length > 0);
  assert.equal(/font-style:[^;]*!important/i.test(romanBlock), false);
  assert.equal(/font-style:[^;]*!important/i.test(italicBlock), false);
});

test("8. OpenType metadata (isFixedPitch, sFamilyClass, PANOSE, widthClass, axes) drives local tags", () => {
  const fixedPitch = inferLocalStyle({
    family: "Custom Utility",
    isFixedPitch: true,
    hasLigatures: true,
  });
  assert.equal(fixedPitch.category, "mono");
  assert.ok(fixedPitch.tags.includes("coding"));
  assert.ok(fixedPitch.tags.includes("ligatures"));

  const condensedRounded = inferLocalStyle({
    family: "Clean UI",
    sFamilyClass: (8 << 8) | 10,
    widthClass: 3,
    axes: [{ tag: "opsz", min: 8, max: 72 }],
  });
  assert.equal(condensedRounded.category, "sans");
  assert.ok(condensedRounded.tags.includes("rounded"));
  assert.ok(condensedRounded.tags.includes("condensed"));
  assert.ok(condensedRounded.tags.includes("editorial"));

  const metrics = metricsFromTables({
    upem: 1000,
    weightClass: 400,
    widthClass: 3,
    panose: [2, 6, 5, 6, 3, 1, 1, 1, 1, 1],
    sFamilyClass: 0x0501,
    isFixedPitch: false,
  });
  assert.equal(metrics.sFamilyClass, 0x0501);
  assert.equal(metrics.isFixedPitch, false);
  assert.equal(metrics.panose?.length, 10);

  const slabFromPanose = inferLocalStyle({
    family: "Foundry Text",
    panose: metrics.panose,
    widthClass: metrics.widthClass,
  });
  assert.equal(slabFromPanose.category, "serif");
  assert.ok(slabFromPanose.tags.includes("slab"));
  assert.ok(slabFromPanose.tags.includes("condensed"));
});

test("9. Session & UI state persistence across exit/reopen and Playground fixes", () => {
  const storeTs = readTsModule(process.cwd(), "src/lib/fonts/store.ts");
  const hydrateTs = readFileSync(join(process.cwd(), "src/lib/fonts/hydrate.ts"), "utf8");
  const playgroundTsx = readFileSync(join(process.cwd(), "src/components/font-studio/playground.tsx"), "utf8");
  const pickerTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-picker.tsx"), "utf8");
  const gridTsx = readFileSync(join(process.cwd(), "src/components/font-studio/library-grid.tsx"), "utf8");

  assert.match(storeTs, /id\.startsWith\("l-"\)/);
  assert.match(storeTs, /ids\.has\(id\) \|\| id\.startsWith\("g:"\) \|\| id\.startsWith\("l-"\)/);
  assert.match(storeTs, /export function setBootRestoringIds/);
  assert.match(storeTs, /selectedId:/);
  assert.match(storeTs, /inspectorOpen:/);
  assert.match(hydrateTs, /setBootRestoringIds\(wantIds\)/);
  assert.match(hydrateTs, /installFontOnSystem\(localFont\)/);

  assert.match(playgroundTsx, /font-manager:playground-v1/);
  assert.match(playgroundTsx, /findFont\(leftId,\s*localFonts,\s*googleFonts\)/);
  assert.match(playgroundTsx, /"wght" in prev/);
  assert.doesNotMatch(pickerTsx, /if \(f\.source !== "system" && !activatedSet\.has\(f\.id\)\) return false;/);
  assert.match(gridTsx, /scroller\.scrollTop = 0;/);
});

test("10. Live Google Fonts command, upload Pause/Resume/Cancel, and cleaned remnants", () => {
  const activateRs = readRustModule(process.cwd(), "src-tauri/src/activate.rs");
  const mainRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");
  const googleApi = readFileSync(join(process.cwd(), "src/lib/fonts/google-api.ts"), "utf8");
  const storeTs = readTsModule(process.cwd(), "src/lib/fonts/store.ts");
  const osActivate = readTsModule(process.cwd(), "src/lib/fonts/os-activate.ts");
  const dropzone = readFileSync(join(process.cwd(), "src/components/font-studio/upload-dropzone.tsx"), "utf8");

  assert.match(activateRs, /pub async fn fetch_live_google_catalog/);
  assert.match(mainRs, /activate::fetch_live_google_catalog/);
  assert.match(perms, /"fetch_live_google_catalog"/);
  assert.match(googleApi, /fetch_live_google_catalog/);

  assert.match(storeTs, /pauseUpload:/);
  assert.match(storeTs, /resumeUpload:/);
  assert.match(storeTs, /cancelUpload:/);
  assert.match(osActivate, /export function setUploadQueuePaused/);
  assert.match(osActivate, /export function cancelUploadDiskQueue/);
  assert.match(dropzone, /data-testid="upload-pause-resume"/);
  assert.match(dropzone, /data-testid="upload-cancel"/);
});

test("11. Suggest pair supports Lock Heading, Lock Body, and unlocked random ideal pairings", () => {
  const heading = makeFont({
    id: "h1",
    family: "Playfair Display",
    category: "serif",
    weights: [400, 700],
    script: "latin",
    popularity: 10,
  });
  const body = makeFont({
    id: "b1",
    family: "Inter",
    category: "sans",
    weights: [400, 500, 700],
    script: "latin",
    popularity: 1,
  });
  const pool = [
    heading,
    body,
    makeFont({ id: "h2", family: "Fraunces", category: "serif", weights: [400, 700], script: "latin", popularity: 25 }),
    makeFont({ id: "b2", family: "Source Sans 3", category: "sans", weights: [400, 600], script: "latin", popularity: 15 }),
    makeFont({ id: "m1", family: "JetBrains Mono", category: "mono", weights: [400, 700], script: "latin", popularity: 20 }),
  ];

  // Body locked: suggest ideal Heading candidates for Inter
  const headingCands = suggestHeadingCandidates({
    right: body,
    pool,
    activatedIds: new Set(["h1"]),
    leftSize: 48,
    rightSize: 18,
    rightWeight: 400,
  });
  assert.ok(headingCands.length >= 3);
  assert.equal(headingCands[0].font.id, "h1");
  assert.ok(headingCands[0].score >= 80);

  // Neither locked: suggest a random ideal Heading + Body pair avoiding current pair
  const randomPair = suggestRandomIdealPair({
    pool,
    activatedIds: new Set(["h1", "b1"]),
    leftSize: 48,
    rightSize: 18,
    currentLeftId: "h1",
    currentRightId: "b1",
    randomFn: () => 0,
  });
  assert.ok(randomPair);
  assert.notEqual(randomPair.heading.id, "h1");
  assert.notEqual(randomPair.body.id, randomPair.heading.id);
  assert.ok(randomPair.score >= 70);
});

test("12. Watched folders run smart duplicate collision detection and stop watching without converting to Collections", () => {
  const googleFonts = [makeFont({ id: "g:Inter", family: "Inter", source: "google" })];
  const localFonts = [makeFont({ id: "l-custom", family: "avenir next", source: "local" })];
  const likes = [
    { name: "Inter-Regular.ttf", size: 120000 },
    { name: "AvenirNext-Bold.otf", size: 90000 },
    { name: "BrandUnique-Regular.ttf", size: 80000 },
  ];
  const { collisions, collidingFileIndices } = findImportGoogleCollisions(
    likes,
    googleFonts,
    new Set(["g:Inter"]),
    new Set(["inter"]),
    localFonts,
  );
  assert.equal(collisions.length, 2);
  assert.equal(collidingFileIndices.has(0), true);
  assert.equal(collidingFileIndices.has(1), true);
  assert.equal(collidingFileIndices.has(2), false);

  const watchTs = readFileSync(join(process.cwd(), "src/lib/fonts/watch-folder.ts"), "utf8");
  const folderTreeTsx = readFileSync(join(process.cwd(), "src/components/font-studio/folder-tree.tsx"), "utf8");
  const appShellTsx = readFileSync(join(process.cwd(), "src/components/font-studio/app-shell.tsx"), "utf8");
  const settingsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/desktop-settings.tsx"), "utf8");

  assert.match(watchTs, /findImportGoogleCollisions/);
  assert.match(watchTs, /requestImportCollisionChoice/);
  assert.doesNotMatch(folderTreeTsx, /Convert to collection/);
  assert.doesNotMatch(appShellTsx, /Upload folder \(Copy\)/);
  assert.doesNotMatch(settingsTsx, /open-uploads-folder-btn/);
});

test("13. Fonths 2.0.0 Skia + Fontations + Windows Activator replaces DirectWrite/ttf-parser cleanly in existing UI", () => {
  const cargoToml = readFileSync(join(process.cwd(), "src-tauri/Cargo.toml"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const scannerRs = readFileSync(join(process.cwd(), "src-tauri/src/scanner.rs"), "utf8");
  const rendererRs = readFileSync(join(process.cwd(), "src-tauri/src/renderer.rs"), "utf8");
  const activatorRs = readFileSync(join(process.cwd(), "src-tauri/src/activator.rs"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");
  const canvasTsx = readFileSync(join(process.cwd(), "src/components/font-studio/skia-font-canvas.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const appShellTsx = readFileSync(join(process.cwd(), "src/components/font-studio/app-shell.tsx"), "utf8");

  assert.match(cargoToml, /version = "2\.\d+\.\d+"/);
  assert.doesNotMatch(cargoToml, /skia-safe/);
  assert.match(cargoToml, /read-fonts/);
  assert.match(cargoToml, /skrifa/);
  assert.match(cargoToml, /harfrust/);
  assert.match(cargoToml, /tiny-skia/);
  assert.match(cargoToml, /rayon/);
  assert.match(cargoToml, /lru/);
  assert.match(cargoToml, /memmap2/);
  assert.doesNotMatch(cargoToml, /ttf-parser/);
  assert.doesNotMatch(cargoToml, /Win32_Graphics_DirectWrite/);

  assert.match(libRs, /pub mod scanner;/);
  assert.match(libRs, /pub mod renderer;/);
  assert.match(libRs, /pub mod activator;/);
  assert.match(libRs, /pub mod font_protocol;/);
  assert.match(libRs, /register_asynchronous_uri_scheme_protocol\("font"/);
  assert.match(libRs, /scanner::scan_font_directory/);
  assert.match(libRs, /renderer::render_font_preview/);
  assert.match(libRs, /activator::activate_font/);
  assert.match(libRs, /activator::deactivate_font/);
  assert.match(libRs, /activator::cleanup_on_exit/);

  assert.match(scannerRs, /pub fn scan_font_directory/);
  assert.match(scannerRs, /par_iter/);
  assert.match(scannerRs, /Mmap::map/);
  assert.match(scannerRs, /font\.gsub\(\)/);
  assert.match(scannerRs, /font\.gpos\(\)/);

  assert.match(rendererRs, /harfrust::shape/);
  assert.match(rendererRs, /LruCache<String, FontationsFont>/);
  assert.match(rendererRs, /instance_builder\(\)\.variations/);
  assert.match(rendererRs, /Feature::new/);
  assert.match(rendererRs, /tiny_skia::\{/);
  assert.match(rendererRs, /tauri::ipc::Response/);

  assert.match(activatorRs, /AddFontResourceExW/);
  assert.match(activatorRs, /RemoveFontResourceExW/);
  assert.match(activatorRs, /FONT_RESOURCE_CHARACTERISTICS\(0\)/);
  assert.match(activatorRs, /WM_FONTCHANGE/);
  assert.match(activatorRs, /#\[cfg\(not\(windows\)\)\]/);

  for (const cmd of ["scan_font_directory", "render_font_preview", "activate_font", "deactivate_font"]) {
    assert.match(perms, new RegExp(`"${cmd}"`));
  }

  assert.match(canvasTsx, /new ImageData\(frame\.pixels/);
  assert.match(canvasTsx, /ctx\.putImageData/);
  assert.match(inspectorTsx, /<SkiaFontCanvas/);
  assert.doesNotMatch(appShellTsx, /Skia Studio/);
});

test("14. Live Skia Inspector (italic/weight/auto-height/scroll), watched dedupe+collision, and lock combat/tray prefs", () => {
  const rendererRs = readFileSync(join(process.cwd(), "src-tauri/src/renderer.rs"), "utf8");
  const activateRs = readRustModule(process.cwd(), "src-tauri/src/activate.rs");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const canvasTsx = readFileSync(join(process.cwd(), "src/components/font-studio/skia-font-canvas.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const settingsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/desktop-settings.tsx"), "utf8");

  assert.match(rendererRs, /pub async fn render_font_specimen/);
  assert.match(rendererRs, /coalesce_latest/);
  assert.match(canvasTsx, /renderFontSpecimen/);
  assert.match(inspectorTsx, /viewportClassName="\[&>div\]:!block \[&>div\]:min-w-0 \[&>div\]:max-w-full"/);
  assert.match(inspectorTsx, /data-testid="inspector-italic-switch"/);
  assert.match(inspectorTsx, /italic=\{effectiveItalicOn\}/);

  // Within-folder batch deduplication (same filename + non-zero size across subfolders)
  const deduped = dedupeWatchedBatch([
    { name: "Inter-Regular.ttf", size: 120_000, webkitRelativePath: "A/Inter-Regular.ttf" },
    { name: "Inter-Regular.ttf", size: 120_000, webkitRelativePath: "Backup/Inter-Regular.ttf" },
    { name: "Inter-Bold.ttf", size: 125_000, webkitRelativePath: "A/Inter-Bold.ttf" },
  ]);
  assert.equal(deduped.duplicateCount, 1);
  assert.deepEqual(deduped.keptIndices, [0, 2]);

  // System + Local collision detection
  const { collisions } = findImportGoogleCollisions(
    [{ name: "Arial-Bold.ttf", size: 50_000 }],
    [],
    new Set(),
    new Set(),
    [],
    [makeFont({ id: "sys:arial", family: "Arial", source: "system" })],
  );
  assert.equal(collisions.length, 1);
  assert.equal(collisions[0].matchSource, "system");

  // Lock combat & Background mode toggles
  assert.match(activateRs, /fn rotated_stage_path/);
  assert.match(activateRs, /fn copy_with_lock_retry/);
  assert.match(libRs, /start_minimized: Option<bool>/);
  assert.match(settingsTsx, /data-testid="activation-mode-group"/);
  assert.match(settingsTsx, /data-testid="start-minimized-switch"/);
});

test("15. Fonths 2.2.2 headless daemon, state honesty, live sliders, and SuperSearch fixes", () => {
  const daemonRs = readFileSync(join(process.cwd(), "src-tauri/src/daemon.rs"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const sliderTsx = readFileSync(join(process.cwd(), "src/components/ui/slider.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const osActivateTs = readTsModule(process.cwd(), "src/lib/fonts/os-activate.ts");
  const hydrateTs = readFileSync(join(process.cwd(), "src/lib/fonts/hydrate.ts"), "utf8");
  const chipsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/search-chips.tsx"), "utf8");

  // 1. Two-process headless daemon architecture
  assert.match(daemonRs, /pub const DAEMON_ARG: &str = "--daemon"/);
  assert.match(daemonRs, /session_begin_headless/);
  assert.match(daemonRs, /session_end_headless/);
  assert.match(libRs, /pub mod daemon;/);
  assert.match(libRs, /fn daemon_status\(\)/);
  assert.match(libRs, /fn handoff_to_daemon_and_exit/);

  // 2. State management honesty (no guessing on upload or boot restore)
  assert.match(osActivateTs, /if \(register\)/);
  assert.match(osActivateTs, /item\.autoActivate/);
  assert.match(hydrateTs, /await installFontOnSystem\(localFont\)/);

  // 3. Preview sliders (no double onInput, live lineHeight + letterSpacing in Inspector)
  assert.doesNotMatch(sliderTsx, /onInput=/);
  assert.match(inspectorTsx, /lineHeight=\{preview\.lineHeight\}/);
  assert.match(inspectorTsx, /letterSpacing=\{preview\.letterSpacing\}/);

  // 4. SuperSearch fixes: discrete static weight matching, OpenType feature search, category:icons, facet-aware chips
  const staticGap = makeFont({ id: "s-gap", family: "Gap Sans", weights: [300, 900], variable: false });
  assert.equal(fontMatchesSearch(staticGap, parseSearchQuery("weight:500"), {}), false);
  assert.equal(fontMatchesSearch(staticGap, parseSearchQuery("weight:300"), {}), true);
  assert.equal(fontMatchesSearch(staticGap, parseSearchQuery("weight:900"), {}), true);

  const otFont = makeFont({ id: "ot-1", family: "Pro Serif", otFeatures: ["liga", "smcp", "ss01"] });
  assert.equal(fontMatchesSearch(otFont, parseSearchQuery("has:smcp"), {}), true);
  assert.equal(fontMatchesSearch(otFont, parseSearchQuery("ot:ss01"), {}), true);
  assert.equal(fontMatchesSearch(otFont, parseSearchQuery("has:dlig"), {}), false);

  const iconFont = makeFont({ id: "ico-1", family: "Material Symbols", category: "display", tags: ["icons"] });
  assert.equal(fontMatchesSearch(iconFont, parseSearchQuery("category:icons"), {}), true);

  assert.match(chipsTsx, /filterLibrary\(pool, scope, "", favorites, liveIds, collections, customTags, facet, recentIds\)/);
});

test("16. Fonths 2.2.2 pure-Rust Fontations/HarfRust/font:// stack, NTFS hardlink staging, daemon adoption, and honest counts", () => {
  const activateRs = readRustModule(process.cwd(), "src-tauri/src/activate.rs");
  const sessionStageRs = readFileSync(join(process.cwd(), "src-tauri/src/session_stage.rs"), "utf8");
  const fontProtoRs = readFileSync(join(process.cwd(), "src-tauri/src/font_protocol.rs"), "utf8");
  const loaderTs = readFileSync(join(process.cwd(), "src/lib/fonts/loader.ts"), "utf8");
  const osActivateTs = readTsModule(process.cwd(), "src/lib/fonts/os-activate.ts");
  const tauriConf = readFileSync(join(process.cwd(), "src-tauri/tauri.conf.json"), "utf8");

  // Zero-copy font:// protocol wired end-to-end
  assert.match(fontProtoRs, /pub fn handle_request/);
  assert.match(fontProtoRs, /Mmap::map/);
  assert.match(tauriConf, /http:\/\/font\.localhost/);
  assert.match(loaderTs, /convertFileSrc\(path, "font"\)/);

  // Fast cached activation: NTFS hardlink staging, daemon session adoption, diff-based HKCU sync, O(1) progress sets
  assert.match(activateRs, /fs::hard_link\(src, &dest\)/);
  assert.match(activateRs, /pub fn adopt_daemon_live_session/);
  assert.match(activateRs, /winfont::adopt_daemon_live_session/);
  assert.match(sessionStageRs, /RegEnumValueW/);

  // Honest summary toast counts: finalize ready/settled marks BEFORE reading store counts for toast
  const finalizeIdx = osActivateTs.indexOf("await finalizeReadyAndClearPending(pendingToClear);");
  const notifyIdx = osActivateTs.indexOf("notifyDownloadResult(doneCount, failedCount, failedNames, failedDetails, settledNames);");
  assert.ok(finalizeIdx > 0 && notifyIdx > finalizeIdx, "finalizeReadyAndClearPending must complete before notifyDownloadResult");
});

test("17. Fonths 2.2.2 watched-folder duplicate toast fix, mmap eviction on delete, zero stale GDI/Skia UI copy, and SuperSearch polish", () => {
  const watchTs = readFileSync(join(process.cwd(), "src/lib/fonts/watch-folder.ts"), "utf8");
  const activateRs = readRustModule(process.cwd(), "src-tauri/src/activate.rs");
  const activatorRs = readFileSync(join(process.cwd(), "src-tauri/src/activator.rs"), "utf8");
  const fontProtoRs = readFileSync(join(process.cwd(), "src-tauri/src/font_protocol.rs"), "utf8");
  const confirmTsx = readFileSync(join(process.cwd(), "src/components/font-studio/activate-confirm-dialog.tsx"), "utf8");
  const toggleTsx = readFileSync(join(process.cwd(), "src/components/font-studio/activate-toggle.tsx"), "utf8");
  const chipsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/search-chips.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const appShellTsx = readFileSync(join(process.cwd(), "src/components/font-studio/app-shell.tsx"), "utf8");

  // 1. Watched-folder duplicate toast loop fix
  assert.match(watchTs, /handledWatchedSigs/);
  assert.match(watchTs, /!latest\.autoHideDuplicates/);

  // 2. Windows mmap eviction before file/folder delete + activator.rs Documents guard
  assert.match(fontProtoRs, /pub fn evict_dir\(/);
  assert.match(activateRs, /crate::font_protocol::evict_path\(path\)/);
  assert.match(activateRs, /crate::font_protocol::evict_dir\(&dir\)/);
  assert.match(activatorRs, /must_not_register_as_gdi_path/);

  // 3. Zero stale GDI/Skia labels in confirm dialogs & toggle toasts
  assert.doesNotMatch(confirmTsx, /GDI/);
  assert.doesNotMatch(toggleTsx, /pre-staged GDI maps/);
  assert.doesNotMatch(inspectorTsx, /preview in Skia/);

  // 4. SuperSearch polish: expanded syntax help, active chip contrast + intersected counts, Inspector ot: chips
  assert.match(appShellTsx, /"ot:smcp"/);
  assert.match(appShellTsx, /"category:icons"/);
  assert.match(chipsTsx, /text-primary-foreground\/85/);
  assert.match(chipsTsx, /effectiveQuery/);
  assert.match(inspectorTsx, /token: `ot:\$\{tag\}`/);
});

test("18. Fonths 2.3.0 daemon hardening, RTL/fallback/color Fontations renderer, HKCU sync toggle, JSON backup/restore, watch cleanup, and path safety", () => {
  // 1. Default HKCU registry sync preference + coercion
  assert.equal(DEFAULT_DESKTOP_PREFS.hkcuRegistrySync, true);
  assert.equal(coerceDesktopPrefs({}).hkcuRegistrySync, true);
  assert.equal(coerceDesktopPrefs({ activationMode: "daemon-tray", hkcuRegistrySync: false }).hkcuRegistrySync, false);

  // 2. JSON Backup & Restore roundtrip, validation, and non-activation guarantee
  const snapshot = buildLibraryBackup({
    favorites: ["g:Inter", "g:Inter"],
    collections: [{ id: "c-1", name: "Brand", fontIds: ["g:Inter"], createdAt: 1700000000000, parentId: null }],
    fontSets: [{ id: "fs-1", name: "Web", families: ["Inter"], updatedAt: 1700000000000 }],
    customTags: { "g:Inter": ["hero", "ui"] },
    featurePrefs: { "g:Inter": { liga: true, smcp: false } },
    previewAxes: { "g:Inter": { wght: 650 } },
    autoHideDuplicates: true,
    desktopPrefs: { ...DEFAULT_DESKTOP_PREFS, activationMode: "daemon-tray", hkcuRegistrySync: false, deleteMode: "permanent" },
  });
  assert.equal(snapshot.version, FONTHS_BACKUP_VERSION);
  assert.equal(snapshot.app, "Fonths");
  assert.deepEqual(snapshot.favorites, ["g:Inter"]);

  const restored = parseLibraryBackup(JSON.stringify(snapshot));
  assert.equal(restored.desktopPrefs.hkcuRegistrySync, false);
  assert.equal(restored.desktopPrefs.deleteMode, "permanent");
  assert.equal(restored.autoHideDuplicates, true);
  assert.throws(() => parseLibraryBackup("not-json"), /not valid JSON/i);
  assert.throws(() => parseLibraryBackup(JSON.stringify({ app: "Other", version: 1 })), /Not a Fonths/);
  assert.throws(() => parseLibraryBackup(JSON.stringify({ app: "Fonths", version: 99, favorites: [], collections: [] })), /Unsupported backup version/);

  const merged = mergeLibraryBackupState(
    {
      favorites: ["g:Roboto"],
      collections: [{ id: "c-watch", name: "Dropbox", fontIds: ["l-1"], createdAt: 1, parentId: null, watchPath: "D:\\Dropbox" }],
      fontSets: [],
      customTags: { "g:Roboto": ["body"] },
      featurePrefs: {},
      previewAxes: {},
      autoHideDuplicates: false,
      desktopPrefs: DEFAULT_DESKTOP_PREFS,
    },
    restored,
  );
  assert.deepEqual(merged.favorites, ["g:Roboto", "g:Inter"]);
  assert.equal(merged.collections.length, 2);
  assert.equal(merged.collections[0].watchPath, "D:\\Dropbox");
  assert.equal(merged.desktopPrefs.hkcuRegistrySync, false);
  assert.equal("activated" in merged, false, "Backup restore must never auto-activate fonts unexpectedly");

  // 3. Daemon + NSIS hook hardening
  const daemonRs = readFileSync(join(process.cwd(), "src-tauri/src/daemon.rs"), "utf8");
  const hooksNsh = readFileSync(join(process.cwd(), "src-tauri/windows/hooks.nsh"), "utf8");
  assert.match(daemonRs, /QUIT_DAEMON_ARG.*--quit-daemon/);
  assert.match(daemonRs, /TaskbarCreated/);
  assert.match(daemonRs, /ExtractIconExW/);
  assert.match(daemonRs, /DestroyIcon/);
  assert.match(daemonRs, /session_end_headless/);
  assert.match(hooksNsh, /FM_STOP_DAEMON/);
  assert.match(hooksNsh, /--quit-daemon/);
  assert.match(hooksNsh, /MainWindowHandle -eq 0/);

  // 4. Fontations RTL, lazy system fallback, and color/emoji DOM fallback
  const rendererRs = readFileSync(join(process.cwd(), "src-tauri/src/renderer.rs"), "utf8");
  const canvasTsx = readFileSync(join(process.cwd(), "src/components/font-studio/skia-font-canvas.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  assert.match(rendererRs, /COLOR_FONT_DOM_FALLBACK/);
  assert.match(rendererRs, /first_strong_is_rtl/);
  assert.match(rendererRs, /Direction::RightToLeft/);
  assert.match(rendererRs, /shape_segment_with_fallbacks/);
  assert.match(canvasTsx, /onColorFallback/);
  assert.match(canvasTsx, /color-font-dom-fallback/);
  assert.match(inspectorTsx, /canvasColorFallback/);

  // 5. HKCU sync toggle, backup UI, watched folder UX, unwatch cleanup, font:// traversal guard, clean copy
  const sessionStageRs = readFileSync(join(process.cwd(), "src-tauri/src/session_stage.rs"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const fontProtoRs = readFileSync(join(process.cwd(), "src-tauri/src/font_protocol.rs"), "utf8");
  const settingsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/desktop-settings.tsx"), "utf8");
  const folderTreeTsx = readFileSync(join(process.cwd(), "src/components/font-studio/folder-tree.tsx"), "utf8");
  const watchTs = readFileSync(join(process.cwd(), "src/lib/fonts/watch-folder.ts"), "utf8");
  const exportColTs = readFileSync(join(process.cwd(), "src/lib/fonts/export-collection.ts"), "utf8");

  assert.match(sessionStageRs, /HKCU_SYNC_ENABLED/);
  assert.match(sessionStageRs, /pub fn set_hkcu_sync_enabled/);
  assert.match(libRs, /hkcu_registry_sync/);
  assert.match(fontProtoRs, /Component::ParentDir/);
  assert.match(settingsTsx, /hkcu-registry-sync-switch/);
  assert.match(settingsTsx, /export-backup-btn/);
  assert.match(settingsTsx, /import-backup-btn/);
  assert.match(folderTreeTsx, /clearWatchedFileMemory/);
  assert.match(watchTs, /watch-folder-op/);
  assert.doesNotMatch(watchTs, /deploy\.bat/);
  assert.match(exportColTs, /Fonths collection/);
  assert.doesNotMatch(exportColTs, /# Font Manager collection export/);
});


test("19. Fonths 2.4.0 instant two-wave startup, ETag/304 Ask-Before-Sync Google CDN, dual-lane pacing, Rust 2024, modular backend/frontend, and Vite 8 chunking", () => {
  const cargoToml = readFileSync(join(process.cwd(), "src-tauri/Cargo.toml"), "utf8");
  const pkgJson = JSON.parse(readFileSync(join(process.cwd(), "package.json"), "utf8"));
  const viteCfg = readFileSync(join(process.cwd(), "vite.config.ts"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const googleCdnRs = readFileSync(join(process.cwd(), "src-tauri/src/google_cdn.rs"), "utf8");
  const gfsourceRs = readFileSync(join(process.cwd(), "src-tauri/src/gfsource.rs"), "utf8");
  const winfontRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/winfont.rs"), "utf8");
  const sessionRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/session.rs"), "utf8");
  const cdnSyncRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/cdn_sync.rs"), "utf8");
  const googleApiTs = readFileSync(join(process.cwd(), "src/lib/fonts/google-api.ts"), "utf8");
  const hydrateTs = readFileSync(join(process.cwd(), "src/lib/fonts/hydrate.ts"), "utf8");
  const hydrateStepsTs = readFileSync(join(process.cwd(), "src/lib/fonts/hydrate-steps.ts"), "utf8");

  // 1. Rust 2024 edition & modular backend/frontend files
  assert.match(cargoToml, /edition\s*=\s*"2024"/);
  assert.match(cargoToml, /version\s*=\s*"2\.\d+\.\d+"/);
  for (const rel of [
    "src-tauri/src/activate/mod.rs",
    "src-tauri/src/activate/winfont.rs",
    "src-tauri/src/activate/session.rs",
    "src-tauri/src/activate/cdn_sync.rs",
    "src-tauri/src/activate/disk_scan.rs",
    "src-tauri/src/activate/downloads.rs",
    "src-tauri/src/activate/tests.rs",
    "src/lib/fonts/store/duplicate-engine.ts",
    "src/lib/fonts/store/folder-helpers.ts",
    "src/lib/fonts/store/library-query.ts",
    "src/lib/fonts/activation/types.ts",
    "src/lib/fonts/activation/session-bridge.ts",
    "src/lib/fonts/activation/disk-ops.ts",
  ]) {
    assert.ok(readFileSync(join(process.cwd(), rel), "utf8").length > 0, `expected module ${rel}`);
  }
  assert.throws(() => readFileSync(join(process.cwd(), "src-tauri/src/activate.rs"), "utf8"));

  // 2. Two-wave instant startup + parallel hydration + deferred idle reclassification
  assert.match(winfontRs, /pub fn register_prestaged_stage_path/);
  assert.match(sessionRs, /WAVE0_SIZE/);
  assert.match(sessionRs, /partition_prestaged_targets/);
  assert.match(hydrateStepsTs, /const catalogTask = Promise\.resolve\(\)/);
  assert.match(hydrateStepsTs, /opts\.loadCatalog\(\)/);
  assert.match(hydrateTs, /requestIdleCallback/);
  assert.match(hydrateTs, /syncManagedDocumentsRoot/);

  // 3. Official Google CDN ETag/304 + Ask-Before-Sync + Dual-Lane HTTP/2 Pacing
  assert.match(googleCdnRs, /shared_blocking_client/);
  assert.match(googleCdnRs, /IF_NONE_MATCH/);
  assert.match(googleCdnRs, /NOT_MODIFIED/);
  assert.match(cdnSyncRs, /pub async fn fetch_live_google_catalog_conditional/);
  assert.match(cdnSyncRs, /pub async fn check_google_cdn_updates/);
  assert.match(libRs, /activate::fetch_live_google_catalog_conditional/);
  assert.match(libRs, /activate::check_google_cdn_updates/);
  assert.match(perms, /"fetch_live_google_catalog_conditional"/);
  assert.match(perms, /"check_google_cdn_updates"/);
  assert.match(gfsourceRs, /CDN_MAX_CONCURRENT/);
  assert.match(gfsourceRs, /CDN_MIN_SPACING/);
  assert.match(gfsourceRs, /is_cdn_binary_url/);
  assert.match(gfsourceRs, /cache_fresh_for_revision/);
  assert.match(gfsourceRs, /Fonths\/2\.(4|5|6)/);
  assert.match(googleApiTs, /checkGoogleCdnUpdates/);
  assert.match(googleApiTs, /label:\s*"Sync now"/);
  assert.match(googleApiTs, /if \(syncFiles\)/);

  // 4. Pruned unused deps & Vite 8 Rolldown chunking
  assert.equal("react-day-picker" in pkgJson.dependencies, false);
  assert.equal("cmdk" in pkgJson.dependencies, false);
  assert.match(viteCfg, /codeSplitting/);
  assert.match(viteCfg, /react-vendor/);
  assert.match(viteCfg, /ui-vendor/);
});

test("20. Fonths 2.5.0 Rust 1.100, Zero-RAM persistent HKCU activation, WebView2/Playground performance, rayon meta-indexer, virtualized Glyphs, Rust duplicates & audit fixes", () => {
  const cargoToml = readFileSync(join(process.cwd(), "src-tauri/Cargo.toml"), "utf8");
  const modRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/mod.rs"), "utf8");
  const sessionStageRs = readFileSync(join(process.cwd(), "src-tauri/src/session_stage.rs"), "utf8");
  const winfontRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/winfont.rs"), "utf8");
  const sessionRs = readFileSync(join(process.cwd(), "src-tauri/src/activate/session.rs"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const hooksNsh = readFileSync(join(process.cwd(), "src-tauri/windows/hooks.nsh"), "utf8");
  const metaIndexRs = readFileSync(join(process.cwd(), "src-tauri/src/meta_index.rs"), "utf8");
  const parseRs = readFileSync(join(process.cwd(), "src-tauri/src/parse.rs"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");

  const prefsTs = readFileSync(join(process.cwd(), "src/lib/desktop/prefs.ts"), "utf8");
  const settingsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/desktop-settings.tsx"), "utf8");
  const loaderTs = readFileSync(join(process.cwd(), "src/lib/fonts/loader.ts"), "utf8");
  const fontCardTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-card.tsx"), "utf8");
  const pickerTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-picker.tsx"), "utf8");
  const playgroundTsx = readFileSync(join(process.cwd(), "src/components/font-studio/playground.tsx"), "utf8");
  const hydrateTs = readFileSync(join(process.cwd(), "src/lib/fonts/hydrate.ts"), "utf8");
  const glyphMapTsx = readFileSync(join(process.cwd(), "src/components/font-studio/glyph-map.tsx"), "utf8");
  const otFeaturesTs = readFileSync(join(process.cwd(), "src/lib/fonts/ot-features.ts"), "utf8");
  const dupEngineTs = readFileSync(join(process.cwd(), "src/lib/fonts/store/duplicate-engine.ts"), "utf8");
  const cssExportTsx = readFileSync(join(process.cwd(), "src/components/font-studio/css-export-dialog.tsx"), "utf8");
  const watchTs = readFileSync(join(process.cwd(), "src/lib/fonts/watch-folder.ts"), "utf8");

  // A1. Rust 1.100 + unused_dependencies + std::mem::DropGuard
  assert.match(cargoToml, /version\s*=\s*"2\.\d+\.\d+"/);
  assert.match(cargoToml, /rust-version\s*=\s*"1\.100"/);
  assert.match(cargoToml, /unused_dependencies\s*=\s*"warn"/);
  assert.match(modRs, /std::mem::DropGuard/);

  // A2-A4. Zero-RAM Persistent HKCU + Diff-Only Reconcile + PREUNINSTALL-only deactivate hook
  assert.match(sessionStageRs, /SESSION_PERSISTENT_ACTIVE_FILE:\s*&str\s*=\s*"\.persistent-active"/);
  assert.match(sessionStageRs, /pub fn list_fm_entries/);
  assert.match(sessionStageRs, /pub fn diff_reconcile_entries/);
  assert.match(winfontRs, /pub fn adopt_persistent_hkcu_session/);
  assert.match(sessionRs, /fn try_adopt_persistent_session/);
  assert.match(sessionRs, /pub fn deactivate_all_and_exit_headless/);
  assert.match(libRs, /--deactivate-all-and-exit/);
  assert.match(libRs, /fn exit_preserving_active_fonts/);
  assert.match(libRs, /activation_mode:\s*Option<String>/);

  const preinstall = hooksNsh.match(/!macro NSIS_HOOK_PREINSTALL[\s\S]*?!macroend/)?.[0] ?? "";
  const preuninstall = hooksNsh.match(/!macro NSIS_HOOK_PREUNINSTALL[\s\S]*?!macroend/)?.[0] ?? "";
  assert.doesNotMatch(preinstall, /--deactivate-all-and-exit/);
  assert.match(preuninstall, /--deactivate-all-and-exit/);

  assert.equal(DEFAULT_DESKTOP_PREFS.activationMode, "instant-zero-ram");
  assert.equal(coerceDesktopPrefs({}).activationMode, "instant-zero-ram");
  assert.equal(coerceDesktopPrefs({ activationMode: "daemon-tray" }).activationMode, "daemon-tray");
  assert.match(prefsTs, /instant-zero-ram/);
  assert.match(settingsTsx, /id:\s*"instant-zero-ram"/);
  assert.match(settingsTsx, /activation-mode-\$\{opt\.id\}/);

  // A5 & A8. Preview pipeline + Playground fix + Fontations toggle
  assert.match(loaderTs, /const FACE_LRU = 512/);
  assert.match(loaderTs, /read_family_font/);
  assert.match(fontCardTsx, /lastFitKeyRef/);
  assert.match(fontCardTsx, /requestAnimationFrame/);
  assert.match(pickerTsx, /pageSize\s*=\s*60/);
  assert.match(pickerTsx, /if \(!open\) return \[\]/);
  assert.match(pickerTsx, /80/);
  assert.doesNotMatch(playgroundTsx, /useEffect\(\(\)\s*=>\s*\{\s*if \(left\) void loadFont/);
  assert.match(playgroundTsx, /loadFontBounded/);
  assert.match(playgroundTsx, /playground-fontations-toggle/);

  // A6. Rust rayon background metadata pre-indexer
  assert.match(metaIndexRs, /meta-index-v1\.json/);
  assert.match(metaIndexRs, /pub async fn index_downloaded_metadata/);
  assert.match(libRs, /meta_index::index_downloaded_metadata/);
  assert.match(perms, /"index_downloaded_metadata"/);
  assert.match(hydrateTs, /indexDownloadedMetadata/);

  // A7 & A9 & A10. Glyphs virtualization + any-font inspection + ot cap 64 + Rust hash_font_paths + audit fixes
  assert.match(glyphMapTsx, /inspectable/);
  assert.match(glyphMapTsx, /glyphs-activate-btn/);
  assert.match(glyphMapTsx, /GLYPH_OVERSCAN_ROWS/);
  assert.match(otFeaturesTs, /\.slice\(0,\s*64\)/);
  assert.match(otFeaturesTs, /hlig:/);
  assert.match(parseRs, /pub async fn hash_font_paths/);
  assert.match(perms, /"hash_font_paths"/);
  assert.match(dupEngineTs, /nativeHashFontPaths/);
  assert.match(cssExportTsx, /if \(!open\) return \{ fonts: \[\], systemSkipped: 0 \}/);
  assert.match(watchTs, /filterOriginPathsByRule\(dedupedFresh,\s*root,\s*folder\.importFilter,\s*useFontStore\.getState\(\)\.localFonts\)/);
});

test("21. Fonths 2.6.0 audit hardening, universal modular font cache cleanup, diagnostics & folder toggle", () => {
  const pkgJson = JSON.parse(readFileSync(join(process.cwd(), "package.json"), "utf8"));
  const tauriConf = JSON.parse(readFileSync(join(process.cwd(), "src-tauri/tauri.conf.json"), "utf8"));
  const cargoToml = readFileSync(join(process.cwd(), "src-tauri/Cargo.toml"), "utf8");
  assert.equal(pkgJson.version, tauriConf.version);
  assert.match(pkgJson.version, /^2\.\d+\.\d+$/);
  assert.ok(cargoToml.includes(`version = "${pkgJson.version}"`));

  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const fontProtoRs = readFileSync(join(process.cwd(), "src-tauri/src/font_protocol.rs"), "utf8");
  const rendererRs = readFileSync(join(process.cwd(), "src-tauri/src/renderer.rs"), "utf8");
  const fontCacheRs = readFileSync(join(process.cwd(), "src-tauri/src/font_cache.rs"), "utf8");
  const metaIndexRs = readFileSync(join(process.cwd(), "src-tauri/src/meta_index.rs"), "utf8");
  const parseRs = readFileSync(join(process.cwd(), "src-tauri/src/parse.rs"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");

  const prefsTs = readFileSync(join(process.cwd(), "src/lib/desktop/prefs.ts"), "utf8");
  const settingsTsx = readFileSync(join(process.cwd(), "src/components/font-studio/desktop-settings.tsx"), "utf8");
  const folderTreeTsx = readFileSync(join(process.cwd(), "src/components/font-studio/folder-tree.tsx"), "utf8");
  const loaderTs = readFileSync(join(process.cwd(), "src/lib/fonts/loader.ts"), "utf8");
  const glyphMapTs = readFileSync(join(process.cwd(), "src/lib/fonts/glyph-map.ts"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const canvasTsx = readFileSync(join(process.cwd(), "src/components/font-studio/skia-font-canvas.tsx"), "utf8");
  const storeTs = readFileSync(join(process.cwd(), "src/lib/fonts/store.ts"), "utf8");
  const dupEngineTs = readFileSync(join(process.cwd(), "src/lib/fonts/store/duplicate-engine.ts"), "utf8");
  const dupFinderTsx = readFileSync(join(process.cwd(), "src/components/font-studio/duplicate-finder.tsx"), "utf8");
  const activeGuardTsx = readFileSync(join(process.cwd(), "src/components/font-studio/active-set-guard.tsx"), "utf8");
  const notifTsx = readFileSync(join(process.cwd(), "src/components/font-studio/notification-center.tsx"), "utf8");

  // 1. Unified activation mode + locked Zero-RAM HKCU sync + Tray clarity
  assert.match(libRs, /Close UI \(keep fonts active\)/);
  assert.match(libRs, /Quit & unload all fonts/);
  assert.match(prefsTs, /export type CacheCleanScope/);
  assert.equal(coerceDesktopPrefs({ activationMode: "instant-zero-ram", hkcuRegistrySync: false }).hkcuRegistrySync, true);
  assert.equal(coerceDesktopPrefs({ activationMode: "daemon-tray", hkcuRegistrySync: false }).hkcuRegistrySync, false);
  assert.match(settingsTsx, /disabled=\{zeroRamLocked\}/);

  // 2. Windows file lock + protocol cache-control + Fontations eviction
  assert.match(fontProtoRs, /open_shared_read/);
  assert.match(fontProtoRs, /"no-cache"/);
  assert.match(fontProtoRs, /pub fn cache_len/);
  assert.match(rendererRs, /open_shared_read/);
  assert.match(rendererRs, /pub fn evict_path/);
  assert.match(rendererRs, /pub fn evict_dir/);
  assert.match(rendererRs, /pub fn cache_len/);

  // 3. Universal software-agnostic modular font cache hygiene & live diagnostics
  for (const pid of ["\"adobe\"", "\"figma\"", "\"affinity\"", "\"corel\"", "\"office-suite\"", "\"fonths-stage\""]) {
    assert.ok(fontCacheRs.includes(pid), `Missing cache provider ${pid}`);
  }
  assert.match(fontCacheRs, /CreateToolhelp32Snapshot/);
  assert.match(fontCacheRs, /CacheCleanScope/);
  assert.match(libRs, /font_cache::scan_font_caches/);
  assert.match(libRs, /font_cache::clean_font_caches/);
  assert.match(libRs, /font_cache::get_diagnostics_snapshot/);
  assert.match(perms, /"scan_font_caches"/);
  assert.match(perms, /"clean_font_caches"/);
  assert.match(perms, /"get_diagnostics_snapshot"/);
  assert.match(settingsTsx, /data-testid="diagnostics-health-card"/);
  assert.match(settingsTsx, /data-testid="font-cache-hygiene-card"/);
  assert.match(settingsTsx, /data-testid=\{`cache-scope-\$\{opt\.id\}`\}/);

  // 4. Memory leaks, SuperSearch, Duplicates & Watched Folder toggle
  assert.match(loaderTs, /const LOCAL_FACE_LRU = 512/);
  assert.doesNotMatch(glyphMapTs, /fm-glyphs-/);
  assert.match(inspectorTsx, /applyBulkMetadataIndex/);
  assert.doesNotMatch(canvasTsx, /new Uint8ClampedArray\(buffer\)/);
  assert.match(storeTs, /prev\.metrics && !font\.metrics/);
  assert.match(metaIndexRs, /extra_paths:\s*Option<Vec<\(String,\s*String\)>>/);
  assert.match(parseRs, /pub async fn diff_font_paths/);
  assert.match(perms, /"diff_font_paths"/);
  assert.match(dupEngineTs, /diffFontPaths/);
  assert.match(dupFinderTsx, /patchLocalChecksums/);
  assert.match(dupFinderTsx, /st\.activatedSet\?\.has\(id\) \|\| st\.pendingSet\?\.has\(id\)/);
  assert.doesNotMatch(folderTreeTsx, /data-testid=\{`folder-toggle-\$\{folder\.id\}`\}/);
  assert.match(folderTreeTsx, /ActivateMenuItem/);
  assert.match(folderTreeTsx, /DeactivateMenuItem/);
  assert.match(activeGuardTsx, /GUARD_DISMISSED_KEY/);
  assert.match(notifTsx, /30_000/);
});

test("v2.7.0 thin-preview defaults, multi-weight duplicates, and UI polish contracts", async () => {
  const { defaultWeightForFont, defaultAxisValues } = await import("../src/lib/fonts/axes.ts");
  const { familyDuplicateHideIds, pickKeepFont } = await import(
    "../src/lib/fonts/store/duplicate-pure.ts"
  );
  const readLocal = (rel) => readFileSync(join(process.cwd(), rel), "utf8");
  const pkgJson = JSON.parse(readLocal("package.json"));
  const tauriConf = JSON.parse(readLocal("src-tauri/tauri.conf.json"));
  const cargoToml = readLocal("src-tauri/Cargo.toml");
  assert.equal(pkgJson.version, "2.7.0");
  assert.equal(tauriConf.version, "2.7.0");
  assert.match(cargoToml, /version\s*=\s*"2\.7\.0"/);

  // Thin-origin VF (fvar def=100, span 100..900) must default preview to 400 (Regular).
  const thinVf = {
    weights: [100],
    variable: true,
    italic: false,
    axes: [{ tag: "wght", name: "Weight", min: 100, max: 900, def: 100 }],
    instances: undefined,
  };
  assert.equal(defaultWeightForFont(thinVf), 400);
  assert.equal(defaultAxisValues(thinVf.axes).wght, 400);

  // Static multi-weight missing 400 picks weight closest to 400 (300 beats 100).
  assert.equal(
    defaultWeightForFont({ weights: [100, 300, 700], variable: false, italic: false }),
    300,
  );

  // Multi-weight local families must NEVER have Bold/Italic cuts hidden by familyDuplicateHideIds.
  const localRegular = {
    id: "l:reg",
    family: "MyBrand",
    source: "local",
    weights: [400],
    italic: false,
    checksum: "aaa",
    fileSize: 50000,
  };
  const localBold = {
    id: "l:bold",
    family: "MyBrand",
    source: "local",
    weights: [700],
    italic: false,
    checksum: "bbb",
    fileSize: 52000,
  };
  const localItalic = {
    id: "l:ital",
    family: "MyBrand",
    source: "local",
    weights: [400],
    italic: true,
    checksum: "ccc",
    fileSize: 51000,
  };
  const localDupReg = {
    id: "l:reg-dup",
    family: "MyBrand",
    source: "local",
    weights: [400],
    italic: false,
    checksum: "ddd",
    fileSize: 48000,
  };
  const hidden = familyDuplicateHideIds([localRegular, localBold, localItalic, localDupReg], [], []);
  assert.deepEqual(hidden, ["l:reg-dup"]);

  assert.equal(
    pickKeepFont({ key: "k", reason: "family-weight", fonts: [localDupReg, localRegular] })?.id,
    "l:reg",
  );

  // Contract checks for v2.7.0 UI polish & Duplicates features.
  const dupFinderTsx = readLocal("src/components/font-studio/duplicate-finder.tsx");
  const cardTsx = readLocal("src/components/font-studio/font-card.tsx");
  const toolbarTsx = readLocal("src/components/font-studio/preview-toolbar.tsx");
  const gridTsx = readLocal("src/components/font-studio/library-grid.tsx");
  const settingsTsx = readLocal("src/components/font-studio/desktop-settings.tsx");
  const storeTs = readTsModule(process.cwd(), "src/lib/fonts/store.ts");
  const watchTs = readLocal("src/lib/fonts/watch-folder.ts");

  assert.match(dupFinderTsx, /data-testid="bulk-clean-duplicates-dialog"/);
  assert.match(dupFinderTsx, /swapActivateFont/);
  assert.match(dupFinderTsx, /ignoreWatchedFont/);
  assert.match(dupFinderTsx, /scanKey/);
  assert.match(storeTs, /ignoredWatchedPaths/);
  assert.match(watchTs, /ignoredWatchedPaths/);
  assert.doesNotMatch(cardTsx, /window\.confirm/);
  assert.match(toolbarTsx, /More surfaces/);
  assert.match(gridTsx, /data-testid="manage-uploads-btn"/);
  assert.match(settingsTsx, /Zero-RAM Instant mode, active HKCU fonts are already live at login/);
});
