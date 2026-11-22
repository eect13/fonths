# Font Manager 2.0.0 (DirectWrite Edition) — Migration & Verification Record

**Target Folder:** `FontManager-2.0.0-DirectWrite` (created from baseline `FontManager-f19f167`, untouched)  
**Target Version:** `2.0.0`

## 1. Architecture & User Requirements

Font Manager 2.0.0 preserves 100% of existing Font Manager workflows (Google Fonts, Fontsource-only catalog, local uploads, watch folders, variable axes, glyph map, playground, duplicate finder, collections/favorites/tags, Repair/Scan/Refresh, tray/startup, and sandbox mode) while modernizing font inspection and system font browsing for Windows **DirectWrite**:

1. **Session-Only OS Activation (Zero Permanent OS Changes):**
   - Library files remain permanently in `Documents\Font Manager`.
   - Active fonts for external apps (Word, Adobe Creative Cloud, Figma, browsers) are staged only in `%LOCALAPPDATA%\Font Manager\gdi-maps`, registered for the active Windows session via `winfont`, and synced to `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts` under `[FM-Session-*]` entries (`session_stage::hkcu_session`).
   - Quitting Font Manager removes active session registrations and `[FM-Session-*]` HKCU entries; `C:\Windows\Fonts` is never written to or modified.
2. **Read-Only System Fonts View:**
   - On Windows, `src-tauri/src/parse.rs` (`list_system_fonts_blocking`) queries `crate::dwrite::list_system_fonts_dwrite()` (`IDWriteFactory::GetSystemFontCollection(false)`) first, falling back to `walk_system_fonts()` only if DirectWrite returns unavailable/empty or on non-Windows platforms.
   - `is_system_windows_fonts_dir` / `is_system_windows_fonts_dir_with_root` in `src-tauri/src/dwrite.rs` strictly restricts System Fonts entries to `%SYSTEMROOT%\Fonts` and explicitly rejects `gdi-maps`, `Documents\Font Manager`, and legacy `Microsoft\Windows\Fonts\FontManager` paths so session-activated fonts never mix into the read-only System Fonts drawer.

## 2. DirectWrite Engine (`src-tauri/src/dwrite.rs`)

- Added `[target.'cfg(windows)'.dependencies]` in `src-tauri/Cargo.toml`: `windows = "0.61"` (`Win32_Graphics_DirectWrite`, `Win32_Foundation`) and `windows-core = "0.61"`.
- Implemented `dwrite::analyze_font_file(path)`: creates an `IDWriteFontFile` (`CreateFontFileReference`), runs `Analyze`, instantiates `IDWriteFontFace3` and queries `IDWriteFontFace5::HasVariations`, returning structured metadata (`supported`, `face_count`, `file_type`, `face_type`, `is_variable`, `typographic_family`, `win32_family`, `full_name`, `weight`, `italic`, `hresult_code`, `error_reason`).
- Enriched `activate::format_register_zero_detail_with` so `AddFontResourceExW == 0` failures inspect the first intact face via DirectWrite and surface actionable DirectWrite diagnostics.
- Exposed async Tauri commands (`pub async fn` + `tauri::async_runtime::spawn_blocking`), registered in `src-tauri/src/main.rs`, allowed in `src-tauri/permissions/font-activate.toml`, and wrapped in `src/lib/fonts/native-parse.ts`:
  - `dwrite_engine_info` (`getDWriteEngineInfo`)
  - `dwrite_inspect_font` (`inspectFontDWrite`)

## 3. FontBase-Style Performance & Startup Upgrades

1. **Single-IPC Cached Font Preview (`parse.rs`, `native-parse.ts`, `loader.ts`):**
   - Added `path_mtime_size_key`, `layout_cache`, `index_cache`, and `parse_layout_for_path_cached` in `src-tauri/src/parse.rs`.
   - Added async command `read_family_preview_info` returning `{ path, layout }`, wrapped with in-memory `previewInfoCache` (`nativeFamilyPreviewInfo`) in `src/lib/fonts/native-parse.ts` and wired into `loadGoogleFromLocal` in `src/lib/fonts/loader.ts` (falling back cleanly to `read_family_font` + `nativeFamilyLayout`).
2. **Batched HKCU Session Registry Sync (`session_stage.rs`, `activate.rs`):**
   - Added `hkcu_session::sync_entries_batch` and `remove_entries_batch` in `src-tauri/src/session_stage.rs` to open the HKCU `Fonts` registry key once per batch instead of once per font face.
   - Added `HkcuBatchGuard` (`begin_hkcu_batch` / `finish_hkcu_batch` / `sync_hkcu_all_active_stages`) in `src-tauri/src/activate.rs` wrapping `session_begin`, `commit_ready_families_at`, and `activate_on_disk_worker`.
3. **Clean-Exit Fast Boot (`session_stage.rs`, `activate.rs`):**
   - `session_end` writes `.session-clean-exit` when all session paths unload without leftover `keep_paths` or failures.
   - `session_begin` consumes `.session-clean-exit` via `take_clean_exit_marker_in` and, when `.session-paths.txt`, `.session-active.json`, and `.session-maps.json` are cleanly cleared, skips expensive full-cache recovery (`recover_stale_session` / `drain_gdi_maps`) while still running folder migration and single-key HKCU cleanup.

## 4. Bug Fixes & Verification Remediation

- **B1 (Missing Families Silently Dropped):** Extracted `classify_download_intents` and updated `start_google_downloads_blocking` + `activate_on_disk_worker` so unresolved families are recorded in `failed_names` / `failed_details` as `"{family} — Not found (no file on disk and no catalog source)"`.
- **B2 / B3 (Unsafe Variable Font Static Purge):** Added `file_is_variable_font` and `vf_primary_purge_safe_for_dir` in `activate.rs`:
  - Blocks purging sibling static faces for `FetchIntent::Local` uploads.
  - Verifies DirectWrite `analyze_font_file` compatibility (`supported && is_variable` and `typographic_family` match) plus OpenType `nameID 1` match and `LF_FACESIZE - 1` (`<= 31` UTF-16 code units).
  - Blocks purge if any variation axis falls outside `{wght, wdth, ital, slnt}` (protecting `opsz`/parametric fonts like Inter).
  - Uses Recycle Bin via `recycle_static_or_test_delete` (`recycle_bin_windows`) outside `#[cfg(test)]`, keeping `delete_font_file` under `#[cfg(test)]`.
- **B6 (Leftover `gdi-maps` Stage Detection & Safe Cleanup):** Added `find_leftover_stage_files_in`, `recycle_stage_files_in`, and async commands `list_leftover_stage_files` / `recycle_leftover_stage_files` strictly scoped inside `gdi-maps` and excluding active session stage files and `Documents\Font Manager` library files. Added Settings UI (`desktop-settings.tsx`) with scan list and two-step Recycle Bin confirmation.
- **B9 (Desktop Settings Privilege Copy & Engine Card):** Replaced inaccurate `"Always-Admin"` copy with normal-user-rights DirectWrite + session-only description and added the **Font Engine (DirectWrite)** status card (`data-testid="dwrite-engine-card"`).
- **B10 (Rust Compiler Warnings):** Resolved all compiler warnings across Rust crates (`0` warnings in non-test build).
- **B11 (Fast-Path HKCU Drift):** Updated `winfont::register_detailed` (`if dest_ok`) to re-assert `remember_map` and `sync_hkcu_session_entry(&dest)` when skipping redundant `AddFontResourceExW`.
- **B12 (ESLint Warnings):** Eliminated all 27 baseline ESLint warnings (`0` errors, `0` warnings) across frontend components and test helpers.
- **B13 (Local Upload & Watched-Folder Session Registration):** Updated `register_font_path(app, path, family)` / `register_font_path_in` in `src-tauri/src/activate.rs` (and callers in `src/lib/fonts/os-activate.ts` `writeAndRegister` and `installOne` `originPath`) to resolve the family name (explicit hint → SFNT `name` table → `.family` marker / folder name), bind the face via `register_family_path`, and record the family in `.session-families.json` (`session_add`) + persist `.session-paths.txt` / `.session-maps.json` (`persist_activation_sidecars`). Prevents sibling active uploads (e.g., `Inter`) from being swept by `leftover_loaded_after_unload` when another uploaded family (`Figtree`) is deactivated. Added unit test `b13_register_font_path_resolves_family_and_guards_system_fonts`.
- **Tauri ACL & Build/Test Tooling:** Added the 5 new commands to `src-tauri/permissions/font-activate.toml` (`commands.allow`), fixed `main.rs` test helper (`val_name` -> `wide`), fixed Windows quoting in `package.json` (`"test"` and `"test:e2e"` scripts), fixed single-specifier `import { t as __exportAll } from "...";` stripping in `vite.config.ts` (`inlineExportAllPlugin`) for Nitro SSR builds, synchronized version `2.0.0` across manifests and test assertions, and added regression suite `scripts/directwrite-2.0.0.test.mjs`.

