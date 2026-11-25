//! Documents library scanning, VF policy refresh, prune/purge, and gdi-maps stage cleanup.

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

#[derive(Clone, Serialize, serde::Deserialize)]
pub struct DiskFamily {
    pub name: String,
    pub bytes: u64,
    pub files: usize,
    pub corrupt: usize,
    /// Intact faces present but honest `.complete` missing, face-count short, or catalog VF missing.
    pub incomplete: bool,
    /// Known GDI-incapable + intact full-size: disk settled (not Incomplete, not Activated).
    #[serde(default)]
    pub settled: bool,
    /// `.complete` marker currently present (after verify).
    pub has_complete: bool,
    /// Intact on-disk `*-variable-*` face present.
    pub has_variable: bool,
    /// Catalog-variable family expecting a public VF, but no intact `*-variable-*` on disk.
    pub missing_variable: bool,
    /// Intact planned face << typical full Google TTF (latin/subset remnant, e.g. Gidugu).
    pub undersized: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDocsResult {
    pub families_seen: u32,
    pub families_purged: u32,
    pub statics_deleted: u32,
    pub locked: u32,
    pub cancelled: bool,
}

/// 1.0.206u / 1.0.206aa: manual Refresh / sync Documents — reconcile installed
/// folders to VF-primary policy (remove redundant statics when intact VF present).
/// Does **not** surprise-download the full catalog. Progress on `font-download`.
///
/// Cancel via `cancel_documents_refresh` (`docs_cancel` only).
/// Async command + `spawn_blocking` so Cancel is not queued behind the walk.
/// Activate keeps its own `cancel` flag and the "Stopping…" label.
pub fn docs_vf_session_is_live() -> bool {
    bulk().docs_vf_session_live.load(Ordering::SeqCst)
}

/// Begin docs VF session. Does not register a global shortcut.
pub fn docs_vf_session_begin() {
    bulk().docs_cancel.store(false, Ordering::SeqCst);
    bulk().docs_vf_session_live.store(true, Ordering::SeqCst);
}

/// End docs VF session. Escape must not keep cancelling after the job settles.
pub fn docs_vf_session_end() {
    let state = bulk();
    state.docs_vf_session_live.store(false, Ordering::SeqCst);
    state.docs_cancel.store(false, Ordering::SeqCst);
}

/// Tray / in-app Cancel: abort Documents refresh only.
/// Does not set Activate's `cancel` and does not emit "Stopping…".
pub fn request_docs_vf_cancel_native(app: AppHandle) {
    if !docs_vf_session_is_live() {
        return;
    }
    bulk().docs_cancel.store(true, Ordering::SeqCst);
    if let Ok(mut p) = bulk().progress.lock()
        && p.running {
            p.current = "Cancelling Documents refresh…".into();
        }
    let _ = app.emit("docs-vf-cancel-requested", ());
    emit_progress_force(&app);
}

#[tauri::command]
pub async fn cancel_documents_refresh(app: AppHandle) -> Result<(), String> {
    request_docs_vf_cancel_native(app);
    Ok(())
}

#[tauri::command]
pub async fn sync_documents_vf_policy(app: AppHandle) -> Result<SyncDocsResult, String> {
    let state = bulk();
    if state.running.load(Ordering::SeqCst) {
        return Err(
            "Activate or download already running — wait or Cancel first".into(),
        );
    }
    // Leftover Activate cancel must not abort this walk. Docs uses docs_cancel only.
    state.cancel.store(false, Ordering::SeqCst);
    state.pause.store(false, Ordering::SeqCst);
    state.running.store(true, Ordering::SeqCst);
    docs_vf_session_begin();
    if let Ok(mut p) = state.progress.lock() {
        *p = GoogleDlProgress {
            running: true,
            done: 0,
            total: 0,
            failed: 0,
            current: "Scanning Documents…".into(),
            failed_names: Vec::new(),
            failed_details: Vec::new(),
            paused: false,
            ready_names: Vec::new(),
            skipped: 0,
            settled_names: Vec::new(),
            kind: "download".into(),
        };
    }
    emit_progress_force(&app);

    // Clone for join-error cleanup — `app` moves into the blocking closure.
    let app_join = app.clone();
    match tauri::async_runtime::spawn_blocking(move || sync_documents_vf_policy_work(app)).await {
        Ok(result) => result,
        Err(e) => {
            // Must reset running/progress or the app sticks on "already running".
            let _ = finish_docs_vf_sync(&app_join, 0, 0, 0, 0, false);
            Err(format!("Documents refresh worker join failed: {e}"))
        }
    }
}

/// Per-dir purge loop — shared by production + unit tests.
/// Cancel checked at the start of each iteration and again immediately before `purge`
/// (after `prepare`). Returns (cancelled, families_purged, statics_deleted, locked, done).
pub fn run_docs_vf_sync_purge_loop<Prep, Purge>(
    dirs: &[PathBuf],
    cancel: &AtomicBool,
    mut prepare: Prep,
    mut purge: Purge,
) -> (bool, u32, u32, u32, u32)
where
    Prep: FnMut(&Path, &str),
    Purge: FnMut(&Path, &str) -> (usize, usize),
{
    let mut families_purged = 0u32;
    let mut statics_deleted = 0u32;
    let mut locked = 0u32;
    let mut cancelled = false;
    let mut done = 0u32;

    for dir in dirs {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("font")
            .to_string();
        prepare(dir, &name);
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let (del, lock) = purge(dir, &name);
        if del > 0 || lock > 0 {
            families_purged = families_purged.saturating_add(1);
        }
        statics_deleted = statics_deleted.saturating_add(del as u32);
        locked = locked.saturating_add(lock as u32);
        done = done.saturating_add(1);
    }

    (cancelled, families_purged, statics_deleted, locked, done)
}

/// After dirs collected: cancel-aware purge. Shared by production work + integration tests.
/// If `cancel` is already set (scan aborted), returns `cancelled: true` with zero purges.
pub fn docs_vf_sync_execute<Prep, Purge>(
    dirs: Vec<PathBuf>,
    cancel: &AtomicBool,
    prepare: Prep,
    purge: Purge,
) -> SyncDocsResult
where
    Prep: FnMut(&Path, &str),
    Purge: FnMut(&Path, &str) -> (usize, usize),
{
    let families_seen = dirs.len() as u32;
    if cancel.load(Ordering::SeqCst) {
        return SyncDocsResult {
            families_seen,
            families_purged: 0,
            statics_deleted: 0,
            locked: 0,
            cancelled: true,
        };
    }
    let (loop_cancelled, families_purged, statics_deleted, locked, _done) =
        run_docs_vf_sync_purge_loop(&dirs, cancel, prepare, purge);
    // If cancel arrived as the last dir finished, still report cancelled:true (206ad mouse race).
    let cancelled = loop_cancelled || cancel.load(Ordering::SeqCst);
    SyncDocsResult {
        families_seen,
        families_purged,
        statics_deleted,
        locked,
        cancelled,
    }
}

/// Heal cached Google variable faces in `dir` when `nameID 6/25` or Unicode
/// platform encoding needs repair for Adobe variation support.
pub fn heal_google_variable_names_in_dir(dir: &Path, family: &str) {
    if !is_official_google_family(family) || !dir.is_dir() {
        return;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    for path in files {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_variable_face_filename(name) || !ttf_intact(&path) {
            continue;
        }
        let italic = variable_face_filename_is_italic(name);
        let _ = heal_google_variable_face_file(&path, family, italic);
    }
}

/// Remove leftover `.part` temp files in `%LOCALAPPDATA%\Font Manager\gdi-maps`.
pub fn sweep_gdi_maps_part_files() {
    let Some(dir) = gdi_maps_root() else {
        return;
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_file()
            && p.extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("part"))
                .unwrap_or(false)
        {
            let _ = fs::remove_file(p);
        }
    }
}

/// Inner Documents VF sync — cancel-aware; uses `docs_vf_sync_execute`.
pub fn sync_documents_vf_policy_work(app: AppHandle) -> Result<SyncDocsResult, String> {
    let state = bulk();
    // Consolidate any legacy Activated/ and Library/ split folders into the single
    // unified Documents\Font Manager library root when not active/cancelled.
    if let Ok(root) = documents_root(&app)
        && !state.docs_cancel.load(Ordering::SeqCst) {
            let _ = consolidate_legacy_roots_in(&root, Some(&state.docs_cancel));
        }
    sweep_gdi_maps_part_files();

    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut scan_cancelled = false;
    for_family_dirs(&app, |dir| {
        if state.docs_cancel.load(Ordering::SeqCst) {
            scan_cancelled = true;
            return;
        }
        dirs.push(dir.to_path_buf());
    });
    if scan_cancelled || state.docs_cancel.load(Ordering::SeqCst) {
        let result = docs_vf_sync_execute(
            dirs,
            &state.docs_cancel,
            |_, _| {},
            |_, _| (0, 0),
        );
        return Ok(finish_docs_vf_sync(
            &app,
            result.families_seen,
            result.families_purged,
            result.statics_deleted,
            result.locked,
            true,
        ));
    }

    let total = dirs.len() as u32;
    if let Ok(mut p) = state.progress.lock() {
        p.total = total;
        p.current = format!("Syncing Documents (0/{total})");
    }
    emit_progress_force(&app);

    let total_cap = total;
    // 1.0.206ae: throttle docs progress emits (time + every-N) — skip per-family text churn.
    let last_docs_emit = std::sync::Mutex::new(Instant::now() - Duration::from_secs(10));
    let dirs_since_emit = std::sync::atomic::AtomicU32::new(0);
    let result = docs_vf_sync_execute(
        dirs,
        &state.docs_cancel,
        |dir, name| {
            migrate_download_source_stamp(dir);
            heal_google_variable_names_in_dir(dir, name);
        },
        |dir, name| {
            let (del, lock) =
                purge_redundant_statics_in_dir_cancelable(dir, name, Some(&state.docs_cancel));
            // After docs_cancel, skip progress emits so the WebView is not flooded.
            if state.docs_cancel.load(Ordering::SeqCst) {
                return (del, lock);
            }
            let done_now = {
                let mut done_now = 0u32;
                if let Ok(mut p) = state.progress.lock() {
                    p.done = p.done.saturating_add(1);
                    p.total = total_cap.max(p.done);
                    p.running = true;
                    p.kind = "download".into();
                    if lock > 0 {
                        p.failed = p.failed.saturating_add(lock as u32);
                    }
                    done_now = p.done;
                }
                done_now
            };
            let n = dirs_since_emit.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            let due_n = n >= docs_vf_emit_every_n_dirs() || done_now <= 1 || done_now >= total_cap;
            let due_t = last_docs_emit
                .lock()
                .map(|t| t.elapsed() >= Duration::from_millis(docs_vf_emit_progress_throttle_ms()))
                .unwrap_or(true);
            if due_n || due_t {
                if let Ok(mut p) = state.progress.lock() {
                    // Stable label — no per-family Name churn for UIA / React.
                    p.current = format!("Refreshing Documents ({done_now}/{total_cap})");
                }
                if done_now <= 1 || done_now >= total_cap {
                    emit_progress_force(&app);
                } else {
                    emit_progress(&app);
                }
                dirs_since_emit.store(0, Ordering::SeqCst);
                if let Ok(mut t) = last_docs_emit.lock() {
                    *t = Instant::now();
                }
            }
            (del, lock)
        },
    );

    Ok(finish_docs_vf_sync(
        &app,
        result.families_seen,
        result.families_purged,
        result.statics_deleted,
        result.locked,
        result.cancelled,
    ))
}

pub fn finish_docs_vf_sync(
    app: &AppHandle,
    families_seen: u32,
    families_purged: u32,
    statics_deleted: u32,
    locked: u32,
    cancelled: bool,
) -> SyncDocsResult {
    let state = bulk();
    state.running.store(false, Ordering::SeqCst);
    docs_vf_session_end();
    if let Ok(mut p) = state.progress.lock() {
        p.running = false;
        p.paused = false;
        p.current = if cancelled {
            "Sync cancelled".into()
        } else {
            format!(
                "Sync done — {families_purged} families, {statics_deleted} statics removed"
            )
        };
    }
    emit_progress_force(app);
    SyncDocsResult {
        families_seen,
        families_purged,
        statics_deleted,
        locked,
        cancelled,
    }
}

#[tauri::command]
pub async fn scan_disk_families(app: AppHandle) -> Result<Vec<DiskFamily>, String> {
    tauri::async_runtime::spawn_blocking(move || scan_disk_families_blocking(app))
        .await
        .map_err(|e| e.to_string())?
}

/// Merge any duplicate family entries across legacy split folders cleanly.
pub fn merge_disk_family_rows(rows: Vec<DiskFamily>) -> Vec<DiskFamily> {
    let mut by_key: HashMap<String, DiskFamily> = HashMap::with_capacity(rows.len());
    for r in rows {
        let key = r.name.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if let Some(prev) = by_key.get_mut(&key) {
            prev.bytes = prev.bytes.saturating_add(r.bytes);
            prev.files = prev.files.saturating_add(r.files);
            prev.corrupt = prev.corrupt.saturating_add(r.corrupt);
            prev.has_variable |= r.has_variable;
            prev.has_complete |= r.has_complete;
            prev.settled |= r.settled;
            prev.undersized |= r.undersized;
            prev.missing_variable = prev.missing_variable && !prev.has_variable;
            prev.incomplete = !prev.settled
                && (prev.incomplete || r.incomplete)
                && !(prev.has_complete && !prev.missing_variable && !prev.undersized);
        } else {
            by_key.insert(key, r);
        }
    }
    let mut out: Vec<DiskFamily> = by_key.into_values().collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[derive(Clone, Serialize, serde::Deserialize)]
pub struct DiskScanCacheEntry {
    pub stamp: (u64, u64, usize, u64),
    pub row: Option<DiskFamily>,
}

pub fn scan_index_mem() -> &'static Mutex<Option<HashMap<String, DiskScanCacheEntry>>> {
    static CACHE: LazyLock<Mutex<Option<HashMap<String, DiskScanCacheEntry>>>> =
        LazyLock::new(|| Mutex::new(None));
    &CACHE
}

pub fn scan_index_file_path() -> Option<PathBuf> {
    fm_local_root().map(|r| r.join("scan-index.json"))
}

pub fn mtime_secs_nanos(meta: &fs::Metadata) -> (u64, u64) {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos() as u64))
        .unwrap_or((0, 0))
}

