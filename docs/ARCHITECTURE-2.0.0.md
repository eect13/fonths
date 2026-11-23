# Fonths 2.0.0 — Skia + Fontations Architecture & Engineering Record

**Version:** `2.0.0`  
**Stack:** Tauri v2 (Rust) · Skia (`skia-safe` with embedded FreeType + HarfBuzz) · Google Fontations (`read-fonts` + `memmap2`) · React 19 / TypeScript / Zustand / Tailwind CSS

---

## 1. Core Font Engine (Skia + Fontations)

Fonths 2.0.0 replaces legacy `ttf-parser` and Windows DirectWrite COM (`dwrite.rs` / `Win32_Graphics_DirectWrite`) with a cross-platform-ready, 100% OpenType-compliant native Rust engine while preserving all existing UI tabs (`Library`, `Playground`, `Duplicates`, `Glyphs`), right-side **Font Inspector**, collections, tags, and Tauri IPC contracts.

### 1.1 Zero-Copy OpenType Scanner & Parser (`src-tauri/src/scanner.rs`, `src-tauri/src/parse.rs`)
- **Zero-Copy Memory-Mapped Parsing:** Uses `memmap2::Mmap` + Google Fontations (`read-fonts`) plus pure-Rust `wuff` (`.woff2`) and `flate2` (`.woff`) decoding to inspect `.ttf`, `.otf`, `.ttc`, `.otc`, `.woff`, and `.woff2` directly on disk without installing them to the OS.
- **Metadata & OpenType Extraction:**
  - Names (`TYPOGRAPHIC_FAMILY_NAME` / `FAMILY_NAME`, `TYPOGRAPHIC_SUBFAMILY_NAME` / `SUBFAMILY_NAME`, `POSTSCRIPT_NAME`, designer, version, license).
  - Variable font `fvar` axes + `STAT` axis-value name overlay.
  - Deduplicated 4-char OpenType feature tags extracted from both `GSUB` and `GPOS` (`liga`, `dlig`, `ss01–ss20`, `smcp`, `onum`, `kern`, etc.).
  - `OS/2` metrics (`us_weight_class`, `us_width_class`, `sx_height`, `s_cap_height`, `panose_10`) and full Unicode `cmap` glyph tables.
  - Parallel read-only `%SYSTEMROOT%\Fonts` scanner (`list_system_fonts_blocking`), strictly excluding session staging (`gdi-maps`) and user library folders.

### 1.2 Dedicated Skia Render Worker & LRU Cache (`src-tauri/src/renderer.rs`)
- **Single-Owner Background OS Thread:** Because `skia_safe::Typeface` is `!Send`, Fonths spawns a dedicated background OS thread on startup owning `FontMgr::custom_empty().unwrap_or_else(FontMgr::new)` (embedded FreeType + HarfBuzz) and an `LruCache<String, skia_safe::Typeface>` (capacity `100`).
- **Async Raw-Pixel IPC (`render_font_preview`):**
  - Bridges Tauri's async executor to the Skia thread via `std::sync::mpsc` + `tokio::sync::oneshot`.
  - Applies variable font axes via `FontArguments` (`VariationPosition`) and OpenType features via HarfBuzz (`TextStyle::add_font_feature`).
  - Renders onto `surfaces::raster_n32_premul` and reads unpremultiplied **RGBA8888** pixel bytes returned directly via `tauri::ipc::Response` (zero PNG encoding/decoding overhead).
- **Inspector Integration (`src/components/font-studio/skia-font-canvas.tsx`, `font-inspector.tsx`):**
  - Embedded directly inside the existing right-side **Font Inspector**, driven live by the Inspector's existing variable axis sliders (`AxisSliders`) and OpenType feature toggles (`OT_TOGGLES`), with automatic fallback for non-downloaded Google Fonts, Arabic two-line previews, and color emoji.

### 1.3 Temporary System-Wide Windows Activation (`src-tauri/src/activator.rs`, `src-tauri/src/activate.rs`)
- **Session-Only Activation:** Uses `AddFontResourceExW` (`FONT_RESOURCE_CHARACTERISTICS(0)`) + `SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE)` and session-only staging under `%LOCALAPPDATA%\Fonths\gdi-maps`, leaving `C:\Windows\Fonts` completely untouched.
- **Automatic Cleanup on Exit:** `ActivatorState` tracks active paths in a `Mutex<HashSet<String>>`, and `RunEvent::Exit` in `src-tauri/src/lib.rs` cleanly unloads every activated font via `RemoveFontResourceExW` + `WM_FONTCHANGE`.
- **Cross-Platform Stubs:** Non-Windows builds compile cleanly with `#[cfg(not(windows))]` stubs.

---

## 2. Key Workflows & Performance

- **Instant Startup Activation State & Fast Session Restore:** Hydrates saved active state on frame 0 (`< 10 ms`), flushes active families on window close (`beforeunload` / `pagehide`), and re-registers staged `gdi-maps` faces in parallel with strictly bounded progress (`done ≤ total ≤ library count`).
- **In-Place Watched Folders & Smart Import Detection:** Watched folders keep font files strictly in place on disk (no internal `Uploads` copy), run smart Variable/Static and Google/Library duplicate collision detection, and cleanly remove their linked typefaces when un-watched without converting into a Collection.
- **Atomic Bulk Google Fonts Activation:** Single prefer-ordered activation queue with `.web-build` marker hygiene before stamping `.complete` and immediate in-flight cancellation support.
- **Playground Pair Locking & Duplicates Auto-Hide:** Lockable Heading/Body `Suggest pair` workflow in Playground and one-switch duplicate auto-hide + deactivation in Duplicates.