## 5. Remnant Cleanup

Removed 30 unreferenced remnant files from the 2.0.0 workspace copy (leaving all `README.md`-referenced screenshots and core project directories intact):
- Root files (4): `AGENTS.md`, `AGENTS.project.md`, `REVIEW-STATUS.md`, `startup.sh`.
- Unreferenced screenshots (26): `activated-empty.png`, `align-center.png`, `duplicates.png`, `fontsource.png`, `fontsource-menu.png`, `fontsource-sans.png`, `glyphs.png`, `google-fonts.png`, `google-fonts-menu.png`, `inspector.png`, `inspector-200-mobile.png`, `inter-card.png`, `library-chips.png`, `library-mobile.png`, `provider-refresh.png`, `provider-refresh-toast.png`, `slider-grid-hover.png`, `slider-grid-idle.png`, `slider-list-hover.png`, `source-sans-3.png`, `toast-fontsource.png`, `toast-google.png`, `weight-400.png`, `weight-800.png`, `weight-compare.png`, `weight-slider.png`.
- Note: Verified pre-existing `tests/fixtures/` files (`Figtree-wght.ttf`, `Inter-opsz-wght.ttf`) remain in use by both JS and Rust tests (`vf-purge-safety` and `dwrite_analyzes_real_vf_fixtures`).

## 6. Verification Scope

Executed on Windows against `FontManager-2.0.0-DirectWrite`:
- `npm.cmd run typecheck` (`tsc --noEmit`) — exit `0`, 0 errors.
- `npm.cmd run lint` (`eslint .`) — exit `0`, 0 errors, 0 warnings.
- `npm.cmd test` (`scripts/**/*.test.mjs`, including `scripts/directwrite-2.0.0.test.mjs`) — exit `0`, `744` tests run (`743` passed, `0` failed, `1` env-gated skip `FM_NOTO_TTC`).
- `npm.cmd run build` (Vite + Nitro SSR + Tauri desktop HTML check) — exit `0`.
- `npm.cmd run test:e2e` (`scripts/e2e/card-click.e2e.mjs`) — exit `0`, `3` passed, `0` failed.
- `cargo check --offline` — exit `0`, 0 errors, 0 warnings.
- `cargo test --offline` — `228` passed, `0` failed, `6` pre-existing `#[ignore]` live-network/benchmark tests (including `b13_register_font_path_resolves_family_and_guards_system_fonts`).

## 7. Release Build & Live Smoke Verification

### Release Artifacts Built Offline (`CARGO_NET_OFFLINE=true .\node_modules\.bin\tauri.cmd build`)
- `src-tauri\target\release\font-manager.exe` (`8,155,648` bytes; `--version` outputs `Font Manager 2.0.0 (commit unknown)`)
- `src-tauri\target\release\bundle\msi\Font Manager_2.0.0_x64_en-US.msi` (`6,443,008` bytes)
- `src-tauri\target\release\bundle\nsis\Font Manager_2.0.0_x64-setup.exe` (`5,470,122` bytes)
- `.vercel\output\static\TIP_SHA.txt` (`version=2.0.0`)

### Live Desktop Release Smoke (`font-manager.exe` under `FM_SANDBOX_DOCS`, WebView2 CDP + Playwright)
All **11/11** live checks passed with `0` uncaught `pageErrors` (`consoleErrors=4` from pre-existing `wawoff2` base64 `data:` fetch blocked by Tauri `connect-src` CSP):
1. **Boot & Catalog + Sandbox:** Rendered `Font Manager 2.0.0`, initial catalog `18` cards, `activation_folder` strictly isolated to `...\Documents\Font Manager` under sandbox.
2. **Desktop Settings + DirectWrite Card + B6 + B9:** Rendered `Font Engine (DirectWrite)` (`DirectWrite · session-only · System Fonts read-only (74 installed faces in Windows\Fonts)`), confirmed B9 removal of `"Always-Admin"`, toggled density and fast session resume, and verified B6 `.part` leftover scan (`smoke-leftover.part`) + Recycle confirmation dialog (cancelled cleanly without modifying host Recycle Bin).
3. **Read-Only DirectWrite System Fonts:** `list_system_fonts` returned `74` families strictly under `Windows\Fonts` (excluding `gdi-maps` / session paths), and Inspector showed read-only notice.
4. **DirectWrite Inspect IPC:** `dwrite_inspect_font` verified `Figtree` (`truetype`, `isVariable=true`) and `Inter` (`truetype`, `isVariable=true`), and rejected missing path (`"font file missing on disk"`).
5. **Local Upload + Variable Axes + Tags + Favorites + Preview IPC:** Uploaded `Figtree-wght.ttf` and `Inter-opsz-wght.ttf`, verified `Variable axes`, `Favorite` toggle, tag `smoke-tag`, and `read_family_preview_info` (`glyphs=459`, `variable=true`).
6. **Session-Only Activate / Deactivate + System Isolation (B13):** Verified local import auto-registered both `Figtree` and `Inter` in `session_families` and `gdi-maps` (`stages=2`), verified `list_system_fonts` excluded `gdi-maps` / `Figtree`, verified Deactivating `Figtree` dropped only `Figtree` while preserving active `Inter` and permanent library files, and re-Activated `Figtree` cleanly.
7. **Search & SuperSearch:** Verified `tag:smoke-tag` → `Figtree`, `axis:wght` filter, and clearing search restored the catalog.
8. **Collections & Font Sets:** Created `Smoke Collection` and `Smoke Set` with live `Inter`.
9. **CSS Export & Theme Toggle:** Opened `CSS export` dialog and toggled theme `dark -> light -> dark`.
10. **All Navigation Routes:** Navigated `Playground` (pairing matrix + sample input), `Duplicates` (`Figtree`/`Inter`), `Glyphs` (`cmap` + `U+0041` search), and back to `Library`.
11. **B1 Missing Family Honest Error (Live IPC):** Verified `resolve_family_fetch_intent("DefinitelyMissingFontXYZ")` returned `null` and `google_download_progress.failed_details` recorded `"DefinitelyMissingFontXYZ — Not found (no file on disk and no catalog source)"`.
- **Graceful Window Close & Teardown:** Posted `WM_CLOSE` to the `"Tauri Window"` / `"Font Manager"` HWND, verified `exited=true`, `exitCode=0`, `.session-clean-exit` written by `session_end`, and sandbox directory deleted with zero locked font handles.

## 8. 2.0.0 Feature Upgrades, Live Fixes & Final Verification

### Implemented Improvements (Across All 6 Areas)
1. **Folder Separation (`Documents\Font Manager\Google` vs `Uploads`):**
   - Updated `src-tauri/src/activate.rs` (`GOOGLE_CONTAINER`, `UPLOADS_CONTAINER`, `family_dir_for_source_in`, `consolidate_legacy_roots_in`, `is_protected_family_dir`, `prune_candidates`, `open_activation_folder`), `src-tauri/src/main.rs`, `src/lib/fonts/family-folder.ts` (`GOOGLE_SUBDIR`, `UPLOADS_SUBDIR`, `documentsFamilySubpath`), `src/lib/fonts/glyph-map.ts`, `src/lib/fonts/os-activate.ts`, `src/components/font-studio/desktop-settings.tsx` (`open-google-folder-btn`, `open-uploads-folder-btn`), and `src/components/font-studio/font-inspector.tsx` so Google/catalog downloads live under `Google/` and added/uploaded fonts live under `Uploads/`, while watched folders remain in place via `originPath`.
