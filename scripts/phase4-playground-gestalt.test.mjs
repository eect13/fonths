import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("Phase 4: Playground wires live size, line-height, tracking, color pickers, zoom, OpenType toggles, and System Font lookup", () => {
  const src = readFileSync(join(root, "src/components/font-studio/playground.tsx"), "utf8");
  assert.match(src, /const systemFonts = useFontStore\(\(s\) => s\.systemFonts\)/);
  assert.match(src, /loadSystemFonts\(\)/);
  assert.match(src, /data-testid="playground-fg-color"/);
  assert.match(src, /data-testid="playground-bg-color"/);
  assert.match(src, /aria-label="Viewport zoom"/);
  assert.match(src, /data-testid=\{`playground-ot-\$\{feat\.tag\}`\}/);
  assert.match(src, /fontFeatureSettings: featureSettings/);
  assert.match(src, /function AutoTextarea/);
  assert.match(src, /\[overflow-wrap:anywhere\]/);
});

test("Phase 4: Gestalt debloat removed FontSetsPanel, duplicate sidebar Variable/Italic, and wired Delete Mode + Temp Cache cleanup", () => {
  const sidebar = readFileSync(join(root, "src/components/font-studio/sidebar.tsx"), "utf8");
  const settings = readFileSync(join(root, "src/components/font-studio/desktop-settings.tsx"), "utf8");
  const inspector = readFileSync(join(root, "src/components/font-studio/font-inspector.tsx"), "utf8");

  assert.equal(existsSync(join(root, "src/components/font-studio/font-sets-panel.tsx")), false);
  assert.doesNotMatch(sidebar, /FontSetsPanel/);
  assert.doesNotMatch(sidebar, /label="Variable"/);
  assert.doesNotMatch(sidebar, /label="Italic"/);
  assert.match(sidebar, /aria-expanded=\{licenseOpen/);

  assert.match(settings, /data-testid="delete-mode-card"/);
  assert.match(settings, /data-testid="clean-temp-cache-btn"/);
  assert.match(settings, /data-testid="sync-google-cdn-btn"/);

  assert.match(inspector, /Permanently Delete/);
  assert.match(inspector, /evictFamilyPreview\(font\.family\)/);
});

test("Notification Center, Playground VF axis loading, and live Google sidebar count are wired", () => {
  const shell = readFileSync(join(root, "src/components/font-studio/app-shell.tsx"), "utf8");
  const themeRoot = readFileSync(join(root, "src/components/font-studio/theme-root.tsx"), "utf8");
  const loader = readFileSync(join(root, "src/lib/fonts/loader.ts"), "utf8");
  const playground = readFileSync(join(root, "src/components/font-studio/playground.tsx"), "utf8");
  const sidebar = readFileSync(join(root, "src/components/font-studio/sidebar.tsx"), "utf8");

  assert.match(shell, /<NotificationCenter \/>/);
  assert.match(themeRoot, /installNotificationRecorder\(\)/);
  assert.match(loader, /function fullVfFamilyParam/);
  assert.match(loader, /loadGoogleFromLocal\(font, "full"\)/);
  assert.match(playground, /left\?\.catalogVariable \|\| leftAxesList\.length > 0/);
  assert.match(sidebar, /googleFonts\.length \|\| GOOGLE_DIRECTORY\.size/);
});
