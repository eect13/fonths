import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(root, rel), "utf8");

const pkg = JSON.parse(read("package.json"));
const pkgLock = JSON.parse(read("package-lock.json"));
const tauriConf = JSON.parse(read("src-tauri/tauri.conf.json"));
const cargoToml = read("src-tauri/Cargo.toml");
const versionTs = read("src/version.ts");
const mainRs = read("src-tauri/src/lib.rs");
const scannerRs = read("src-tauri/src/scanner.rs");
const rendererRs = read("src-tauri/src/renderer.rs");
const parseRs = read("src-tauri/src/parse.rs");
const stageRs = read("src-tauri/src/session_stage.rs");
const activateRs = read("src-tauri/src/activate.rs");
const aclToml = read("src-tauri/permissions/font-activate.toml");
const nativeParseTs = read("src/lib/fonts/native-parse.ts");
const loaderTs = read("src/lib/fonts/loader.ts");
const desktopSettingsTsx = read("src/components/font-studio/desktop-settings.tsx");

test("2.0.0 version metadata synced across Cargo, Tauri, npm, and version.ts", () => {
  assert.equal(pkg.version, "2.2.2");
  assert.equal(pkgLock.version, "2.2.2");
  assert.equal(pkgLock.packages[""].version, "2.2.2");
  assert.equal(tauriConf.version, "2.2.2");
  assert.match(cargoToml, /version = "2\.2\.2"/);
  assert.match(versionTs, /export const APP_VERSION = "2\.2\.2";/);
});

test("Pure-Rust Fontations + HarfRust + font:// engine replaces legacy DirectWrite, ttf-parser, and skia-safe", () => {
  assert.doesNotMatch(cargoToml, /Win32_Graphics_DirectWrite/);
  assert.doesNotMatch(cargoToml, /ttf-parser/);
  assert.doesNotMatch(cargoToml, /skia-safe/);
  assert.equal(existsSync(join(root, "src-tauri/src/dwrite.rs")), false, "legacy dwrite.rs must be removed");
  assert.match(cargoToml, /read-fonts/);
  assert.match(cargoToml, /skrifa/);
  assert.match(cargoToml, /harfrust/);
  assert.match(cargoToml, /tiny-skia/);
  assert.match(cargoToml, /rayon/);
  assert.match(cargoToml, /memmap2/);
  assert.match(cargoToml, /Win32_Graphics_Gdi/);
  assert.match(cargoToml, /Win32_UI_WindowsAndMessaging/);
  assert.match(cargoToml, /Win32_Foundation/);
  assert.match(scannerRs, /pub fn analyze_font_file/);
  assert.match(scannerRs, /pub fn is_system_windows_fonts_dir/);
  assert.match(scannerRs, /\\gdi-maps\\/);
  assert.match(scannerRs, /\\documents\\font manager\\/);
  assert.match(rendererRs, /harfrust::shape/);
  assert.match(rendererRs, /ScalerFont/);
  assert.match(parseRs, /crate::scanner::is_system_windows_fonts_dir/);
  assert.match(parseRs, /fn layout_cache\(/);
  assert.match(parseRs, /fn index_cache\(/);
});

test("all 5 new 2.0.0 commands are async spawn_blocking, registered, ACL-allowed, and wrapped", () => {
  const newCmds = [
    ["dwrite_engine_info", scannerRs, "getDWriteEngineInfo"],
    ["dwrite_inspect_font", scannerRs, "inspectFontDWrite"],
    ["read_family_preview_info", parseRs, "nativeFamilyPreviewInfo"],
    ["list_leftover_stage_files", activateRs, "listLeftoverStageFiles"],
    ["recycle_leftover_stage_files", activateRs, "recycleLeftoverStageFiles"],
  ];
  for (const [cmd, rustSrc, tsFn] of newCmds) {
    assert.match(
      rustSrc,
      new RegExp(`#\\[tauri::command\\]\\s*pub async fn ${cmd}\\s*\\([\\s\\S]{0,400}spawn_blocking`),
      `${cmd} must be async + spawn_blocking`,
    );
    assert.match(mainRs, new RegExp(`::${cmd}\\b`), `${cmd} registered in main.rs`);
    assert.match(aclToml, new RegExp(`"${cmd}"`), `${cmd} allowed in font-activate.toml`);
    assert.match(nativeParseTs, new RegExp(`export async function ${tsFn}\\b`), `${tsFn} exported`);
  }
  assert.match(loaderTs, /nativeFamilyPreviewInfo\(font\.family\)/);
});

test("session performance & Card 292 fixes (batched HKCU, clean-exit boot, B1/B2/B3/B6/B9/B11)", () => {
  // Batched HKCU sync + clean-exit fast boot
  assert.match(stageRs, /pub fn sync_entries_batch/);
  assert.match(stageRs, /pub fn write_clean_exit_marker_in/);
  assert.match(stageRs, /pub fn take_clean_exit_marker_in/);
  assert.match(activateRs, /struct HkcuBatchGuard/);

  // B11 fast-path HKCU re-assert
  assert.match(activateRs, /if dest_ok \{[\s\S]{0,220}sync_hkcu_session_entry\(&dest\)/);

  // B1 explicit Not found surfacing + unit test
  assert.match(activateRs, /Not found \(no file on disk and no catalog source\)/);
  assert.match(activateRs, /fn b1_missing_family_reported_not_silently_skipped/);

  // B2/B3 safe VF static purge + Recycle Bin helper
  assert.match(activateRs, /fn vf_primary_purge_safe_for_dir/);
  assert.match(activateRs, /fn recycle_static_or_test_delete/);
  assert.match(activateRs, /fn b2_b3_vf_purge_skips_local_and_mismatched_win32_family/);

  // B6 safe gdi-maps leftover detection & permanent temp cache cleanup UI
  assert.match(activateRs, /fn find_leftover_stage_files_in/);
  assert.match(activateRs, /fn recycle_stage_files_in/);
  assert.match(activateRs, /fn b6_leftover_stage_files_excludes_library_and_active_maps/);
  assert.match(desktopSettingsTsx, /data-testid="dwrite-engine-card"/);
  assert.match(desktopSettingsTsx, /data-testid="delete-mode-card"/);
  assert.match(desktopSettingsTsx, /data-testid="clean-temp-cache-btn"/);
  assert.match(desktopSettingsTsx, /data-testid="sync-google-cdn-btn"/);

  // B9 removal of false Always-Admin text while preserving kept session stage copy
  assert.doesNotMatch(desktopSettingsTsx, /Always-Admin/);
  assert.match(desktopSettingsTsx, /Hardlinked session stage stays ready/);
});

test("root remnant files remain removed in 2.0.0 copy", () => {
  for (const remnant of ["AGENTS.md", "AGENTS.project.md", "REVIEW-STATUS.md", "startup.sh"]) {
    assert.equal(existsSync(join(root, remnant)), false, `${remnant} must not exist`);
  }
});
