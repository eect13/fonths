//! Session stage bookkeeping: Documents library stays source-of-truth; GDI only
//! Adds **copies** under `%LOCALAPPDATA%\Font Manager\gdi-maps` (never Documents).
//! `.session-maps.json` tracks source ↔ stage; `.session-paths.txt` lists stage
//! paths only. Legacy 1.0.156 FontManager maps are accepted for unload/migrate.

use std::path::{Path, PathBuf};

/// Sidecar under Documents/Font Manager tracking source ↔ LocalAppData stage.
pub const SESSION_MAPS_FILE: &str = ".session-maps.json";
/// Marker written only on a clean session_end so next boot skips heavy recovery scans.
pub const SESSION_CLEAN_EXIT_FILE: &str = ".session-clean-exit";
/// Marker written when the user exits in Zero-RAM Instant mode keeping HKCU per-user fonts active.
pub const SESSION_PERSISTENT_ACTIVE_FILE: &str = ".persistent-active";

pub fn clean_exit_marker_in(root: &Path) -> PathBuf {
    root.join(SESSION_CLEAN_EXIT_FILE)
}

pub fn persistent_active_marker_in(root: &Path) -> PathBuf {
    root.join(SESSION_PERSISTENT_ACTIVE_FILE)
}

pub fn write_persistent_active_marker_in(root: &Path) {
    let marker = persistent_active_marker_in(root);
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, b"1\n");
}

pub fn has_persistent_active_marker_in(root: &Path) -> bool {
    persistent_active_marker_in(root).is_file()
}

pub fn clear_persistent_active_marker_in(root: &Path) {
    let _ = std::fs::remove_file(persistent_active_marker_in(root));
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_clean_exit_marker_in(root: &Path) {
    let marker = clean_exit_marker_in(root);
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, b"1\n");
}

#[allow(dead_code)]
pub fn has_clean_exit_marker_in(root: &Path) -> bool {
    clean_exit_marker_in(root).is_file()
}

/// Pure diff calculator for `HKCU\...\Fonts` session entries.
/// Returns `(to_upsert, to_delete)` where `to_upsert` contains `(value_name, normalized_path)`
/// pairs that are missing or differ from `existing`, and `to_delete` contains existing
/// `Fonths Session ` keys no longer present in `paths`.
pub fn compute_hkcu_diff(
    existing: &std::collections::HashMap<String, String>,
    paths: &[PathBuf],
) -> (Vec<(String, String)>, Vec<String>) {
    let mut desired: std::collections::HashMap<String, String> =
        std::collections::HashMap::with_capacity(paths.len());
    for gdi in paths {
        let target = normalize_registry_font_path(gdi);
        if target.is_empty() {
            continue;
        }
        desired.insert(session_hkcu_value_name(gdi), target);
    }
    let mut to_upsert = Vec::new();
    for (val_name, target) in &desired {
        let same = existing
            .get(val_name)
            .map(|cur| cur.eq_ignore_ascii_case(target))
            .unwrap_or(false);
        if !same {
            to_upsert.push((val_name.clone(), target.clone()));
        }
    }
    let mut to_delete: Vec<String> = existing
        .keys()
        .filter(|name| is_fm_session_hkcu_value_name(name) && !desired.contains_key(*name))
        .cloned()
        .collect();
    to_upsert.sort_by(|a, b| a.0.cmp(&b.0));
    to_delete.sort();
    (to_upsert, to_delete)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn take_clean_exit_marker_in(root: &Path) -> bool {
    let marker = clean_exit_marker_in(root);
    let existed = marker.is_file();
    if existed {
        let _ = std::fs::remove_file(&marker);
    }
    existed
}

/// One activated face mapping (Documents original → LocalAppData GDI stage copy).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FaceMap {
    pub source: String,
    /// Stage path under gdi-maps (or legacy FontManager). Accepts older JSON key `per_user`.
    #[serde(alias = "per_user")]
    pub stage: String,
    #[serde(default)]
    pub registry_name: String,
    #[serde(default)]
    pub family: String,
}