/// Folder fingerprint: (dir_mtime_secs, dir_mtime_nanos, entry_count, marker_mtime_hash).
/// Any file add/remove or marker update changes the stamp and invalidates the cached row.
pub fn dir_scan_stamp(dir: &Path) -> Option<(u64, u64, usize, u64)> {
    let dir_meta = fs::metadata(dir).ok()?;
    let (secs, nanos) = mtime_secs_nanos(&dir_meta);
    let entry_count = fs::read_dir(dir).ok()?.count();
    let mut marker_hash = 0u64;
    for marker in [
        ".complete",
        ".google-planned",
        ".expected",
        ".full-source",
        ".web-build",
        ".settled-add-zero",
        ".family",
    ] {
        let p = dir.join(marker);
        if let Ok(m) = fs::metadata(&p) {
            let (ms, mn) = mtime_secs_nanos(&m);
            marker_hash = marker_hash.wrapping_mul(1315423911).wrapping_add(ms ^ mn ^ m.len());
        }
    }
    Some((secs, nanos, entry_count, marker_hash))
}

pub fn load_scan_index_map(path: Option<&Path>) -> HashMap<String, DiskScanCacheEntry> {
    if let Ok(mut g) = scan_index_mem().lock() {
        if let Some(ref map) = *g {
            return map.clone();
        }
        let loaded = path
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<HashMap<String, DiskScanCacheEntry>>(&b).ok())
            .unwrap_or_default();
        *g = Some(loaded.clone());
        return loaded;
    }
    HashMap::new()
}

