import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// C253 #2: Tauri runs non-async commands on the main (UI) thread. Scans, downloads,
// Repair and Retry blocked the window for seconds (IsHungAppWindow, Alt-Tab ghost).
// Every heavy command must be `async` and run its work on the blocking pool.
const read = (p) => readFileSync(new URL(`../${p}`, import.meta.url), "utf8");
const HEAVY = {
  "src-tauri/src/activate.rs": [
    "scan_disk_families",
    "list_activated_families",
    "plan_google_activation",
    "retry_google_downloads",
    "repair_incomplete_families",
    "uninstall_font_family",
    "read_family_font",
    "list_family_font_files",
    "register_existing_on_disk",
    "activate_families_on_disk",
    "unload_font_families",
    "start_google_downloads",
    "prune_unknown_folders",
    "try_fontsource_gdi_offer",
    "flush_font_cache",
    "font_family_installed",
  ],
  "src-tauri/src/parse.rs": [
    "parse_family_cmap",
    "parse_family_layout",
    "parse_font_layout",
    "parse_font_layouts",
    "parse_font_cmap",
    "index_font_paths",
    "hash_font_path",
    "diff_font_bytes",
    "list_system_fonts",
  ],
};

for (const [file, names] of Object.entries(HEAVY)) {
  const src = read(file);
  for (const name of names) {
    test(`c253: ${name} is async and off the UI thread`, () => {
      const re = new RegExp(`#\\[tauri::command\\]\\s*pub async fn ${name}\\b[\\s\\S]*?\\n\\}`);
      const m = src.match(re);
      assert.ok(m, `${name} must be #[tauri::command] pub async fn`);
      assert.match(m[0], /spawn_blocking/, `${name} must run its body via spawn_blocking`);
    });
  }
}
