//! Zero-copy OpenType metadata & feature scanner powered by Google Fontations (`read-fonts`)
//! and `memmap2` (plus pure-Rust `wuff` decoding for `.woff2` containers).

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
fn name_record_priority(platform_id: u16, encoding_id: u16, language_id: u16) -> u8 {
    match (platform_id, encoding_id, language_id) {
        (3, 1 | 10, 0x0409) => 0, // Windows Unicode BMP/Full, en-US
        (0, _, _) => 1,           // Unicode platform
        (3, 1 | 10, _) => 2,      // Windows Unicode, other language
        (1, 0, 0) => 3,           // Mac Roman, English
        (1, _, _) => 4,           // Mac other
        _ => 5,
    }
}

fn read_name_string(font: &FontRef<'_>, target: NameId) -> Option<String> {
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
    // SAFETY: Read-only memory map of an existing font file; we only read immutable bytes.
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
}