pub fn save_scan_index_map(path: Option<&Path>, map: &HashMap<String, DiskScanCacheEntry>) {
    if let Ok(mut g) = scan_index_mem().lock() {
        *g = Some(map.clone());
    }
    let Some(p) = path else {
        return;
    };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(map) {
        let tmp = p.with_extension("json.tmp");
        if fs::write(&tmp, &bytes).is_ok() {
            let _ = fs::rename(&tmp, p);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
}

pub fn scan_disk_families_blocking(app: AppHandle) -> Result<Vec<DiskFamily>, String> {
    fn scan_single_disk_family(dir: &Path) -> Option<DiskFamily> {
        // u-<hash> upload folders carry the real name in `.family`.
        let name = dir_family_name(dir);
        // 1.0.205: migrate legacy folders missing `.download-source`.
        migrate_download_source_stamp(dir);
        upgrade_legacy_cbdt_color_emoji_dir(dir, &name);
        // 1.0.188: purge FS remnants before Scan honesty / undersized flag.
        if family_known_gdi_session_incapable(&name) {
            let _ = purge_known_incapable_fontsource_remnants(dir, &name);
        }
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        let mut bytes = 0u64;
        let mut intact = 0usize;
        let mut corrupt = 0usize;
        let mut preview_only = 0usize;
        for path in files {
            let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            bytes += len;
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ttf_intact(&path) {
                intact += 1;
            } else if matches!(ext.as_str(), "woff" | "woff2") {
                // Preview wrappers — not installable, not "corrupt".
                preview_only += 1;
            } else {
                corrupt += 1;
            }
        }
        if intact == 0 && corrupt == 0 && preview_only == 0 {
            return None;
        }
        verify_complete_marker(dir);
        // Known GDI-incapable + intact full-size ⇒ disk settled (no Repair-1 churn).
        stamp_known_incapable_dir_settled(dir, &name);
        // Soft: wipe bare `.complete` lacking `.settled-add-zero` (hard-emoji tip upgrade).
        wipe_soft_complete_lacking_provenance(dir, &name);
        let has_complete = dir_is_complete(dir);
        let has_variable = dir_has_intact_variable(dir);
        let missing_variable = dir_missing_variable(dir, &name, has_variable, intact);
        let undersized = dir_has_undersized_google_static(dir, &name);
        let soft = family_soft_try_add_then_settle(&name);
        let full_face_ok = soft_emoji_full_face_ok(dir, &name);
        let has_prov = dir_has_settled_add_zero_provenance(dir);
        // Hard Gidugu: intact full-size ⇒ Settled (may stamp `.complete` above).
        // Soft emoji: Settled only with Add=0 provenance (never bare `.complete`).
        let settled = if family_known_gdi_session_incapable(&name) {
            family_disk_settled_known_gdi_incapable(true, intact > 0, undersized)
        } else if soft {
            soft_scan_settled_from_provenance(has_prov, full_face_ok, intact > 0, undersized)
        } else {
            false
        };
        // Settled: never Incomplete. Soft full-face without provenance: also not Incomplete
        // (Activate try-Add first — no huge TTF Repair churn).
        let incomplete = soft_scan_incomplete(
            settled,
            soft,
            full_face_ok,
            intact > 0,
            has_complete,
            missing_variable,
            undersized,
        );
        Some(DiskFamily {
            name,
            bytes,
            files: intact,
            corrupt,
            incomplete,
            settled,
            has_complete,
            has_variable,
            missing_variable,
            undersized,
        })
    }
    // Collect family directories in one pass (consolidating empty legacy checks).
    let mut dirs: Vec<PathBuf> = Vec::new();
    for_family_dirs(&app, |dir| dirs.push(dir.to_path_buf()));
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    type DirScanStamp = (u64, u64, usize, u64);
    type ScanMissResult = (String, Option<DirScanStamp>, Option<DiskFamily>);
    let cache_path = scan_index_file_path();
    let mut cached_map = load_scan_index_map(cache_path.as_deref());
    let mut hits: Vec<DiskFamily> = Vec::new();
    let mut misses: Vec<(PathBuf, Option<DirScanStamp>)> = Vec::new();
    let mut live_keys: HashSet<String> = HashSet::with_capacity(dirs.len());
    let mut cache_dirty = false;
    for dir in &dirs {
        let key = dir.to_string_lossy().into_owned();
        live_keys.insert(key.clone());
        let stamp = dir_scan_stamp(dir);
        if let (Some(st), Some(entry)) = (stamp, cached_map.get(&key))
            && entry.stamp == st {
                if let Some(ref row) = entry.row {
                    hits.push(row.clone());
                }
                continue;
            }
        misses.push((dir.clone(), stamp));
    }
    if cached_map.len() > live_keys.len() {
        cached_map.retain(|k, _| live_keys.contains(k));
        cache_dirty = true;
    }
    if !misses.is_empty() {
        cache_dirty = true;
        let scan_miss = |dir: &Path, stamp: Option<DirScanStamp>| {
            let row = scan_single_disk_family(dir);
            (dir.to_string_lossy().into_owned(), stamp, row)
        };
        let computed: Vec<ScanMissResult> =
            if misses.len() <= 8 {
                misses
                    .into_iter()
                    .map(|(d, st)| scan_miss(&d, st))
                    .collect()
            } else {
                misses
                    .into_par_iter()
                    .map(|(d, st)| scan_miss(&d, st))
                    .collect()
            };
        for (key, stamp, row) in computed {
            if let Some(ref df) = row {
                hits.push(df.clone());
            }
            if let Some(st) = stamp {
                cached_map.insert(key, DiskScanCacheEntry { stamp: st, row });
            }
        }
    }
    if cache_dirty {
        save_scan_index_map(cache_path.as_deref(), &cached_map);
    }
    Ok(merge_disk_family_rows(hits))
}

/// Family folders not in `keep` (catalog + uploads), conservatively. A folder is a
/// candidate only when its family name, its `.family` marker and the name-table family
/// of every font inside are all unknown to `keep`. Upload folders (`.family` marker),
/// folders with any unreadable / non-SFNT font, and protected dirs are never candidates.
pub fn prune_candidates(dirs: &[PathBuf], root: &Path, keep_keys: &HashSet<String>) -> Vec<PathBuf> {
    let kept = |name: &str| alias_keys(name).iter().any(|k| keep_keys.contains(k));
    let mut out = Vec::new();
    for dir in dirs {
        let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.is_empty() || name.starts_with('.') || is_protected_family_dir(root, dir) {
            continue;
        }
        if dir_is_upload_container_child(root, dir)
            || matches!(read_download_source(dir), Some(FetchIntent::Local))
            || read_family_name_marker(dir).is_some()
        {
            continue;
        }
        if kept(name) || kept(&dir_family_name(dir)) {
            continue;
        }
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        let spare = files.iter().any(|f| match sfnt_upload_family(f) {
            Some(fam) => kept(&fam),
            None => true,
        });
        if spare {
            continue;
        }
        out.push(dir.clone());
    }
    out
}

/// Card 36: never a permanent delete. Windows → Recycle Bin (undo); elsewhere the
/// folder is left in place and logged. `Ok(true)` only when it went to the Bin.
pub fn recycle_or_leave(dir: &Path) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        for path in &files {
            unregister_path(path);
            crate::font_protocol::evict_path(path);
            intact_forget(path);
        }
        crate::font_protocol::evict_dir(dir);
        gdi_flush_local();
        recycle_bin_windows(dir)?;
        eprintln!("[fm] prune: moved {} to the Recycle Bin", dir.display());
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        eprintln!("[fm] prune: left {} (no Recycle Bin on this OS)", dir.display());
        Ok(false)
    }
}

