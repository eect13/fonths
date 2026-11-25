//! Live Google Fonts catalog fetch, CDN revision diffing, and update queueing.

#[allow(unused_imports)]
use super::*;
#[allow(unused_imports)]
use std::collections::{HashMap, HashSet, VecDeque};
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::io::{ErrorKind, Read, Write};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::Command;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[allow(unused_imports)]
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
#[allow(unused_imports)]
use std::thread;
#[allow(unused_imports)]
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use rayon::prelude::*;
#[allow(unused_imports)]
use serde::Serialize;
#[allow(unused_imports)]
use tauri::{AppHandle, Emitter, Manager};

pub fn family_cdn_revision_marker(dir: &Path) -> PathBuf {
    crate::google_cdn::family_cdn_revision_marker(dir)
}

pub fn load_cached_google_revisions() -> HashMap<String, String> {
    crate::google_cdn::load_cached_revisions(local_app_data())
}

pub fn stamp_cdn_revision_if_known(dir: &Path, family: &str) {
    let revs = load_cached_google_revisions();
    let key = family.trim().to_ascii_lowercase();
    if let Some(rev) = revs.get(&key)
        && !rev.is_empty() {
            write_marker_if_changed(&family_cdn_revision_marker(dir), rev);
        }
}

#[cfg(test)]
pub fn parse_live_google_catalog_json_with_revisions(
    body: &str,
) -> Result<(Vec<serde_json::Value>, HashMap<String, String>), String> {
    crate::google_cdn::parse_live_catalog_json(body)
}

#[cfg(test)]
pub fn parse_live_google_catalog_json(body: &str) -> Result<Vec<serde_json::Value>, String> {
    parse_live_google_catalog_json_with_revisions(body).map(|(rows, _)| rows)
}