2. **Slant (`slnt`) Preview Slider & Live Specimen Updates:**
   - Updated `src/lib/fonts/axes.ts` (`0.1` step on `slnt`, oblique formatting without `-0deg`, `vfStyleDescriptor`, preserving explicit `stored.slnt`/`ital` in `previewAxisValues`), `src/lib/fonts/loader.ts` (full `slnt` range descriptor on `FontFace` registration), `src/components/font-studio/axis-sliders.tsx` (`°` readout + `Reset axes`), `src/components/font-studio/font-card.tsx` (card hover `slnt` slider, direct DOM `fontStyle` painting, `effectiveItalicOn`, spacing re-fit), `src/components/font-studio/font-inspector.tsx` (live `resolvedFontStyle` on main & OpenType demo specimens), and `src/components/font-studio/preview-toolbar.tsx` (`Letter spacing` slider + `Reset size & spacing`).
   - Removed `!important` from `font-style` inside `.fm-spec.fm-spec-roman` and `.fm-spec.fm-spec-italic` in `src/styles.css` (detected during live desktop smoke testing, where `!important` previously overrode inline `style={{ fontStyle: "oblique ...deg" }}` after React re-rendered).
3. **Tagging Fixes & Custom Tag Filtering:**
   - Updated `src/lib/fonts/style-tags.ts` (prioritized icon-font detection into `"icons"`, added `\b` word boundaries on `EXTRA_TAGS` eliminating substring false positives such as `Monoton->noto`, `Winter->neo-grotesque`, and `Background->rounded`, plus exported `normalizeCustomTag` and `parseTagInput`), `src/lib/fonts/store.ts` (comma-delimited tag creation, case-insensitive tag removal and `tag:` search), `src/components/font-studio/sidebar.tsx` (custom tag counts & filtering below `TAG_ORDER`), and `src/components/font-studio/font-inspector.tsx` (clickable filter badges and comma-separated input).
4. **Suggest Pair Algorithm & Live Playground:**
   - Created `src/lib/fonts/pairing.ts` (`PAIRING_PRESETS`, `CLASSIC_PAIR_GRAPH`, `computePairingMatrix`, `suggestCompanionCandidates`, category-diversified `pickNextCompanion` ring) sorting by raw score prior to `25..99` clamping and penalizing Latin/non-Latin script mismatches; wired `src/components/font-studio/playground.tsx` to preserve suggested/preset weights and axes across selection changes and emit pairing CSS with `font-style` / `font-stretch`.
5. **SuperSearch & CSS Export Syntax:**
   - Upgraded `src/lib/fonts/metrics.ts` (`tokenizeSearchQuery` quoted phrases, `-`/`!` negation, `>=`/`<=`/`>`/`<`/`+` comparators, negative `slnt` bounds, `axis:tag`, `slant:`, `has:`, `category:`/`cat:`/`style:`, `source:`/`from:`, `script:`/`lang:`, comma OR lists, `variable:false`/`italic:false`).
   - Added Search Clear (`[data-testid="search-clear-btn"]`) and SuperSearch syntax popover (`[data-testid="search-syntax-btn"]` / `[data-testid="search-syntax-popover"]`) in `src/components/font-studio/app-shell.tsx` (scoping `HelpTip` directly to `<Input>` so Tooltip `Escape` does not swallow Popover `Escape`).
   - Created pure `src/lib/fonts/css-format.ts` (`fontFormatForFile`, `tailwindFontVarKey`, `formatLocalFontFace`, `formatInspectorCss`) and updated `src/lib/fonts/css-export.ts` + Inspector `Copy CSS` for accurate `@font-face` format tokens, variable `font-weight`/`font-style`/`font-stretch` ranges, URI-encoded filenames, and ASCII-safe Tailwind variables.
6. **Live Popups & Themed Confirmation Dialog:**
   - Added `closeButton` and `visibleToasts={4}` on `<Toaster>` in `src/components/font-studio/theme-root.tsx`, added in-place toast `id`s across `src/lib/fonts/os-activate.ts`, `src/components/font-studio/download-bar.tsx`, `src/components/font-studio/playground.tsx` (`fm-pair-suggest`), and `src/components/font-studio/font-inspector.tsx` (`fm-copy-css`), and replaced both native `window.confirm` prompts in `font-inspector.tsx` with `<Dialog data-testid="inspector-delete-dialog">`.

### Final Verification & Release Artifacts
- `npm.cmd run typecheck` (`tsc --noEmit`) — exit `0`, `0` errors.
- `npm.cmd run lint` (`eslint .`) — exit `0`, `0` errors, `0` warnings.
- `npm.cmd test` (including `scripts/v2-improvements.test.mjs`) — exit `0`, `751` tests run (`750` passed, `0` failed, `1` env-gated skip `FM_NOTO_TTC`).
- `npm.cmd run build` — exit `0`.
- `npm.cmd run test:e2e` — exit `0`, `3` passed, `0` failed.
- `cargo check --offline --tests` & `cargo test --offline` — exit `0`, `0` compiler warnings, `229` passed, `0` failed, `6` pre-existing `#[ignore]` tests (including `v2_google_and_uploads_folder_separation_and_prune_protection`).
- **Final Offline Release Artifacts (`$env:CARGO_NET_OFFLINE="true"; .\node_modules\.bin\tauri.cmd build`):**
  - Executable: `src-tauri\target\release\font-manager.exe` (`8,167,424` bytes; `--version`: `Font Manager 2.0.0 (commit unknown)`)
  - MSI Installer: `src-tauri\target\release\bundle\msi\Font Manager_2.0.0_x64_en-US.msi` (`6,451,200` bytes)
  - NSIS Setup EXE: `src-tauri\target\release\bundle\nsis\Font Manager_2.0.0_x64-setup.exe` (`5,473,507` bytes)
- **Expanded Live Desktop Smoke Suite (WebView2 CDP + Playwright under isolated `FM_SANDBOX_DOCS`):** **11/11** checks passed, `0` `pageErrors` (`consoleErrors=4` from pre-existing `wawoff2` `data:` CSP fetch), verifying `Uploads/` vs `Google/` separation, Settings folder buttons, live `slnt` (`oblique 10deg` / `7.5deg`) & `Letter spacing` previews without snapback, comma multi-tags + live Sidebar counts, upgraded SuperSearch syntax + popover + clear, `truetype` CSS export, Playground `Suggest pair` in-place toast updates, live Inspector `Copy CSS` toast + themed Radix delete dialog cancellation, session-only activation isolation, and graceful `WM_CLOSE` teardown (`exited=true`, `exitCode=0`, `.session-clean-exit` created, sandbox deleted).



## 9. Persistence, Metadata Tagging, Live Catalog Sync & Import Control Upgrades

### Code & Architecture Improvements
1. **Remembering Activated Fonts & UI State Across Exit/Reopen (While Dropping OS Session Fonts When Closed):**
   - `src-tauri/src/activate.rs` (`family_dir_is_ready_with_mode`): recognized `Uploads/<Family>`, `FetchIntent::Local`, and `.family-name`-stamped directories directly via intact font verification so uploaded local fonts restore without requiring Google's `.complete` marker, while `session_end` continues unloading session registrations cleanly on app exit.
   - `src/lib/fonts/store.ts` (`PersistedSlice`, `merge`, `partialize`, `setBootRestoringIds`): preserved `l-*` local activation IDs through `merge()` (prior to async IndexedDB `localFonts` hydration), prevented transient boot `restoreActivation([], [])` calls from wiping `activated` in `localStorage`, and persisted `query`, `selectedId`, and `inspectorOpen`.
   - `src/lib/fonts/hydrate.ts`: pruned missing `l-*` IDs only after `loadLocalFontsMeta()` resolves, re-registered persisted local/watched-folder (`originPath`) fonts via `installFontOnSystem` on startup, and cleared the boot-restore guard on completion.
2. **OpenType Metadata-Driven Local Style Tagging:**
   - Added optional `panose`, `sFamilyClass`, and `isFixedPitch` fields to `FontMetrics` (`src/lib/fonts/types.ts`, `src/lib/fonts/metrics.ts`).
   - Extracted `OS/2.sFamilyClass` (`offset 30`) and `post.isFixedPitch` (`offset 12`) in both the JS SFNT parser (`src/lib/fonts/sfnt.ts`) and the Rust parser (`src-tauri/src/parse.rs` + `src/lib/fonts/native-parse.ts`).
   - Updated `inferLocalStyle` (`src/lib/fonts/style-tags.ts`) and all ingestion/reclassification paths (`src/lib/fonts/parse-font.ts`, `src/lib/fonts/store.ts`, `src/lib/fonts/hydrate.ts`) to consume `subfamily`, `fullName`, `panose`, `sFamilyClass`, `widthClass`, `isFixedPitch`, `hasLigatures`, and `axes` (`wdth`, `opsz`).