pub fn normalize_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\")
}

/// Documents\\Fonths (and legacy Documents\\Font Manager) library path — must never be AddFontResourceExW'd.
pub fn is_documents_library_path(path: &Path) -> bool {
    let lower = normalize_path_key(path);
    lower.contains("\\documents\\fonths\\")
        || lower.ends_with("\\documents\\fonths")
        || lower.contains("\\documents\\font manager\\")
        || lower.ends_with("\\documents\\font manager")
}

/// Invariant: Documents library paths must not be passed to AddFontResourceEx.
pub fn must_not_register_as_gdi_path(path: &Path) -> bool {
    is_documents_library_path(path)
}

/// Current stage root: `%LOCALAPPDATA%\Fonths\gdi-maps` (plus legacy `%LOCALAPPDATA%\Font Manager\gdi-maps`).
#[allow(dead_code)]
pub fn is_gdi_maps_stage_path(path: &Path) -> bool {
    let lower = normalize_path_key(path);
    lower.contains("\\fonths\\gdi-maps\\")
        || lower.ends_with("\\fonths\\gdi-maps")
        || lower.contains("\\font manager\\gdi-maps\\")
        || lower.ends_with("\\font manager\\gdi-maps")
}

/// Legacy 1.0.156 stage: `%LOCALAPPDATA%\Microsoft\Windows\Fonts\FontManager`.
#[allow(dead_code)]
pub fn is_legacy_fontmanager_stage_path(path: &Path) -> bool {
    let lower = normalize_path_key(path);
    lower.contains("\\microsoft\\windows\\fonts\\fontmanager\\")
        || lower.ends_with("\\microsoft\\windows\\fonts\\fontmanager")
}

/// Any LocalAppData stage we own (current gdi-maps or legacy FontManager).
#[allow(dead_code)]
pub fn is_session_stage_path(path: &Path) -> bool {
    is_gdi_maps_stage_path(path) || is_legacy_fontmanager_stage_path(path)
}

pub fn session_maps_file_in(root: &Path) -> PathBuf {
    root.join(SESSION_MAPS_FILE)
}

pub fn parse_session_maps_json(text: &str) -> Vec<FaceMap> {
    serde_json::from_str(text).unwrap_or_default()
}

pub fn session_maps_to_json(maps: &[FaceMap]) -> String {
    serde_json::to_string_pretty(maps).unwrap_or_else(|_| "[]".into())
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn load_session_maps_in(root: &Path) -> Vec<FaceMap> {
    let path = session_maps_file_in(root);
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_session_maps_json(&text)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn save_session_maps_in(root: &Path, maps: &[FaceMap]) {
    let file = session_maps_file_in(root);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, session_maps_to_json(maps));
}

pub fn clear_session_maps_in(root: &Path) {
    let _ = std::fs::remove_file(session_maps_file_in(root));
}

/// Partition `.session-paths.txt` into Documents (unload-only, never re-Add)
/// vs stage / other paths.
pub fn partition_legacy_session_paths(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut documents = Vec::new();
    let mut other = Vec::new();
    for p in paths {
        if must_not_register_as_gdi_path(p) {
            documents.push(p.clone());
        } else {
            other.push(p.clone());
        }
    }
    (documents, other)
}

/// Keep maps whose stage file exists; drop stale stage-missing entries unless
/// `restaged` supplies a fresh stage path for that source.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn validate_session_maps(
    maps: &[FaceMap],
    restaged: &[(String, String)],
) -> Vec<FaceMap> {
    let mut out = Vec::new();
    let mut seen_source = std::collections::HashSet::new();
    for (src, stage) in restaged {
        if src.is_empty() || stage.is_empty() {
            continue;
        }
        if !Path::new(stage).is_file() {
            continue;
        }
        if !seen_source.insert(normalize_path_key(Path::new(src))) {
            continue;
        }
        out.push(FaceMap {
            source: src.clone(),
            stage: stage.clone(),
            registry_name: String::new(),
            family: String::new(),
        });
    }
    for m in maps {
        let src_key = normalize_path_key(Path::new(&m.source));
        if seen_source.contains(&src_key) {
            continue;
        }
        let stage = Path::new(&m.stage);
        if !stage.is_file() {
            // Stale: stage gone — drop (caller may re-stage from source).
            continue;
        }
        // Refuse to keep a map that points GDI at Documents.
        if must_not_register_as_gdi_path(stage) {
            continue;
        }
        seen_source.insert(src_key);
        out.push(m.clone());
    }
    out
}

