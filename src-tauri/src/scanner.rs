//! Zero-copy OpenType metadata, feature, and validation engine powered by Google Fontations
//! (`read-fonts`) + `memmap2` (plus pure-Rust `wuff` decoding for `.woff2` containers).
//!
//! Replaces legacy `ttf-parser` and Windows DirectWrite COM inspection with a single
//! cross-platform, zero-copy Fontations pipeline.

use memmap2::Mmap;
use read_fonts::{types::NameId, FileRef, FontRef, TableProvider};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariableAxis {
    pub tag: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FontMetadata {
    pub path: String,
    pub family_name: String,
    pub subfamily_name: String,
    pub postscript_name: String,
    pub is_variable: bool,
    pub axes: Vec<VariableAxis>,
    pub opentype_features: Vec<String>,
}

/// Engine diagnostics payload (keeps existing frontend `DWriteEngineInfo` TypeScript schema intact).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub engine: String,
    pub available: bool,
    pub activation_mode: String,
    pub system_fonts_read_only: bool,
    pub system_font_count: usize,
    pub platform: String,
    pub elevated: bool,
}

/// Per-file font analysis payload (keeps existing frontend `DWriteInspectResult` schema intact).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInspectResult {
    pub supported: bool,
    pub face_count: u32,
    pub file_type: String,
    pub face_type: String,
    pub is_variable: bool,
    pub typographic_family: Option<String>,
    pub win32_family: Option<String>,
    pub full_name: Option<String>,
    pub weight: u16,
    pub italic: bool,
    pub hresult_code: Option<i32>,
    pub error_reason: Option<String>,
}

fn normalize_win_path_str(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    s
}

/// Resolve the read-only OS system fonts directory (`C:\Windows\Fonts`).
pub fn windows_system_fonts_dir() -> PathBuf {
    let windir = std::env::var("SYSTEMROOT")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(windir).join("Fonts")
}