3. **Library & Playground Bug Fixes and State Persistence:**
   - `src/components/font-studio/playground.tsx`: persisted Playground state (`leftId`, `rightId`, `leftSize`, `rightSize`, `leftWeight`, `rightWeight`, `leftAxes`, `rightAxes`, `heading`, `body`, `invert`) under `localStorage["font-manager:playground-v1"]`, fixed fallback validation (`findFont` / `!hydrated && l-`) so presets and `Suggest pair` selections are never overwritten, and synchronized static weight clicks with `leftAxes.wght`/`rightAxes.wght`.
   - `src/components/font-studio/font-picker.tsx`: allowed selecting local uploads, activated fonts, system fonts, and searched or top-80 popular Google Fonts directly in Playground and Glyphs.
   - `src/components/font-studio/library-grid.tsx`: reset scroll position to the top when `scope`, `facet`, or `deferredQuery` changes; `src/components/font-studio/uploads-reset-dialog.tsx`: preserved `.fm-dialog-footer` spacing (`fm-dialog-footer mt-5`).
4. **Desktop Settings & Remnant Cleanup:**
   - Removed `"Prefer static faces"` and `"Fast session resume (bypass deep disk check)"` switches from `src/components/font-studio/desktop-settings.tsx` while keeping backend settings APIs intact.
   - Removed unreferenced remnant files (`src/lib/multiplayer/`, `src/lib/fonts/providers.ts`, `docs/LETTER-TIPS.md`, `tests/statcheck/`, `fix-install.bat`) and updated `scripts/bug-audit-fixes.test.mjs` and `scripts/fast-resume-options.test.mjs`.
5. **Live Google Fonts Catalog Fetch (`fetch_live_google_catalog`):**
   - Added async Tauri command `fetch_live_google_catalog` (`src-tauri/src/activate.rs`, registered in `src-tauri/src/main.rs`, allowed in `src-tauri/permissions/font-activate.toml`) to fetch and parse `https://fonts.google.com/metadata/fonts` via Rust `reqwest` (avoiding browser CORS limits).
   - Exported `SnapshotRow`, `replaceGoogleDirectory`, and `snapshotToRecord` from `src/lib/fonts/catalog.ts`, and wired `refreshGoogleCatalog` + `loadCachedCatalog` in `src/lib/fonts/google-api.ts` to persist refreshed catalogs in IndexedDB (`catalog:live`).
6. **Live Pause / Resume / Cancel for User Font Imports:**
   - Added `uploadJob` progress state plus `pauseUpload()`, `resumeUpload()`, and `cancelUpload()` in `src/lib/fonts/store.ts` (`importFiles` / `importOriginPaths`), coordinated with `setUploadQueuePaused()` / `cancelUploadDiskQueue()` in `src/lib/fonts/os-activate.ts` and rendered an interactive progress card (`[data-testid="upload-progress-card"]`, `upload-pause-resume`, `upload-cancel`) in `src/components/font-studio/upload-dropzone.tsx`.

### Final Verification & Updated Release Artifacts
- `npm.cmd run typecheck` (`tsc --noEmit`) — exit `0`.
- `npm.cmd run lint` (`eslint .`) — exit `0`.
- `npm.cmd test` — exit `0`, `754` tests run (`753` passed, `0` failed, `1` skipped `real Noto Sans CJK TTC: 10 faces (set FM_NOTO_TTC)`).
- `npm.cmd run build` — exit `0` (`2052` client modules transformed).
- `npm.cmd run test:e2e` — exit `0`, `3` passed, `0` failed.
- `cargo check --manifest-path src-tauri/Cargo.toml --offline --tests` — exit `0`.
- `cargo test --manifest-path src-tauri/Cargo.toml --offline` — exit `0`, `237` total tests (`231` passed, `0` failed, `6` pre-existing `#[ignore]` tests).
- **Updated Offline Release Artifacts (`$env:CARGO_NET_OFFLINE="true"; .\node_modules\.bin\tauri.cmd build`):**
  - Executable: `src-tauri\target\release\font-manager.exe` (`8,186,368` bytes, `2026-11-22 14:59:28`)
  - MSI Installer: `src-tauri\target\release\bundle\msi\Font Manager_2.0.0_x64_en-US.msi` (`6,459,392` bytes, `2026-11-22 14:59:14`)
  - NSIS Setup EXE: `src-tauri\target\release\bundle\nsis\Font Manager_2.0.0_x64-setup.exe` (`5,482,879` bytes, `2026-11-22 14:59:28`)
- **Live 2-Launch Sandbox WebView2 CDP Smoke (`src-tauri\target\debug\font-manager.exe` under `app.fontmanager.sandbox` + `FM_SANDBOX_DOCS`):**
  - Run 1 + Run 2 exited cleanly (`cleanExitRun1: true`, `cleanExitRun2: true`, `exitCode: 0`, `pageErrorsRun1: []`, `pageErrorsRun2: []`), verifying `dwrite_engine_info` (`engine: "DirectWrite"`, `available: true`, `activationMode: "session-only"`, `systemFontsReadOnly: true`, `systemFontCount: 74`), cleaned Desktop Settings (`Prefer static faces` and `Fast session resume` removed), Playground persistence (`g:JetBrains Mono` saved and restored in Heading picker), `Documents\Font Manager\Uploads` staging (`Figtree`, `Inter`), local activation persistence across exit and reopen without Google catalog eviction (`localActBeforeCount: 2` → `localActAfterCount: 2`, `evictedByGoogle: false`, both cards active on reopen), and live `fetch_live_google_catalog` returning `1,961` families.

## 10. Performance Architecture Refactor, Admin Elevation, CDN File Sync, Smart Deletion & Gestalt UI

### 1. Measured Windows Benchmarks (Phase 1)
- **SFNT Parsing (`C:\Windows\Fonts`, 131 `.ttf`/`.otf` files, `131.5 MB`):**
  - Full file read (`std::fs::read`): `363.27 ms` (`1.0×`)
  - Memory-mapped 4 KB view (`CreateFileMappingW`): `52.80 ms` (`6.9×`)
  - DirectWrite `CreateFontFileReference` + `Analyze`: `33.25 ms` (`10.9×`)
  - Targeted 4 KB partial SFNT table read (`seek` + selective metadata tables): **`16.31 ms` (`22.3×`)**
  - DirectWrite `GetSystemFontCollection` (85 families): **`1.73 ms`**
- **Windows Font Registration/Unloading (50 files / 400 faces on temp fixtures):**
  - `AddFontResourceExW(FR_PRIVATE | FR_NOT_ENUM)`: `add = 111.79 ms`, `remove = 15.96 ms` (process-private only)
  - `AddFontResourceExW(FR_PRIVATE)`: `add = 470.91 ms`, `remove = 15.71 ms` (process-private only)
  - `AddFontResourceExW(0)` (cross-app public session): `add = 11,317.62 ms`, `remove = 11,916.51 ms` (required for Word/Adobe/Figma visibility; runs asynchronously off the UI thread)
- **Frontend Search (20,000 synthetic fonts, 5 queries):**
  - Previous linear `hayFor()` rebuild: `84.73 ms` (`~16.95 ms/query`)
  - Inverted prefix/trigram index: **`5.94 ms` (`14.3×` faster, `~1.19 ms/query`)**

### 2. Core Engine, Elevation & Algorithms (Phase 2)
- **Windows Admin Manifest (`src-tauri/app.manifest`, `src-tauri/build.rs`, `src-tauri/src/main.rs`, `src-tauri/src/dwrite.rs`):**
  - Added `src-tauri/app.manifest` specifying `<requestedExecutionLevel level="requireAdministrator" uiAccess="false" />`, `PerMonitorV2` DPI awareness, and `longPathAware`. Embedded automatically into release binaries via `tauri_build::WindowsAttributes::app_manifest` (verified directly in compiled `src-tauri\target\release\font-manager.exe`).
  - Added `is_elevated()` in `main.rs` + `elevated` on `DWriteEngineInfo`, and updated `apply_start_with_windows` to register/delete a Windows Task Scheduler logon task (`FontManagerAutoStart`, `/RL HIGHEST`) when elevated.
- **Fast Targeted SFNT Parser & Parallel Indexer (`src-tauri/src/fast_parse.rs`, `src-tauri/src/parse.rs`):**
  - Added `open_shared_read` (`FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`), `sha256_file_streaming`, and `read_metadata_sfnt_bytes` skipping glyph outline tables (`glyf`, `CFF `, `CFF2`, `COLR`, `CBDT`, `sbix`, `SVG `, `cmap`).
  - Wired `fast_parse` into `parse_layout_for_path_cached` and `index_one_path` (for files `> 256 KB`), and upgraded `index_font_paths_blocking` to bounded multi-threaded indexing via `std::thread::scope` (up to 8 workers, preserving deterministic order).
