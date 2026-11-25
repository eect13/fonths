//! Session lifecycle: cold-boot restore, headless daemon handoff, quit cleanup, sidecars.

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

/// How long quit_gracefully should wait for session_end (scales with path count).
pub fn quit_unload_budget(app: &AppHandle) -> Duration {
    #[cfg(windows)]
    {
        let n = load_session_paths(app)
            .len()
            .max(winfont::snapshot_loaded().len());
        quit_unload_budget_for(n)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        quit_unload_budget_for(0)
    }
}

pub fn session_add(app: &AppHandle, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut cur = load_session_families(app);
    for n in names {
        let t = n.trim();
        if t.is_empty() {
            continue;
        }
        if !cur.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            cur.push(t.to_string());
        }
    }
    save_session_families(app, &cur);
}

/// c253 #6: after a Deactivate that empties the session, any face we still hold in
/// GDI (e.g. Added during a failed Activate, never entered the session list) is a leak.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn leftover_loaded_after_unload(
    remaining_session: &[String],
    loaded: &[PathBuf],
    is_system: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    if !remaining_session.is_empty() {
        return Vec::new();
    }
    loaded.iter().filter(|p| !is_system(p)).cloned().collect()
}

/// c255 P2: per-family leak sweep — stage paths still loaded whose face-map source
/// lives in this family's Documents folder (failed Activate never entered by_family).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn leftover_loaded_for_family(
    keys: &[String],
    loaded: &[PathBuf],
    maps: &[(PathBuf, PathBuf)],
) -> Vec<PathBuf> {
    let norm = |p: &Path| crate::session_stage::normalize_path_key(p);
    let loaded_set: HashSet<String> = loaded.iter().map(|p| norm(p)).collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (src, stage) in maps {
        let folder = src
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if folder.is_empty() || !keys.iter().any(|k| folder.eq_ignore_ascii_case(k)) {
            continue;
        }
        let key = norm(stage);
        if loaded_set.contains(&key) && seen.insert(key) {
            out.push(stage.clone());
        }
    }
    out
}

pub fn session_remove(app: &AppHandle, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let drop: HashSet<String> = names.iter().map(|n| n.trim().to_ascii_lowercase()).collect();
    let cur: Vec<String> = load_session_families(app)
        .into_iter()
        .filter(|n| !drop.contains(&n.to_ascii_lowercase()))
        .collect();
    save_session_families(app, &cur);
}

#[tauri::command]
pub fn set_session_families(app: AppHandle, families: Vec<String>) -> Result<(), String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for n in families {
        let t = n.trim().to_string();
        let key = t.to_ascii_lowercase();
        if t.is_empty() || !seen.insert(key) {
            continue;
        }
        out.push(t);
    }
    save_session_families(&app, &out);
    Ok(())
}

#[tauri::command]
pub fn session_families(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(load_session_families(&app))
}

/// One-shot on upgrade to 1.0.147: apply Google-complete honesty to every family
/// folder. Old verify (trust understated `.expected`) would clear **0** latin packs;
/// the new rule clears them so Scan/Repair discover previously “complete” lies.
pub fn invalidate_google_latin_lies_once(app: &AppHandle) {
    let Ok(root) = documents_root(app) else {
        return;
    };
    let marker = root.join(".google-complete-honesty-147");
    if marker.is_file() {
        return;
    }
    for_family_dirs(app, |dir| {
        verify_complete_marker(dir);
    });
    let _ = fs::write(marker, b"1.0.147\n");
}

/// Payload for startup fail-loud toast when recovery keeps locked leftovers.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)] // emitted from Windows-only recover_stale_session
pub struct SessionRecoveryNotice {
    locked: usize,
    attempted: usize,
}

/// Emit after a short delay so the webview can bind listeners during hydrate.
#[allow(dead_code)] // called from Windows-only recover_stale_session
pub fn emit_session_recovery_toast(app: &AppHandle, locked: usize, attempted: usize) {
    if locked == 0 {
        return;
    }
    let handle = app.clone();
    let notice = SessionRecoveryNotice { locked, attempted };
    thread::spawn(move || {
        // setup spawns session_begin before UI listen; one delayed emit avoids a
        // lost event without toast-storming (name-heal style single notice).
        thread::sleep(Duration::from_millis(2200));
        let _ = handle.emit("session-recovery", &notice);
    });
}

/// Payload when Font Cache still holds Documents TTFs after Deactivate flush.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct FontCacheHeldNotice {
    locked: usize,
    access_denied: bool,
    holders: Vec<String>,
    message: String,
}

/// Only a real write-lock after Deactivate warrants the error toast. A standard
/// (non-admin) user always gets AccessDenied from the FontCache SCM restart; that
/// alone is not a failure because Documents files are never handed to GDI.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn font_cache_toast_needed(still_locked: usize, _access_denied: bool) -> bool {
    still_locked > 0
}

#[allow(dead_code)] // unload_now Windows path
pub fn emit_font_cache_held_toast(
    app: &AppHandle,
    still_paths: &[PathBuf],
    access_denied: bool,
) {
    let locked_n = still_paths.len();
    if !font_cache_toast_needed(locked_n, access_denied) {
        return;
    }
    #[cfg(windows)]
    let holders = winfont::query_file_lock_holders(still_paths);
    #[cfg(not(windows))]
    let holders: Vec<String> = Vec::new();
    let base = font_cache_held_message(locked_n);
    let message = if holders.is_empty() {
        base
    } else {
        format!("{base} (held by {})", holders.join(", "))
    };
    let notice = FontCacheHeldNotice {
        locked: locked_n,
        access_denied,
        holders,
        message,
    };
    let _ = app.emit("font-cache-held", &notice);
}

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct GdiPressureNotice {
    objects: u32,
    peak: u32,
    quota: u32,
    message: String,
}