/// Strict guard: true ONLY for font files physically located inside `%SYSTEMROOT%\Fonts`.
/// Explicitly rejects `gdi-maps`, `Documents\Fonths`, and `Documents\Font Manager`.
pub fn is_system_windows_fonts_dir(path: &Path) -> bool {
    let norm = normalize_win_path_str(path);
    if norm.contains(r"\gdi-maps\")
        || norm.ends_with(r"\gdi-maps")
        || norm.contains(r"\documents\fonths\")
        || norm.ends_with(r"\documents\fonths")
        || norm.contains(r"\documents\font manager\")
        || norm.ends_with(r"\documents\font manager")
        || norm.contains(r"\microsoft\windows\fonts\")
    {
        return false;
    }
    let sys = normalize_win_path_str(&windows_system_fonts_dir());
    norm == sys || norm.starts_with(&format!(r"{sys}\"))
}

fn is_supported_font_ext(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "woff2")
}

fn collect_font_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, out);
        } else if path.is_file() && is_supported_font_ext(&path) {
            out.push(path);
        }
    }
}

/// Rank `name` table platform/encoding/language records so English Windows/Unicode
/// names win cleanly over legacy encodings.
pub fn name_record_priority(platform_id: u16, encoding_id: u16, language_id: u16) -> u8 {
    match (platform_id, encoding_id, language_id) {
        (3, 1 | 10, 0x0409) => 0, // Windows Unicode BMP/Full, en-US
        (0, _, _) => 1,           // Unicode platform
        (3, 1 | 10, _) => 2,      // Windows Unicode, other language
        (1, 0, 0) => 3,           // Mac Roman, English
        (1, _, _) => 4,           // Mac other
        _ => 5,
    }
}

pub fn read_name_string(font: &FontRef<'_>, target: NameId) -> Option<String> {
    let name_table = font.name().ok()?;
    let string_data = name_table.string_data();
    let mut best: Option<(u8, String)> = None;

    for record in name_table.name_record() {
        if record.name_id() != target {
            continue;
        }
        let Ok(decoded) = record.string(string_data) else {
            continue;
        };
        let value = decoded.to_string().trim().to_string();
        if value.is_empty() {
            continue;
        }
        let prio = name_record_priority(
            record.platform_id(),
            record.encoding_id(),
            record.language_id(),
        );
        if best.as_ref().map(|(p, _)| prio < *p).unwrap_or(true) {
            best = Some((prio, value));
            if prio == 0 {
                break;
            }
        }
    }

    best.map(|(_, s)| s)
}

fn extract_font_metadata(font: &FontRef<'_>, path_str: &str, fallback_stem: &str) -> FontMetadata {
    let family_name = read_name_string(font, NameId::TYPOGRAPHIC_FAMILY_NAME)
        .or_else(|| read_name_string(font, NameId::FAMILY_NAME))
        .unwrap_or_else(|| fallback_stem.to_string());

    let subfamily_name = read_name_string(font, NameId::TYPOGRAPHIC_SUBFAMILY_NAME)
        .or_else(|| read_name_string(font, NameId::SUBFAMILY_NAME))
        .unwrap_or_else(|| "Regular".to_string());

    let postscript_name = read_name_string(font, NameId::POSTSCRIPT_NAME)
        .unwrap_or_else(|| family_name.replace(' ', "-"));

    let mut axes = Vec::new();
    if let Ok(fvar) = font.fvar() {
        if let Ok(axis_records) = fvar.axes() {
            for axis in axis_records {
                axes.push(VariableAxis {
                    tag: axis.axis_tag().to_string(),
                    min: axis.min_value().to_f32(),
                    default: axis.default_value().to_f32(),
                    max: axis.max_value().to_f32(),
                });
            }
        }
    }
    let is_variable = !axes.is_empty();

    let mut features = BTreeSet::new();
    if let Ok(gsub) = font.gsub() {
        if let Ok(flist) = gsub.feature_list() {
            for rec in flist.feature_records() {
                features.insert(rec.feature_tag().to_string());
            }
        }
    }
    if let Ok(gpos) = font.gpos() {
        if let Ok(flist) = gpos.feature_list() {
            for rec in flist.feature_records() {
                features.insert(rec.feature_tag().to_string());
            }
        }
    }

    FontMetadata {
        path: path_str.to_string(),
        family_name,
        subfamily_name,
        postscript_name,
        is_variable,
        axes,
        opentype_features: features.into_iter().collect(),
    }
}

fn parse_bytes(bytes: &[u8], path_str: &str, fallback_stem: &str) -> Vec<FontMetadata> {
    let Ok(file_ref) = FileRef::new(bytes) else {
        return Vec::new();
    };
    match file_ref {
        FileRef::Font(font) => vec![extract_font_metadata(&font, path_str, fallback_stem)],
        FileRef::Collection(collection) => collection
            .iter()
            .filter_map(|face| face.ok())
            .map(|font| extract_font_metadata(&font, path_str, fallback_stem))
            .collect(),
    }
}

/// Zero-copy inspect a single font file (`.ttf`, `.otf`, `.ttc`, `.woff2`) using `memmap2` + `read-fonts`.
pub fn scan_font_file(path: &Path) -> Vec<FontMetadata> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    if meta.len() < 12 {
        return Vec::new();
    }
    // SAFETY: Read-only memory map of an existing font file; immutable reads only.
    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return Vec::new();
    };
    let slice: &[u8] = &mmap;
    let path_str = path.to_string_lossy().into_owned();
    let fallback_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown");

    if slice.starts_with(b"wOF2") {
        if let Ok(decoded) = wuff::decompress_woff2(slice) {
            return parse_bytes(&decoded, &path_str, fallback_stem);
        }
        return Vec::new();
    }

    parse_bytes(slice, &path_str, fallback_stem)
}

/// Zero-copy check if a font file on disk has an OpenType `fvar` variable table.
pub fn file_has_variations(path: &Path) -> Option<bool> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() < 12 {
        return Some(false);
    }
    let mmap = unsafe { Mmap::map(&file) }.ok()?;
    let slice: &[u8] = &mmap;
    let decoded_owned;
    let data = if slice.starts_with(b"wOF2") {
        decoded_owned = wuff::decompress_woff2(slice).ok()?;
        decoded_owned.as_slice()
    } else {
        slice
    };
    let file_ref = FileRef::new(data).ok()?;
    let font = match file_ref {
        FileRef::Font(f) => f,
        FileRef::Collection(c) => c.get(0).ok()?,
    };
    // Require a valid OpenType `head` table so synthetic/truncated SFNT headers
    // return `None` instead of being misclassified as valid static fonts.
    if font.head().is_err() {
        return None;
    }
    let has_axes = font
        .fvar()
        .ok()
        .and_then(|fv| fv.axes().ok())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    Some(has_axes)
}

/// Return all `fvar` axis 4-char tags from raw font bytes (or `None` if unparseable).
pub fn variation_axis_tags_from_bytes(bytes: &[u8]) -> Option<Vec<String>> {
    let file_ref = FileRef::new(bytes).ok()?;
    let font = match file_ref {
        FileRef::Font(f) => f,
        FileRef::Collection(c) => c.get(0).ok()?,
    };
    if font.head().is_err() {
        return None;
    }
    let fvar = match font.fvar() {
        Ok(f) => f,
        Err(_) => return Some(Vec::new()),
    };
    let axes = fvar.axes().ok()?;
    Some(axes.iter().map(|a| a.axis_tag().to_string()).collect())
}

