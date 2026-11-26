//! Background `rayon` parallel metadata pre-indexer for downloaded font libraries.
//!
//! Scans `Documents\Fonths` (`Providers\Google` and `Uploads`), caches extracted OS/2 + GSUB + `fvar`
//! summaries per primary face file `(path, mtime, size)` in `%LOCALAPPDATA%\Fonths\meta-index-v1.json`,
//! and hydrates SuperSearch filters (`xh:`, `contrast:`, `ot:`, `axis:`) without touching glyph outlines.

use crate::activate;
use crate::parse::{
    parse_layout_for_path_cached, path_mtime_size_key, FontAxisOut, FontLayout, FontMetricsOut,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const META_INDEX_FILE: &str = "meta-index-v1.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFamilyMeta {
    pub family: String,
    #[serde(rename = "xHeightRatio", skip_serializing_if = "Option::is_none")]
    pub x_height_ratio: Option<f64>,
    #[serde(rename = "contrastRatio", skip_serializing_if = "Option::is_none")]
    pub contrast_ratio: Option<f64>,
    #[serde(rename = "widthClass")]
    pub width_class: u16,
    #[serde(rename = "isFixedPitch", skip_serializing_if = "Option::is_none")]
    pub is_fixed_pitch: Option<bool>,
    #[serde(rename = "glyphCount")]
    pub glyph_count: u16,
    #[serde(rename = "otFeatures")]
    pub ot_features: Vec<String>,
    pub axes: Vec<FontAxisOut>,
    pub variable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<FontMetricsOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetaCacheEntry {
    mtime: u64,
    size: u64,
    row: IndexedFamilyMeta,
}

fn meta_index_cache_path() -> Option<PathBuf> {
    activate::fm_local_root().map(|dir| dir.join(META_INDEX_FILE))
}

fn load_meta_cache(path: Option<&Path>) -> HashMap<String, MetaCacheEntry> {
    let Some(p) = path else {
        return HashMap::new();
    };
    fs::read(p)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_meta_cache(path: Option<&Path>, map: &HashMap<String, MetaCacheEntry>) {
    let Some(p) = path else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec(map) {
        let tmp = p.with_extension("json.tmp");
        if fs::write(&tmp, &json).is_ok() {
            let _ = fs::rename(&tmp, p);
        } else {
            let _ = fs::write(p, &json);
        }
    }
}

fn collect_family_dirs(docs_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let push_children = |container: &Path, dirs: &mut Vec<PathBuf>, seen: &mut HashSet<String>| {
        let Ok(rd) = fs::read_dir(container) else {
            return;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let norm = p.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
            if seen.insert(norm) {
                dirs.push(p);
            }
        }
    };

    push_children(&activate::google_container_path(docs_root), &mut dirs, &mut seen);
    push_children(&docs_root.join(activate::UPLOADS_CONTAINER), &mut dirs, &mut seen);

    if let Ok(rd) = fs::read_dir(docs_root) {
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.')
                || name.eq_ignore_ascii_case("Providers")
                || name.eq_ignore_ascii_case(activate::GOOGLE_CONTAINER)
                || name.eq_ignore_ascii_case(activate::UPLOADS_CONTAINER)
                || name.eq_ignore_ascii_case("Activated")
                || name.eq_ignore_ascii_case("Library")
            {
                continue;
            }
            let norm = p.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
            if seen.insert(norm) {
                dirs.push(p);
            }
        }
    }
    dirs
}

fn pick_primary_face_file(dir: &Path) -> Option<PathBuf> {
    let mut files = Vec::new();
    activate::walk_font_files(dir, &mut files);
    files.into_iter().max_by_key(|p| {
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_vf = u8::from(name.contains("variable") || (name.contains('[') && name.contains(']')));
        let is_ttf = u8::from(name.ends_with(".ttf") || name.ends_with(".otf"));
        let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        (is_vf, is_ttf, size)
    })
}

fn panose_contrast_ratio(panose: Option<&[u8]>) -> Option<f64> {
    let p = panose?;
    if p.len() < 5 || p[0] != 2 || p[4] <= 1 {
        return None;
    }
    Some(((f64::from(p[4]) - 2.0) / 7.0).clamp(0.0, 1.0))
}

fn meta_from_layout(family: String, layout: FontLayout) -> IndexedFamilyMeta {
    let (x_height_ratio, contrast_ratio, width_class, is_fixed_pitch) =
        if let Some(ref m) = layout.metrics {
            let upem = if m.upem > 0 { f64::from(m.upem) } else { 1000.0 };
            let xh = m
                .x_height
                .filter(|&v| v > 0)
                .map(|v| (f64::from(v) / upem).clamp(0.0, 1.0));
            let ctr = panose_contrast_ratio(m.panose.as_deref());
            (xh, ctr, m.width_class.clamp(1, 9), m.is_fixed_pitch)
        } else {
            (None, None, 5, None)
        };
    IndexedFamilyMeta {
        family,
        x_height_ratio,
        contrast_ratio,
        width_class,
        is_fixed_pitch,
        glyph_count: layout.glyph_count,
        ot_features: layout.ot_features,
        axes: layout.axes,
        variable: layout.variable,
        metrics: layout.metrics,
    }
}

pub fn index_downloaded_metadata_in(
    docs_root: &Path,
    extra_paths: Option<&[(String, String)]>,
    cache_path: Option<&Path>,
) -> Vec<IndexedFamilyMeta> {
    let dirs = if docs_root.is_dir() {
        collect_family_dirs(docs_root)
    } else {
        Vec::new()
    };
    let extras = extra_paths.unwrap_or(&[]);
    if dirs.is_empty() && extras.is_empty() {
        return Vec::new();
    }
    let mut cache = load_meta_cache(cache_path);
    let mut hits: Vec<IndexedFamilyMeta> = Vec::with_capacity(dirs.len() + extras.len());
    let mut misses: Vec<(String, String, u64, u64, PathBuf)> = Vec::new();
    let mut live_keys: HashSet<String> = HashSet::with_capacity(dirs.len() + extras.len());
    let mut dirty = false;

    for dir in dirs {
        let family = activate::dir_family_name(&dir);
        if family.trim().is_empty() {
            continue;
        }
        let Some(face_path) = pick_primary_face_file(&dir) else {
            continue;
        };
        let Some((key, mtime, size)) = path_mtime_size_key(&face_path) else {
            continue;
        };
        live_keys.insert(key.clone());
        if let Some(hit) = cache.get(&key)
            && hit.mtime == mtime
            && hit.size == size
            && hit.row.family.eq_ignore_ascii_case(&family)
        {
            hits.push(hit.row.clone());
            continue;
        }
        misses.push((family, key, mtime, size, face_path));
    }

    for (raw_family, raw_path) in extras {
        let family = raw_family.trim().to_string();
        if family.is_empty() || raw_path.trim().is_empty() {
            continue;
        }
        let face_path = PathBuf::from(raw_path);
        if !face_path.is_file() {
            continue;
        }
        let Some((key, mtime, size)) = path_mtime_size_key(&face_path) else {
            continue;
        };
        if !live_keys.insert(key.clone()) {
            continue;
        }
        if let Some(hit) = cache.get(&key)
            && hit.mtime == mtime
            && hit.size == size
            && hit.row.family.eq_ignore_ascii_case(&family)
        {
            hits.push(hit.row.clone());
            continue;
        }
        misses.push((family, key, mtime, size, face_path));
    }

    if cache.len() > live_keys.len() {
        cache.retain(|k, _| live_keys.contains(k));
        dirty = true;
    }

    if !misses.is_empty() {
        dirty = true;
        let parsed: Vec<(String, u64, u64, IndexedFamilyMeta)> = misses
            .into_par_iter()
            .filter_map(|(family, key, mtime, size, path)| {
                let layout = parse_layout_for_path_cached(&path).ok()?;
                let row = meta_from_layout(family, layout);
                Some((key, mtime, size, row))
            })
            .collect();
        for (key, mtime, size, row) in parsed {
            hits.push(row.clone());
            cache.insert(key, MetaCacheEntry { mtime, size, row });
        }
    }

    if dirty {
        save_meta_cache(cache_path, &cache);
    }
    hits.sort_by_key(|a| a.family.to_ascii_lowercase());
    hits
}

/// Parallel `rayon` pre-indexer of OS/2, GSUB, and `fvar` metadata across downloaded libraries
/// and optional watched-folder font paths.
#[tauri::command]
pub async fn index_downloaded_metadata(
    app: AppHandle,
    extra_paths: Option<Vec<(String, String)>>,
) -> Result<Vec<IndexedFamilyMeta>, String> {
    let root = activate::documents_root(&app)?;
    let cache = meta_index_cache_path();
    tauri::async_runtime::spawn_blocking(move || {
        index_downloaded_metadata_in(&root, extra_paths.as_deref(), cache.as_deref())
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_index_extracts_real_fixtures_hits_cache_and_invalidates_on_change() {
        let tmp = std::env::temp_dir().join(format!("fm-meta-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let docs = tmp.join("Documents").join("Fonths");
        let cache = tmp.join("LocalAppData").join("meta-index-v1.json");
        let fam_dir = docs.join("Providers").join("Google").join("Figtree");
        let up_dir = docs.join("Uploads").join("Inter Test");
        fs::create_dir_all(&fam_dir).unwrap();
        fs::create_dir_all(&up_dir).unwrap();

        let fig_path = fam_dir.join("Figtree[wght].ttf");
        let inter_path = up_dir.join("Inter-VariableFont.ttf");
        fs::write(&fig_path, include_bytes!("../../tests/fixtures/Figtree-wght.ttf")).unwrap();
        fs::write(&inter_path, include_bytes!("../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();

        let rows1 = index_downloaded_metadata_in(&docs, None, Some(&cache));
        assert_eq!(rows1.len(), 2);
        assert!(cache.is_file(), "cache file must be written on first run");
        let fig = rows1.iter().find(|r| r.family == "Figtree").expect("Figtree indexed");
        assert!(fig.variable);
        assert!(fig.axes.iter().any(|a| a.tag == "wght"));
        assert!(fig.glyph_count > 0);
        let inter = rows1.iter().find(|r| r.family == "Inter Test").expect("Upload indexed");
        assert!(inter.axes.iter().any(|a| a.tag == "opsz"));

        // Second pass hits on-disk cache cleanly even when called again.
        let rows2 = index_downloaded_metadata_in(&docs, None, Some(&cache));
        assert_eq!(rows2.len(), 2);
        assert_eq!(rows2[0].glyph_count, rows1[0].glyph_count);

        // Changing file bytes updates size -> cache entry invalidates and re-parses new face.
        let prev_glyphs = fig.glyph_count;
        fs::write(&fig_path, include_bytes!("../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();
        let rows3 = index_downloaded_metadata_in(&docs, None, Some(&cache));
        let fig_after = rows3.iter().find(|r| r.family == "Figtree").unwrap();
        assert_ne!(fig_after.glyph_count, prev_glyphs);

        // External watched-folder face path is also indexed & cached alongside Documents.
        let watched_path = tmp.join("Watched").join("FigtreeWatched.ttf");
        fs::create_dir_all(watched_path.parent().unwrap()).unwrap();
        fs::write(&watched_path, include_bytes!("../../tests/fixtures/Figtree-wght.ttf")).unwrap();
        let extra = vec![(
            "Watched Figtree".to_string(),
            watched_path.to_string_lossy().into_owned(),
        )];
        let rows4 = index_downloaded_metadata_in(&docs, Some(&extra), Some(&cache));
        assert_eq!(rows4.len(), 3);
        assert!(rows4.iter().any(|r| r.family == "Watched Figtree" && r.variable));

        let _ = fs::remove_dir_all(&tmp);
    }
}
