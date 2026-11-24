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
  const storeTs = readFileSync(join(process.cwd(), "src/lib/fonts/store.ts"), "utf8");
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
  const activateRs = readFileSync(join(process.cwd(), "src-tauri/src/activate.rs"), "utf8");
  const mainRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const perms = readFileSync(join(process.cwd(), "src-tauri/permissions/font-activate.toml"), "utf8");
  const googleApi = readFileSync(join(process.cwd(), "src/lib/fonts/google-api.ts"), "utf8");
  const storeTs = readFileSync(join(process.cwd(), "src/lib/fonts/store.ts"), "utf8");
  const osActivate = readFileSync(join(process.cwd(), "src/lib/fonts/os-activate.ts"), "utf8");
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

  assert.match(cargoToml, /version = "2\.1\.0"/);
  assert.match(cargoToml, /skia-safe.*textlayout.*embed-freetype.*freetype-woff2/);
  assert.match(cargoToml, /read-fonts/);
  assert.match(cargoToml, /lru/);
  assert.match(cargoToml, /memmap2/);
  assert.doesNotMatch(cargoToml, /ttf-parser/);
  assert.doesNotMatch(cargoToml, /Win32_Graphics_DirectWrite/);

  assert.match(libRs, /pub mod scanner;/);
  assert.match(libRs, /pub mod renderer;/);
  assert.match(libRs, /pub mod activator;/);
  assert.match(libRs, /scanner::scan_font_directory/);
  assert.match(libRs, /renderer::render_font_preview/);
  assert.match(libRs, /activator::activate_font/);
  assert.match(libRs, /activator::deactivate_font/);
  assert.match(libRs, /activator::cleanup_on_exit/);

  assert.match(scannerRs, /pub fn scan_font_directory/);
  assert.match(scannerRs, /Mmap::map/);
  assert.match(scannerRs, /font\.gsub\(\)/);
  assert.match(scannerRs, /font\.gpos\(\)/);

  assert.match(rendererRs, /FontMgr::custom_empty\(\)\.unwrap_or_else\(FontMgr::new\)/);
  assert.match(rendererRs, /LruCache<String, Typeface>/);
  assert.match(rendererRs, /VariationPosition/);
  assert.match(rendererRs, /add_font_feature/);
  assert.match(rendererRs, /surfaces::raster_n32_premul/);
  assert.match(rendererRs, /tauri::ipc::Response/);

  assert.match(activatorRs, /AddFontResourceExW/);
  assert.match(activatorRs, /RemoveFontResourceExW/);
  assert.match(activatorRs, /FONT_RESOURCE_CHARACTERISTICS\(0\)/);
  assert.match(activatorRs, /WM_FONTCHANGE/);
  assert.match(activatorRs, /#\[cfg\(not\(windows\)\)\]/);

  for (const cmd of ["scan_font_directory", "render_font_preview", "activate_font", "deactivate_font"]) {
    assert.match(perms, new RegExp(`"${cmd}"`));
  }

  assert.match(canvasTsx, /new ImageData\(new Uint8ClampedArray\(buffer\)/);
  assert.match(canvasTsx, /ctx\.putImageData/);
  assert.match(inspectorTsx, /<SkiaFontCanvas/);
  assert.doesNotMatch(appShellTsx, /Skia Studio/);
});

test("14. Live Skia Inspector (italic/weight/auto-height/scroll), watched dedupe+collision, and lock combat/tray prefs", () => {
  const rendererRs = readFileSync(join(process.cwd(), "src-tauri/src/renderer.rs"), "utf8");
  const activateRs = readFileSync(join(process.cwd(), "src-tauri/src/activate.rs"), "utf8");
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
  assert.match(settingsTsx, /data-testid="run-in-background-switch"/);
  assert.match(settingsTsx, /data-testid="start-minimized-switch"/);
});

test("15. Fonths 2.1.0 headless daemon, state honesty, live sliders, and SuperSearch fixes", () => {
  const daemonRs = readFileSync(join(process.cwd(), "src-tauri/src/daemon.rs"), "utf8");
  const libRs = readFileSync(join(process.cwd(), "src-tauri/src/lib.rs"), "utf8");
  const sliderTsx = readFileSync(join(process.cwd(), "src/components/ui/slider.tsx"), "utf8");
  const inspectorTsx = readFileSync(join(process.cwd(), "src/components/font-studio/font-inspector.tsx"), "utf8");
  const osActivateTs = readFileSync(join(process.cwd(), "src/lib/fonts/os-activate.ts"), "utf8");
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