/// Read Win32/typographic family name from an on-disk font file via zero-copy `memmap2` + `read-fonts`.
pub fn win32_family_name(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() < 12 {
        return None;
    }
    let mmap = unsafe { Mmap::map(&file) }.ok()?;
    let file_ref = FileRef::new(&mmap).ok()?;
    let font = match file_ref {
        FileRef::Font(f) => f,
        FileRef::Collection(c) => c.get(0).ok()?,
    };
    read_name_string(&font, NameId::FAMILY_NAME)
        .or_else(|| read_name_string(&font, NameId::TYPOGRAPHIC_FAMILY_NAME))
}

/// Validate and inspect an on-disk font file via `read-fonts` + `memmap2` (zero DirectWrite).
pub fn analyze_font_file(path: &Path) -> EngineInspectResult {
    let fail = |reason: String| EngineInspectResult {
        supported: false,
        face_count: 0,
        file_type: "unknown".to_string(),
        face_type: "unknown".to_string(),
        is_variable: false,
        typographic_family: None,
        win32_family: None,
        full_name: None,
        weight: 400,
        italic: false,
        hresult_code: None,
        error_reason: Some(reason),
    };

    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return fail(format!("Failed to open font file: {e}")),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len < 12 {
        return fail("Font file is truncated (< 12 bytes)".to_string());
    }
    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => m,
        Err(e) => return fail(format!("Failed to mmap font file: {e}")),
    };
    let slice: &[u8] = &mmap;
    let decoded_owned;
    let (data, is_woff2) = if slice.starts_with(b"wOF2") {
        match wuff::decompress_woff2(slice) {
            Ok(v) => {
                decoded_owned = v;
                (decoded_owned.as_slice(), true)
            }
            Err(e) => return fail(format!("Invalid WOFF2 container: {e:?}")),
        }
    } else {
        (slice, false)
    };

    let file_ref = match FileRef::new(data) {
        Ok(r) => r,
        Err(e) => return fail(format!("Invalid OpenType SFNT/TTC table directory: {e}")),
    };

    let (face_count, font, is_ttc) = match file_ref {
        FileRef::Font(f) => (1u32, f, false),
        FileRef::Collection(c) => {
            let count = c.len();
            match c.get(0) {
                Ok(f) => (count, f, true),
                Err(e) => return fail(format!("Corrupt font collection face 0: {e}")),
            }
        }
    };

    if let Err(e) = font.head() {
        return fail(format!("Missing or invalid OpenType head table: {e}"));
    }

    let has_cff = font.table_data(read_fonts::types::Tag::new(b"CFF ")).is_some()
        || font.table_data(read_fonts::types::Tag::new(b"CFF2")).is_some();
    let file_type = if is_woff2 {
        "woff2"
    } else if is_ttc {
        "truetype-collection"
    } else if has_cff {
        "cff"
    } else {
        "truetype"
    }
    .to_string();
    let face_type = if has_cff { "cff" } else { "truetype" }.to_string();

    let is_variable = font
        .fvar()
        .ok()
        .and_then(|fv| fv.axes().ok())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    let typographic_family = read_name_string(&font, NameId::TYPOGRAPHIC_FAMILY_NAME);
    let win32_family = read_name_string(&font, NameId::FAMILY_NAME);
    let full_name = read_name_string(&font, NameId::FULL_NAME);

    let weight = font
        .os2()
        .ok()
        .map(|o| o.us_weight_class())
        .filter(|&w| w > 0)
        .unwrap_or(400);

    let italic = font
        .os2()
        .ok()
        .map(|o| (o.fs_selection().bits() & 0x01) != 0)
        .or_else(|| font.head().ok().map(|h| (h.mac_style().bits() & 0x02) != 0))
        .unwrap_or(false);

    EngineInspectResult {
        supported: true,
        face_count,
        file_type,
        face_type,
        is_variable,
        typographic_family,
        win32_family,
        full_name,
        weight,
        italic,
        hresult_code: None,
        error_reason: None,
    }
}