/// Sources that still need a stage copy (map missing stage file, source exists).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn maps_needing_restage(maps: &[FaceMap]) -> Vec<FaceMap> {
    maps.iter()
        .filter(|m| {
            let stage = Path::new(&m.stage);
            let source = Path::new(&m.source);
            (!stage.is_file() || must_not_register_as_gdi_path(stage)) && source.is_file()
        })
        .cloned()
        .collect()
}

/// Stage paths only — what `.session-paths.txt` must list (never Documents).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn stage_paths_from_maps(maps: &[FaceMap]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in maps {
        let p = PathBuf::from(&m.stage);
        if must_not_register_as_gdi_path(&p) {
            continue;
        }
        if !p.as_os_str().is_empty() && seen.insert(normalize_path_key(&p)) {
            out.push(p);
        }
    }
    out
}

/// Filter a mixed path list down to stage-only entries (drop Documents).
pub fn filter_session_paths_refuse_documents(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| !must_not_register_as_gdi_path(p))
        .cloned()
        .collect()
}

/// Normalize a staged Windows font path for `HKCU\...\Fonts` registry values.
/// Strips `\\?\` / `\\?\UNC\` extended prefixes and normalizes slashes so Adobe
/// CoolType and DirectWrite always resolve the font file reliably.
pub fn normalize_registry_font_path(gdi: &Path) -> String {
    let raw = gdi.to_string_lossy().replace('/', "\\");
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw
    }
}

pub fn session_hkcu_value_name(gdi: &Path) -> String {
    let stem = gdi
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("font");
    // Preserve full 32-char SHA-256 gdi-maps stem (up to 48 sanitized chars) so
    // distinct staged faces never collide in HKCU\...\Fonts (Adobe CoolType).
    let tag: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect();
    let tag = if tag.is_empty() {
        "font".to_string()
    } else {
        tag
    };
    let ext = gdi
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("ttf")
        .to_ascii_lowercase();
    let fmt = if matches!(ext.as_str(), "otf" | "otc") {
        "OpenType"
    } else {
        "TrueType"
    };
    format!("Fonths [FM-Session-{tag}] ({fmt})")
}