pub fn fetch_live_google_catalog_blocking() -> Result<Vec<serde_json::Value>, String> {
    let fetched = crate::google_cdn::fetch_live_catalog_conditional(local_app_data(), true)?;
    register_live_google_families(fetched.rows.iter().filter_map(|r| r.get(0).and_then(|v| v.as_str())));
    Ok(fetched.rows)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionalCatalogResponse {
    pub unchanged: bool,
    pub rows: Vec<serde_json::Value>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[tauri::command]
pub async fn fetch_live_google_catalog_conditional(
    force: Option<bool>,
) -> Result<ConditionalCatalogResponse, String> {
    let force_fetch = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let fetched = crate::google_cdn::fetch_live_catalog_conditional(local_app_data(), force_fetch)?;
        register_live_google_families(fetched.rows.iter().filter_map(|r| r.get(0).and_then(|v| v.as_str())));
        Ok(ConditionalCatalogResponse {
            unchanged: fetched.unchanged,
            rows: fetched.rows,
            etag: fetched.etag,
            last_modified: fetched.last_modified,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn fetch_live_google_catalog() -> Result<Vec<serde_json::Value>, String> {
    tauri::async_runtime::spawn_blocking(fetch_live_google_catalog_blocking)
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleCdnSyncResult {
    pub checked: usize,
    pub outdated: usize,
    pub incomplete: usize,
    pub queued: usize,
    pub up_to_date: usize,
    pub families_queued: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleCdnCheckResult {
    pub checked: usize,
    pub outdated: usize,
    pub incomplete: usize,
    pub up_to_date: usize,
    pub outdated_families: Vec<String>,
    pub incomplete_families: Vec<String>,
}

fn dir_has_any_intact_font(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let ok_ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
            .unwrap_or(false);
        if ok_ext && p.is_file() && ttf_intact(&p) {
            return true;
        }
    }
    false
}

enum CdnDirStatus {
    UpToDate,
    Outdated(String),
    Incomplete(String),
}

pub fn find_google_families_needing_cdn_update_in(
    docs_root: &Path,
    revisions: &HashMap<String, String>,
    force: bool,
) -> (usize, Vec<String>, Vec<String>) {
    let google_dir = google_container_path(docs_root);
    if !google_dir.is_dir() {
        return (0, Vec::new(), Vec::new());
    }
    let Ok(entries) = fs::read_dir(&google_dir) else {
        return (0, Vec::new(), Vec::new());
    };
    let dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    let statuses: Vec<CdnDirStatus> = dirs
        .par_iter()
        .filter_map(|dir| {
            let family = read_family_name_marker(dir).unwrap_or_else(|| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string()
            });
            let family = family.trim().to_string();
            if family.is_empty() || family.starts_with('.') {
                return None;
            }
            if !dir_is_complete(dir) || !dir_has_any_intact_font(dir) {
                return Some(CdnDirStatus::Incomplete(family));
            }
            if force {
                return Some(CdnDirStatus::Outdated(family));
            }
            let key = family.to_ascii_lowercase();
            let live_rev = revisions.get(&key).map(|s| s.as_str());
            let local_rev = fs::read_to_string(family_cdn_revision_marker(dir))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if let Some(live) = live_rev
                && local_rev.as_deref() != Some(live) {
                    return Some(CdnDirStatus::Outdated(family));
                }
            Some(CdnDirStatus::UpToDate)
        })
        .collect();

    let checked = statuses.len();
    let mut outdated = Vec::new();
    let mut incomplete = Vec::new();
    for s in statuses {
        match s {
            CdnDirStatus::Outdated(f) => outdated.push(f),
            CdnDirStatus::Incomplete(f) => incomplete.push(f),
            CdnDirStatus::UpToDate => {}
        }
    }
    outdated.sort();
    incomplete.sort();
    (checked, outdated, incomplete)
}

pub fn check_google_cdn_updates_blocking(app: AppHandle) -> Result<GoogleCdnCheckResult, String> {
    let docs_root = documents_root(&app)?;
    let mut revisions = load_cached_google_revisions();
    if revisions.is_empty() {
        let _ = crate::google_cdn::fetch_live_catalog_conditional(local_app_data(), false);
        revisions = load_cached_google_revisions();
    }
    let (checked, outdated_families, incomplete_families) =
        find_google_families_needing_cdn_update_in(&docs_root, &revisions, false);
    let outdated = outdated_families.len();
    let incomplete = incomplete_families.len();
    let up_to_date = checked.saturating_sub(outdated + incomplete);
    Ok(GoogleCdnCheckResult {
        checked,
        outdated,
        incomplete,
        up_to_date,
        outdated_families,
        incomplete_families,
    })
}

#[tauri::command]
pub async fn check_google_cdn_updates(app: AppHandle) -> Result<GoogleCdnCheckResult, String> {
    tauri::async_runtime::spawn_blocking(move || check_google_cdn_updates_blocking(app))
        .await
        .map_err(|e| e.to_string())?
}

pub fn sync_google_cdn_updates_blocking(
    app: AppHandle,
    force: bool,
) -> Result<GoogleCdnSyncResult, String> {
    let docs_root = documents_root(&app)?;
    let mut revisions = load_cached_google_revisions();
    if revisions.is_empty() {
        let _ = fetch_live_google_catalog_blocking();
        revisions = load_cached_google_revisions();
    }
    let (checked, outdated, incomplete) =
        find_google_families_needing_cdn_update_in(&docs_root, &revisions, force);
    let outdated_count = outdated.len();
    let incomplete_count = incomplete.len();
    let mut to_queue: Vec<String> = Vec::with_capacity(outdated_count + incomplete_count);
    let mut seen = HashSet::new();
    for f in outdated.into_iter().chain(incomplete) {
        if seen.insert(f.to_ascii_lowercase()) {
            to_queue.push(f);
        }
    }
    if to_queue.is_empty() {
        return Ok(GoogleCdnSyncResult {
            checked,
            outdated: 0,
            incomplete: 0,
            queued: 0,
            up_to_date: checked,
            families_queued: Vec::new(),
        });
    }
    let active_before = load_session_families(&app);
    for family in &to_queue {
        let locs = family_locations_in(&docs_root, family);
        for d in &locs {
            clear_complete_marker(d);
        }
        // Keep inactive families inactive on disk while refreshing their font files.
        if !active_before.iter().any(|a| a.eq_ignore_ascii_case(family)) {
            mark_repair_files_only(family, &locs);
        }
        forget_queued(family);
    }
    let queued = start_google_downloads_blocking(app, to_queue.clone(), None)?;
    Ok(GoogleCdnSyncResult {
        checked,
        outdated: outdated_count,
        incomplete: incomplete_count,
        queued,
        up_to_date: checked.saturating_sub(to_queue.len()),
        families_queued: to_queue,
    })
}

#[tauri::command]
pub async fn sync_google_cdn_updates(
    app: AppHandle,
    force: Option<bool>,
) -> Result<GoogleCdnSyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        sync_google_cdn_updates_blocking(app, force.unwrap_or(false))
    })
    .await
    .map_err(|e| e.to_string())?
}

