//! Modular Google Fonts Live Catalog & CDN Revision Tracker.
//!
//! Extracted from `activate.rs` so live Google catalog parsing, `lastModified`
//! revision caching (`google-cdn-revisions.json`), `.cdn-revision` markers, and
//! dynamic `live_google_directory()` membership live in a clean, testable module.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

/// Shared HTTP/2 + keep-alive client pool for official Google catalog/CDN requests.
pub fn shared_blocking_client() -> &'static reqwest::blocking::Client {
    static CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(25))
            .pool_max_idle_per_host(8)
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent(crate::gfsource::POLITE_UA)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    });
    &CLIENT
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GoogleCdnMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub checked_at: u64,
}

pub fn cdn_meta_path(local_app_data: Option<PathBuf>) -> Option<PathBuf> {
    local_app_data.map(|base| base.join("Fonths").join("google-cdn-meta.json"))
}

pub fn load_cdn_meta(local_app_data: Option<PathBuf>) -> GoogleCdnMeta {
    let Some(path) = cdn_meta_path(local_app_data) else {
        return GoogleCdnMeta::default();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return GoogleCdnMeta::default();
    };
    serde_json::from_str::<GoogleCdnMeta>(&text).unwrap_or_default()
}

pub fn save_cdn_meta(local_app_data: Option<PathBuf>, meta: &GoogleCdnMeta) {
    let Some(path) = cdn_meta_path(local_app_data) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(meta) {
        let _ = fs::write(path, json);
    }
}

pub struct ConditionalCatalogFetch {
    pub unchanged: bool,
    pub rows: Vec<serde_json::Value>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

fn unix_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Conditional GET against `https://fonts.google.com/metadata/fonts` using `If-None-Match`
/// and `If-Modified-Since` so unchanged startups cost one tiny 304 roundtrip.
pub fn fetch_live_catalog_conditional(
    local_app_data: Option<PathBuf>,
    force: bool,
) -> Result<ConditionalCatalogFetch, String> {
    let cached_revs = load_cached_revisions(local_app_data.clone());
    let prev_meta = if force || cached_revs.is_empty() {
        GoogleCdnMeta::default()
    } else {
        load_cdn_meta(local_app_data.clone())
    };
    let mut req = shared_blocking_client()
        .get("https://fonts.google.com/metadata/fonts")
        .header("Cache-Control", "no-cache");
    if !force && !cached_revs.is_empty() {
        if let Some(etag) = prev_meta.etag.as_deref().filter(|s| !s.is_empty()) {
            req = req.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(lm) = prev_meta.last_modified.as_deref().filter(|s| !s.is_empty()) {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
        }
    }
    let res = req.send().map_err(|e| e.to_string())?;
    let now = unix_secs_now();
    if res.status() == reqwest::StatusCode::NOT_MODIFIED {
        let updated = GoogleCdnMeta {
            etag: res
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .or(prev_meta.etag),
            last_modified: res
                .headers()
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .or(prev_meta.last_modified),
            checked_at: now,
        };
        save_cdn_meta(local_app_data, &updated);
        register_live_families(cached_revs.keys(), None);
        return Ok(ConditionalCatalogFetch {
            unchanged: true,
            rows: Vec::new(),
            etag: updated.etag,
            last_modified: updated.last_modified,
        });
    }
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status().as_u16()));
    }
    let etag = res
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let last_modified = res
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let text = res.text().map_err(|e| e.to_string())?;
    let (rows, revisions) = parse_live_catalog_json(&text)?;
    register_live_families(rows.iter().filter_map(|r| r.get(0).and_then(|v| v.as_str())), None);
    save_cached_revisions(local_app_data.clone(), &revisions);
    save_cdn_meta(
        local_app_data,
        &GoogleCdnMeta {
            etag: etag.clone(),
            last_modified: last_modified.clone(),
            checked_at: now,
        },
    );
    Ok(ConditionalCatalogFetch {
        unchanged: false,
        rows,
        etag,
        last_modified,
    })
}

