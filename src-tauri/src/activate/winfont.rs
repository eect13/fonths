//! Win32 Session Table engine (AddFontResourceExW / RemoveFontResourceExW, HKCU sync, Restart Manager).

use super::*;

#[cfg(windows)]
pub mod winfont {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    #[cfg(not(test))]
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn AddFontResourceExW(lpsz_filename: *const u16, fl: u32, pdv: *mut core::ffi::c_void) -> i32;
        fn RemoveFontResourceExW(lpsz_filename: *const u16, fl: u32, pdv: *mut core::ffi::c_void) -> i32;
        fn GdiFlush() -> i32;
    }

    // c260 P2: tests never touch the real Windows font session — Add/Remove/
    // broadcast are inert fakes (Add returns 0, like a refused synthetic font).
    #[cfg(test)]
    #[allow(non_snake_case)]
    unsafe fn AddFontResourceExW(_f: *const u16, _fl: u32, _p: *mut core::ffi::c_void) -> i32 {
        0
    }
    #[cfg(test)]
    #[allow(non_snake_case)]
    unsafe fn RemoveFontResourceExW(_f: *const u16, _fl: u32, _p: *mut core::ffi::c_void) -> i32 {
        0
    }
    #[cfg(test)]
    #[allow(non_snake_case)]
    unsafe fn GdiFlush() -> i32 {
        1
    }
    #[cfg(test)]
    #[allow(non_snake_case)]
    unsafe fn SendNotifyMessageW(_h: isize, _m: u32, _w: usize, _l: isize) -> i32 {
        1
    }

    #[cfg(not(test))]
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendNotifyMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetGuiResources(h_process: isize, ui_flags: u32) -> u32;
    }

    #[cfg(not(test))]
    #[link(name = "rstrtmgr")]
    unsafe extern "system" {
        fn RmStartSession(p_session_handle: *mut u32, dw_session_flags: u32, str_session_key: *mut u16) -> u32;
        fn RmRegisterResources(
            dw_session_handle: u32,
            n_files: u32,
            rgs_file_names: *const *const u16,
            n_applications: u32,
            rg_applications: *const core::ffi::c_void,
            n_services: u32,
            rgs_service_names: *const *const u16,
        ) -> u32;
        fn RmGetList(
            dw_session_handle: u32,
            pn_proc_info_needed: *mut u32,
            pn_proc_info: *mut u32,
            rg_affected_apps: *mut RmProcessInfo,
            lpdw_reboot_reasons: *mut u32,
        ) -> u32;
        fn RmEndSession(dw_session_handle: u32) -> u32;
    }

    #[cfg(not(test))]
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(non_snake_case)]
    struct RmUniqueProcess {
        dwProcessId: u32,
        ProcessStartTime: [u32; 2],
    }

    #[cfg(not(test))]
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(non_snake_case)]
    struct RmProcessInfo {
        Process: RmUniqueProcess,
        strAppName: [u16; 256],
        strServiceShortName: [u16; 64],
        ApplicationType: u32,
        AppStatus: u32,
        TSSessionId: u32,
        bRestartable: i32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
    }

    const HWND_BROADCAST: isize = 0xffff;
    const WM_FONTCHANGE: u32 = 0x001D;
    const GR_GDIOBJECTS: u32 = 0;
    const GR_GDIOBJECTS_PEAK: u32 = 2;
    /// Enumerable session font (same as AddFontResourceW). Not FR_PRIVATE — Word/Adobe must see it.
    const FR_ENUMERABLE: u32 = 0;

    fn loaded() -> &'static Mutex<HashSet<PathBuf>> {
        static LOADED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
        LOADED.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn by_family() -> &'static Mutex<HashMap<String, HashSet<PathBuf>>> {
        static M: OnceLock<Mutex<HashMap<String, HashSet<PathBuf>>>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn last_notify() -> &'static Mutex<Option<Instant>> {
        static T: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(None))
    }

    fn dirty() -> &'static AtomicBool {
        static D: OnceLock<AtomicBool> = OnceLock::new();
        D.get_or_init(|| AtomicBool::new(false))
    }

    fn unloading() -> &'static AtomicBool {
        static U: OnceLock<AtomicBool> = OnceLock::new();
        U.get_or_init(|| AtomicBool::new(false))
    }

    fn in_gdi() -> &'static AtomicU32 {
        static N: OnceLock<AtomicU32> = OnceLock::new();
        N.get_or_init(|| AtomicU32::new(0))
    }

    /// Process-wide lock for AddFontResourceExW / RemoveFontResourceExW only.
    /// Parallel session register (≤6 families) may walk disk concurrently; GDI
    /// Add/Remove must not overlap (Skye P1 — intermittent missed Adds / rare
    /// GDI weirdness on ~11k-path restore).
    fn gdi_api() -> &'static Mutex<()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    fn maps() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
        static M: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn remember_map(src: &Path, gdi: &Path) {
        if src == gdi {
            return;
        }
        if let Ok(mut m) = maps().lock() {
            m.insert(src.to_path_buf(), gdi.to_path_buf());
        }
    }

    fn gdi_path_for(src: &Path) -> PathBuf {
        if let Ok(m) = maps().lock()
            && let Some(p) = m.get(src) {
                return p.clone();
            }
        super::gdi_map_dest_for(src)
            .filter(|p| p.is_file())
            .unwrap_or_else(|| src.to_path_buf())
    }

    fn drop_map_file(gdi: &Path) {
        if super::is_gdi_maps_dir_name(gdi) && gdi.is_file() {
            let _ = fs::remove_file(gdi);
        }
    }

    fn remove_mapped_keep_file(path: &Path) -> bool {
        let gdi = gdi_path_for(path);
        remove_hkcu_session_entry(&gdi);
        let mut any = remove_one(&gdi);
        if !any
            && gdi != path
            && !crate::session_stage::must_not_register_as_gdi_path(path)
            && remove_one(path)
        {
            any = true;
        }
        any
    }

    fn defer_hkcu_depth() -> &'static AtomicUsize {
        static D: OnceLock<AtomicUsize> = OnceLock::new();
        D.get_or_init(|| AtomicUsize::new(0))
    }

    pub fn begin_hkcu_batch() {
        defer_hkcu_depth().fetch_add(1, Ordering::SeqCst);
    }

    pub fn finish_hkcu_batch() {
        if defer_hkcu_depth().load(Ordering::SeqCst) == 0 {
            sync_hkcu_all_active_stages();
            return;
        }
        if defer_hkcu_depth().fetch_sub(1, Ordering::SeqCst) == 1 {
            sync_hkcu_all_active_stages();
        }
    }

    pub fn sync_hkcu_all_active_stages() {
        let stages = snapshot_stage_paths();
        let _ = diff_reconcile_hkcu_entries(&stages);
    }

    pub fn list_fm_hkcu_entries() -> HashMap<String, String> {
        crate::session_stage::hkcu_session::list_fm_entries(super::sandbox_active())
    }

    pub fn diff_reconcile_hkcu_entries(paths: &[PathBuf]) -> (usize, usize) {
        crate::session_stage::hkcu_session::diff_reconcile_entries(paths, super::sandbox_active())
    }

    fn sync_hkcu_session_entry(gdi: &Path) {
        if defer_hkcu_depth().load(Ordering::SeqCst) > 0 {
            return;
        }
        crate::session_stage::hkcu_session::sync_entry(gdi, super::sandbox_active());
    }

    fn remove_hkcu_session_entry(gdi: &Path) {
        crate::session_stage::hkcu_session::remove_entry(gdi, super::sandbox_active());
    }

    pub fn clear_all_hkcu_session_entries() {
        crate::session_stage::hkcu_session::clear_all(super::sandbox_active());
    }

    pub fn count_hkcu_session_entries() -> usize {
        crate::session_stage::hkcu_session::list_fm_entries(super::sandbox_active()).len()
    }

    pub fn prune_stale_hkcu_session_entries(dry_run: bool) -> (usize, usize) {
        crate::session_stage::hkcu_session::prune_stale_entries(dry_run, super::sandbox_active())
    }

    fn forget_map(path: &Path) {
        let gdi = gdi_path_for(path);
        drop_map_file(&gdi);
        if let Ok(mut m) = maps().lock() {
            m.remove(path);
            m.remove(&gdi);
        }
    }

    /// Enumerate all staged font files currently inside `%LOCALAPPDATA%\Font Manager\gdi-maps`.
    #[allow(dead_code)]
    pub fn list_gdi_maps_files() -> Vec<PathBuf> {
        let Some(dir) = super::gdi_maps_root() else {
            return Vec::new();
        };
        let Ok(rd) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() {
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Drain GDI on leftover map copies. Keep the files so the next register is a cheap Add.
    pub fn drain_gdi_maps() {
        let Some(dir) = super::gdi_maps_root() else {
            return;
        };
        let Ok(rd) = fs::read_dir(&dir) else {
            return;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() {
                let _ = remove_one(&p);
            }
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Cap so a stuck font driver cannot hang Quit/Deactivate.
    const REMOVE_DRAIN_MAX: u32 = 32;

    /// Drain GDI refcount: call RemoveFontResourceExW (same flags as Add) until
    /// it returns 0 — MS docs / FontBase-style drain loop. Not just a double-Remove.
    /// Serialized behind `gdi_api()` so parallel register cannot overlap Removes.
    fn remove_one(path: &Path) -> bool {
        if is_windows_fonts_path(path) {
            return false;
        }
        let w = wide(path);
        let mut any = false;
        let _gdi = gdi_api().lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            for _ in 0..REMOVE_DRAIN_MAX {
                let n = RemoveFontResourceExW(w.as_ptr(), FR_ENUMERABLE, std::ptr::null_mut());
                if n == 0 {
                    break;
                }
                any = true;
            }
            // Also drain any process-private GDI handles (FR_PRIVATE = 0x10) so file locks
            // held by in-process GDI references never block move/rename/delete.
            const FR_PRIVATE_FLAG: u32 = 0x10;
            for _ in 0..4 {
                let n = RemoveFontResourceExW(w.as_ptr(), FR_PRIVATE_FLAG, std::ptr::null_mut());
                if n == 0 {
                    break;
                }
                any = true;
            }
        }
        any
    }

    pub fn begin_unload() {
        unloading().store(true, Ordering::SeqCst);
    }

    pub fn wait_in_flight(timeout: Duration) {
        let start = Instant::now();
        while in_gdi().load(Ordering::SeqCst) > 0 && start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn snapshot_loaded() -> Vec<PathBuf> {
        loaded()
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// LocalAppData stage paths currently mapped for GDI (never Documents).
    pub fn snapshot_stage_paths() -> Vec<PathBuf> {
        let loaded = snapshot_loaded();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(m) = maps().lock() {
            for src in &loaded {
                if let Some(gdi) = m.get(src) {
                    if crate::session_stage::must_not_register_as_gdi_path(gdi) {
                        continue;
                    }
                    if seen.insert(gdi.clone()) {
                        out.push(gdi.clone());
                    }
                    continue;
                }
                // Already a stage path (re-Add of gdi-maps file).
                if super::is_gdi_maps_dir_name(src)
                    && !crate::session_stage::must_not_register_as_gdi_path(src)
                    && seen.insert(src.clone())
                {
                    out.push(src.clone());
                }
            }
        }
        out
    }

    /// Persistable FaceMap rows for `.session-maps.json`.
    pub fn snapshot_face_maps() -> Vec<crate::session_stage::FaceMap> {
        let mut out = Vec::new();
        if let Ok(m) = maps().lock() {
            for (src, gdi) in m.iter() {
                if crate::session_stage::must_not_register_as_gdi_path(gdi) {
                    continue;
                }
                out.push(crate::session_stage::FaceMap {
                    source: src.to_string_lossy().into_owned(),
                    stage: gdi.to_string_lossy().into_owned(),
                    registry_name: crate::session_stage::session_hkcu_value_name(gdi),
                    family: String::new(),
                });
            }
        }
        out
    }

    pub fn remember_face_map(map: crate::session_stage::FaceMap) {
        let src = PathBuf::from(&map.source);
        let gdi = PathBuf::from(&map.stage);
        if src.as_os_str().is_empty() || gdi.as_os_str().is_empty() {
            return;
        }
        if crate::session_stage::must_not_register_as_gdi_path(&gdi) {
            return;
        }
        remember_map(&src, &gdi);
    }

    fn adopt_face_map(
        m: &crate::session_stage::FaceMap,
        require_min_bytes: Option<u64>,
        hkcu_entries: Option<&HashMap<String, String>>,
        allowed_families: Option<&HashSet<String>>,
    ) -> Option<String> {
        let src = PathBuf::from(&m.source);
        let gdi = PathBuf::from(&m.stage);
        if src.as_os_str().is_empty()
            || gdi.as_os_str().is_empty()
            || crate::session_stage::must_not_register_as_gdi_path(&gdi)
            || is_windows_fonts_path(&gdi)
            || is_windows_fonts_path(&src)
            || !gdi.is_file()
        {
            return None;
        }
        if let Some(min_bytes) = require_min_bytes {
            let len = fs::metadata(&gdi).map(|md| md.len()).unwrap_or(0);
            if len < min_bytes {
                return None;
            }
        }
        if let Some(entries) = hkcu_entries {
            let val_name = crate::session_stage::session_hkcu_value_name(&gdi);
            let target = crate::session_stage::normalize_registry_font_path(&gdi);
            let matched = entries
                .get(&val_name)
                .map(|cur| cur.eq_ignore_ascii_case(&target))
                .unwrap_or(false);
            if !matched {
                return None;
            }
        }
        let fam = m.family.trim();
        let chosen = if !fam.is_empty() {
            fam.to_string()
        } else if let Some(parent_name) = src
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
        {
            let fam_name = super::dir_family_name(src.parent().unwrap_or(Path::new("")));
            if fam_name.is_empty() {
                parent_name.to_string()
            } else {
                fam_name
            }
        } else {
            String::new()
        };
        if chosen.is_empty() {
            return None;
        }
        let lower = chosen.to_lowercase();
        if let Some(allow) = allowed_families
            && !allow.contains(&lower)
        {
            return None;
        }
        remember_map(&src, &gdi);
        if let Ok(mut g) = loaded().lock() {
            g.insert(src.clone());
        }
        bind(&chosen, &src);
        Some(lower)
    }

    /// Adopt the live Windows session held by the running headless `--daemon` process so
    /// reopening the UI window is instant (<20ms) without re-scanning or re-calling GDI.
    /// ONLY call when `crate::daemon::is_daemon_running()` is verified true.
    pub fn adopt_daemon_live_session(
        disk_maps: &[crate::session_stage::FaceMap],
        session_families: &[String],
    ) -> Vec<String> {
        let mut adopted_families: HashSet<String> = HashSet::new();
        for m in disk_maps {
            if let Some(lower) = adopt_face_map(m, None, None, None) {
                adopted_families.insert(lower);
            }
        }
        session_families
            .iter()
            .filter(|f| adopted_families.contains(&f.trim().to_lowercase()))
            .cloned()
            .collect()
    }

    /// Adopt persistent per-user HKCU session entries when `.persistent-active` is present,
    /// verifying that each staged file (`>= 256` bytes) exists outside Documents / `Windows\Fonts`
    /// and matches `HKCU\...\Fonts` (`hkcu_entries`).
    pub fn adopt_persistent_hkcu_session(
        disk_maps: &[crate::session_stage::FaceMap],
        session_families: &[String],
        hkcu_entries: &HashMap<String, String>,
    ) -> Vec<String> {
        if disk_maps.is_empty() || session_families.is_empty() || hkcu_entries.is_empty() {
            return Vec::new();
        }
        let allowed: HashSet<String> = session_families
            .iter()
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty())
            .collect();
        let mut adopted_families: HashSet<String> = HashSet::new();
        for m in disk_maps {
            if let Some(lower) = adopt_face_map(m, Some(256), Some(hkcu_entries), Some(&allowed)) {
                adopted_families.insert(lower);
            }
        }
        session_families
            .iter()
            .filter(|f| adopted_families.contains(&f.trim().to_lowercase()))
            .cloned()
            .collect()
    }

    /// System + other per-user fonts we must never Add/Remove.
    /// Exception: our 1.0.156 stage `%LOCALAPPDATA%\Microsoft\Windows\Fonts\FontManager`
    /// — those files are session copies, not installed fonts. Treating them as
    /// sacred left faces in Settings → Fonts after Quit.
    pub(crate) fn is_windows_fonts_path(path: &Path) -> bool {
        if crate::session_stage::is_legacy_fontmanager_stage_path(path) {
            return false;
        }
        let lower = path.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
        lower.contains("\\windows\\fonts")
    }

    /// This process's GDI object count (HFONT/HDC/HBITMAP…). Not "fonts Added".
    pub fn process_gdi_objects() -> u32 {
        unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) }
    }

    pub fn process_gdi_objects_peak() -> u32 {
        unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS_PEAK) }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RegisterOutcome {
        Ok,
        StageCopyFailed,
        AddReturnedZero,
        GdiQuotaExceeded,
        InvalidFontHeader,
        Unloading,
        Refused,
    }

    impl RegisterOutcome {
        pub fn ok(self) -> bool {
            matches!(self, Self::Ok)
        }

        #[allow(dead_code)] // debug / toast twin of RegisterFailKind::label
        pub fn label(self) -> &'static str {
            match self {
                Self::Ok => "ok",
                Self::StageCopyFailed => "stage-copy failed (ensure_gdi_session_copy)",
                Self::AddReturnedZero => "AddFontResourceExW returned 0",
                Self::GdiQuotaExceeded => "paused — Windows GDI object ceiling reached (prevented crash)",
                Self::InvalidFontHeader => "invalid or corrupt font header (skipped before GDI)",
                Self::Unloading => "unloading — register aborted",
                Self::Refused => "register refused",
            }
        }
    }

    /// True when this source has a size-matched LocalAppData gdi-map.
    /// Map file = skip **copy** only (`ensure_gdi_session_copy`). Never hydrate
    /// `loaded()` — skip-Add requires a real in-process AddFontResourceEx success.
    pub fn is_session_live_mapped(path: &Path) -> bool {
        if unloading().load(Ordering::SeqCst) {
            return false;
        }
        let src_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if src_len < 256 {
            return false;
        }
        let dest_ok = super::gdi_map_dest_for(path)
            .and_then(|d| fs::metadata(d).ok())
            .map(|m| m.len() == src_len)
            .unwrap_or(false);
        if dest_ok {
            return true;
        }
        // Also accept an explicit maps() entry (legacy path key variants).
        maps()
            .lock()
            .ok()
            .and_then(|m| m.get(path).cloned())
            .and_then(|d| fs::metadata(d).ok())
            .map(|m| m.len() == src_len)
            .unwrap_or(false)
    }

    /// True when this process already successfully Add'd `path` (in-memory only).
    pub fn is_loaded(path: &Path) -> bool {
        loaded()
            .lock()
            .map(|g| g.contains(path))
            .unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn register(path: &Path) -> bool {
        register_detailed(path).ok()
    }

    pub fn register_detailed(path: &Path) -> RegisterOutcome {
        register_detailed_with_fast(path, false)
    }

    pub fn register_detailed_with_fast(path: &Path, preverified_stage: bool) -> RegisterOutcome {
        if is_windows_fonts_path(path) {
            return RegisterOutcome::Refused;
        }
        if unloading().load(Ordering::SeqCst) {
            return RegisterOutcome::Unloading;
        }
        // Crash guard: refuse cleanly before hitting Windows's 10,000 per-process GDI handle wall.
        if super::gdi_objects_at_hard_ceiling(process_gdi_objects()) {
            return RegisterOutcome::GdiQuotaExceeded;
        }
        // Stability guard: reject truncated/HTML/non-SFNT files before passing to win32kfull/fontdrvhost.
        // Skip redundant source file open when a size-matched pre-staged gdi-maps copy is already verified.
        if !preverified_stage && !super::has_valid_sfnt_magic(path) {
            return RegisterOutcome::InvalidFontHeader;
        }
        let already = loaded()
            .lock()
            .map(|g| g.contains(path))
            .unwrap_or(false);
        if already {
            let src_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let dest_ok = super::gdi_map_dest_for(path)
                .and_then(|d| fs::metadata(d).ok())
                .map(|m| m.len() == src_len && src_len >= 256)
                .unwrap_or(false);
            if dest_ok {
                if let Some(dest) = super::gdi_map_dest_for(path) {
                    remember_map(path, &dest);
                    sync_hkcu_session_entry(&dest);
                }
                return RegisterOutcome::Ok;
            }
            // Source changed — drain GDI on the stale map, then recopy below.
            let _ = remove_mapped_keep_file(path);
            forget_map(path);
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
        }
        {
            let Ok(mut g) = loaded().lock() else {
                return RegisterOutcome::Refused;
            };
            if !g.insert(path.to_path_buf()) {
                // Another worker already inserted this exact path into `loaded()`;
                // verify its staged map is intact instead of reporting a false failure.
                drop(g);
                if is_session_live_mapped(path) {
                    return RegisterOutcome::Ok;
                }
                return RegisterOutcome::Refused;
            }
        }
        if unloading().load(Ordering::SeqCst) {
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
            return RegisterOutcome::Unloading;
        }
        // Copy to %LOCALAPPDATA%\Font Manager\gdi-maps and Add THAT path.
        // Documents library paths must never reach AddFontResourceExW.
        let Some(gdi) = super::ensure_gdi_session_copy(path) else {
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
            return RegisterOutcome::StageCopyFailed;
        };
        if crate::session_stage::must_not_register_as_gdi_path(&gdi) {
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
            return RegisterOutcome::Refused;
        }
        remember_map(path, &gdi);
        in_gdi().fetch_add(1, Ordering::SeqCst);
        let w = wide(&gdi);
        let n = {
            let _gdi = gdi_api().lock().unwrap_or_else(|e| e.into_inner());
            unsafe { AddFontResourceExW(w.as_ptr(), FR_ENUMERABLE, std::ptr::null_mut()) }
        };
        in_gdi().fetch_sub(1, Ordering::SeqCst);
        if unloading().load(Ordering::SeqCst) {
            if n > 0 {
                let _ = remove_mapped_keep_file(path);
            }
            forget_map(path);
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
            return RegisterOutcome::Unloading;
        }
        if n <= 0 {
            forget_map(path);
            if let Ok(mut g) = loaded().lock() {
                g.remove(path);
            }
            return RegisterOutcome::AddReturnedZero;
        }
        sync_hkcu_session_entry(&gdi);
        dirty().store(true, Ordering::SeqCst);
        RegisterOutcome::Ok
    }

    /// Direct fast-path registration from an intact pre-staged `gdi-maps` entry
    /// recorded in `.session-maps.json`: skips Documents walks and SHA-256 stage recopying.
    pub fn register_prestaged_stage_path(src: &Path, stage: &Path) -> RegisterOutcome {
        if is_windows_fonts_path(src)
            || is_windows_fonts_path(stage)
            || crate::session_stage::must_not_register_as_gdi_path(stage)
        {
            return RegisterOutcome::Refused;
        }
        if unloading().load(Ordering::SeqCst) {
            return RegisterOutcome::Unloading;
        }
        if super::gdi_objects_at_hard_ceiling(process_gdi_objects()) {
            return RegisterOutcome::GdiQuotaExceeded;
        }
        if !stage.is_file() {
            return RegisterOutcome::StageCopyFailed;
        }
        let already = loaded().lock().map(|g| g.contains(src)).unwrap_or(false);
        if already {
            remember_map(src, stage);
            sync_hkcu_session_entry(stage);
            return RegisterOutcome::Ok;
        }
        {
            let Ok(mut g) = loaded().lock() else {
                return RegisterOutcome::Refused;
            };
            if !g.insert(src.to_path_buf()) {
                drop(g);
                if is_session_live_mapped(src) {
                    return RegisterOutcome::Ok;
                }
                return RegisterOutcome::Refused;
            }
        }
        if unloading().load(Ordering::SeqCst) {
            if let Ok(mut g) = loaded().lock() {
                g.remove(src);
            }
            return RegisterOutcome::Unloading;
        }
        remember_map(src, stage);
        in_gdi().fetch_add(1, Ordering::SeqCst);
        let w = wide(stage);
        let n = {
            let _gdi = gdi_api().lock().unwrap_or_else(|e| e.into_inner());
            unsafe { AddFontResourceExW(w.as_ptr(), FR_ENUMERABLE, std::ptr::null_mut()) }
        };
        in_gdi().fetch_sub(1, Ordering::SeqCst);
        if unloading().load(Ordering::SeqCst) {
            if n > 0 {
                let _ = remove_mapped_keep_file(src);
            }
            forget_map(src);
            if let Ok(mut g) = loaded().lock() {
                g.remove(src);
            }
            return RegisterOutcome::Unloading;
        }
        if n <= 0 {
            forget_map(src);
            if let Ok(mut g) = loaded().lock() {
                g.remove(src);
            }
            return RegisterOutcome::AddReturnedZero;
        }
        sync_hkcu_session_entry(stage);
        dirty().store(true, Ordering::SeqCst);
        RegisterOutcome::Ok
    }

    pub fn bind(family: &str, path: &Path) {
        let key = family.trim().to_lowercase();
        if key.is_empty() {
            return;
        }
        if let Ok(mut g) = by_family().lock() {
            g.entry(key).or_default().insert(path.to_path_buf());
        }
    }

    /// In-memory: every bound path is in `loaded()` from a real Add this process.
    /// Never hydrates from gdi-maps. `None` = walk + Add still required.
    pub fn family_live_face_count(family: &str) -> Option<usize> {
        let key = family.trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        let paths = family_bound_paths(family)?;
        if paths.is_empty() {
            return None;
        }
        let loaded = loaded().lock().ok()?;
        if paths.iter().all(|p| loaded.contains(p)) {
            Some(paths.len())
        } else {
            None
        }
    }

    pub fn family_bound_paths(family: &str) -> Option<HashSet<PathBuf>> {
        let key = family.trim().to_lowercase();
        by_family()
            .lock()
            .ok()
            .and_then(|g| g.get(&key).cloned())
    }

    /// Drop the family index so the next register walks (new faces / Repair).
    /// Does **not** unload GDI — already-Add'd paths still skip-Add in `register_detailed`.
    pub fn invalidate_family(family: &str) {
        let key = family.trim().to_lowercase();
        if key.is_empty() {
            return;
        }
        if let Ok(mut g) = by_family().lock() {
            g.remove(&key);
        }
    }

    pub fn unregister_family(family: &str) -> u32 {
        let key = family.trim().to_lowercase();
        let paths = by_family()
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&key))
            .unwrap_or_default();
        let mut n = 0u32;
        for path in paths {
            unregister(&path);
            n += 1;
        }
        n
    }

    fn prune_path_from_by_family(path: &Path) {
        if let Ok(mut g) = by_family().lock() {
            g.retain(|_, paths| {
                paths.remove(path);
                !paths.is_empty()
            });
        }
    }

    /// Authoritative live GDI family keys currently backed by at least one loaded face.
    pub fn live_family_names() -> Vec<String> {
        let Ok(loaded_guard) = loaded().lock() else {
            return Vec::new();
        };
        let Ok(mut fam_guard) = by_family().lock() else {
            return Vec::new();
        };
        let mut live = Vec::new();
        fam_guard.retain(|fam, paths| {
            paths.retain(|p| loaded_guard.contains(p));
            if !paths.is_empty() {
                live.push(fam.clone());
                true
            } else {
                false
            }
        });
        live.sort();
        live
    }

    /// Authoritative snapshot of (live_families, live_faces, staged_maps).
    pub fn live_stats() -> (usize, usize, usize) {
        let live_families = live_family_names().len();
        let live_faces = loaded().lock().map(|g| g.len()).unwrap_or(0);
        let staged_maps = maps().lock().map(|m| m.len()).unwrap_or(0);
        (live_families, live_faces, staged_maps)
    }

    pub fn unregister(path: &Path) {
        if is_windows_fonts_path(path) {
            return;
        }
        if let Ok(mut g) = loaded().lock() {
            g.remove(path);
        }
        prune_path_from_by_family(path);
        // Drain the LocalAppData map AND the Documents original (1.0.156 in-place
        // Adds). Then drop the map file so Documents can be rewritten/deleted.
        let _ = remove_mapped_keep_file(path);
        forget_map(path);
        dirty().store(true, Ordering::SeqCst);
    }

    /// Unload from GDI while keeping the pre-cached `gdi-maps` file on disk so
    /// subsequent re-activations skip file copying (zero-copy re-Add).
    pub fn unregister_keep_stage(path: &Path) {
        if is_windows_fonts_path(path) {
            return;
        }
        if let Ok(mut g) = loaded().lock() {
            g.remove(path);
        }
        prune_path_from_by_family(path);
        let _ = remove_mapped_keep_file(path);
        dirty().store(true, Ordering::SeqCst);
    }

    pub fn unregister_family_keep_stage(family: &str, out: &mut Vec<PathBuf>) -> u32 {
        let key = family.trim().to_lowercase();
        let paths = by_family()
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&key))
            .unwrap_or_default();
        let mut n = 0u32;
        for path in paths {
            unregister_keep_stage(&path);
            out.push(path);
            n += 1;
        }
        n
    }

    pub fn loaded_is_empty() -> bool {
        loaded().lock().map(|g| g.is_empty()).unwrap_or(true)
    }

    /// Query Windows Restart Manager (`rstrtmgr.dll`) for live process names holding `paths`.
    pub fn query_file_lock_holders(paths: &[PathBuf]) -> Vec<String> {
        if paths.is_empty() {
            return Vec::new();
        }
        #[cfg(test)]
        {
            let _ = paths;
            Vec::new()
        }
        #[cfg(not(test))]
        unsafe {
            let mut handle: u32 = 0;
            let mut session_key = [0u16; 36];
            if RmStartSession(&mut handle, 0, session_key.as_mut_ptr()) != 0 {
                return Vec::new();
            }
            let wide_paths: Vec<Vec<u16>> = paths
                .iter()
                .take(64)
                .map(|p| {
                    p.as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect()
                })
                .collect();
            let ptrs: Vec<*const u16> = wide_paths.iter().map(|v| v.as_ptr()).collect();
            let mut raw_names: Vec<String> = Vec::new();
            if !ptrs.is_empty()
                && RmRegisterResources(
                    handle,
                    ptrs.len() as u32,
                    ptrs.as_ptr(),
                    0,
                    core::ptr::null(),
                    0,
                    core::ptr::null(),
                ) == 0
            {
                let mut needed: u32 = 0;
                let mut count: u32 = 0;
                let mut reboot: u32 = 0;
                let rc = RmGetList(handle, &mut needed, &mut count, core::ptr::null_mut(), &mut reboot);
                // ERROR_MORE_DATA (234) with needed > 0
                if (rc == 234 || rc == 0) && needed > 0 {
                    let cap = (needed as usize).min(64);
                    let mut buf = vec![
                        RmProcessInfo {
                            Process: RmUniqueProcess {
                                dwProcessId: 0,
                                ProcessStartTime: [0, 0],
                            },
                            strAppName: [0u16; 256],
                            strServiceShortName: [0u16; 64],
                            ApplicationType: 0,
                            AppStatus: 0,
                            TSSessionId: 0,
                            bRestartable: 0,
                        };
                        cap
                    ];
                    count = cap as u32;
                    if RmGetList(handle, &mut needed, &mut count, buf.as_mut_ptr(), &mut reboot) == 0 {
                        for info in buf.iter().take(count as usize) {
                            let end = info
                                .strAppName
                                .iter()
                                .position(|&c| c == 0)
                                .unwrap_or(info.strAppName.len());
                            let app = String::from_utf16_lossy(&info.strAppName[..end])
                                .trim()
                                .to_string();
                            if !app.is_empty() {
                                raw_names.push(app);
                            }
                        }
                    }
                }
            }
            let _ = RmEndSession(handle);
            format_lock_holders(&raw_names)
        }
    }

    pub fn format_lock_holders(raw: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for item in raw {
            let t = item.trim();
            if t.is_empty() {
                continue;
            }
            if seen.insert(t.to_ascii_lowercase()) {
                out.push(t.to_string());
            }
        }
        out
    }

    /// Flush this thread's GDI batch so RemoveFontResourceExW takes effect
    /// before DeleteFile. Do not pair with HWND_BROADCAST — that hung Quit.
    pub fn flush_local() {
        unsafe {
            GdiFlush();
        }
    }

    /// Tell GDI + other apps. Windows Font Cache rebuilds on WM_FONTCHANGE — call sparingly.
    pub fn notify() {
        unsafe {
            GdiFlush();
            SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0, 0);
        }
        if let Ok(mut t) = last_notify().lock() {
            *t = Some(Instant::now());
        }
        dirty().store(false, Ordering::SeqCst);
    }

    fn pending_trailing_notify() -> &'static AtomicBool {
        static TRAILING: OnceLock<AtomicBool> = OnceLock::new();
        TRAILING.get_or_init(|| AtomicBool::new(false))
    }

    /// At most once per `gap` with a trailing-edge guarantee so rapid consecutive
    /// single-font activations never lose their final `WM_FONTCHANGE` broadcast to Adobe/Word.
    pub fn notify_maybe(gap: Duration) -> bool {
        if !dirty().load(Ordering::SeqCst) {
            return false;
        }
        let elapsed = last_notify()
            .lock()
            .ok()
            .and_then(|t| *t)
            .map(|t| t.elapsed());
        let due = elapsed.map(|e| e >= gap).unwrap_or(true);
        if !due {
            let wait = gap.saturating_sub(elapsed.unwrap_or_default()).max(Duration::from_millis(50));
            if !pending_trailing_notify().swap(true, Ordering::SeqCst) {
                std::thread::spawn(move || {
                    std::thread::sleep(wait);
                    pending_trailing_notify().store(false, Ordering::SeqCst);
                    if dirty().load(Ordering::SeqCst) && !unloading().load(Ordering::SeqCst) {
                        sync_hkcu_all_active_stages();
                        notify();
                    }
                });
            }
            return false;
        }
        notify();
        true
    }

    pub fn flush_cache() {
        notify();
    }

    #[derive(Debug, Clone, Default)]
    pub struct UnloadStats {
        pub attempted: usize,
        /// Paths where at least one RemoveFontResourceExW returned non-zero.
        pub removed_ok: usize,
    }

    /// Drain this process's Adds plus leftover paths from a previous incomplete
    /// quit. RemoveFontResourceExW loops until 0 (refcount drain). Local GdiFlush
    /// only on the quit path — HWND_BROADCAST WM_FONTCHANGE can re-lock Documents
    /// files in Explorer. Live Deactivate may still broadcast; FontCache service
    /// restart runs after this (see restart_font_cache_service).
    pub fn unload_paths(extra: Vec<PathBuf>, broadcast: bool) -> UnloadStats {
        let mut paths = loaded()
            .lock()
            .map(|mut g| g.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Ok(mut g) = by_family().lock() {
            g.clear();
        }
        let mut seen: HashSet<PathBuf> = paths.iter().cloned().collect();
        for path in extra {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        let attempted = paths.len();
        let removed_ok = std::sync::atomic::AtomicUsize::new(0);
        let retry_paths: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        if paths.len() <= 32 {
            for path in paths.iter() {
                if remove_mapped_keep_file(path) {
                    removed_ok.fetch_add(1, Ordering::Relaxed);
                } else if let Ok(mut r) = retry_paths.lock() {
                    r.push(path.clone());
                }
            }
        } else {
            let workers = 8usize.min(paths.len());
            let chunk_size = paths.len().div_ceil(workers);
            std::thread::scope(|s| {
                for chunk in paths.chunks(chunk_size) {
                    let removed_ref = &removed_ok;
                    let retry_ref = &retry_paths;
                    s.spawn(move || {
                        let mut local_retry = Vec::new();
                        let mut local_ok = 0usize;
                        for path in chunk {
                            if remove_mapped_keep_file(path) {
                                local_ok += 1;
                            } else {
                                local_retry.push(path.clone());
                            }
                        }
                        if local_ok > 0 {
                            removed_ref.fetch_add(local_ok, Ordering::Relaxed);
                        }
                        if !local_retry.is_empty()
                            && let Ok(mut r) = retry_ref.lock() {
                                r.extend(local_retry);
                            }
                    });
                }
            });
        }
        if !paths.is_empty() {
            unsafe {
                GdiFlush();
            }
            // Second pass after flush only for paths that missed pass 1 (raced Adds / leftovers),
            // avoiding 2x serial unload stalls on large 2,000+ font libraries during quit.
            let leftovers = retry_paths
                .into_inner()
                .unwrap_or_default();
            if !leftovers.is_empty() {
                for path in leftovers.iter() {
                    if remove_mapped_keep_file(path) {
                        removed_ok.fetch_add(1, Ordering::Relaxed);
                    }
                }
                unsafe {
                    GdiFlush();
                }
            }
            // Keep gdi-maps files — next boot re-Adds without recopying Documents.
        }
        let removed_ok = removed_ok.load(Ordering::Relaxed);
        clear_all_hkcu_session_entries();
        if broadcast {
            unsafe {
                SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE, 0, 0);
            }
        }
        dirty().store(false, Ordering::SeqCst);
        UnloadStats {
            attempted,
            removed_ok,
        }
    }

    // --- Windows Font Cache (svchost / LOCAL SERVICE) unlock -----------------
    // After enumerable Remove, Font Cache often keeps Documents TTF handles.
    // Best-effort SCM restart beats HWND_BROADCAST for unlock; soft-fail
    // AccessDenied may still need admin once. Unlock is proven only after
    // WRITE_OK on Eric's box — do not treat soft-fail as FontBase-or-better.
    // Do NOT wipe %WINDIR%\ServiceProfiles\...\FontCache here — service
    // restart first; dir wipe is last-resort and left unimplemented.

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenSCManagerW(
            lpMachineName: *const u16,
            lpDatabaseName: *const u16,
            dwDesiredAccess: u32,
        ) -> isize;
        fn OpenServiceW(
            hSCManager: isize,
            lpServiceName: *const u16,
            dwDesiredAccess: u32,
        ) -> isize;
        fn CloseServiceHandle(hSCObject: isize) -> i32;
        fn ControlService(hService: isize, dwControl: u32, lpServiceStatus: *mut ServiceStatus) -> i32;
        fn StartServiceW(
            hService: isize,
            dwNumServiceArgs: u32,
            lpServiceArgVectors: *const *const u16,
        ) -> i32;
        fn QueryServiceStatus(hService: isize, lpServiceStatus: *mut ServiceStatus) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLastError() -> u32;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ServiceStatus {
        dw_service_type: u32,
        dw_current_state: u32,
        dw_controls_accepted: u32,
        dw_win32_exit_code: u32,
        dw_service_specific_exit_code: u32,
        dw_check_point: u32,
        dw_wait_hint: u32,
    }

    const SC_MANAGER_CONNECT: u32 = 0x0001;
    const SERVICE_QUERY_STATUS: u32 = 0x0004;
    const SERVICE_START: u32 = 0x0010;
    const SERVICE_STOP: u32 = 0x0020;
    const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;
    const SERVICE_STOPPED: u32 = 0x0000_0001;
    const SERVICE_RUNNING: u32 = 0x0000_0004;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
    const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FontCacheRestartOutcome {
        Restarted,
        AccessDenied,
        OpenFailed,
        StopTimedOut,
        StartTimedOut,
        NotFound,
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wait_service_state(svc: isize, want: u32, deadline: Instant) -> bool {
        let mut status = ServiceStatus {
            dw_service_type: 0,
            dw_current_state: 0,
            dw_controls_accepted: 0,
            dw_win32_exit_code: 0,
            dw_service_specific_exit_code: 0,
            dw_check_point: 0,
            dw_wait_hint: 0,
        };
        while Instant::now() < deadline {
            let ok = unsafe { QueryServiceStatus(svc, &mut status) };
            if ok == 0 {
                return false;
            }
            if status.dw_current_state == want {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn restart_one_font_cache_service(name: &str, budget: Duration) -> FontCacheRestartOutcome {
        let start = Instant::now();
        let scm = unsafe {
            OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT)
        };
        if scm == 0 {
            let err = unsafe { GetLastError() };
            return if err == ERROR_ACCESS_DENIED {
                FontCacheRestartOutcome::AccessDenied
            } else {
                FontCacheRestartOutcome::OpenFailed
            };
        }
        let access = SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP;
        let name_w = wide_z(name);
        let svc = unsafe { OpenServiceW(scm, name_w.as_ptr(), access) };
        if svc == 0 {
            let err = unsafe { GetLastError() };
            unsafe {
                CloseServiceHandle(scm);
            }
            return match err {
                ERROR_ACCESS_DENIED => FontCacheRestartOutcome::AccessDenied,
                ERROR_SERVICE_DOES_NOT_EXIST => FontCacheRestartOutcome::NotFound,
                _ => FontCacheRestartOutcome::OpenFailed,
            };
        }

        let mut status = ServiceStatus {
            dw_service_type: 0,
            dw_current_state: 0,
            dw_controls_accepted: 0,
            dw_win32_exit_code: 0,
            dw_service_specific_exit_code: 0,
            dw_check_point: 0,
            dw_wait_hint: 0,
        };
        let _ = unsafe { QueryServiceStatus(svc, &mut status) };

        let stop_deadline = start + budget / 2;
        if status.dw_current_state != SERVICE_STOPPED {
            let ctrl = unsafe { ControlService(svc, SERVICE_CONTROL_STOP, &mut status) };
            if ctrl == 0 {
                let err = unsafe { GetLastError() };
                if err == ERROR_ACCESS_DENIED {
                    unsafe {
                        CloseServiceHandle(svc);
                        CloseServiceHandle(scm);
                    }
                    return FontCacheRestartOutcome::AccessDenied;
                }
                let _ = err == ERROR_SERVICE_NOT_ACTIVE;
            }
            if !wait_service_state(svc, SERVICE_STOPPED, stop_deadline) {
                let _ = unsafe { QueryServiceStatus(svc, &mut status) };
                if status.dw_current_state != SERVICE_STOPPED {
                    unsafe {
                        CloseServiceHandle(svc);
                        CloseServiceHandle(scm);
                    }
                    return FontCacheRestartOutcome::StopTimedOut;
                }
            }
        }

        let remaining = budget.saturating_sub(start.elapsed());
        let start_deadline = Instant::now() + remaining.max(Duration::from_millis(500));
        let started = unsafe { StartServiceW(svc, 0, std::ptr::null()) };
        if started == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_ACCESS_DENIED {
                unsafe {
                    CloseServiceHandle(svc);
                    CloseServiceHandle(scm);
                }
                return FontCacheRestartOutcome::AccessDenied;
            }
        }
        let ok = wait_service_state(svc, SERVICE_RUNNING, start_deadline);
        unsafe {
            CloseServiceHandle(svc);
            CloseServiceHandle(scm);
        }
        if ok {
            FontCacheRestartOutcome::Restarted
        } else {
            FontCacheRestartOutcome::StartTimedOut
        }
    }

    /// Time-bounded FontCache (+ WPF FontCache3) restart. Never blocks beyond `budget`.
    pub fn restart_font_cache_service(budget: Duration) -> FontCacheRestartOutcome {
        let overall = Instant::now();
        let names = super::font_cache_service_names();
        let mut worst = FontCacheRestartOutcome::NotFound;
        let mut any_restarted = false;
        let mut access_denied = false;
        let n = names.len().max(1);
        for name in names {
            let elapsed = overall.elapsed();
            if elapsed >= budget {
                break;
            }
            let remaining = budget - elapsed;
            let per = (budget / n as u32).min(remaining);
            if per.is_zero() {
                break;
            }
            let outcome = restart_one_font_cache_service(name, per);
            match outcome {
                FontCacheRestartOutcome::Restarted => {
                    any_restarted = true;
                }
                FontCacheRestartOutcome::AccessDenied => access_denied = true,
                FontCacheRestartOutcome::NotFound => {}
                other => {
                    if matches!(
                        worst,
                        FontCacheRestartOutcome::NotFound
                            | FontCacheRestartOutcome::OpenFailed
                    ) {
                        worst = other;
                    }
                }
            }
        }
        if access_denied && !any_restarted {
            return FontCacheRestartOutcome::AccessDenied;
        }
        if any_restarted {
            return FontCacheRestartOutcome::Restarted;
        }
        worst
    }

    #[cfg(test)]
    mod persistent_tests {
        use super::*;
        use crate::session_stage::{
            normalize_registry_font_path, session_hkcu_value_name, FaceMap,
        };

        #[test]
        fn adopt_persistent_hkcu_session_verifies_stage_size_hkcu_and_excludes_unsafe_paths() {
            let tmp = std::env::temp_dir().join(format!("fm-adopt-hkcu-{}", std::process::id()));
            let _ = fs::remove_dir_all(&tmp);
            fs::create_dir_all(&tmp).unwrap();

            let valid_stage = tmp.join("gdi-maps").join("valid_a.ttf");
            fs::create_dir_all(valid_stage.parent().unwrap()).unwrap();
            fs::write(&valid_stage, vec![0x41u8; 512]).unwrap();

            let tiny_stage = tmp.join("gdi-maps").join("tiny_b.ttf");
            fs::write(&tiny_stage, vec![0x42u8; 64]).unwrap();

            let missing_hkcu_stage = tmp.join("gdi-maps").join("no_reg_c.ttf");
            fs::write(&missing_hkcu_stage, vec![0x43u8; 512]).unwrap();

            let docs_stage = tmp.join("Documents").join("Fonths").join("DocsFont").join("d.ttf");
            fs::create_dir_all(docs_stage.parent().unwrap()).unwrap();
            fs::write(&docs_stage, vec![0x44u8; 512]).unwrap();

            let mut hkcu = HashMap::new();
            hkcu.insert(
                session_hkcu_value_name(&valid_stage),
                normalize_registry_font_path(&valid_stage),
            );
            hkcu.insert(
                session_hkcu_value_name(&tiny_stage),
                normalize_registry_font_path(&tiny_stage),
            );
            hkcu.insert(
                session_hkcu_value_name(&docs_stage),
                normalize_registry_font_path(&docs_stage),
            );

            let maps = vec![
                FaceMap {
                    source: tmp.join("Fonts").join("ValidA.ttf").to_string_lossy().into(),
                    stage: valid_stage.to_string_lossy().into(),
                    registry_name: String::new(),
                    family: "Valid A".into(),
                },
                FaceMap {
                    source: tmp.join("Fonts").join("TinyB.ttf").to_string_lossy().into(),
                    stage: tiny_stage.to_string_lossy().into(),
                    registry_name: String::new(),
                    family: "Tiny B".into(),
                },
                FaceMap {
                    source: tmp.join("Fonts").join("NoRegC.ttf").to_string_lossy().into(),
                    stage: missing_hkcu_stage.to_string_lossy().into(),
                    registry_name: String::new(),
                    family: "NoReg C".into(),
                },
                FaceMap {
                    source: docs_stage.to_string_lossy().into(),
                    stage: docs_stage.to_string_lossy().into(),
                    registry_name: String::new(),
                    family: "DocsFont".into(),
                },
            ];
            let fams = vec![
                "Valid A".into(),
                "Tiny B".into(),
                "NoReg C".into(),
                "DocsFont".into(),
            ];
            let adopted = adopt_persistent_hkcu_session(&maps, &fams, &hkcu);
            assert_eq!(adopted, vec!["Valid A".to_string()]);

            let _ = fs::remove_dir_all(&tmp);
        }
    }
}