static HKCU_SYNC_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn hkcu_sync_enabled() -> bool {
    HKCU_SYNC_ENABLED.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn set_hkcu_sync_enabled(enabled: bool) {
    HKCU_SYNC_ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
}

pub fn is_fm_session_hkcu_value_name(name: &str) -> bool {
    name.contains("[FM-Session-")
}

/// Pure predicate for stale `HKCU\...\Fonts` entries managed by Fonths.
pub fn is_stale_hkcu_entry(name: &str, value_path: &str) -> bool {
    if !is_fm_session_hkcu_value_name(name) {
        return false;
    }
    let trimmed = value_path.trim();
    if trimmed.is_empty() {
        return true;
    }
    let p = Path::new(trimmed);
    !must_not_register_as_gdi_path(p) && !p.is_file()
}

/// Temporary per-user HKCU session registration under
/// `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts` pointing to
/// `%LOCALAPPDATA%\Font Manager\gdi-maps\<file>`, enabling DirectWrite and
/// Adobe Creative Cloud to read OpenType variable (`fvar`/`STAT`) axes
/// without installing anything into Windows Fonts directories.
#[cfg(all(windows, not(test)))]
pub mod hkcu_session {
    use std::path::Path;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(
            hKey: isize,
            lpSubKey: *const u16,
            Reserved: u32,
            lpClass: *mut u16,
            dwOptions: u32,
            samDesired: u32,
            lpSecurityAttributes: *mut core::ffi::c_void,
            phkResult: *mut isize,
            lpdwDisposition: *mut u32,
        ) -> i32;
        fn RegOpenKeyExW(
            hKey: isize,
            lpSubKey: *const u16,
            ulOptions: u32,
            samDesired: u32,
            phkResult: *mut isize,
        ) -> i32;
        fn RegSetValueExW(
            hKey: isize,
            lpValueName: *const u16,
            Reserved: u32,
            dwType: u32,
            lpData: *const u8,
            cbData: u32,
        ) -> i32;
        fn RegDeleteValueW(hKey: isize, lpValueName: *const u16) -> i32;
        fn RegEnumValueW(
            hKey: isize,
            dwIndex: u32,
            lpValueName: *mut u16,
            lpcchValueName: *mut u32,
            lpReserved: *mut u32,
            lpType: *mut u32,
            lpData: *mut u8,
            lpcbData: *mut u32,
        ) -> i32;
        fn RegCloseKey(hKey: isize) -> i32;
    }

    const HKEY_CURRENT_USER: isize = -2147483647i32 as isize;
    const KEY_SET_VALUE: u32 = 0x0002;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const REG_SZ: u32 = 1;
    const ERROR_NO_MORE_ITEMS: i32 = 259;

    fn wide_str(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn fonts_subkey() -> Vec<u16> {
        wide_str("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts")
    }

    pub fn sync_entry(gdi: &Path, sandbox: bool) {
        if sandbox || !super::hkcu_sync_enabled() {
            return;
        }
        let val_name = super::session_hkcu_value_name(gdi);
        let target = super::normalize_registry_font_path(gdi);
        if target.is_empty() {
            return;
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null_mut(),
                0,
                KEY_SET_VALUE,
                std::ptr::null_mut(),
                &mut hkey,
                std::ptr::null_mut(),
            ) != 0
            {
                return;
            }
            let wname = wide_str(&val_name);
            let wdata = wide_str(&target);
            let _ = RegSetValueExW(
                hkey,
                wname.as_ptr(),
                0,
                REG_SZ,
                wdata.as_ptr() as *const u8,
                (wdata.len() * 2) as u32,
            );
            RegCloseKey(hkey);
        }
    }

    pub fn remove_entry(gdi: &Path, sandbox: bool) {
        if sandbox {
            return;
        }
        let val_name = super::session_hkcu_value_name(gdi);
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            ) != 0
            {
                return;
            }
            let wname = wide_str(&val_name);
            let _ = RegDeleteValueW(hkey, wname.as_ptr());
            RegCloseKey(hkey);
        }
    }

    unsafe fn enum_fm_entries_open(hkey: isize) -> std::collections::HashMap<String, String> {
        let mut existing: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut idx: u32 = 0;
        loop {
            let mut name_buf = vec![0u16; 512];
            let mut name_len = name_buf.len() as u32;
            let mut val_type: u32 = 0;
            let mut data_buf = vec![0u8; 1024];
            let mut data_len = data_buf.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    hkey,
                    idx,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null_mut(),
                    &mut val_type,
                    data_buf.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc == 0 && val_type == REG_SZ {
                let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                if super::is_fm_session_hkcu_value_name(&name) {
                    let u16_len = (data_len as usize) / 2;
                    let mut wide = Vec::with_capacity(u16_len);
                    for chunk in data_buf[..u16_len * 2].as_chunks::<2>().0 {
                        let w = u16::from_le_bytes([chunk[0], chunk[1]]);
                        if w == 0 {
                            break;
                        }
                        wide.push(w);
                    }
                    existing.insert(name, String::from_utf16_lossy(&wide));
                }
            }
            idx = idx.saturating_add(1);
            if idx > 65_536 {
                break;
            }
        }
        existing
    }

    pub fn list_fm_entries(sandbox: bool) -> std::collections::HashMap<String, String> {
        if sandbox {
            return std::collections::HashMap::new();
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut hkey,
            ) != 0
            {
                return std::collections::HashMap::new();
            }
            let out = enum_fm_entries_open(hkey);
            RegCloseKey(hkey);
            out
        }
    }

    /// Reconcile `HKCU\...\Fonts` `Fonths Session ` entries against `paths` via diff only:
    /// deletes stale keys not present in `paths`, inserts/updates only missing or changed keys,
    /// and never wipes unchanged entries. Returns `(upserted, deleted)`.
    pub fn diff_reconcile_entries(paths: &[std::path::PathBuf], sandbox: bool) -> (usize, usize) {
        if sandbox || !super::hkcu_sync_enabled() {
            return (0, 0);
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null_mut(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null_mut(),
                &mut hkey,
                std::ptr::null_mut(),
            ) != 0
            {
                return (0, 0);
            }
            let existing = enum_fm_entries_open(hkey);
            let (to_upsert, to_delete) = super::compute_hkcu_diff(&existing, paths);
            let mut deleted = 0usize;
            for name in &to_delete {
                let wname = wide_str(name);
                if RegDeleteValueW(hkey, wname.as_ptr()) == 0 {
                    deleted += 1;
                }
            }
            let mut upserted = 0usize;
            for (val_name, target) in &to_upsert {
                let wname = wide_str(val_name);
                let wdata = wide_str(target);
                if RegSetValueExW(
                    hkey,
                    wname.as_ptr(),
                    0,
                    REG_SZ,
                    wdata.as_ptr() as *const u8,
                    (wdata.len() * 2) as u32,
                ) == 0
                {
                    upserted += 1;
                }
            }
            RegCloseKey(hkey);
            (upserted, deleted)
        }
    }

    #[allow(dead_code)]
    pub fn sync_entries_batch(paths: &[std::path::PathBuf], sandbox: bool) {
        if sandbox || paths.is_empty() || !super::hkcu_sync_enabled() {
            return;
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null_mut(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null_mut(),
                &mut hkey,
                std::ptr::null_mut(),
            ) != 0
            {
                return;
            }
            let existing = enum_fm_entries_open(hkey);
            for gdi in paths {
                let target = super::normalize_registry_font_path(gdi);
                if target.is_empty() {
                    continue;
                }
                let val_name = super::session_hkcu_value_name(gdi);
                if existing
                    .get(&val_name)
                    .map(|cur| cur.eq_ignore_ascii_case(&target))
                    .unwrap_or(false)
                {
                    continue;
                }
                let wname = wide_str(&val_name);
                let wdata = wide_str(&target);
                let _ = RegSetValueExW(
                    hkey,
                    wname.as_ptr(),
                    0,
                    REG_SZ,
                    wdata.as_ptr() as *const u8,
                    (wdata.len() * 2) as u32,
                );
            }
            RegCloseKey(hkey);
        }
    }

    #[allow(dead_code)]
    pub fn remove_entries_batch(paths: &[std::path::PathBuf], sandbox: bool) {
        if sandbox || paths.is_empty() {
            return;
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            ) != 0
            {
                return;
            }
            for gdi in paths {
                let val_name = super::session_hkcu_value_name(gdi);
                let wname = wide_str(&val_name);
                let _ = RegDeleteValueW(hkey, wname.as_ptr());
            }
            RegCloseKey(hkey);
        }
    }

    pub fn clear_all(sandbox: bool) {
        if sandbox {
            return;
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut hkey,
            ) != 0
            {
                return;
            }
            let mut to_delete: Vec<Vec<u16>> = Vec::new();
            let mut idx: u32 = 0;
            loop {
                let mut buf = [0u16; 512];
                let mut len = buf.len() as u32;
                let rc = RegEnumValueW(
                    hkey,
                    idx,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if rc == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if rc == 0 {
                    let name = String::from_utf16_lossy(&buf[..len as usize]);
                    if super::is_fm_session_hkcu_value_name(&name) {
                        let mut w = buf[..len as usize].to_vec();
                        w.push(0);
                        to_delete.push(w);
                    }
                }
                idx = idx.saturating_add(1);
            }
            for wname in to_delete {
                let _ = RegDeleteValueW(hkey, wname.as_ptr());
            }
            RegCloseKey(hkey);
        }
    }

    /// Inspect all `Fonths [FM-Session-*]` registry entries whose target stage file
    /// no longer exists on disk, optionally deleting them (`!dry_run`).
    /// Returns `(stale_found, deleted)`.
    pub fn prune_stale_entries(dry_run: bool, sandbox: bool) -> (usize, usize) {
        if sandbox {
            return (0, 0);
        }
        let subkey = fonts_subkey();
        let mut hkey: isize = 0;
        let desired = if dry_run {
            KEY_QUERY_VALUE
        } else {
            KEY_QUERY_VALUE | KEY_SET_VALUE
        };
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, desired, &mut hkey) != 0 {
                return (0, 0);
            }
            let existing = enum_fm_entries_open(hkey);
            let stale_names: Vec<String> = existing
                .iter()
                .filter(|(name, val)| super::is_stale_hkcu_entry(name, val))
                .map(|(name, _)| name.clone())
                .collect();
            let stale_found = stale_names.len();
            let mut deleted = 0usize;
            if !dry_run {
                for name in stale_names {
                    let wname = wide_str(&name);
                    if RegDeleteValueW(hkey, wname.as_ptr()) == 0 {
                        deleted += 1;
                    }
                }
            }
            RegCloseKey(hkey);
            (stale_found, deleted)
        }
    }
}

