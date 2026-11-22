import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { filterFontsWithIndex, getCachedSearchIndex } from "../src/lib/fonts/search-index.ts";
import { computeVirtualWindow, MAX_VIRTUAL_SCROLL_HEIGHT } from "../src/lib/fonts/virtual-window.ts";
import { clampMenuPosition, shouldAllowNativeContextMenu } from "../src/lib/context-menu-bus.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function makeSyntheticLibrary(count) {
  const cats = ["sans-serif", "serif", "display", "handwriting", "monospace"];
  const fonts = new Array(count);
  for (let i = 0; i < count; i += 1) {
    fonts[i] = {
      id: `l:syn-${i}`,
      family: i === 77777 ? "Agrave Pro Needle" : `SynthFont ${String(i).padStart(6, "0")}`,
      source: "local",
      category: cats[i % cats.length],
      weights: [400],
      italic: false,
      variable: i % 10 === 0,
      tags: i % 7 === 0 ? ["editorial"] : [],
      popularity: i + 1,
      fileName: `synth-${i}.ttf`,
      fileSize: 40_000 + (i % 997),
      checksum: i === 100 || i === 50_100 ? "deadbeef-dup-hash-01" : `hash-${i}`,
    };
  }
  return fonts;
}

test("100k fonts: trigram search index builds & queries cleanly", () => {
  const fonts = makeSyntheticLibrary(100_000);
  const customTags = {};
  const t0 = performance.now();
  const idx = getCachedSearchIndex(fonts, customTags);
  const buildMs = performance.now() - t0;
  assert.equal(idx.entries.length, 100_000);

  const q0 = performance.now();
  const hits = filterFontsWithIndex(fonts, "needle", customTags);
  const queryMs = performance.now() - q0;
  assert.equal(hits.length, 1);
  assert.equal(hits[0].family, "Agrave Pro Needle");
  assert.ok(buildMs < 15_000, `100k index build took too long: ${buildMs.toFixed(1)}ms`);
  assert.ok(queryMs < 100, `100k cached query took too long: ${queryMs.toFixed(1)}ms`);
});

test("100k fonts: computeVirtualWindow scales safely below Chromium 16.7M px limit and reaches item 100,000", () => {
  const topWin = computeVirtualWindow({
    totalItems: 100_000,
    cols: 1,
    rowH: 200,
    gap: 12,
    scrollTop: 0,
    viewportH: 800,
  });
  assert.equal(topWin.scaled, true);
  assert.equal(topWin.totalH, MAX_VIRTUAL_SCROLL_HEIGHT);
  assert.ok(topWin.rawTotalH > 16_777_216);
  assert.equal(topWin.start, 0);
  assert.ok(topWin.end > 0 && topWin.end < 20);

  const bottomWin = computeVirtualWindow({
    totalItems: 100_000,
    cols: 1,
    rowH: 200,
    gap: 12,
    scrollTop: MAX_VIRTUAL_SCROLL_HEIGHT - 800,
    viewportH: 800,
  });
  assert.equal(bottomWin.scaled, true);
  assert.equal(bottomWin.end, 100_000, "scaled scroll reaches the 100,000th font");
  assert.ok(bottomWin.offsetY >= 0 && bottomWin.offsetY <= MAX_VIRTUAL_SCROLL_HEIGHT);

  const normalWin = computeVirtualWindow({
    totalItems: 2000,
    cols: 4,
    rowH: 200,
    gap: 12,
    scrollTop: 424,
    viewportH: 800,
  });
  assert.equal(normalWin.scaled, false);
  assert.equal(normalWin.totalH, normalWin.rawTotalH);
});

test("100k fonts & Duplicates: store uses O(N) family indexing, same-style local dedup, and fast >5k sort", () => {
  const storeSrc = readFileSync(join(root, "src/lib/fonts/store.ts"), "utf8");
  assert.match(storeSrc, /const localByFamily = new Map<string, FontRecord\[\]>\(\)/);
  assert.ok(
    !storeSrc.includes("const locals = localFonts.filter((f) => f.family.trim().toLowerCase() === name)"),
    "O(N^2) localFonts.filter inside names loop must stay eliminated",
  );
  assert.match(storeSrc, /local-fam:\$\{name\}:\$\{styleKey\}/);
  assert.match(storeSrc, /if \(fonts\.length > 5000\)/);
  assert.match(storeSrc, /MAX_BUCKET_PAIRWISE = 32/);
});

test("Notification Center uses native scrollable container + copy + issue filter", () => {
  const src = readFileSync(
    join(root, "src/components/font-studio/notification-center.tsx"),
    "utf8",
  );
  assert.ok(!src.includes("<ScrollArea"), "must not use height-unconstrained Radix ScrollArea");
  assert.match(src, /data-testid="notifications-list"/);
  assert.match(src, /overflow-y-auto overscroll-contain/);
  assert.match(src, /aria-label="Copy notification"/);
});

test("Right-click context menu: clamping, native input preservation, and wiring", () => {
  const clamped = clampMenuPosition(1250, 780, 220, 260, 1280, 800, 8);
  assert.equal(clamped.left, 1280 - 220 - 8);
  assert.equal(clamped.top, 800 - 260 - 8);

  assert.equal(shouldAllowNativeContextMenu({ tagName: "INPUT" }), true);
  assert.equal(shouldAllowNativeContextMenu({ tagName: "TEXTAREA" }), true);
  assert.equal(shouldAllowNativeContextMenu({ tagName: "DIV", isContentEditable: true }), true);
  assert.equal(shouldAllowNativeContextMenu({ tagName: "DIV" }, "selected font name"), true);
  assert.equal(shouldAllowNativeContextMenu({ tagName: "DIV" }, ""), false);

  const menuSrc = readFileSync(join(root, "src/components/font-studio/context-menu.tsx"), "utf8");
  const cardSrc = readFileSync(join(root, "src/components/font-studio/font-card.tsx"), "utf8");
  const shellSrc = readFileSync(join(root, "src/components/font-studio/app-shell.tsx"), "utf8");
  const dupSrc = readFileSync(join(root, "src/components/font-studio/duplicate-finder.tsx"), "utf8");

  assert.match(menuSrc, /data-testid="app-context-menu"/);
  assert.match(cardSrc, /onContextMenu=\{handleCardContextMenu\}/);
  assert.match(shellSrc, /<AppContextMenu \/>/);
  assert.match(shellSrc, /window\.addEventListener\("contextmenu", onGlobalContextMenu\)/);
  assert.match(dupSrc, /loadSystemFonts\(\)/);
  assert.match(dupSrc, /onContextMenu=\{handleContextMenu\}/);
});

test("Fonths 1.0.0 branding and Adobe quit-leftover cleanup contracts", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const tauri = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));
  const versionTs = readFileSync(join(root, "src/version.ts"), "utf8");
  const activateRs = readFileSync(join(root, "src-tauri/src/activate.rs"), "utf8");
  assert.equal(pkg.name, "fonths");
  assert.equal(pkg.version, "1.0.0");
  assert.equal(tauri.productName, "Fonths");
  assert.equal(tauri.version, "1.0.0");
  assert.equal(tauri.identifier, "app.fonths.desktop");
  assert.match(versionTs, /APP_NAME = "Fonths"/);
  assert.match(versionTs, /APP_VERSION = "1\.0\.0"/);
  assert.match(activateRs, /winfont::unload_paths\(extra\.clone\(\), true\)/);
});
