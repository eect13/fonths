import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { fontMatchesSearch, parseSearchQuery } from "../src/lib/fonts/metrics.ts";
import { filterFontsWithIndex } from "../src/lib/fonts/search-index.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("Phase 2: Windows app.manifest requests requireAdministrator, PerMonitorV2, and longPathAware", () => {
  const manifest = readFileSync(join(root, "src-tauri/app.manifest"), "utf8");
  assert.match(manifest, /requestedExecutionLevel\s+level="requireAdministrator"\s+uiAccess="false"/);
  assert.match(manifest, /<dpiAwareness[^>]*>PerMonitorV2<\/dpiAwareness>/);
  assert.match(manifest, /<longPathAware[^>]*>true<\/longPathAware>/);

  const buildRs = readFileSync(join(root, "src-tauri/build.rs"), "utf8");
  assert.match(buildRs, /app_manifest\(include_str!\("app\.manifest"\)\)/);
  assert.match(buildRs, /PROFILE.*==\s*Ok\("release"\)/);
});

test("Phase 2: search-index matches fontMatchesSearch identically including mid-word substrings", () => {
  const fonts = [
    { id: "g:Roboto", family: "Roboto", fullName: "Roboto Regular", source: "google", category: "sans", weights: [400, 700], italic: true, variable: true, popularity: 1, tags: ["neo-grotesque"], axes: [{ tag: "wght", min: 100, max: 900, default: 400 }] },
    { id: "g:Inter", family: "Inter", fullName: "Inter Display", source: "google", category: "sans", weights: [400, 700], italic: false, variable: true, popularity: 2, tags: ["geometric"], axes: [{ tag: "opsz", min: 14, max: 32, default: 14 }, { tag: "wght", min: 100, max: 900, default: 400 }] },
    { id: "g:JetBrains Mono", family: "JetBrains Mono", fullName: "JetBrains Mono", source: "google", category: "mono", weights: [400, 700], italic: true, variable: true, popularity: 3, tags: ["coding"], axes: [{ tag: "wght", min: 100, max: 800, default: 400 }] },
    { id: "l-1", family: "Playfair Custom", fullName: "Playfair Custom Italic", source: "local", category: "serif", weights: [400], italic: true, variable: false, popularity: 99, tags: ["editorial"] },
  ];
  const customTags = { "l-1": ["brand-hero"] };

  const queries = [
    "rob",
    "boto", // mid-word substring!
    "inter",
    "category:mono",
    "source:local",
    "tag:brand-hero",
    "axis:opsz",
    "sans -inter",
    "\"jetbrains mono\"",
    "re",
  ];
  for (const q of queries) {
    const clauses = parseSearchQuery(q);
    const expected = fonts.filter((f) => fontMatchesSearch(f, clauses, customTags)).map((f) => f.id);
    const actual = filterFontsWithIndex(fonts, q, customTags).map((f) => f.id);
    assert.deepEqual(actual, expected, `query "${q}" parity mismatch`);
  }
});

test("Phase 2: loader.ts O(1) Map LRU revokes blob URLs on eviction", () => {
  const src = readFileSync(join(root, "src/lib/fonts/loader.ts"), "utf8");
  assert.match(src, /const familyBlobUrls = new Map<string, string\[\]>\(\)/);
  assert.match(src, /URL\.revokeObjectURL\(url\)/);
  assert.match(src, /export function evictFamilyPreview/);
  assert.doesNotMatch(src, /faceOrder\.indexOf/);
});