#[cfg(any(not(windows), test))]
pub mod hkcu_session {
    use std::path::{Path, PathBuf};
    pub fn sync_entry(_gdi: &Path, _sandbox: bool) {}
    pub fn remove_entry(_gdi: &Path, _sandbox: bool) {}
    #[allow(dead_code)]
    pub fn sync_entries_batch(_paths: &[PathBuf], _sandbox: bool) {}
    #[allow(dead_code)]
    pub fn remove_entries_batch(_paths: &[PathBuf], _sandbox: bool) {}
    pub fn list_fm_entries(_sandbox: bool) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
    pub fn diff_reconcile_entries(_paths: &[PathBuf], _sandbox: bool) -> (usize, usize) {
        (0, 0)
    }
    pub fn clear_all(_sandbox: bool) {}
    pub fn prune_stale_entries(_dry_run: bool, _sandbox: bool) -> (usize, usize) {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-stage-{}-{}-{}",
            label,
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn documents_library_must_not_be_registered() {
        let docs = Path::new(r"C:\Users\Eric\Documents\Font Manager\Nunito\nunito-400.ttf");
        assert!(is_documents_library_path(docs));
        assert!(must_not_register_as_gdi_path(docs));
        let gdi = Path::new(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\abcd.ttf");
        assert!(!must_not_register_as_gdi_path(gdi));
        assert!(is_gdi_maps_stage_path(gdi));
        let legacy = Path::new(
            r"C:\Users\Eric\AppData\Local\Microsoft\Windows\Fonts\FontManager\Nunito__nunito-400.ttf",
        );
        assert!(!must_not_register_as_gdi_path(legacy));
        assert!(is_legacy_fontmanager_stage_path(legacy));
        assert!(is_session_stage_path(legacy));
        let system = Path::new(r"C:\Windows\Fonts\arial.ttf");
        let other_user = Path::new(r"C:\Users\Eric\AppData\Local\Microsoft\Windows\Fonts\Comic.ttf");
        assert!(!is_legacy_fontmanager_stage_path(system));
        assert!(!is_legacy_fontmanager_stage_path(other_user));
        assert!(!is_session_stage_path(system));
    }

    #[test]
    fn partition_legacy_paths_splits_documents() {
        let paths = vec![
            PathBuf::from(r"C:\Users\Eric\Documents\Font Manager\A\a.ttf"),
            PathBuf::from(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\aa.ttf"),
        ];
        let (docs, other) = partition_legacy_session_paths(&paths);
        assert_eq!(docs.len(), 1);
        assert_eq!(other.len(), 1);
        assert!(must_not_register_as_gdi_path(&docs[0]));
    }

    #[test]
    fn session_maps_roundtrip_json_and_legacy_per_user_alias() {
        let maps = vec![FaceMap {
            source: r"C:\Users\Eric\Documents\Font Manager\A\a.ttf".into(),
            stage: r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\aa.ttf".into(),
            registry_name: String::new(),
            family: "A".into(),
        }];
        let json = session_maps_to_json(&maps);
        let got = parse_session_maps_json(&json);
        assert_eq!(got, maps);
        // Legacy 1.0.156 key `per_user` still parses.
        let legacy = r#"[{"source":"S","per_user":"P","registry_name":"R","family":"F"}]"#;
        let parsed = parse_session_maps_json(legacy);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].stage, "P");
        assert_eq!(parsed[0].source, "S");
    }

    #[test]
    fn validate_drops_stale_missing_stage_keeps_existing() {
        let root = temp_root("validate");
        let stage_ok = root.join("ok.ttf");
        fs::write(&stage_ok, b"\x00\x01\x00\x00").unwrap();
        let maps = vec![
            FaceMap {
                source: r"C:\Users\Eric\Documents\Font Manager\A\a.ttf".into(),
                stage: stage_ok.to_string_lossy().into(),
                registry_name: String::new(),
                family: "A".into(),
            },
            FaceMap {
                source: r"C:\Users\Eric\Documents\Font Manager\B\b.ttf".into(),
                stage: root.join("missing.ttf").to_string_lossy().into(),
                registry_name: String::new(),
                family: "B".into(),
            },
        ];
        let got = validate_session_maps(&maps, &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].family, "A");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_accepts_restaged_and_refuses_documents_stage() {
        let root = temp_root("restage");
        let stage = root.join("fresh.ttf");
        fs::write(&stage, b"\x00\x01").unwrap();
        let maps = vec![FaceMap {
            source: "src".into(),
            stage: r"C:\Users\Eric\Documents\Font Manager\bad.ttf".into(),
            registry_name: String::new(),
            family: String::new(),
        }];
        let bad = validate_session_maps(&maps, &[]);
        assert!(bad.is_empty(), "Documents stage must be refused");
        let got = validate_session_maps(
            &maps,
            &[("src".into(), stage.to_string_lossy().into_owned())],
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].stage, stage.to_string_lossy());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stage_paths_from_maps_skips_documents() {
        let maps = vec![
            FaceMap {
                source: "s".into(),
                stage: r"C:\Users\Eric\Documents\Font Manager\a.ttf".into(),
                registry_name: String::new(),
                family: String::new(),
            },
            FaceMap {
                source: "s2".into(),
                stage: r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\x.ttf".into(),
                registry_name: String::new(),
                family: String::new(),
            },
        ];
        let paths = stage_paths_from_maps(&maps);
        assert_eq!(paths.len(), 1);
        assert!(is_gdi_maps_stage_path(&paths[0]));
    }

    #[test]
    fn filter_session_paths_drops_documents() {
        let paths = vec![
            PathBuf::from(r"C:\Users\Eric\Documents\Font Manager\A\a.ttf"),
            PathBuf::from(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\a.ttf"),
        ];
        let got = filter_session_paths_refuse_documents(&paths);
        assert_eq!(got.len(), 1);
        assert!(!must_not_register_as_gdi_path(&got[0]));
    }

    #[test]
    fn clear_session_maps_removes_file() {
        let root = temp_root("clear-maps");
        save_session_maps_in(
            &root,
            &[FaceMap {
                source: "a".into(),
                stage: "b".into(),
                registry_name: String::new(),
                family: String::new(),
            }],
        );
        assert!(session_maps_file_in(&root).is_file());
        clear_session_maps_in(&root);
        assert!(!session_maps_file_in(&root).is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn maps_needing_restage_when_stage_missing() {
        let root = temp_root("need-restage");
        let src = root.join("src.ttf");
        fs::write(&src, b"x").unwrap();
        let maps = vec![FaceMap {
            source: src.to_string_lossy().into(),
            stage: root.join("gone.ttf").to_string_lossy().into(),
            registry_name: String::new(),
            family: String::new(),
        }];
        let need = maps_needing_restage(&maps);
        assert_eq!(need.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_exit_marker_roundtrip_and_consume() {
        let root = temp_root("clean-exit");
        assert!(!has_clean_exit_marker_in(&root));
        assert!(!take_clean_exit_marker_in(&root));
        write_clean_exit_marker_in(&root);
        assert!(has_clean_exit_marker_in(&root));
        assert!(take_clean_exit_marker_in(&root));
        assert!(!has_clean_exit_marker_in(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_hkcu_value_name_preserves_full_hash_without_collisions() {
        let a = session_hkcu_value_name(Path::new(
            r"C:\Stage\0123456789ab_face_one_aaaaaaaaaaaaaaaa.ttf",
        ));
        let b = session_hkcu_value_name(Path::new(
            r"C:\Stage\0123456789ab_face_two_bbbbbbbbbbbbbbbb.ttf",
        ));
        let otc = session_hkcu_value_name(Path::new(r"C:\Stage\abcdef0123456789abcdef0123456789.otc"));
        assert_ne!(a, b);
        assert!(is_fm_session_hkcu_value_name(&a));
        assert!(is_fm_session_hkcu_value_name(&b));
        assert!(otc.ends_with("(OpenType)"));
    }

    #[test]
    fn persistent_active_marker_roundtrip_and_clear() {
        let root = temp_root("persistent-active");
        assert!(!has_persistent_active_marker_in(&root));
        write_persistent_active_marker_in(&root);
        assert!(has_persistent_active_marker_in(&root));
        clear_persistent_active_marker_in(&root);
        assert!(!has_persistent_active_marker_in(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compute_hkcu_diff_keeps_unchanged_deletes_stale_and_upserts_missing() {
        let keep_p = PathBuf::from(r"C:\Stage\aaaa1111_keep.ttf");
        let update_p = PathBuf::from(r"C:\Stage\bbbb2222_upd.ttf");
        let add_p = PathBuf::from(r"C:\Stage\cccc3333_add.ttf");
        let stale_p = PathBuf::from(r"C:\Stage\deadbeef.ttf");
        let stale_key = session_hkcu_value_name(&stale_p);
        let mut existing = std::collections::HashMap::new();
        existing.insert(
            session_hkcu_value_name(&keep_p),
            normalize_registry_font_path(&keep_p),
        );
        existing.insert(
            session_hkcu_value_name(&update_p),
            r"C:\Stage\old_path.ttf".to_string(),
        );
        existing.insert(
            stale_key.clone(),
            r"C:\Stage\deadbeef.ttf".to_string(),
        );
        existing.insert("Segoe UI (TrueType)".to_string(), "segoeui.ttf".to_string());

        let (to_upsert, to_delete) = compute_hkcu_diff(&existing, &[keep_p, update_p.clone(), add_p.clone()]);
        assert_eq!(to_upsert.len(), 2);
        assert!(to_upsert.iter().any(|(k, _)| k == &session_hkcu_value_name(&update_p)));
        assert!(to_upsert.iter().any(|(k, _)| k == &session_hkcu_value_name(&add_p)));
        assert_eq!(to_delete, vec![stale_key]);
    }

    #[test]
    fn normalize_registry_font_path_strips_extended_prefixes() {
        assert_eq!(
            normalize_registry_font_path(Path::new(r"\\?\C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\a.ttf")),
            r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\a.ttf"
        );
        assert_eq!(
            normalize_registry_font_path(Path::new(r"\\?\UNC\server\share\font.otf")),
            r"\\server\share\font.otf"
        );
    }

    #[test]
    fn hkcu_sync_toggle_roundtrip() {
        assert!(hkcu_sync_enabled());
        set_hkcu_sync_enabled(false);
        assert!(!hkcu_sync_enabled());
        set_hkcu_sync_enabled(true);
        assert!(hkcu_sync_enabled());
    }
}