fn slug_family(family: &str) -> String {
    let mut out = String::with_capacity(family.len());
    let mut prev_dash = false;
    for ch in family.chars() {
        if ch == '\'' || ch == '\u{2019}' {
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn revisions_cache_path(local_app_data: Option<PathBuf>) -> Option<PathBuf> {
    local_app_data.map(|base| base.join("Fonths").join("google-cdn-revisions.json"))
}

pub fn family_cdn_revision_marker(dir: &Path) -> PathBuf {
    dir.join(".cdn-revision")
}

pub fn load_cached_revisions(local_app_data: Option<PathBuf>) -> HashMap<String, String> {
    let Some(path) = revisions_cache_path(local_app_data) else {
        return HashMap::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, String>>(&text).unwrap_or_default()
}

pub fn save_cached_revisions(local_app_data: Option<PathBuf>, revs: &HashMap<String, String>) {
    if revs.is_empty() {
        return;
    }
    let Some(path) = revisions_cache_path(local_app_data) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(revs) {
        let _ = fs::write(path, json);
    }
}

fn live_directory(seed_revs: Option<HashMap<String, String>>) -> &'static Mutex<(HashSet<String>, HashSet<String>)> {
    static LIVE_DIR: OnceLock<Mutex<(HashSet<String>, HashSet<String>)>> = OnceLock::new();
    LIVE_DIR.get_or_init(|| {
        let mut by_lower = HashSet::new();
        let mut by_slug = HashSet::new();
        if let Some(revs) = seed_revs {
            for key in revs.keys() {
                let t = key.trim();
                if t.is_empty() {
                    continue;
                }
                by_lower.insert(t.to_ascii_lowercase());
                by_slug.insert(slug_family(t));
            }
        }
        Mutex::new((by_lower, by_slug))
    })
}

pub fn register_live_families<I, S>(families: I, seed_revs: Option<HashMap<String, String>>)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if let Ok(mut guard) = live_directory(seed_revs).lock() {
        for fam in families {
            let t = fam.as_ref().trim();
            if t.is_empty() {
                continue;
            }
            guard.0.insert(t.to_ascii_lowercase());
            guard.1.insert(slug_family(t));
        }
    }
}

pub fn is_in_live_directory(family: &str, seed_revs: Option<HashMap<String, String>>) -> bool {
    let key = family.trim().to_ascii_lowercase();
    if key.is_empty() {
        return false;
    }
    let slug = slug_family(family);
    if let Ok(live) = live_directory(seed_revs).lock() {
        return live.0.contains(&key) || live.1.contains(&slug);
    }
    false
}

/// Parse `https://fonts.google.com/metadata/fonts` (stripping XSSI `)]}'` prefix) into
/// frontend `SnapshotRow` tuples AND a `family_lowercase -> lastModified` revision map.
pub fn parse_live_catalog_json(
    body: &str,
) -> Result<(Vec<serde_json::Value>, HashMap<String, String>), String> {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    let json_text = trimmed
        .strip_prefix(")]}'\n")
        .or_else(|| trimmed.strip_prefix(")]}'\r\n"))
        .or_else(|| trimmed.strip_prefix(")]}'"))
        .unwrap_or(trimmed);
    let parsed: serde_json::Value =
        serde_json::from_str(json_text).map_err(|e| format!("invalid catalog json: {e}"))?;
    let list = parsed
        .get("familyMetadataList")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing familyMetadataList".to_string())?;
    if list.len() < 1000 {
        return Err(format!("live catalog unexpectedly small ({})", list.len()));
    }
    let mut rows = Vec::with_capacity(list.len());
    let mut revisions = HashMap::with_capacity(list.len());
    for item in list {
        let Some(family) = item.get("family").and_then(|v| v.as_str()).map(str::trim) else {
            continue;
        };
        if family.is_empty() {
            continue;
        }
        if let Some(last_mod) = item
            .get("lastModified")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            revisions.insert(family.to_ascii_lowercase(), last_mod.to_string());
        }
        let category = match item.get("category").and_then(|v| v.as_str()).unwrap_or("") {
            "Sans Serif" => "sans",
            "Serif" => "serif",
            "Display" => "display",
            "Handwriting" => "handwriting",
            "Monospace" => "mono",
            "Icons" => "icons",
            _ => "sans",
        };
        let mut weights = BTreeSet::<u16>::new();
        let mut italic = false;
        if let Some(fonts_map) = item.get("fonts").and_then(|v| v.as_object()) {
            for key in fonts_map.keys() {
                if key.ends_with('i') || key.ends_with('I') {
                    italic = true;
                }
                let num_part = key.trim_end_matches(|c: char| !c.is_ascii_digit());
                if let Ok(w) = num_part.parse::<u16>()
                    && (100..=1000).contains(&w) {
                        weights.insert(w);
                    }
            }
        }
        if weights.is_empty() {
            weights.insert(400);
        }
        let weight_vec: Vec<u16> = weights.into_iter().collect();
        let variable = item
            .get("axes")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);
        let popularity = item
            .get("popularity")
            .and_then(|v| v.as_u64())
            .unwrap_or(9999);
        let mut tags: Vec<&'static str> = Vec::new();
        if category == "mono" {
            tags.push("coding");
        } else if category == "handwriting" {
            tags.push("script");
        }
        if family
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|tok| tok == "noto")
        {
            tags.push("noto");
        }
        rows.push(serde_json::json!([
            family,
            category,
            weight_vec,
            italic,
            variable,
            popularity,
            tags
        ]));
    }
    if rows.len() < 1000 {
        return Err(format!("live catalog rows too few ({})", rows.len()));
    }
    Ok((rows, revisions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_cdn_module_tracks_live_families_and_revisions() {
        let mut items = Vec::with_capacity(1002);
        items.push(serde_json::json!({
            "family": "Agrave Pro",
            "category": "Handwriting",
            "lastModified": "2026-04-01",
            "popularity": 100,
            "axes": [{ "tag": "wght" }],
            "fonts": { "400": {}, "700": {} }
        }));
        items.push(serde_json::json!({
            "family": "Firjar",
            "category": "Sans Serif",
            "lastModified": "2026-04-02",
            "popularity": 101,
            "axes": [{ "tag": "wdth" }, { "tag": "wght" }],
            "fonts": { "400": {}, "800": {} }
        }));
        for i in 0..1000 {
            items.push(serde_json::json!({
                "family": format!("Catalog Family {i}"),
                "category": "Sans Serif",
                "popularity": 200 + i,
                "axes": [],
                "fonts": { "400": {} }
            }));
        }
        let body = format!("{}{}", ")]}'\n", serde_json::json!({ "familyMetadataList": items }));
        let (rows, revs) = parse_live_catalog_json(&body).expect("parse");
        assert_eq!(rows.len(), 1002);
        assert_eq!(revs.get("agrave pro").map(String::as_str), Some("2026-04-01"));
        assert_eq!(revs.get("firjar").map(String::as_str), Some("2026-04-02"));

        register_live_families(["Agrave Pro", "Firjar"], None);
        assert!(is_in_live_directory("Agrave Pro", None));
        assert!(is_in_live_directory("firjar", None));

        let tmp = std::env::temp_dir().join(format!("fonths-cdn-meta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let meta = GoogleCdnMeta {
            etag: Some("\"v240-etag\"".to_string()),
            last_modified: Some("Wed, 25 Nov 2026 12:00:00 GMT".to_string()),
            checked_at: 1_795_536_000,
        };
        save_cdn_meta(Some(tmp.clone()), &meta);
        assert_eq!(load_cdn_meta(Some(tmp.clone())), meta);
        let _ = fs::remove_dir_all(&tmp);
    }
}
