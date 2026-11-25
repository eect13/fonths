import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { coerceDesktopPrefs, DEFAULT_DESKTOP_PREFS } from "../src/lib/desktop/prefs.ts";
import { readRustModule, readTsModule } from "./lib/read-source.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");

test("Fast GDI session restore is automatic on fresh install and respects explicit opt-out", () => {
  assert.equal(DEFAULT_DESKTOP_PREFS.fastSessionResume, true);
  assert.equal(coerceDesktopPrefs(undefined).fastSessionResume, true);
  assert.equal(coerceDesktopPrefs({}).fastSessionResume, true);
  assert.equal(coerceDesktopPrefs({ fastSessionResume: false }).fastSessionResume, false);
  assert.equal(coerceDesktopPrefs({ fastSessionResume: true }).fastSessionResume, true);

  const gfsource = fs.readFileSync(path.join(root, "src-tauri/src/gfsource.rs"), "utf8");
  assert.match(gfsource, /pub const NO_FAST_RESUME_MARKER: &str = "no-fast-resume";/);
  assert.match(gfsource, /NO_FAST_RESUME_MARKER\)\.exists\(\)/);

  const mainRs = fs.readFileSync(path.join(root, "src-tauri/src/lib.rs"), "utf8");
  assert.match(mainRs, /set_fast_session_resume\(fast_session_resume\.unwrap_or\(true\)\)/);
});

test("Session GDI restore deduplicates families, clamps done <= total <= library count, and never invents phantom IDs", () => {
  const activateRs = readRustModule(root, "src-tauri/src/activate.rs");
  const hydrateTs = fs.readFileSync(path.join(root, "src/lib/fonts/hydrate.ts"), "utf8");
  const osActivateTs = readTsModule(root, "src/lib/fonts/os-activate.ts");
  const downloadBarTsx = fs.readFileSync(path.join(root, "src/components/font-studio/download-bar.tsx"), "utf8");
  const storeTs = readTsModule(root, "src/lib/fonts/store.ts");

  assert.match(activateRs, /pub fn dedupe_family_names\(/);
  assert.match(activateRs, /p\.done = p\.done\.max\(step_done\)\.min\(want_total\)/);
  assert.match(activateRs, /p\.done = \(p\.ready_names\.len\(\) as u32\)\.min\(cap\)/);
  assert.match(activateRs, /pub fn gdi_session_state\(\) -> GdiSessionState/);
  assert.match(activateRs, /fn prune_path_from_by_family\(/);

  // hydrate.ts must never fabricate phantom `g:<name>` IDs outside the library
  assert.doesNotMatch(hydrateTs, /consider\(byFamily\.get\(key\) \?\? `g:\$\{name\.trim\(\)\}`\)/);
  assert.match(hydrateTs, /!isKnownGdiSessionIncapable\(n\)/);

  // DownloadBar + applyPayload clamp restoring progress to library font count
  assert.match(downloadBarTsx, /isRestoreCurrent && libraryFontCount > 0 \? Math\.min\(rawTotal, libraryFontCount\) : rawTotal/);
  assert.match(osActivateTs, /export async function getGdiSessionState\(/);
  assert.match(storeTs, /const validId = \(id: string\) => !hasCatalog \|\| Boolean\(findFont\(id, local, google\)\)/);

  const permToml = fs.readFileSync(path.join(root, "src-tauri/permissions/font-activate.toml"), "utf8");
  assert.match(permToml, /"gdi_session_state"/);
});