- **Clean Handle Release, `O(1)` Preview LRU & Inverted Trigram Search (`src-tauri/src/activate.rs`, `src/lib/fonts/loader.ts`, `src/lib/fonts/search-index.ts`, `src/lib/fonts/metrics.ts`, `src/lib/fonts/store.ts`):**
  - Drained both public and `FR_PRIVATE` GDI handles in `winfont::remove_one` and added elevated `FontCache` unlock retry in `delete_font_file`.
  - Converted `loader.ts` `familyFaces` to an `O(1)` Map-insertion-order LRU, tracked & revoked fallback `Blob` URLs on eviction (fixing memory leak), and exported `evictFamilyPreview()`.
  - Created `src/lib/fonts/search-index.ts` (memoized `WeakMap` inverted trigram + token index preserving exact substring fidelity) and eliminated `O(N)` per-font `parseSearchQuery()` calls in `store.ts`.

### 3. Google Fonts CDN File Sync & Smart Deletion (Phase 3)
- **Google CDN Revision Sync (`src-tauri/src/activate.rs`, `src-tauri/src/main.rs`, `src-tauri/permissions/font-activate.toml`, `src/lib/fonts/native-parse.ts`, `src/lib/fonts/google-api.ts`):**
  - Upgraded `fetch_live_google_catalog` to record per-family `lastModified` dates (`google-cdn-revisions.json`) and stamp `.cdn-revision` on completed Google downloads.
  - Added async `sync_google_cdn_updates` command + `syncGoogleCdnFiles()` wiring so refreshing Google Fonts checks downloaded families under `Documents\Font Manager\Google`, detects outdated revisions or incomplete folders, and queues real CDN font downloads/updates without activating inactive families.
- **Smart Deletion & Temporary Fonts Rule (`src-tauri/src/delete_ops.rs`, `src/lib/desktop/prefs.ts`, `src/components/font-studio/font-inspector.tsx`, `src/components/font-studio/uploads-reset-dialog.tsx`):**
  - Added global `deleteMode: "recycle" | "permanent"` preference synced to Rust (`set_desktop_prefs`) and displayed clearly in Inspector and Uploads Reset confirmation modals.
  - Enforced the **Temporary Fonts Rule**: temporary/stage files (`gdi-maps`, `.part`) are permanently unlinked via `delete_temp_file_permanently` and one-click `cleanup_temp_stage_files` without ever cluttering the Recycle Bin.

### 4. Live Playground & Gestalt UI Refactor (Phase 4)
- **Live Playground (`src/components/font-studio/playground.tsx`):**
  - Fixed System Font (`s:*`) reactive rendering bug (`systemFonts` subscription + `loadSystemFonts()`).
  - Added 100% live Size (`12–144px`), Line Height (`0.85–2.40`), Tracking (`-0.08..0.30em`), Ink/Paper color pickers, Viewport Zoom (`50%–200%`), OpenType feature pills (`liga`, `dlig`, `smcp`, `tnum`, `onum`, `zero`, `frac`, `ss01`), debounced `localStorage` persistence, and auto-growing `AutoTextarea` + `overflow-wrap: anywhere` eliminating nested scrollbars and clipping.
- **Gestalt Debloat (`sidebar.tsx`, `preview-toolbar.tsx`, `desktop-settings.tsx`):**
  - Removed redundant `font-sets-panel.tsx` and duplicate Sidebar `Variable`/`Italic` rows; converted `Style`, `License`, and `Tags` into collapsible sections.
  - Surfaced live Tracking in `PreviewToolbar` and folded niche `Newsprint`/`Blueprint` proofs into the overflow menu.
  - Streamlined `DesktopSettings` into 4 clean Gestalt groups (`Appearance`, `Startup & Tray`, `Delete behavior & Temp Cache`, `Font Engine & Folders`) with `Admin` elevation badge, `Delete behavior` toggle, `Clean temp cache`, and `Update Google files` CDN sync button.

### 5. Final Phase 5 Verification & Updated Production Release Artifacts
- `npm.cmd run typecheck` (`tsc --noEmit`) — exit `0`.
- `npm.cmd run lint` (`eslint .`) — exit `0` (`0` warnings).
- `npm.cmd test` (including `scripts/phase2-core-engine.test.mjs` and `scripts/phase4-playground-gestalt.test.mjs`) — exit `0`, `759` tests run (`758` passed, `0` failed, `1` skipped `real Noto Sans CJK TTC: 10 faces (set FM_NOTO_TTC)`).
- `npm.cmd run build` — exit `0`.
- `npm.cmd run test:e2e` — exit `0`, `3` passed, `0` failed.
- `cargo test --manifest-path src-tauri/Cargo.toml --offline` — exit `0`, `240` total tests (`234` passed, `0` failed, `6` pre-existing `#[ignore]` tests; `0` compiler warnings).
- **Live 2-Launch Sandbox WebView2 CDP Smoke (`app.fontmanager.sandbox` + `FM_SANDBOX_DOCS`):**
  - `ok: true`, `pageErrorsRun1: []`, `pageErrorsRun2: []`, `cleanExitRun1: true`, `cleanExitRun2: true`, verifying `dwrite_engine_info` (`DirectWrite`, `elevated` flag, `systemFontCount: 74`), `delete-mode-permanent` toggle + persistence (`deleteModeRestored: "permanent"`) + Inspector `"Permanently Delete"` confirmation dialog, `cleanup_temp_stage_files` & `sync_google_cdn_updates` IPC, live Playground color (`rgb(34, 68, 136)`) & OpenType (`"liga", "smcp", "tnum"`) DOM reactivity, Gestalt Sidebar debloat, `Documents\Font Manager\Uploads\{Figtree,Inter}` staging, and `2/2` local activations restored cleanly on reopen.
- **Final Offline Release Artifacts (`$env:CARGO_NET_OFFLINE="true"; .\node_modules\.bin\tauri.cmd build`):**
  - Executable: `src-tauri\target\release\font-manager.exe` (`8,246,272` bytes, `2026-11-22 16:43:54` — verified embedded `requireAdministrator`, `PerMonitorV2`, and `longPathAware` PE manifest)
  - MSI Installer: `src-tauri\target\release\bundle\msi\Font Manager_2.0.0_x64_en-US.msi` (`6,488,064` bytes, `2026-11-22 16:43:41`)
  - NSIS Setup EXE: `src-tauri\target\release\bundle\nsis\Font Manager_2.0.0_x64-setup.exe` (`5,501,924` bytes, `2026-11-22 16:43:54`)

## 11. Adobe CoolType Fixes, GDI Crash/Restore Hardening, Scrollable Notifications, Right-Click Menus, Duplicate Tab & 100k Font Scale