/// Move family folders that are not in `keep` to the Recycle Bin (user-initiated from
/// Scan). Refuses if `keep` is too small so a bad catalog cannot empty Documents.
#[tauri::command]
pub async fn prune_unknown_folders(app: AppHandle, keep: Vec<String>) -> Result<u32, String> {
    tauri::async_runtime::spawn_blocking(move || prune_unknown_folders_blocking(app, keep))
        .await
        .map_err(|e| e.to_string())?
}

pub fn prune_unknown_folders_blocking(app: AppHandle, keep: Vec<String>) -> Result<u32, String> {
    if keep.len() < 500 {
        return Err("catalog too small to prune against".into());
    }
    let root = documents_root(&app)?;
    let mut keep_keys: HashSet<String> = HashSet::new();
    for name in &keep {
        for key in alias_keys(name) {
            keep_keys.insert(key);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    for_family_dirs(&app, |path| dirs.push(path.to_path_buf()));
    let mut n = 0u32;
    for dir in prune_candidates(&dirs, &root, &keep_keys) {
        match recycle_or_leave(&dir) {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(err) => eprintln!("[fm] prune: left {} ({err})", dir.display()),
        }
    }
    Ok(n)
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PurgeNonGoogleResult {
    pub candidates: Vec<String>,
    pub removed: u32,
    pub failed: u32,
    pub permanent: bool,
}

pub fn purge_non_google_fonts_in(
    root: &Path,
    keep: &[String],
    dry_run: bool,
    permanent: bool,
) -> Result<PurgeNonGoogleResult, String> {
    if keep.len() < 500 {
        return Err("Google catalog list too small to safely purge against".into());
    }
    let mut keep_keys: HashSet<String> = HashSet::new();
    for name in keep {
        for key in alias_keys(name) {
            keep_keys.insert(key);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    for container in [
        google_container_path(root),
        root.join(GOOGLE_CONTAINER),
        root.to_path_buf(),
        root.join("Activated"),
        root.join("Library"),
    ] {
        let Ok(rd) = fs::read_dir(&container) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_dir() || is_protected_family_dir(root, &p) || dir_is_upload_container_child(root, &p) {
                continue;
            }
            dirs.push(p);
        }
    }
    let targets = prune_candidates(&dirs, root, &keep_keys);
    let mut candidates: Vec<String> = targets.iter().map(|d| dir_family_name(d)).collect();
    candidates.sort_by_key(|s| s.to_ascii_lowercase());
    candidates.dedup();
    if dry_run {
        return Ok(PurgeNonGoogleResult {
            candidates,
            removed: 0,
            failed: 0,
            permanent,
        });
    }
    let mut removed = 0u32;
    let mut failed = 0u32;
    for dir in &targets {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        for path in &files {
            unregister_path(path);
            crate::font_protocol::evict_path(path);
            intact_forget(path);
        }
        crate::font_protocol::evict_dir(dir);
        gdi_flush_local();
        let res = if permanent {
            fs::remove_dir_all(dir).map_err(|e| e.to_string())
        } else {
            recycle_or_leave(dir).map(|_| ())
        };
        match res {
            Ok(()) => removed = removed.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }
    if removed > 0 {
        notify_fonts_changed();
    }
    Ok(PurgeNonGoogleResult {
        candidates,
        removed,
        failed,
        permanent,
    })
}

/// Purge non-Google family folders from `Documents\Fonths\Providers\Google` (and stray root folders),
/// strictly sparing `Uploads` and `C:\Windows\Fonts`. Supports `dry_run` preview and honors `deleteMode`.
#[tauri::command]
pub async fn purge_non_google_fonts(
    app: AppHandle,
    keep: Vec<String>,
    dry_run: Option<bool>,
) -> Result<PurgeNonGoogleResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = documents_root(&app)?;
        let permanent = crate::delete_ops::is_permanent_delete();
        purge_non_google_fonts_in(&root, &keep, dry_run.unwrap_or(false), permanent)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- B6: Safe gdi-maps leftover detection & selective Recycle Bin cleanup ----

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverStageFile {
    pub path: String,
    pub file_name: String,
    pub bytes: u64,
    pub modified_ms: u64,
    pub family: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleStageFailure {
    pub path: String,
    pub error: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleLeftoversResult {
    pub requested: usize,
    pub recycled: usize,
    pub failed: Vec<RecycleStageFailure>,
}

pub fn canonical_win_key(p: &Path) -> String {
    let canon = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy().replace('/', "\\");
    let trimmed = s.strip_prefix(r"\\?\").unwrap_or(&s);
    crate::session_stage::normalize_path_key(Path::new(trimmed))
}

pub fn is_stage_child_of(path: &Path, root: &Path) -> bool {
    if crate::session_stage::must_not_register_as_gdi_path(path) {
        return false;
    }
    #[cfg(windows)]
    if winfont::is_windows_fonts_path(path) {
        return false;
    }
    let pk = canonical_win_key(path);
    let rk = canonical_win_key(root);
    !rk.is_empty() && pk.starts_with(&format!("{rk}\\"))
}

pub fn find_leftover_stage_files_in(
    maps_root: &Path,
    active_stages: &HashSet<String>,
    mapped_sources: &HashMap<String, (String, String)>,
) -> Vec<LeftoverStageFile> {
    let Ok(rd) = fs::read_dir(maps_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_file() || !is_stage_child_of(&p, maps_root) {
            continue;
        }
        let key = crate::session_stage::normalize_path_key(&p);
        if active_stages.contains(&key) {
            continue;
        }
        let file_name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let meta = fs::metadata(&p).ok();
        let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified_ms = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let is_part = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("part"))
            .unwrap_or(false);

        let (family, reason) = if is_part {
            (None, "Incomplete temporary stage file (.part)".to_string())
        } else if let Some((src, fam)) = mapped_sources.get(&key) {
            let fam_opt = if !fam.trim().is_empty() {
                Some(fam.trim().to_string())
            } else {
                Path::new(src)
                    .parent()
                    .and_then(|d| d.file_name())
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .or_else(|| crate::scanner::win32_family_name(&p))
            };
            let src_exists = Path::new(src).is_file();
            let reason = if src_exists {
                "Inactive cached stage copy (library font currently deactivated)".to_string()
            } else {
                "Orphaned stage copy (source file removed from library)".to_string()
            };
            (fam_opt, reason)
        } else {
            let fam_opt = crate::scanner::win32_family_name(&p);
            (fam_opt, "Unreferenced stage copy in gdi-maps".to_string())
        };

        out.push(LeftoverStageFile {
            path: p.to_string_lossy().into_owned(),
            file_name,
            bytes,
            modified_ms,
            family,
            reason,
        });
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.file_name.cmp(&b.file_name)));
    out
}

pub fn collect_active_stages_and_sources(
    app: &AppHandle,
) -> (HashSet<String>, HashMap<String, (String, String)>) {
    let mut active_stages: HashSet<String> = HashSet::new();
    let mut mapped_sources: HashMap<String, (String, String)> = HashMap::new();
    if let Ok(root) = documents_root(app) {
        for m in crate::session_stage::load_session_maps_in(&root) {
            let key = crate::session_stage::normalize_path_key(Path::new(&m.stage));
            if !key.is_empty() {
                mapped_sources.insert(key, (m.source, m.family));
            }
        }
    }
    #[cfg(windows)]
    {
        for p in winfont::snapshot_stage_paths() {
            active_stages.insert(crate::session_stage::normalize_path_key(&p));
        }
        for m in winfont::snapshot_face_maps() {
            let key = crate::session_stage::normalize_path_key(Path::new(&m.stage));
            if !key.is_empty() {
                mapped_sources.entry(key).or_insert((m.source, m.family));
            }
        }
    }
    (active_stages, mapped_sources)
}

pub fn list_leftover_stage_files_blocking(app: AppHandle) -> Result<Vec<LeftoverStageFile>, String> {
    #[cfg(windows)]
    {
        let Some(maps_root) = gdi_maps_root() else {
            return Ok(Vec::new());
        };
        if !maps_root.is_dir() {
            return Ok(Vec::new());
        }
        let (active_stages, mapped_sources) = collect_active_stages_and_sources(&app);
        Ok(find_leftover_stage_files_in(
            &maps_root,
            &active_stages,
            &mapped_sources,
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(Vec::new())
    }
}

pub fn recycle_stage_files_in(
    maps_root: &Path,
    active_stages: &HashSet<String>,
    paths: Vec<String>,
) -> RecycleLeftoversResult {
    let requested = paths.len();
    let mut recycled = 0usize;
    let mut failed = Vec::new();
    for raw in paths {
        let p = PathBuf::from(&raw);
        if !is_stage_child_of(&p, maps_root) {
            failed.push(RecycleStageFailure {
                path: raw,
                error: "Refused: path is outside gdi-maps stage folder".into(),
            });
            continue;
        }
        let key = crate::session_stage::normalize_path_key(&p);
        if active_stages.contains(&key) {
            failed.push(RecycleStageFailure {
                path: raw,
                error: "Refused: stage file belongs to an active session font".into(),
            });
            continue;
        }
        if !p.exists() {
            recycled = recycled.saturating_add(1);
            continue;
        }
        // Temporary Fonts Rule: release handles & permanently remove temp stage files without cluttering Recycle Bin.
        unregister_path(&p);
        intact_forget(&p);
        gdi_flush_local();
        match crate::delete_ops::delete_temp_file_permanently(&p) {
            Ok(()) => recycled = recycled.saturating_add(1),
            Err(err) => failed.push(RecycleStageFailure { path: raw, error: err }),
        }
    }
    RecycleLeftoversResult {
        requested,
        recycled,
        failed,
    }
}

pub fn recycle_leftover_stage_files_blocking(
    app: AppHandle,
    paths: Vec<String>,
) -> Result<RecycleLeftoversResult, String> {
    #[cfg(windows)]
    {
        let Some(maps_root) = gdi_maps_root() else {
            return Ok(RecycleLeftoversResult {
                requested: paths.len(),
                recycled: 0,
                failed: Vec::new(),
            });
        };
        let (active_stages, _) = collect_active_stages_and_sources(&app);
        Ok(recycle_stage_files_in(&maps_root, &active_stages, paths))
    }
    #[cfg(not(windows))]
    {
        let _ = (app, paths);
        Ok(RecycleLeftoversResult {
            requested: 0,
            recycled: 0,
            failed: Vec::new(),
        })
    }
}

#[tauri::command]
pub async fn list_leftover_stage_files(app: AppHandle) -> Result<Vec<LeftoverStageFile>, String> {
    tauri::async_runtime::spawn_blocking(move || list_leftover_stage_files_blocking(app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn recycle_leftover_stage_files(
    app: AppHandle,
    paths: Vec<String>,
) -> Result<RecycleLeftoversResult, String> {
    tauri::async_runtime::spawn_blocking(move || recycle_leftover_stage_files_blocking(app, paths))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempStageCleanupResult {
    pub scanned: usize,
    pub deleted: usize,
    pub failed: usize,
}

pub fn cleanup_temp_stage_files_blocking(app: AppHandle) -> Result<TempStageCleanupResult, String> {
    let items = list_leftover_stage_files_blocking(app.clone())?;
    if items.is_empty() {
        return Ok(TempStageCleanupResult {
            scanned: 0,
            deleted: 0,
            failed: 0,
        });
    }
    let paths: Vec<String> = items.into_iter().map(|i| i.path).collect();
    let res = recycle_leftover_stage_files_blocking(app, paths)?;
    Ok(TempStageCleanupResult {
        scanned: res.requested,
        deleted: res.recycled,
        failed: res.failed.len(),
    })
}

#[tauri::command]
pub async fn cleanup_temp_stage_files(app: AppHandle) -> Result<TempStageCleanupResult, String> {
    tauri::async_runtime::spawn_blocking(move || cleanup_temp_stage_files_blocking(app))
        .await
        .map_err(|e| e.to_string())?
}
