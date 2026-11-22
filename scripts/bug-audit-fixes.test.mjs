import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const gfsourceRs = readFileSync(join(root, "src-tauri/src/gfsource.rs"), "utf8");
const activateRs = readFileSync(join(root, "src-tauri/src/activate.rs"), "utf8");
const osActivateTs = readFileSync(join(root, "src/lib/fonts/os-activate.ts"), "utf8");
const loaderTs = readFileSync(join(root, "src/lib/fonts/loader.ts"), "utf8");
const storeTs = readFileSync(join(root, "src/lib/fonts/store.ts"), "utf8");
const exportCollectionTs = readFileSync(join(root, "src/lib/fonts/export-collection.ts"), "utf8");
const desktopSettingsTsx = readFileSync(
  join(root, "src/components/font-studio/desktop-settings.tsx"),
  "utf8",
);

test("A1: 1.0.206u VF-Primary is default unless Static-first opted in natively", () => {
  assert.match(gfsourceRs, /pub const PREFER_STATICS_MARKER: &str = "prefer-statics"/);
  assert.match(activateRs, /pub fn set_prefer_static_faces\(prefer_statics: bool\)/);
  assert.doesNotMatch(desktopSettingsTsx, /Prefer static faces/);
  assert.doesNotMatch(desktopSettingsTsx, /fast-session-resume-toggle/);
});

test("A2: manifest fetch and repo METADATA.pb probe use 20s probe timeout + negative cache + circuit breaker", () => {
  assert.match(gfsourceRs, /pub const PROBE_TIMEOUT: Duration = Duration::from_secs\(20\)/);
  assert.match(gfsourceRs, /manifest_missed_this_session\(family\) \|\| !manifest_circuit_allow\(\)/);
  assert.match(gfsourceRs, /timeout: Some\(PROBE_TIMEOUT\)/);
  assert.match(activateRs, /gfsource::reset_manifest_circuit\(\)/);
  const repoFn = activateRs.slice(
    activateRs.indexOf("fn google_fonts_repo_statics_pref("),
    activateRs.indexOf("fn replace_tiny_cjk_static_faces("),
  );
  assert.match(repoFn, /timeout: Some\(CATALOG_PROBE_TIMEOUT\)/);
  assert.doesNotMatch(repoFn, /gfsource::polite_get\(/);
});

test("A2: os-activate.ts accumulates readyCumulative without dropping earlier families on final 64-item tail", () => {
  assert.match(osActivateTs, /const merged = absorbReadyNames\(readyCumulative, p\.ready_names \?\? \[\]\);\s*readyCumulative = merged;/);
  assert.doesNotMatch(osActivateTs, /readyCumulative = p\.ready_names;/);
  assert.match(osActivateTs, /readyCumulative = absorbReadyNames\(readyCumulative, p\.ready_names\);/);
});

test("A2: Noto Color Emoji and Noto Emoji URLs point to live upstream paths in os-activate.ts and loader.ts", () => {
  assert.match(osActivateTs, /2D\/fonts\/NotoColorEmoji\.ttf/);
  assert.match(osActivateTs, /ofl\/notoemoji\/NotoEmoji%5Bwght%5D\.ttf/);
  assert.match(loaderTs, /2D\/fonts\/NotoColorEmoji\.ttf/);
  assert.match(loaderTs, /ofl\/notoemoji\/NotoEmoji%5Bwght%5D\.ttf/);
});

test("A3: Collection ZIP export packs all font files in a family folder via list_family_font_files", () => {
  assert.match(activateRs, /pub async fn list_family_font_files\(/);
  assert.match(exportCollectionTs, /list_family_font_files/);
});

test("A3: removeLocalFont keeps sibling uploaded faces in multi-file families and aborts store delete on disk lock", () => {
  assert.match(osActivateTs, /keepSiblingFaces && font\.fileName/);
  assert.match(osActivateTs, /remove_library_file/);
  const rmBlock = storeTs.slice(
    storeTs.indexOf("removeLocalFont: async (id) =>"),
    storeTs.indexOf("clearLocalFonts: async () =>"),
  );
  assert.match(rmBlock, /hasSiblingFaces/);
  assert.match(rmBlock, /const ok = await removeUploadFromDisk\(font, hasSiblingFaces\);\s*if \(!ok\) return;/);
});