/// Soft-warn once per process when GetGuiResources ≥ 8_000. Does not skip Add.
pub fn emit_gdi_pressure_if_high(app: &AppHandle) {
    #[cfg(windows)]
    {
        static WARNED: AtomicBool = AtomicBool::new(false);
        let count = winfont::process_gdi_objects();
        if !gdi_objects_near_quota(count) {
            return;
        }
        if WARNED.swap(true, Ordering::SeqCst) {
            return;
        }
        let peak = winfont::process_gdi_objects_peak();
        let message = gdi_pressure_message(count);
        eprintln!("Fonths: {message}");
        let notice = GdiPressureNotice {
            objects: count,
            peak,
            quota: GDI_OBJECT_DEFAULT_QUOTA,
            message,
        };
        let handle = app.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1600));
            let _ = handle.emit("gdi-pressure", &notice);
        });
    }
    let _ = app;
}

/// Drop leftover 1.0.156 session copies from the per-user Fonts tree.
/// Files there look like installed fonts in Settings after Quit. Owned folder
/// only (`...\Fonts\FontManager`). Never touch `C:\Windows\Fonts` or other
/// user fonts. Does not write the registry.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn purge_legacy_fontmanager_user_fonts_stage() {
    let Some(local) = local_app_data() else {
        return;
    };
    let dir = local
        .join("Microsoft")
        .join("Windows")
        .join("Fonts")
        .join("FontManager");
    if !dir.is_dir() {
        return;
    }
    let mut files = Vec::new();
    walk_font_files(&dir, &mut files);
    for path in &files {
        #[cfg(windows)]
        winfont::unregister(path);
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir(&dir);
}

/// Unload Documents library paths still listed in `.session-paths.txt`
/// (legacy 1.0.156). Must run before `rebuild_session_maps_in` rewrites that
/// file to stage-only. Never re-Adds Documents.
#[cfg(windows)]
pub fn unload_documents_session_leftovers(app: &AppHandle, root: &Path) {
    let leftover = load_session_paths_in(root);
    let (docs_paths, _) = crate::session_stage::partition_legacy_session_paths(&leftover);
    let docs_paths = keep_owned_recovery_paths(app, docs_paths, "Documents");
    if docs_paths.is_empty() {
        return;
    }
    eprintln!(
        "Fonths: refusing {} Documents session path(s) for GDI — unloading leftovers only",
        docs_paths.len()
    );
    let stats = winfont::unload_paths(docs_paths.clone(), false);
    if plan_font_cache_flush(stats.attempted) {
        let _ = winfont::restart_font_cache_service(font_cache_restart_budget());
    }
    for _ in 0..4 {
        let still = filter_still_write_locked(&docs_paths);
        if still.is_empty() {
            break;
        }
        let _ = winfont::unload_paths(still, false);
        thread::sleep(Duration::from_millis(120));
    }
    let still_docs = filter_still_write_locked(&docs_paths);
    if !still_docs.is_empty() {
        eprintln!(
            "Fonths: startup session recovery — {} Documents path(s) still write-locked after Remove+FontCache (attempted {}). {}",
            still_docs.len(),
            stats.attempted,
            font_cache_held_message(still_docs.len())
        );
        emit_session_recovery_toast(
            app,
            still_docs.len(),
            stats.attempted.max(still_docs.len()),
        );
    }
}

/// Recover crash/quit-without-unload leftovers before any fresh Add.
/// Called after maps validate/rebuild (see `session_begin`). Missing stage ≠
/// unlocked: never wholesale-clear maps + session-active on empty `still_locked`
/// when targets were absent. Locked leftovers stay + fail-loud.
#[allow(dead_code)]
/// c255 P2: startup recovery only touches paths this instance owns (its own
/// LOCALAPPDATA stage roots or its own Documents library). A copied/synced library
/// carrying another instance's absolute paths must never unload that instance's fonts.
pub fn recovery_path_owned(path: &Path, own_roots: &[PathBuf]) -> bool {
    let norm = |p: &Path| {
        let mut k = crate::session_stage::normalize_path_key(p);
        while k.ends_with('\\') {
            k.pop();
        }
        k
    };
    let key = norm(path);
    // Absolute only (drive or UNC) — relative ledger entries are never trusted.
    let absolute = key.len() > 2 && (key.as_bytes()[1] == b':' || key.starts_with("\\\\"));
    absolute
        && own_roots.iter().any(|r| {
            let root = norm(r);
            !root.is_empty() && key.starts_with(&format!("{root}\\"))
        })
}

/// This process's own roots for recovery: LOCALAPPDATA (gdi-maps + legacy stage)
/// and the Documents library root.
#[cfg(windows)]
pub fn recovery_own_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(local) = local_app_data() {
        roots.push(local);
    }
    if let Ok(docs) = documents_root(app) {
        roots.push(docs);
    }
    roots
}

#[cfg(windows)]
pub fn keep_owned_recovery_paths(app: &AppHandle, paths: Vec<PathBuf>, what: &str) -> Vec<PathBuf> {
    let roots = recovery_own_roots(app);
    let (own, foreign): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.into_iter().partition(|p| recovery_path_owned(p, &roots));
    if !foreign.is_empty() {
        eprintln!(
            "Fonths: startup recovery ignored {} foreign {what} path(s) not under this instance's roots",
            foreign.len()
        );
    }
    own
}