1. **Adobe Font Loading Fixes (`src-tauri/src/session_stage.rs`, `src-tauri/src/namepatch.rs`, `src-tauri/src/activate.rs`):**
   - Stopped truncating `session_hkcu_value_name` to 12 chars (now preserves up to 48 sanitized chars / full 32-char SHA-256 `gdi-maps` stem) and added `.otc` `(OpenType)` support so staged faces never overwrite each other in `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts`.
   - Added `normalize_registry_font_path` stripping `\\?\` / `\\?\UNC\` extended prefixes before writing HKCU values so Adobe CoolType resolves 100% of staged paths.
   - Guarded `namepatch.rs` (`rebuild_instance_name_with_mode` & `rebuild_name_family_only`) against `u16` string-table offset overflow that could silently corrupt large multi-language/CJK `name` tables and cause Adobe CoolType to reject the font.
   - Preserved `.otc` extension in `gdi_map_file_name`, ensured `sync_hkcu_all_active_stages()` flushes before `notify_fonts_changed()`, and added a trailing-edge timer in `winfont::notify_maybe()` so rapid back-to-back single-font activations never lose their final `WM_FONTCHANGE` broadcast to Adobe/Word.
2. **GDI Stability, Crash Prevention & Accurate Session Restore (`src-tauri/src/activate.rs`):**
   - Added `GDI_OBJECT_HARD_STOP = 9_200` (`RegisterOutcome::GdiQuotaExceeded`) and fast 4-byte SFNT/TTC magic validation (`has_valid_sfnt_magic`) inside `winfont::register_detailed` so corrupt files never reach Windows GDI (`AddFontResourceExW`) and massive batches pause cleanly before Windows's 10,000 per-process GDI handle wall instead of crashing WebView2/Tauri.
   - Replaced destructive `session_begin` truncation (`save_session_families(app, &ready)`) with `merge_session_families_after_restore(...)` so interrupted, cancelled, GDI-capped, or transiently locked restores never erase saved active fonts.
   - Added cooperative `thread::yield_now()` pacing every 64 faces during parallel GDI registration.
3. **Scrollable Notification Center (`src/components/font-studio/notification-center.tsx`):**
   - Replaced height-unconstrained Radix `<ScrollArea>` with native `<div data-testid="notifications-list" className="fm-scroll min-h-0 flex-1 overflow-y-auto overscroll-contain">`, viewport height clamp (`max-h-[min(28rem,calc(100vh-5rem))]`), `All` / `Issues` filter pills, and one-click copy per notification.
4. **Desktop Right-Click Context Menus (`src/lib/context-menu-bus.ts`, `src/components/font-studio/context-menu.tsx`, `app-shell.tsx`, `font-card.tsx`, `duplicate-finder.tsx`):**
   - Added a viewport-clamped `AppContextMenu` singleton (zero per-card portal overhead at 100k cards) and global WebView2 context-menu suppression on non-editable UI while preserving native browser Cut/Copy/Paste on inputs, textareas, `[contenteditable]`, and selected text.
   - Added right-click menus on Font Cards (`Activate/Deactivate`, `Open Inspector`, `Favorite`, `Collections` quick-toggle, `Copy Family Name`, `Copy CSS font-family`, `Open Folder`, and `Delete` honoring `deleteMode`) and Duplicate rows.
5. **Duplicate Tab Fixes (`src/lib/fonts/store.ts`, `src/components/font-studio/duplicate-finder.tsx`):**
   - Eliminated the `O(N²)` `localFonts.filter(...)` inside `findDuplicates` with an `O(N)` `Map` index, added same-family + same-weight + same-italic local duplicate detection (without falsely flagging Regular + Bold), and capped per-size-bucket pairwise byte diffs (`MAX_BUCKET_PAIRWISE = 32`).
   - Wired automatic `loadSystemFonts()` on mount, added `All` / `Identical files` / `Same family` filter buttons, 50-group pagination (`Show more`), and delete confirmation dialog honoring `deleteMode` + `evictFamilyPreview()`.
6. **100k Fonts Scale Support (`src/lib/fonts/virtual-window.ts`, `library-grid.tsx`, `store.ts`, `search-index.ts`, `persist-local.ts`, `src-tauri/src/parse.rs`):**
   - Implemented virtual scroll height scaling (`MAX_VIRTUAL_SCROLL_HEIGHT = 12_000_000`) to defeat Chromium's ~`16.7M px` CSS height ceiling so 100,000+ fonts scroll smoothly from item 1 to item 100,000.
   - Added fast pre-lowercased Schwartzian sort in `sortLibrary()` for libraries `> 5,000` fonts, allocation-free char-code trigram scanning in `search-index.ts`, chunked compact JSON serialization in `persist-local.ts`, and raised Rust `parse.rs` `index_cache` ceiling to `131,072`.
7. **Verification & Release Artifacts:**
   - `npm.cmd run typecheck` — exit `0`; `npm.cmd run lint` — exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test` — exit `0`, `765` tests (`764` passed, `0` failed, `1` skipped `FM_NOTO_TTC`), including `scripts/phase6-adobe-gdi-100k.test.mjs`.
   - `npm.cmd run build` — exit `0`; `npm.cmd run test:e2e` — exit `0` (`3/3` passed).
   - `cargo test --manifest-path src-tauri/Cargo.toml --offline` — exit `0`, `245` tests (`239` passed, `0` failed, `6` pre-existing `#[ignore]`).
   - `npm.cmd run test:desktop` (2-launch WebView2 CDP E2E) — `PASS` (including scrollable `notifications-list`, Font Card right-click menu, and live Duplicates detection).
   - **Updated Offline Release Artifacts:**
     - Executable: `src-tauri\target\release\font-manager.exe` (`8,260,096` bytes, `2026-11-22 18:32:27`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Font Manager_2.0.0_x64_en-US.msi` (`6,496,256` bytes, `2026-11-22 18:32:13`)
     - NSIS Setup EXE: `src-tauri\target\release\bundle\nsis\Font Manager_2.0.0_x64-setup.exe` (`5,510,763` bytes, `2026-11-22 18:32:27`)

## 12. Finalization as **Fonths 1.0.0** & Adobe Quit-Leftover Fix

1. **Why Closing the App Left Leftovers in Adobe (Root Causes & Fixes in `src-tauri/src/activate.rs`):**
   - **Missing `WM_FONTCHANGE` broadcast on quit:** `session_end()` previously called `winfont::unload_paths(extra.clone(), false)` (`broadcast = false`), so Windows never notified Adobe CoolType (`WM_FONTCHANGE`) that session fonts had been unloaded. Updated to `winfont::unload_paths(extra.clone(), true)` so `SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0, 0)` fires non-blockingly immediately after GDI drain + `GdiFlush` + HKCU cleanup.
   - **Late HKCU session cleanup:** Moved `winfont::clear_all_hkcu_session_entries()` to run **immediately** at the start of `session_end()` (< 5 ms) as well as at the end of `unload_paths`, guaranteeing zero `[FM-Session-...]` registry values remain in `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts` even if quit is interrupted.
   - **Persisted session-map coverage & sandbox isolation:** Merged `stage_paths_from_maps` from both live `snapshot_face_maps()` and persisted `.session-maps.json` into the quit drain set without touching dormant cache files, and isolated sandbox `gdi_maps_root()` under `<sandbox>/gdi-maps`.
2. **Remnant Cleanup & Clean New Folder (`Fonths-1.0.0`):**
   - Created a clean standalone folder `c:\Users\Eric\Desktop\FontManager_TEST-f19f167-source\Fonths-1.0.0` omitting stale template/dev remnants (`screenshots/*.png`, `migrations/`, `scripts/migration-plan.test.mjs`, `deploy.bat`, `deploy.command`, `desktop-setup.bat`, `desktop-setup.command`, `.grok/references/`, and 17 unused `.grok/skills/*` directories).
3. **Rebrand to `Fonths` (`1.0.0`):**
   - Updated `package.json`, `package-lock.json`, `src/version.ts`, `src/routes/__root.tsx`, `desktop.html`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`, `src-tauri/tauri.sandbox.conf.json`, `src-tauri/app.manifest`, `src-tauri/src/main.rs`, `src-tauri/windows/hooks.nsh`, and UI copy to **Fonths 1.0.0** (`app.fonths.desktop`, `fonths.exe`, `FonthsAutoStart` logon task, elevated `perMachine` installer closing both `fonths` and legacy `font-manager`).
   - Preserved `Documents\Font Manager` storage paths and `localStorage` keys so existing user font libraries and preferences carry over seamlessly.
4. **Final Verification & Release Artifacts (`Fonths-1.0.0`):**
   - `npm.cmd run typecheck` — exit `0`; `npm.cmd run lint` — exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test` — `759` tests (`758` passed, `0` failed, `1` skipped `FM_NOTO_TTC`).
   - `npm.cmd run build` — exit `0`; `npm.cmd run test:e2e` — `3/3` passed.
   - `cargo test --offline` — `246` tests (`240` passed, `0` failed, `6` pre-existing `#[ignore]`).
   - `npm.cmd run test:desktop` (2-launch WebView2 CDP E2E against `fonths.exe`) — `PASS`.
   - **Final Production Release Artifacts:**
     - NSIS Setup EXE (`perMachine` elevated): `src-tauri\target\release\bundle\nsis\Fonths_1.0.0_x64-setup.exe` (`5,510,476` bytes, `2026-11-22 19:25:00`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Fonths_1.0.0_x64_en-US.msi` (`6,492,160` bytes, `2026-11-22 19:24:45`)
     - Executable: `src-tauri\target\release\fonths.exe` (`8,260,096` bytes, `2026-11-22 19:25:00`)

## 13. Fonths File-Structure Rebrand, FontBase-Style `Providers\Google`, Noto Emoji Fix, Purge Non-Google, Playground & Cancel Polish

1. **Noto Emoji Settled Root Cause & Fix (`src-tauri/src/activate.rs`):**
   - `RegisterFaceMode::AllIntactFallback` previously skipped `is_compat_sidecar_filename(name)`, so when `noto-emoji-variable-wght.ttf` returned `Add=0` on Windows GDI, `noto-emoji-compat-outline.ttf` was skipped and `.settled-add-zero` was stamped. Updated `AllIntactFallback` to allow `is_compat_sidecar_filename` for soft emoji families (`Noto Emoji`, `Noto Color Emoji`) so their Windows-compatible outline TTF loads live.
2. **Full `Fonths` File Structure + FontBase-Style `Providers\Google` Separation (`activate.rs`, `google_cdn.rs`, `dwrite.rs`, `session_stage.rs`, `open-fonts.ts`, `watch-folder.ts`, `os-activate.ts`):**
   - Rebranded library paths to `Documents\Fonths` (`Documents\Fonths\Providers\Google` for Google Fonts, `Documents\Fonths\Uploads` for local user fonts) and `%LOCALAPPDATA%\Fonths` (`gdi-maps`, `scan-index.json`, `google-cdn-revisions.json`), with HKCU session value prefix `Fonths [FM-Session-{tag}] ({fmt})`.
   - Legacy `Google`, `Activated`, and `Library` subfolders under `Documents\Fonths` automatically consolidate into `Providers\Google` & `Uploads`, while keeping `dwrite.rs` / `session_stage.rs` / `open-fonts.ts` guards active for both `Fonths` and legacy `Font Manager`.
3. **Purge Non-Google Fonts (`purge_non_google_fonts` in `activate.rs`, `main.rs`, `font-activate.toml`, `native-parse.ts`, `desktop-settings.tsx`):**
   - Added async `purge_non_google_fonts(keep, dry_run)` command and **Purge non-Google** button in Settings with dry-run candidate preview and confirmation modal honoring `deleteMode` (`Recycle Bin` vs `Permanent Delete`), strictly sparing `Uploads` and `C:\Windows\Fonts`.
4. **Playground Viewport & Top Controls (`src/components/font-studio/playground.tsx`):**
   - Made top controls independently scrollable (`max-h-[42vh] overflow-y-auto fm-scroll`), added **Focus viewport / Show controls** collapse toggle (`playground-toggle-controls`) and **Single / Split** viewport toggle (`playground-layout-toggle`), keeping the live preview viewport generous (`min-h-[260px]`).
5. **Separated Settings & Cancel Scope Choice (`desktop-settings.tsx`, `download-bar.tsx`, `os-activate.ts`):**
   - Separated **Startup & Tray** (`Start with Windows`, `Close to tray`) from **Fast GDI session restore** (`fastSessionResume` toggle in its own dedicated card).
   - Updated Cancel on multi-font batches to ask **"Cancel current font only"** (`cancelSingleQueuedFamily`) vs **"Cancel all operations"** (`cancelDownloadQueue`, which now also stops active user upload imports).
6. **Final Verification & Release Artifacts (`Fonths-1.0.0`):**
   - `npm.cmd run typecheck`: exit `0`; `npm.cmd run lint`: exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test`: `758` passed, `0` failed, `1` skipped (`FM_NOTO_TTC`).
   - `cargo test --offline`: `241` passed, `0` failed, `6` ignored.
   - `npm.cmd run test:e2e`: `3/3` passed; `npm.cmd run test:desktop` (live WebView2 2-launch CDP E2E): `PASS`.
   - **Final Production Release Artifacts:**
     - NSIS Setup EXE (`perMachine` elevated): `src-tauri\target\release\bundle\nsis\Fonths_1.0.0_x64-setup.exe` (`5,517,990` bytes, `2026-11-22 20:13:26`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Fonths_1.0.0_x64_en-US.msi` (`6,504,448` bytes, `2026-11-22 20:13:12`)
     - Executable: `src-tauri\target\release\fonths.exe` (`8,281,088` bytes, `2026-11-22 20:13:26`)

## 14. Crash-Proof Delete / Reset Library, Resilient Toast Pipeline & Smart Local Font Import

1. **Crash-Proof Delete & Reset Library (`src/lib/fonts/os-activate.ts`, `src/lib/fonts/store.ts`, `src/components/font-studio/uploads-reset-dialog.tsx`):**
   - Added batched `removeUploadsBatchFromDisk()` which deduplicates unique uploaded families (strictly sparing watched `originPath` and system fonts), fires ONE batched `unload_font_families` GDI unload, deletes folders sequentially (yielding every 4 items so Explorer/WebView2 never stall), updates Zustand `diskFamilies` once, and emits at most one summary error toast if any files are locked.
   - Updated `deleteFontFiles()` to honor `deleteMode === "permanent"` vs `"recycle"` wording and support silent/batched calls.
   - Updated `clearLocalFonts()`, `resetLibrary()`, `deleteCollection()`, `removeLocalFont()`, and `removeLocalFontsBatch()` to cancel active upload/download queues first, evict WebView2 previews, delete IDB blobs in 48-item chunks, persist `saveLocalFontsMeta`, and unload remaining non-local fonts via a single batched `syncFontsOnSystem(remaining, false)` instead of N parallel IPC loops.
   - Hardened `UploadsResetDialog` with `busy` re-entrancy lock, spinner state, and `try/catch/finally` error handling.
2. **Crash-Proof Toast System (`src/lib/toast-guard.ts`, `src/lib/notifications.ts`, `src/components/font-studio/theme-root.tsx`):**
   - Debounced `localStorage` notification persistence (`300ms` + `pagehide` flush) to eliminate synchronous disk stalls during rapid updates.
   - Added `sanitizeToastText()` (extracts `.message` from `Error`, strips ASCII control chars, caps title at 200 chars & description at 480 chars), `1200ms` duplicate-ID reuse, and burst coalescing (`> 5` distinct un-ID'd toasts inside `1000ms` coalesce into one `fm-toast-burst` summary popup while preserving every entry in Notification Center), wrapped in `try/catch`, with `<Toaster visibleToasts={3} duration={4500} expand={false} />`.
3. **Smart Local Font Import (`src/lib/fonts/import-options.ts`, `src/components/font-studio/import-options-dialog.tsx`, `src/components/font-studio/import-fonts.ts`, `src/components/font-studio/app-shell.tsx`):**
   - Automatically detects Variable vs. Static overlaps for the same family (both within a dropped batch/folder AND across existing `localFonts`), mixed desktop + web formats (`.ttf`/`.otf`/`.ttc` + `.woff`/`.woff2`), multi-format drops, or `>= 6`-file bundles, while keeping small clean drops zero-friction.
   - Presents `ImportOptionsDialog` with **Variable only (Recommended)** / **Static only** / **Both** radio cards, optional replacement of existing opposite-kind faces in the library, smart **Font kinds** checkboxes (`Variable`, `Static`, `Italic / Oblique`), smart **File formats** checkboxes (auto-unchecking web-only `WOFF`/`WOFF2` when desktop `TTF`/`OTF` files are present), and an **Auto-activate imported fonts** checkbox with live selected file counts.
4. **Verification & Updated Production Release Artifacts (`Fonths-1.0.0`):**
   - `npm.cmd run typecheck`: exit `0`; `npm.cmd run lint`: exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test`: `766` passed, `0` failed, `1` skipped (`FM_NOTO_TTC`), including `scripts/phase7-delete-toast-smart-import.test.mjs`.
   - `npm.cmd run build`: exit `0`; `npm.cmd run test:e2e`: `3/3` passed.
   - `cargo test --offline`: `241` passed, `0` failed, `6` pre-existing `#[ignore]`.
   - `npm.cmd run test:desktop` (live 2-launch WebView2 CDP E2E including `ImportOptionsDialog` Variable vs. Static prompt): `PASS`.
   - **Updated Production Release Artifacts:**
     - NSIS Setup EXE (`perMachine` elevated): `src-tauri\target\release\bundle\nsis\Fonths_1.0.0_x64-setup.exe` (`5,522,418` bytes, `2026-11-22 21:45:19`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Fonths_1.0.0_x64_en-US.msi` (`6,508,544` bytes, `2026-11-22 21:45:02`)
     - Executable: `src-tauri\target\release\fonths.exe` (`8,286,208` bytes, `2026-11-22 21:45:19`)

## 15. Emoji & Special Fonts Fix, Single-Click Reopen & Fast Restore, True Google Cancel, Watch vs Upload Differentiation & Smart Rules

1. **Emoji & Special Fonts Loading (`src-tauri/src/activate.rs`):**
   - Stopped marking `note_session_gdi_refused(family)` mid-pass inside `register_intact_family_pass` (where `VarsOnly` ran before `AllIntactFallback`); now clears both `session_gdi_refused` and `.settled-add-zero` whenever ANY pass (`VarsOnly`, `AllIntactFallback`, or `RespectPlan`) registers `>= 1` face, and also falls back from `RespectPlan` → `AllIntactFallback` for soft emoji families.
   - Upgraded `install_compat_pack` to fetch a static `Noto Emoji` Regular TTF (`wght@400`, pure `glyf`/`loca`, no `fvar`) via `UA_DESKTOP_TTF` first for `{slug}-compat-outline.ttf`, and upgraded `upgrade_legacy_cbdt_color_emoji_dir` so stale `.settled-add-zero` markers on `Noto Emoji` clear automatically when an intact compat-outline face is present. Verified upstream `NotoColorEmoji_WindowsCompatible.ttf` (`10.7 MB`) and `NotoEmoji[wght].ttf` (`1.98 MB`) both return `200 OK`.
2. **Eliminated Open-Twice Bug After Exit & Sped Up Exit/Reopen (`src-tauri/src/main.rs`, `src-tauri/src/activate.rs`):**
   - Replaced the fragile hidden PowerShell relauncher with native `--relaunch-after-pid <PID>` (`OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject`) spawning `current_exe()` directly so elevation is inherited cleanly and single-instance acquires on the first launch every time.
   - Updated `quit_gracefully()` to call `handle.exit(0)` after `session_end()` (with a 1.5s hard-exit backstop) so WebView2 cleanly releases `EBWebView-Admin\SingletonLock`.
   - Parallelized `winfont::unload_paths()` across up to 8 threads (`std::thread::scope`) and limited pass 2 to missed paths only (~8× faster exit on 2,000+ active fonts).
   - In `register_intact_family_pass`, fast-resume mode now skips redundant `sanitize_on_disk_for_gdi` file opens when a size-matched pre-staged `gdi-maps` copy already exists.
3. **Fixed Google Fonts Cancel So Downloads Actually Stop Immediately (`activate-toggle.tsx`, `os-activate.ts`, `activate.rs`):**
   - Added monotonic `getActivateCancelEpoch()` checked on every `activateInWaves()` tick and `activateSet()` confirm resolution, plus `myBatch !== batchId` guards after every `await` in `syncFontsOnSystem()` so cancelled multi-wave activations NEVER restart `start_google_downloads` or `startActivateOnDisk`.
   - Added `Bulk.active_queue` drained immediately in `cancel_google_downloads` and `drop_google_download_families`, added `cancel_requested()` checks before `pull_emoji_upstream_color_ttf` / `install_compat_pack` / `register_intact_family_detailed`, removed stray mid-download `register_path(&dest)` in `download_google_variable_ttfs`, and emitted `emit_progress_force(&app)` immediately on cancel.
4. **Watch Folders vs Upload Folder Differentiation & Upgraded Smart Import (`import-options.ts`, `import-options-dialog.tsx`, `watch-folder.ts`, `folder-tree.tsx`, `app-shell.tsx`, `types.ts`, `store.ts`):**
   - Clearly differentiated header **Upload Files / Upload Folder (Copies to `Documents\Fonths\Uploads`)** from sidebar **Watched Folders (Live-linked in place, auto-syncs)** with explicit mode banners inside `ImportOptionsDialog`.
   - Extended Smart Import (Variable vs. Static + kind/format checkboxes + live skip summary + `Remember these smart choices as my default`) to **Watch Folders** as well, saving `collection.importFilter` so future files dropped into watched Dropbox/Drive folders automatically follow those exact smart rules.
5. **Verification & Final Production Release Artifacts (`Fonths-1.0.0`):**
   - `npm.cmd run typecheck`: exit `0`; `npm.cmd run lint`: exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test`: `768` passed, `0` failed, `1` skipped (`FM_NOTO_TTC`).
   - `cargo test --offline`: `241` passed, `0` failed, `6` ignored.
   - `npm.cmd run build` & `npm.cmd run test:e2e`: `3/3` passed; `npm.cmd run test:desktop` (live 2-launch WebView2 CDP E2E): `PASS`.
   - **Final Production Release Artifacts:**
     - NSIS Setup EXE (`perMachine` elevated): `src-tauri\target\release\bundle\nsis\Fonths_1.0.0_x64-setup.exe` (`5,525,777` bytes, `2026-11-23 00:08:08`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Fonths_1.0.0_x64_en-US.msi` (`6,512,640` bytes, `2026-11-23 00:07:54`)
     - Executable: `src-tauri\target\release\fonths.exe` (`8,291,840` bytes, `2026-11-23 00:08:08`)

## 16. Zero-Pre-Download Activate All, Deterministic Noto Emoji / Noto Color Emoji Loading & FontBase-Class Performance

1. **Google Fonts `... → Activate all` Zero Pre-Download & Honest Cancel (`activate-toggle.tsx`, `activate-confirm-dialog.tsx`):**
   - Removed the legacy pre-confirm `wave0` enqueue that immediately started downloading/activating selected/favorites/visible/first-page/recent fonts *before* the user confirmed.
   - Changed `Esc` / `X` / outside-click / **Cancel** to resolve `"abort"` (starts **zero** downloads and **zero** activations); **Visible only / First page only** and **Activate all** now execute only after explicit confirmation.
   - Verified live in WebView2 E2E (`scripts/desktop-cdp.mjs`): opening `Google Fonts → ... → Activate all` and clicking `Cancel` keeps `google_download_progress.running === false` and `total === 0`.
2. **Noto Emoji & Noto Color Emoji Deterministic GDI Loading (`src-tauri/src/activate.rs`):**
   - Verified directly via Windows `AddFontResourceExW`: `NotoColorEmoji_WindowsCompatible.ttf` returns `Add(PUBLIC)=1` and `NotoEmoji[wght].ttf` returns `Add(PUBLIC)=3`.
   - Reordered `ttf_urls("noto-color-emoji")` so BOTH `NotoColorEmoji_WindowsCompatible.ttf` mirrors (`cdn.jsdelivr.net` + `raw.githubusercontent.com`) are tried before CBDT-only `NotoColorEmoji.ttf`, and made `pull_emoji_upstream_color_ttf` verify `has_table(glyf)` so CBDT-only responses never preempt a working `WindowsCompatible` mirror.
   - Reordered `ttf_urls("noto-emoji")` so official `google/fonts` `NotoEmoji[wght].ttf` mirrors come **first** before Fontsource mirrors.
   - Restricted `stamp_settle_after_add_zero` strictly to genuine `RegisterFailKind::AddReturnedZero` (never transient `GdiQuotaExceeded` / `StageCopyFailed` / `Unloading`), and cleared stale `.settled-add-zero` on `Noto Emoji` when a full-size (`>= 256 KB`) intact face or compat-outline is present.
3. **FontBase-Class Performance Upgrades (`src-tauri/src/activate.rs`):**
   - Raised `FACE_STREAM_SLOTS` / `DOWNLOAD_WORKERS` from `2` → `4` (doubling concurrent download throughput).
   - Added `O(1)` stamp-validated `scan_index_cached_dir_ready()` in `family_dir_is_ready_with_mode()` so unchanged complete folders skip deep `ttf_intact` SFNT opens even on default startup.
   - Added `register_detailed_with_fast(path, preverified_stage)` to skip redundant `has_valid_sfnt_magic` + `sanitize_on_disk_for_gdi` file opens when a size-matched pre-staged `gdi-maps` copy already exists.
4. **Verification & Final Production Release Artifacts (`Fonths-1.0.0`):**
   - `npm.cmd run typecheck`: exit `0`; `npm.cmd run lint`: exit `0` (`0` errors, `0` warnings).
   - `npm.cmd test`: `769` passed, `0` failed, `1` skipped (`FM_NOTO_TTC`).
   - `cargo test --offline`: `241` passed, `0` failed, `6` ignored.
   - `npm.cmd run build` & `npm.cmd run test:e2e`: `3/3` passed; `npm.cmd run test:desktop`: `PASS`.
   - **Final Production Release Artifacts:**
     - NSIS Setup EXE (`perMachine` elevated): `src-tauri\target\release\bundle\nsis\Fonths_1.0.0_x64-setup.exe` (`5,526,321` bytes, `2026-11-23 00:59:08`)
     - MSI Installer: `src-tauri\target\release\bundle\msi\Fonths_1.0.0_x64_en-US.msi` (`6,512,640` bytes, `2026-11-23 00:58:55`)
     - Executable: `src-tauri\target\release\fonths.exe` (`8,293,888` bytes, `2026-11-23 00:59:08`)