/// Engine metadata command (preserves `dwrite_engine_info` IPC name for frontend compatibility,
/// reporting the active Skia FreeType + HarfBuzz + Fontations engine).
#[tauri::command]
pub async fn dwrite_engine_info() -> Result<EngineInfo, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let system_font_count = crate::parse::list_system_fonts_blocking()
            .map(|r| r.len())
            .unwrap_or(0);
        Ok(EngineInfo {
            engine: "Skia (FreeType + HarfBuzz) + Fontations".to_string(),
            available: true,
            activation_mode: "session-temporary".to_string(),
            system_fonts_read_only: true,
            system_font_count,
            platform: std::env::consts::OS.to_string(),
            elevated: crate::is_elevated(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// File inspection command (preserves `dwrite_inspect_font` IPC name, powered by `read-fonts` + `memmap2`).
#[tauri::command]
pub async fn dwrite_inspect_font(path: String) -> Result<EngineInspectResult, String> {
    tauri::async_runtime::spawn_blocking(move || analyze_font_file(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())
}

/// Recursively scan a directory for `.ttf`, `.otf`, `.ttc`, and `.woff2` files without
/// installing them to the OS, returning OpenType metadata, variable axes, and GSUB/GPOS features.
#[tauri::command]
pub fn scan_font_directory(dir_path: String) -> Result<Vec<FontMetadata>, String> {
    let trimmed = dir_path.trim();
    if trimmed.is_empty() {
        return Err("Directory path is empty".to_string());
    }
    let root = PathBuf::from(trimmed);
    if !root.exists() {
        return Err(format!("Directory does not exist: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("Path is not a directory: {}", root.display()));
    }

    let mut files = Vec::new();
    collect_font_files(&root, &mut files);
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(files.len());

    let mut results: Vec<FontMetadata> = if workers <= 1 || files.len() < 8 {
        files.iter().flat_map(|p| scan_font_file(p)).collect()
    } else {
        let chunk_size = files.len().div_ceil(workers);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for chunk in files.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    let mut local = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        local.extend(scan_font_file(path));
                    }
                    local
                }));
            }
            let mut merged = Vec::with_capacity(files.len());
            for handle in handles {
                if let Ok(part) = handle.join() {
                    merged.extend(part);
                }
            }
            merged
        })
    };

    results.sort_by(|a, b| {
        a.family_name
            .to_ascii_lowercase()
            .cmp(&b.family_name.to_ascii_lowercase())
            .then_with(|| {
                a.subfamily_name
                    .to_ascii_lowercase()
                    .cmp(&b.subfamily_name.to_ascii_lowercase())
            })
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_real_variable_font_fixtures_with_axes_and_opentype_features() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures");
        let scanned = scan_font_directory(fixtures.to_string_lossy().into_owned())
            .expect("scan_font_directory should succeed on fixtures");
        assert!(
            scanned.len() >= 2,
            "expected at least 2 fixtures, got {}",
            scanned.len()
        );

        let inter = scanned
            .iter()
            .find(|f| f.family_name.eq_ignore_ascii_case("Inter"))
            .expect("Inter fixture must be parsed");
        assert!(inter.is_variable, "Inter fixture should be variable");
        assert!(
            inter.axes.iter().any(|a| a.tag == "wght"),
            "Inter must expose wght axis"
        );
        assert!(
            inter.axes.iter().any(|a| a.tag == "opsz"),
            "Inter must expose opsz axis"
        );
        assert!(
            !inter.opentype_features.is_empty(),
            "Inter must expose GSUB/GPOS OpenType feature tags"
        );

        let figtree = scanned
            .iter()
            .find(|f| f.family_name.eq_ignore_ascii_case("Figtree"))
            .expect("Figtree fixture must be parsed");
        assert!(figtree.is_variable, "Figtree fixture should be variable");
        assert!(figtree.axes.iter().any(|a| a.tag == "wght"));
    }

    #[test]
    fn system_font_dir_guard_rejects_session_paths() {
        assert!(is_system_windows_fonts_dir(Path::new(r"C:\Windows\Fonts\arial.ttf")));
        assert!(!is_system_windows_fonts_dir(Path::new(r"C:\Users\Eric\AppData\Local\Fonths\gdi-maps\a.ttf")));
        assert!(!is_system_windows_fonts_dir(Path::new(r"C:\Users\Eric\Documents\Fonths\Providers\Google\Inter\Inter.ttf")));
        assert!(!is_system_windows_fonts_dir(Path::new(r"C:\Users\Eric\Documents\Font Manager\Inter\Inter.ttf")));
    }

    #[test]
    fn analyze_font_file_rejects_corrupt_and_validates_vf_fixtures() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures");
        let res = analyze_font_file(&fixtures.join("Inter-opsz-wght.ttf"));
        assert!(res.supported);
        assert!(res.is_variable);
        assert_eq!(res.face_count, 1);

        let corrupt = std::env::temp_dir().join("fonths-corrupt-test.ttf");
        let _ = fs::write(&corrupt, b"NOT_A_FONT_FILE_123456");
        let bad = analyze_font_file(&corrupt);
        assert!(!bad.supported);
        assert!(bad.error_reason.is_some());
        let _ = fs::remove_file(&corrupt);
    }
}