pub fn recover_stale_session(app: &AppHandle) {
    #[cfg(windows)]
    {
        let Ok(root) = documents_root(app) else {
            return;
        };
        winfont::drain_gdi_maps();
        let leftover = load_session_paths_in(&root);
        let maps = crate::session_stage::load_session_maps_in(&root);
        let had_active = session_active_file_in(&root).is_file();
        if leftover.is_empty() && maps.is_empty() && !had_active {
            return;
        }
        // Documents already peeled in session_begin; keep stage / other only.
        let (_docs_paths, keep_paths) =
            crate::session_stage::partition_legacy_session_paths(&leftover);
        let mut keep_paths = keep_owned_recovery_paths(app, keep_paths, "stage");
        // Remember maps (incl. stale) so unload can Remove both stage + legacy.
        // c255 P2: never adopt another instance's stage paths.
        let roots = recovery_own_roots(app);
        for m in &maps {
            if !recovery_path_owned(Path::new(&m.stage), &roots) {
                continue;
            }
            winfont::remember_face_map(m.clone());
            let pu = PathBuf::from(&m.stage);
            if keep_paths.iter().all(|p| p != &pu) {
                keep_paths.push(pu);
            }
        }
        if !keep_paths.is_empty() {
            // Snapshot existence BEFORE Remove — missing files look "unlocked".
            let missing_before = keep_paths.iter().any(|p| !p.is_file());
            let stats = winfont::unload_paths(keep_paths.clone(), false);
            if plan_font_cache_flush(stats.attempted) {
                let _ = winfont::restart_font_cache_service(font_cache_restart_budget());
            }
            for _ in 0..4 {
                let still = filter_still_write_locked(&keep_paths);
                if still.is_empty() {
                    break;
                }
                let _ = winfont::unload_paths(still, false);
                thread::sleep(Duration::from_millis(120));
            }
            let still = filter_still_write_locked(&keep_paths);
            let plan = plan_recover_sidecar_clear(&still, missing_before);
            apply_recover_sidecar_plan(&root, &plan);
            if !plan.keep_locked_paths.is_empty() {
                eprintln!(
                    "Fonths: startup session recovery — {} path(s) still write-locked after Remove+FontCache (attempted {}). {} Deactivate-all as admin or reboot, then Repair.",
                    still.len(),
                    stats.attempted,
                    font_cache_held_message(still.len())
                );
                emit_session_recovery_toast(app, still.len(), stats.attempted.max(still.len()));
            }
        } else {
            // No stage keep_paths. Drop legacy Documents ledger only —
            // never wipe maps / session-active (rebuild + re-register need them).
            clear_session_paths_in(&root);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

pub struct SessionBoot {
    pub running: AtomicBool,
    pub done: AtomicBool,
    pub ready: Mutex<Vec<String>>,
}

pub fn session_boot() -> &'static SessionBoot {
    static BOOT: LazyLock<SessionBoot> = LazyLock::new(|| SessionBoot {
        running: AtomicBool::new(false),
        done: AtomicBool::new(false),
        ready: Mutex::new(Vec::new()),
    });
    &BOOT
}

pub fn session_boot_note_ready(ready: &[String]) {
    if let Ok(mut g) = session_boot().ready.lock() {
        *g = ready.to_vec();
    }
}

/// Progressive Live (1.0.190): flush each successful Add into session_boot.ready
/// so hydrate can mark Activated before boot.done (~2099).
pub fn session_boot_push_ready(family: &str) {
    if !session_boot().running.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut g) = session_boot().ready.lock()
        && !g.iter().any(|n| n.eq_ignore_ascii_case(family)) {
            g.push(family.to_string());
        }
}

#[derive(Clone, Serialize)]
pub struct SessionBootState {
    pub running: bool,
    pub done: bool,
    pub ready: Vec<String>,
}

/// Hydrate waits here so last-session sidecar is not treated as this-process GDI live.
#[tauri::command]
pub fn session_boot_state() -> SessionBootState {
    let boot = session_boot();
    SessionBootState {
        running: boot.running.load(Ordering::SeqCst),
        done: boot.done.load(Ordering::SeqCst),
        ready: boot.ready.lock().map(|g| g.clone()).unwrap_or_default(),
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GdiSessionState {
    pub live_families: Vec<String>,
    pub live_family_count: usize,
    pub live_face_count: usize,
    pub staged_map_count: usize,
    pub gdi_objects: u32,
    pub gdi_objects_peak: u32,
    pub gdi_quota: u32,
    pub fast_resume: bool,
    pub boot_running: bool,
    pub boot_done: bool,
}

/// Authoritative in-process GDI session state for UI reconciliation and diagnostics.
#[tauri::command]
pub fn gdi_session_state() -> GdiSessionState {
    let boot = session_boot();
    #[cfg(windows)]
    {
        let live_families = winfont::live_family_names();
        let (live_family_count, live_face_count, staged_map_count) = winfont::live_stats();
        GdiSessionState {
            live_families,
            live_family_count,
            live_face_count,
            staged_map_count,
            gdi_objects: winfont::process_gdi_objects(),
            gdi_objects_peak: winfont::process_gdi_objects_peak(),
            gdi_quota: GDI_OBJECT_DEFAULT_QUOTA,
            fast_resume: fast_session_resume(),
            boot_running: boot.running.load(Ordering::SeqCst),
            boot_done: boot.done.load(Ordering::SeqCst),
        }
    }
    #[cfg(not(windows))]
    {
        GdiSessionState {
            live_families: Vec::new(),
            live_family_count: 0,
            live_face_count: 0,
            staged_map_count: 0,
            gdi_objects: 0,
            gdi_objects_peak: 0,
            gdi_quota: GDI_OBJECT_DEFAULT_QUOTA,
            fast_resume: fast_session_resume(),
            boot_running: boot.running.load(Ordering::SeqCst),
            boot_done: boot.done.load(Ordering::SeqCst),
        }
    }
}

pub fn session_boot_begin() {
    let boot = session_boot();
    boot.done.store(false, Ordering::SeqCst);
    boot.running.store(true, Ordering::SeqCst);
    if let Ok(mut g) = boot.ready.lock() {
        g.clear();
    }
}

pub fn session_boot_finish(ready: &[String]) {
    session_boot_note_ready(ready);
    let boot = session_boot();
    boot.running.store(false, Ordering::SeqCst);
    boot.done.store(true, Ordering::SeqCst);
}

pub type PrestagedTarget = (String, Vec<(PathBuf, PathBuf)>);

/// Partition saved session families into those already backed by valid on-disk
/// `gdi-maps` entries in `.session-maps.json` vs. those needing Documents folder scans.
pub fn partition_prestaged_targets(
    families: &[String],
    disk_maps: &[crate::session_stage::FaceMap],
) -> (Vec<PrestagedTarget>, Vec<String>) {
    if disk_maps.is_empty() {
        return (Vec::new(), families.to_vec());
    }
    let mut by_family: HashMap<String, Vec<(PathBuf, PathBuf)>> = HashMap::new();
    for m in disk_maps {
        let fam_key = m.family.trim().to_ascii_lowercase();
        if fam_key.is_empty() {
            continue;
        }
        let stage = PathBuf::from(&m.stage);
        let src = PathBuf::from(&m.source);
        if crate::session_stage::must_not_register_as_gdi_path(&stage) || !stage.is_file() {
            continue;
        }
        #[cfg(windows)]
        if winfont::is_windows_fonts_path(&src) || winfont::is_windows_fonts_path(&stage) {
            continue;
        }
        let list = by_family.entry(fam_key).or_default();
        if list.iter().all(|(s, st)| s != &src || st != &stage) {
            list.push((src, stage));
        }
    }

    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for fam in families {
        let key = fam.trim().to_ascii_lowercase();
        if let Some(pairs) = by_family.get(&key)
            && !pairs.is_empty() {
                covered.push((fam.clone(), pairs.clone()));
                continue;
            }
        uncovered.push(fam.clone());
    }
    (covered, uncovered)
}

#[cfg(windows)]
fn register_prestaged_wave(
    app: &AppHandle,
    wave: &[(String, Vec<(PathBuf, PathBuf)>)],
    own_progress: bool,
) -> (Vec<String>, Vec<String>) {
    if wave.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let workers = session_register_workers(wave.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(wave.to_vec())));
    let ready = Arc::new(Mutex::new(Vec::new()));
    let retry = Arc::new(Mutex::new(Vec::new()));
    let last_emit = Arc::new(Mutex::new(Instant::now()));
    let total_cap = bulk()
        .progress
        .lock()
        .map(|p| p.total as usize)
        .unwrap_or(wave.len());

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let app = app.clone();
        let queue = Arc::clone(&queue);
        let ready = Arc::clone(&ready);
        let retry = Arc::clone(&retry);
        let last_emit = Arc::clone(&last_emit);
        handles.push(thread::spawn(move || loop {
            if bulk().cancel.load(Ordering::SeqCst) {
                break;
            }
            let item = queue.lock().ok().and_then(|mut g| g.pop_front());
            let Some((fam, pairs)) = item else {
                break;
            };
            let mut fam_ok = false;
            let mut needs_fallback = false;
            for (src, stage) in &pairs {
                if bulk().cancel.load(Ordering::SeqCst) {
                    break;
                }
                match winfont::register_prestaged_stage_path(src, stage) {
                    winfont::RegisterOutcome::Ok => {
                        winfont::bind(&fam, src);
                        fam_ok = true;
                    }
                    winfont::RegisterOutcome::StageCopyFailed => {
                        needs_fallback = true;
                    }
                    winfont::RegisterOutcome::GdiQuotaExceeded
                    | winfont::RegisterOutcome::Unloading => {
                        needs_fallback = true;
                        break;
                    }
                    _ => {}
                }
            }
            if fam_ok {
                session_boot_push_ready(&fam);
                if let Ok(mut rg) = ready.lock() {
                    rg.push(fam.clone());
                }
                if own_progress
                    && let Ok(mut p) = bulk().progress.lock() {
                        p.done = p.done.saturating_add(1).min(total_cap as u32);
                        p.current = format!("Restoring {}/{}", p.done, total_cap);
                        if !p.ready_names.iter().any(|n| n.eq_ignore_ascii_case(&fam)) {
                            p.ready_names.push(fam.clone());
                        }
                    }
            } else if needs_fallback
                && let Ok(mut rf) = retry.lock() {
                    rf.push(fam);
                }
            if own_progress {
                let due = last_emit
                    .lock()
                    .map(|t| t.elapsed() >= Duration::from_millis(emit_progress_throttle_ms()))
                    .unwrap_or(false);
                if due {
                    emit_progress(&app);
                    if let Ok(mut t) = last_emit.lock() {
                        *t = Instant::now();
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let out_ready = ready.lock().map(|g| g.clone()).unwrap_or_default();
    let out_retry = retry.lock().map(|g| g.clone()).unwrap_or_default();
    (out_ready, out_retry)
}

#[cfg(windows)]
fn try_adopt_persistent_session(app: &AppHandle, root: &Path) -> Vec<String> {
    let session_fams = dedupe_family_names(&load_session_families(app));
    if session_fams.is_empty() {
        return Vec::new();
    }
    if let Some(maps_root) = gdi_maps_root()
        && crate::session_stage::load_session_maps_in(root).is_empty()
    {
        let _ = rebuild_session_maps_in(root, &maps_root);
    }
    let disk_maps = crate::session_stage::load_session_maps_in(root);
    if disk_maps.is_empty() {
        return Vec::new();
    }
    let hkcu_entries = winfont::list_fm_hkcu_entries();
    winfont::adopt_persistent_hkcu_session(&disk_maps, &session_fams, &hkcu_entries)
}

pub fn session_begin(app: &AppHandle) {
    session_boot_begin();
    if sandbox_root().is_some() {
        // c262 sandbox: no unload / migrate / maps / recover / restore on boot.
        session_boot_finish(&[]);
        return;
    }
    invalidate_google_latin_lies_once(app);
    #[cfg(not(windows))]
    migrate_shared_family_folders_on_boot(app);
    #[cfg(windows)]
    {
        // Ordering (Skye HOLD):
        // 1) Unload Documents leftovers from the existing session-paths ledger
        //    (rebuild rewrites that file to stage-only and would drop them).
        // 2) Validate/rebuild maps (preserve valid maps + session-active;
        //    re-stage missing copy-only) BEFORE recover clear.
        // 3) recover_stale_session — missing stage ≠ unlock→nuke.
        let daemon_active = crate::daemon::is_daemon_running();
        let root_opt = documents_root(app).ok();
        let persistent_hkcu = !daemon_active
            && crate::session_stage::hkcu_sync_enabled()
            && root_opt
                .as_ref()
                .map(|r| crate::session_stage::has_persistent_active_marker_in(r))
                .unwrap_or(false);
        let clean_exit = root_opt
            .as_ref()
            .map(|root| {
                crate::session_stage::take_clean_exit_marker_in(root)
                    && load_session_paths_in(root).is_empty()
                    && !session_active_file_in(root).is_file()
                    && crate::session_stage::load_session_maps_in(root).is_empty()
            })
            .unwrap_or(false);
        let mut persistent_adopted: Vec<String> = Vec::new();
        if daemon_active {
            // Headless daemon already holds the live GDI session — adopt its active face maps
            // immediately (<20ms) without re-scanning or re-calling AddFontResourceExW.
            if let (Some(root), Some(maps_root)) = (root_opt.as_ref(), gdi_maps_root()) {
                let _ = rebuild_session_maps_in(root, &maps_root);
                let disk_maps = crate::session_stage::load_session_maps_in(root);
                let session_fams = load_session_families(app);
                if !disk_maps.is_empty() && !session_fams.is_empty() {
                    let adopted = winfont::adopt_daemon_live_session(&disk_maps, &session_fams);
                    if !adopted.is_empty() {
                        seed_known_gdi_incapable_settled(app);
                        session_boot_finish(&adopted);
                        let handle = app.clone();
                        thread::spawn(move || {
                            let _ = index_disk(&handle, true);
                        });
                        return;
                    }
                }
            }
        } else if persistent_hkcu {
            if let Some(root) = root_opt.as_ref() {
                persistent_adopted = try_adopt_persistent_session(app, root);
                if !persistent_adopted.is_empty() {
                    let requested = dedupe_family_names(&load_session_families(app));
                    if persistent_adopted.len() >= requested.len() {
                        seed_known_gdi_incapable_settled(app);
                        session_boot_finish(&persistent_adopted);
                        let handle = app.clone();
                        thread::spawn(move || {
                            let _ = index_disk(&handle, true);
                        });
                        return;
                    }
                } else {
                    crate::session_stage::clear_persistent_active_marker_in(root);
                    if let Some(maps_root) = gdi_maps_root() {
                        let _ = rebuild_session_maps_in(root, &maps_root);
                    }
                    recover_stale_session(app);
                }
            }
        } else if clean_exit {
            migrate_shared_family_folders_on_boot(app);
        } else {
            if let Some(root) = root_opt.as_ref() {
                unload_documents_session_leftovers(app, root);
                migrate_shared_family_folders_on_boot(app);
                if let Some(maps_root) = gdi_maps_root() {
                    let _ = rebuild_session_maps_in(root, &maps_root);
                }
            } else {
                migrate_shared_family_folders_on_boot(app);
            }
            recover_stale_session(app);
        }
        if !daemon_active && persistent_adopted.is_empty() {
            winfont::clear_all_hkcu_session_entries();
        }
        // 1.0.156 leftover: session copies under the per-user Fonts tree.
        // Remove + delete that folder only — never C:\Windows\Fonts.
        purge_legacy_fontmanager_user_fonts_stage();
        // 1.0.206: seed allowlist Settled before restore / Activate All can queue Add.
        seed_known_gdi_incapable_settled(app);
        // Do NOT hydrate loaded() from gdi-maps — maps skip copy only; always Add
        // after drain / new process (session-active + maps = toast exemption only).
        // Fast-path: restore pre-staged `.session-maps.json` entries in two waves
        // (Wave 0: first 32 screen families -> immediate WM_FONTCHANGE; Wave 1: remainder)
        // skipping Documents folder walks whenever pre-staged gdi-maps already exist on disk.
        let families = dedupe_family_names(&load_session_families(app));
        let fast_resume = fast_session_resume();
        let disk_maps = if fast_resume {
            documents_root(app)
                .map(|r| crate::session_stage::load_session_maps_in(&r))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let (candidate_fams, mut settled_skip) = partition_session_restore_targets(app, &families);
        let remaining_candidates: Vec<String> = if persistent_adopted.is_empty() {
            candidate_fams
        } else {
            let adopted_set: HashSet<String> = persistent_adopted
                .iter()
                .map(|f| f.trim().to_ascii_lowercase())
                .collect();
            candidate_fams
                .into_iter()
                .filter(|f| !adopted_set.contains(&f.trim().to_ascii_lowercase()))
                .collect()
        };
        let (covered_prestaged, mut uncovered_fams) = if fast_resume && !disk_maps.is_empty() {
            partition_prestaged_targets(&remaining_candidates, &disk_maps)
        } else {
            (Vec::new(), remaining_candidates.clone())
        };

        let own_progress = !bulk().running.load(Ordering::SeqCst);
        let mut prestaged_ready: Vec<String> = persistent_adopted;
        let initial_total = families.len().saturating_sub(settled_skip.len());
        if own_progress && (initial_total > 0 || !settled_skip.is_empty()) {
            if let Ok(mut p) = bulk().progress.lock() {
                p.running = initial_total > 0;
                p.paused = false;
                p.kind = "download".into();
                p.done = 0;
                p.total = initial_total as u32;
                p.failed = 0;
                p.skipped = 0;
                p.current = if initial_total == 0 {
                    String::new()
                } else {
                    format!("Restoring 0/{initial_total}")
                };
                p.ready_names.clear();
                p.failed_names.clear();
                p.failed_details.clear();
                p.settled_names.clear();
                for name in &settled_skip {
                    if !p.settled_names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                        p.settled_names.push(name.clone());
                    }
                }
            }
            if initial_total > 0 {
                bulk().running.store(true, Ordering::SeqCst);
            }
            emit_progress_force(app);
        }

        if !covered_prestaged.is_empty() {
            const WAVE0_SIZE: usize = 32;
            let split_at = WAVE0_SIZE.min(covered_prestaged.len());
            let (wave0, wave1) = covered_prestaged.split_at(split_at);
            let _w0_hkcu_batch = HkcuBatchGuard::new();
            let (w0_ready, w0_retry) = register_prestaged_wave(app, wave0, own_progress);
            drop(_w0_hkcu_batch);
            prestaged_ready.extend(w0_ready);
            uncovered_fams.extend(w0_retry);
            if !prestaged_ready.is_empty() {
                emit_progress_force(app);
                notify_fonts_changed();
            }
            if !wave1.is_empty() && !bulk().cancel.load(Ordering::SeqCst) {
                let _hkcu_batch = HkcuBatchGuard::new();
                let (w1_ready, w1_retry) = register_prestaged_wave(app, wave1, own_progress);
                drop(_hkcu_batch);
                prestaged_ready.extend(w1_ready);
                uncovered_fams.extend(w1_retry);
            }
        }

        uncovered_fams = dedupe_family_names(&uncovered_fams);
        let mut ready_all = if uncovered_fams.is_empty() {
            Vec::new()
        } else {
            filter_ready_families_parallel(app, &uncovered_fams)
        };
        // c260 P2: partly-cut families load their good faces (cut faces are refused
        // by face_allowed_for_register) and report the real state for the UI.
        let partial = if uncovered_fams.is_empty() {
            Vec::new()
        } else {
            restore_partial_targets(app, &uncovered_fams, &ready_all)
        };
        for (fam, _) in &partial {
            if !ready_all.iter().any(|r| r.eq_ignore_ascii_case(fam)) {
                ready_all.push(fam.clone());
            }
        }
        ready_all = dedupe_family_names(&ready_all);
        // 1.0.190: known-incapable Settled never queued for Add (early-skip only).
        let (ready_targets, extra_settled) = partition_session_restore_targets(app, &ready_all);
        for name in extra_settled {
            if !settled_skip.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                settled_skip.push(name);
            }
        }
        let prestaged_done = prestaged_ready.len();
        let fallback_ready = if own_progress {
            let final_total = prestaged_done + ready_targets.len();
            if !ready_targets.is_empty() {
                if let Ok(mut p) = bulk().progress.lock() {
                    p.running = true;
                    p.total = final_total as u32;
                }
                emit_progress_force(app);
                let _hkcu_batch = HkcuBatchGuard::new();
                let registered = register_on_disk_parallel_progress(app, &ready_targets, prestaged_done);
                drop(_hkcu_batch);
                if let Ok(mut p) = bulk().progress.lock() {
                    p.running = false;
                    p.paused = false;
                    p.current.clear();
                    let cap = final_total as u32;
                    p.total = cap;
                    p.done = (p.ready_names.len() as u32).min(cap);
                }
                bulk().running.store(false, Ordering::SeqCst);
                emit_progress_force(app);
                registered
            } else {
                if let Ok(mut p) = bulk().progress.lock() {
                    p.running = false;
                    p.paused = false;
                    p.current.clear();
                    let cap = prestaged_done as u32;
                    p.total = cap;
                    p.done = (p.ready_names.len() as u32).min(cap);
                }
                bulk().running.store(false, Ordering::SeqCst);
                emit_progress_force(app);
                Vec::new()
            }
        } else if !ready_targets.is_empty() {
            // User job owns the bar — still register (and progressive boot.ready).
            let _hkcu_batch = HkcuBatchGuard::new();
            register_ready_families_parallel(app, &ready_targets).1
        } else {
            Vec::new()
        };
        let mut ready = prestaged_ready;
        for r in fallback_ready {
            if !ready.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
                ready.push(r);
            }
        }
        if !partial.is_empty() {
            if let Ok(mut p) = bulk().progress.lock() {
                for (fam, cut) in &partial {
                    if ready.iter().any(|r| r.eq_ignore_ascii_case(fam)) {
                        let d = restore_partial_detail(fam, *cut);
                        if !p.failed_details.iter().any(|x| x == &d) {
                            p.failed_details.push(d);
                        }
                    }
                }
            }
            emit_progress_force(app);
        }
        if !ready.is_empty() {
            // Reconcile HKCU diff-only so unchanged registry values avoid redundant rewrites.
            winfont::sync_hkcu_all_active_stages();
            persist_activation_sidecars(app);
            notify_fonts_changed();
        }
        let cancelled = bulk().cancel.load(Ordering::SeqCst);
        let gdi_capped = gdi_objects_at_hard_ceiling(winfont::process_gdi_objects());
        let reconciled = merge_session_families_after_restore(
            &families,
            &ready,
            cancelled,
            gdi_capped,
            |fam| {
                family_locations(app, fam)
                    .iter()
                    .any(|dir| dir.is_dir() && dir_has_intact(dir))
            },
        );
        if reconciled.len() != families.len() {
            save_session_families(app, &reconciled);
        }
        // Restore may have cleared settled_names; re-seed allowlist on disk
        // (Gidugu hard-allowlist not in last-session list still skip Activate All).
        seed_known_gdi_incapable_settled(app);
        session_boot_finish(&ready);
        emit_gdi_pressure_if_high(app);
        let handle = app.clone();
        thread::spawn(move || {
            let _ = index_disk(&handle, true);
        });
    }
    #[cfg(not(windows))]
    {
        // Non-Windows: still drop leftover sidecar files from a copied library.
        if let Ok(root) = documents_root(app) {
            if session_paths_file_in(&root).is_file() || session_active_file_in(&root).is_file() {
                clear_session_sidecars_in(&root);
            }
        }
        session_boot_finish(&[]);
        let handle = app.clone();
        thread::spawn(move || {
            let _ = index_disk(&handle, true);
        });
    }
}

pub fn session_end(app: &AppHandle) {
    #[cfg(windows)]
    {
        static ENDING: AtomicBool = AtomicBool::new(false);
        if ENDING.swap(true, Ordering::SeqCst) {
            return;
        }
        bulk().cancel.store(true, Ordering::SeqCst);
        bulk().running.store(false, Ordering::SeqCst);
        winfont::begin_unload();
        // Immediately clear HKCU session font entries (<5ms) so even an interrupted quit
        // leaves zero `[FM-Session-...]` registry entries for Adobe CoolType / DirectWrite.
        winfont::clear_all_hkcu_session_entries();
        winfont::wait_in_flight(Duration::from_millis(250));
        // Persist stage-only paths before Remove so a hung quit watchdog still
        // has leftovers for next boot. Never write Documents into session-paths.
        let mut extra = load_session_paths(app);
        extra = merge_unique_paths(extra, winfont::snapshot_stage_paths());
        let live_maps = winfont::snapshot_face_maps();
        extra = merge_unique_paths(extra, crate::session_stage::stage_paths_from_maps(&live_maps));
        if let Ok(root) = documents_root(app) {
            let disk_maps = crate::session_stage::load_session_maps_in(&root);
            extra = merge_unique_paths(extra, crate::session_stage::stage_paths_from_maps(&disk_maps));
            if !live_maps.is_empty() {
                crate::session_stage::save_session_maps_in(&root, &live_maps);
            }
            extra = crate::session_stage::filter_session_paths_refuse_documents(&extra);
            save_session_paths_in(&root, &extra);
        } else {
            extra = crate::session_stage::filter_session_paths_refuse_documents(&extra);
        }
        let attempted = extra.len();
        // Broadcast WM_FONTCHANGE (non-blocking SendNotifyMessageW) after RemoveFontResourceExW
        // + GdiFlush + HKCU cleanup so Adobe Photoshop / Illustrator / InDesign drop all
        // unloaded session fonts immediately on app close.
        let stats = winfont::unload_paths(extra.clone(), true);
        // No write-lock probe of thousands of files — that stalled quit.
        // Next boot recover_stale_session Removes leftovers.
        let plan = plan_session_end_cleanup(attempted.max(stats.attempted), &[]);
        if let Some(msg) = &plan.fail_loud {
            eprintln!("{msg}");
        } else if stats.attempted > 0 && stats.removed_ok * 2 < stats.attempted {
            eprintln!(
                "Fonths: session unload Remove acknowledged {}/{} paths (rest already absent or refcount miss). Sidecars cleared.",
                stats.removed_ok, stats.attempted
            );
        }
        if plan.clear_active && plan.clear_paths {
            clear_session_sidecars(app);
        } else if plan.clear_active {
            clear_session_active(app);
            if !plan.keep_paths.is_empty() {
                let keep = crate::session_stage::filter_session_paths_refuse_documents(&plan.keep_paths);
                save_session_paths(app, &keep);
            }
        } else if plan.clear_paths {
            clear_session_paths(app);
        } else if !plan.keep_paths.is_empty() {
            let keep = crate::session_stage::filter_session_paths_refuse_documents(&plan.keep_paths);
            save_session_paths(app, &keep);
        }
        if plan.clear_maps {
            clear_session_maps(app);
        }
        if let Ok(root) = documents_root(app) {
            crate::session_stage::clear_persistent_active_marker_in(&root);
        }
        if plan.fail_loud.is_none() && plan.keep_paths.is_empty()
            && let Ok(root) = documents_root(app) {
                crate::session_stage::write_clean_exit_marker_in(&root);
            }
    }
    #[cfg(not(windows))]
    {
        if let Ok(root) = documents_root(app) {
            crate::session_stage::clear_persistent_active_marker_in(&root);
        }
        clear_session_sidecars(app);
    }
}

/// Flush live session maps & paths right before handing ownership off to the headless daemon.
pub fn persist_session_handoff(app: &AppHandle) {
    persist_activation_sidecars(app);
}

/// Flush live session maps, diff-reconcile HKCU per-user font keys, and stamp `.persistent-active`
/// before exiting in Zero-RAM Instant mode (active fonts stay live with 0 MB app RAM).
pub fn persist_persistent_zero_ram_session(app: &AppHandle) {
    persist_activation_sidecars(app);
    #[cfg(windows)]
    {
        if crate::session_stage::hkcu_sync_enabled() {
            let stages = crate::session_stage::filter_session_paths_refuse_documents(
                &winfont::snapshot_stage_paths(),
            );
            let _ = winfont::diff_reconcile_hkcu_entries(&stages);
        }
        if let Ok(root) = documents_root(app) {
            let has_active = !winfont::loaded_is_empty()
                || !crate::session_stage::load_session_maps_in(&root).is_empty();
            if has_active {
                let _ = crate::session_stage::take_clean_exit_marker_in(&root);
                crate::session_stage::write_persistent_active_marker_in(&root);
            } else {
                crate::session_stage::clear_persistent_active_marker_in(&root);
            }
        }
        notify_fonts_changed();
    }
}

/// Headless pure-Rust login activation (`fonths.exe --daemon`): registers remembered
/// `.session-families.json` families + pre-staged `gdi-maps` in parallel without WebView2.
pub fn session_begin_headless() {
    #[cfg(windows)]
    {
        let Some(root) = documents_root_headless() else {
            return;
        };
        let active_file = session_active_file_in(&root);
        let families: Vec<String> = fs::read(&active_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .map(|v| dedupe_family_names(&v))
            .unwrap_or_default();
        let disk_maps = crate::session_stage::load_session_maps_in(&root);
        if families.is_empty() && disk_maps.is_empty() {
            return;
        }
        let fast_resume = fast_session_resume();
        let mut to_register_direct: Vec<(String, PathBuf, PathBuf)> = Vec::new();
        let mut to_register: Vec<(String, PathBuf)> = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        let (covered_prestaged, uncovered_fams) = if fast_resume && !disk_maps.is_empty() {
            partition_prestaged_targets(&families, &disk_maps)
        } else {
            (Vec::new(), families.clone())
        };
        for (fam, pairs) in covered_prestaged {
            if known_gdi_incapable_entry(&fam).is_some() {
                continue;
            }
            for (src, stage) in pairs {
                if seen_paths.insert(src.clone()) {
                    to_register_direct.push((fam.clone(), src, stage));
                }
            }
        }

        for fam in &uncovered_fams {
            if known_gdi_incapable_entry(fam).is_some() {
                continue;
            }
            for dir in family_locations_in(&root, fam) {
                if !dir.is_dir() {
                    continue;
                }
                let mut files = Vec::new();
                walk_font_files(&dir, &mut files);
                sort_faces_var_first(&mut files);
                for path in files {
                    if !fast_resume && !ttf_intact(&path) {
                        continue;
                    }
                    if seen_paths.insert(path.clone()) {
                        to_register.push((fam.clone(), path));
                    }
                }
            }
        }
        // Also re-register any watched-folder / external local font paths recorded in `.session-maps.json`.
        for entry in &disk_maps {
            let src = PathBuf::from(&entry.source);
            let stage = PathBuf::from(&entry.stage);
            if src.is_file() && seen_paths.insert(src.clone()) {
                let fam = if !entry.family.trim().is_empty() {
                    entry.family.trim().to_string()
                } else {
                    src.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Local")
                        .to_string()
                };
                if fast_resume && stage.is_file() {
                    to_register_direct.push((fam, src, stage));
                } else {
                    to_register.push((fam, src));
                }
            }
        }
        if to_register_direct.is_empty() && to_register.is_empty() {
            return;
        }

        let _hkcu_batch = HkcuBatchGuard::new();
        let added_any = Arc::new(AtomicBool::new(false));
        if !to_register_direct.is_empty() {
            let workers = session_register_workers(to_register_direct.len());
            let queue = Arc::new(Mutex::new(VecDeque::from(to_register_direct)));
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let q = Arc::clone(&queue);
                let flag = Arc::clone(&added_any);
                handles.push(thread::spawn(move || loop {
                    let next = q.lock().ok().and_then(|mut g| g.pop_front());
                    let Some((fam, src, stage)) = next else {
                        break;
                    };
                    if winfont::register_prestaged_stage_path(&src, &stage)
                        == winfont::RegisterOutcome::Ok
                    {
                        winfont::bind(&fam, &src);
                        flag.store(true, Ordering::SeqCst);
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
        }
        if !to_register.is_empty() {
            let workers = session_register_workers(to_register.len());
            let queue = Arc::new(Mutex::new(VecDeque::from(to_register)));
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                let q = Arc::clone(&queue);
                let flag = Arc::clone(&added_any);
                handles.push(thread::spawn(move || loop {
                    let next = q.lock().ok().and_then(|mut g| g.pop_front());
                    let Some((fam, path)) = next else {
                        break;
                    };
                    if winfont::register_detailed_with_fast(&path, fast_resume)
                        == winfont::RegisterOutcome::Ok
                    {
                        winfont::bind(&fam, &path);
                        flag.store(true, Ordering::SeqCst);
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
        }
        drop(_hkcu_batch);

        if added_any.load(Ordering::SeqCst) {
            winfont::sync_hkcu_all_active_stages();
            let maps = winfont::snapshot_face_maps();
            let stage_paths = crate::session_stage::filter_session_paths_refuse_documents(
                &winfont::snapshot_stage_paths(),
            );
            if !maps.is_empty() {
                crate::session_stage::save_session_maps_in(&root, &maps);
            }
            if !stage_paths.is_empty() {
                save_session_paths_in(&root, &stage_paths);
            }
            notify_fonts_changed();
        }
    }
}

#[cfg(windows)]
fn unload_headless_session_paths(include_all_staged: bool) {
    winfont::begin_unload();
    winfont::clear_all_hkcu_session_entries();
    let mut extra = winfont::snapshot_stage_paths();
    let live_maps = winfont::snapshot_face_maps();
    extra = merge_unique_paths(extra, crate::session_stage::stage_paths_from_maps(&live_maps));
    if include_all_staged {
        extra = merge_unique_paths(extra, winfont::list_gdi_maps_files());
    }
    let root_opt = documents_root_headless();
    if let Some(ref root) = root_opt {
        extra = merge_unique_paths(extra, load_session_paths_in(root));
        let disk_maps = crate::session_stage::load_session_maps_in(root);
        extra = merge_unique_paths(extra, crate::session_stage::stage_paths_from_maps(&disk_maps));
    }
    extra = crate::session_stage::filter_session_paths_refuse_documents(&extra);
    let _ = winfont::unload_paths(extra, true);
    if let Some(ref root) = root_opt {
        clear_session_sidecars_in(root);
        crate::session_stage::clear_persistent_active_marker_in(root);
        crate::session_stage::clear_session_maps_in(root);
        crate::session_stage::write_clean_exit_marker_in(root);
    }
    notify_fonts_changed();
}

/// Headless pure-Rust daemon exit: unloads every staged session path, clears HKCU keys,
/// broadcasts `WM_FONTCHANGE`, clears sidecars, and writes `.session-clean-exit`.
pub fn session_end_headless() {
    #[cfg(windows)]
    unload_headless_session_paths(false);
}

/// CLI `--deactivate-all-and-exit` hook (NSIS uninstaller): cleanly unloads all staged
/// session paths, removes all `Fonths Session ` HKCU entries, clears sidecars + `.persistent-active`,
/// and broadcasts `WM_FONTCHANGE` without touching `C:\Windows\Fonts`.
pub fn deactivate_all_and_exit_headless() {
    #[cfg(windows)]
    unload_headless_session_paths(true);
}
