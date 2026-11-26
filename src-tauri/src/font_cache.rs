//! Universal, software-agnostic font cache scanner, cleaner, and live diagnostics snapshot.
//!
//! Designed around a declarative provider registry (`PROVIDERS`) covering:
//! - Fonths' own orphaned `gdi-maps` stage files + stale HKCU session registry entries
//! - Adobe Creative Apps (`AdobeFnt*.lst`, CoolType `CT Font Cache`, `FontFeaturePinUI`, safe CoreSync indexes)
//! - Figma Desktop (`FontCache` / `font_cache` files)
//! - Affinity Suite (Designer / Photo / Publisher temporary font cache files)
//! - CorelDRAW & Corel Font Manager (`FontCache` / `FontNav` cache files)
//! - Microsoft Office & LibreOffice font cache files
//!
//! Safety invariants:
//! - Never modifies `C:\Windows\Fonts`, `%SYSTEMROOT%\Fonts`, or user Documents folders.
//! - Never deletes real font binaries (`.ttf`, `.otf`, `.ttc`, `.otc`, `.woff`, `.woff2`) from third-party cache trees.
//! - Refuses to clean external app caches while their host `.exe` is actively running (detected via Toolhelp32).
//! - Sandbox runs (`activate::sandbox_active()`) and unit tests never touch real user app directories or HKCU.

use crate::activate;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const MAX_SCAN_DEPTH: usize = 6;
const SAMPLE_PATH_CAP: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheCleanScope {
    Minimal,
    Standard,
    Deep,
}

impl CacheCleanScope {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.unwrap_or("standard").trim().to_ascii_lowercase().as_str() {
            "minimal" | "min" => Self::Minimal,
            "deep" => Self::Deep,
            _ => Self::Standard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Minimal => 0,
            Self::Standard => 1,
            Self::Deep => 2,
        }
    }

    fn includes(self, min_scope: Self) -> bool {
        self.rank() >= min_scope.rank()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheProviderPreview {
    pub id: String,
    pub label: String,
    pub file_count: usize,
    pub bytes: u64,
    pub stale_hkcu_count: usize,
    pub running_processes: Vec<String>,
    pub blocked_by_running_app: bool,
    pub requires_admin: bool,
    pub sample_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontCacheScanResult {
    pub scope: CacheCleanScope,
    pub total_files: usize,
    pub total_bytes: u64,
    pub cleanable_files: usize,
    pub cleanable_bytes: u64,
    pub stale_hkcu_count: usize,
    pub running_blocked_providers: Vec<String>,
    pub providers: Vec<CacheProviderPreview>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontCacheCleanResult {
    pub scope: CacheCleanScope,
    pub deleted_files: usize,
    pub freed_bytes: u64,
    pub failed_files: usize,
    pub pruned_hkcu: usize,
    pub skipped_running_providers: Vec<String>,
    pub restarted_windows_font_cache: bool,
    pub broadcast_sent: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSnapshot {
    pub activation_mode: String,
    pub hkcu_entry_count: usize,
    pub live_family_count: usize,
    pub live_face_count: usize,
    pub gdi_objects: u32,
    pub gdi_quota: u32,
    pub gdi_maps_file_count: usize,
    pub gdi_maps_bytes: u64,
    pub protocol_cache_entries: usize,
    pub renderer_cache_entries: usize,
    pub detected_cache_files: usize,
    pub detected_cache_bytes: u64,
    pub elevated: bool,
}

type MatchFn = fn(&Path, &str, CacheCleanScope) -> bool;

struct ProviderRoot {
    path: PathBuf,
    requires_admin: bool,
}

struct CacheProviderSpec {
    id: &'static str,
    label: &'static str,
    min_scope: CacheCleanScope,
    process_names: &'static [&'static str],
    matcher: MatchFn,
}

fn is_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return true;
        }
    }
    false
}

fn is_protected_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    lower.contains("\\windows\\fonts")
        || lower.contains("\\programdata\\microsoft\\windows\\fonts")
        || lower.contains("\\documents\\")
        || lower.ends_with("\\documents")
}

fn has_font_binary_ext(name_lower: &str) -> bool {
    matches!(
        Path::new(name_lower)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
        "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2" | "dfont"
    )
}

fn path_lower(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_ascii_lowercase()
}

fn match_adobe_cache(path: &Path, name_lower: &str, scope: CacheCleanScope) -> bool {
    if has_font_binary_ext(name_lower) {
        return false;
    }
    // Always skip user presets, brushes, plugins, or workspace files.
    let full = path_lower(path);
    if full.contains("\\presets\\")
        || full.contains("\\plug-ins\\")
        || full.contains("\\plugins\\")
        || full.contains("\\workspaces\\")
    {
        return false;
    }
    if name_lower.starts_with("adobefnt") && name_lower.ends_with(".lst") {
        return true;
    }
    if scope == CacheCleanScope::Minimal {
        return false;
    }
    if (full.contains("\\ct font cache")
        || full.contains("\\fontfeaturepinui")
        || full.contains("\\typesupport"))
        && matches!(
            Path::new(name_lower).extension().and_then(|s| s.to_str()),
            Some("lst" | "cache" | "dat" | "db" | "idx")
        )
    {
        return true;
    }
    if scope == CacheCleanScope::Deep
        && (full.contains("\\coresync\\") || full.contains("\\typekit\\"))
        && matches!(
            Path::new(name_lower).extension().and_then(|s| s.to_str()),
            Some("lst" | "cache" | "db-wal" | "db-shm")
        )
    {
        return true;
    }
    false
}

fn match_figma_cache(path: &Path, name_lower: &str, scope: CacheCleanScope) -> bool {
    if !scope.includes(CacheCleanScope::Standard) || has_font_binary_ext(name_lower) {
        return false;
    }
    let full = path_lower(path);
    if !full.contains("\\figma\\") {
        return false;
    }
    let in_font_cache_dir = full.contains("\\fontcache") || full.contains("\\font_cache");
    let has_cache_name = name_lower.contains("fontcache") || name_lower.contains("font_cache");
    if !in_font_cache_dir && !has_cache_name {
        return false;
    }
    matches!(
        Path::new(name_lower).extension().and_then(|s| s.to_str()),
        None | Some("dat" | "bin" | "cache" | "db" | "lst" | "json")
    )
}

fn match_affinity_cache(path: &Path, name_lower: &str, scope: CacheCleanScope) -> bool {
    if !scope.includes(CacheCleanScope::Standard) || has_font_binary_ext(name_lower) {
        return false;
    }
    let full = path_lower(path);
    (full.contains("\\affinity\\") || full.contains("\\.affinity\\"))
        && full.contains("\\temp\\")
        && (name_lower.contains("font") || name_lower.contains("typeface"))
}

fn match_corel_cache(path: &Path, name_lower: &str, scope: CacheCleanScope) -> bool {
    if !scope.includes(CacheCleanScope::Standard) || has_font_binary_ext(name_lower) {
        return false;
    }
    let full = path_lower(path);
    full.contains("\\corel\\")
        && (name_lower.contains("fontcache") || name_lower.contains("fontnav"))
        && matches!(
            Path::new(name_lower).extension().and_then(|s| s.to_str()),
            Some("cache" | "dat" | "db" | "lst")
        )
}

fn match_office_cache(path: &Path, name_lower: &str, scope: CacheCleanScope) -> bool {
    if !scope.includes(CacheCleanScope::Standard) || has_font_binary_ext(name_lower) {
        return false;
    }
    let full = path_lower(path);
    if full.contains("\\microsoft\\fontcache") {
        return matches!(
            Path::new(name_lower).extension().and_then(|s| s.to_str()),
            Some("dat" | "cache" | "bin")
        );
    }
    if full.contains("\\libreoffice\\") && full.contains("\\cache\\") {
        return name_lower.contains("font");
    }
    false
}

const PROVIDERS: &[CacheProviderSpec] = &[
    CacheProviderSpec {
        id: "adobe",
        label: "Adobe Creative Apps (CoolType)",
        min_scope: CacheCleanScope::Minimal,
        process_names: &[
            "photoshop.exe",
            "illustrator.exe",
            "indesign.exe",
            "adobe premiere pro.exe",
            "afterfx.exe",
            "acrobat.exe",
            "acrord32.exe",
            "xd.exe",
            "animate.exe",
            "dimension.exe",
        ],
        matcher: match_adobe_cache,
    },
    CacheProviderSpec {
        id: "figma",
        label: "Figma Desktop",
        min_scope: CacheCleanScope::Standard,
        process_names: &["figma.exe", "figma_agent.exe"],
        matcher: match_figma_cache,
    },
    CacheProviderSpec {
        id: "affinity",
        label: "Affinity Suite (Photo / Designer / Publisher)",
        min_scope: CacheCleanScope::Standard,
        process_names: &["photo.exe", "designer.exe", "publisher.exe"],
        matcher: match_affinity_cache,
    },
    CacheProviderSpec {
        id: "corel",
        label: "CorelDRAW & Font Manager",
        min_scope: CacheCleanScope::Standard,
        process_names: &["coreldrw.exe", "fontmanager.exe"],
        matcher: match_corel_cache,
    },
    CacheProviderSpec {
        id: "office-suite",
        label: "Microsoft Office & LibreOffice",
        min_scope: CacheCleanScope::Standard,
        process_names: &["winword.exe", "powerpnt.exe", "excel.exe", "soffice.bin"],
        matcher: match_office_cache,
    },
];

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn provider_roots(id: &str, elevated: bool) -> Vec<ProviderRoot> {
    let mut roots = Vec::new();
    let local = env_dir("LOCALAPPDATA");
    let roaming = env_dir("APPDATA");
    let temp = env_dir("TEMP");
    let profile = env_dir("USERPROFILE");
    match id {
        "adobe" => {
            if let Some(ref d) = local {
                roots.push(ProviderRoot { path: d.join("Adobe"), requires_admin: false });
            }
            if let Some(ref d) = roaming {
                roots.push(ProviderRoot { path: d.join("Adobe"), requires_admin: false });
            }
            if let Some(ref d) = temp {
                roots.push(ProviderRoot { path: d.clone(), requires_admin: false });
            }
            if elevated {
                if let Some(pf) = env_dir("ProgramFiles") {
                    roots.push(ProviderRoot {
                        path: pf.join("Common Files").join("Adobe").join("TypeSupport"),
                        requires_admin: true,
                    });
                }
                if let Some(pf86) = env_dir("ProgramFiles(x86)") {
                    roots.push(ProviderRoot {
                        path: pf86.join("Common Files").join("Adobe").join("TypeSupport"),
                        requires_admin: true,
                    });
                }
            }
        }
        "figma" => {
            if let Some(ref d) = roaming {
                roots.push(ProviderRoot { path: d.join("Figma"), requires_admin: false });
            }
            if let Some(ref d) = local {
                roots.push(ProviderRoot { path: d.join("Figma"), requires_admin: false });
            }
        }
        "affinity" => {
            if let Some(ref d) = roaming {
                roots.push(ProviderRoot { path: d.join("Affinity"), requires_admin: false });
            }
            if let Some(ref d) = profile {
                roots.push(ProviderRoot { path: d.join(".affinity"), requires_admin: false });
            }
        }
        "corel" => {
            if let Some(ref d) = local {
                roots.push(ProviderRoot { path: d.join("Corel"), requires_admin: false });
            }
            if let Some(ref d) = roaming {
                roots.push(ProviderRoot { path: d.join("Corel"), requires_admin: false });
            }
        }
        "office-suite" => {
            if let Some(ref d) = local {
                roots.push(ProviderRoot {
                    path: d.join("Microsoft").join("FontCache"),
                    requires_admin: false,
                });
            }
            if let Some(ref d) = roaming {
                roots.push(ProviderRoot {
                    path: d.join("LibreOffice").join("4").join("user").join("cache"),
                    requires_admin: false,
                });
            }
        }
        _ => {}
    }
    roots
}

/// Snapshot currently running Windows `.exe` filenames (lowercased) using Toolhelp32.
/// Returns empty in tests, sandbox mode, or non-Windows platforms.
#[cfg(all(windows, not(test)))]
pub fn list_running_process_names() -> HashSet<String> {
    if activate::sandbox_active() {
        return HashSet::new();
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessEntry32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; 260],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(h_object: isize) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap == 0 {
        return HashSet::new();
    }

    let mut out = HashSet::new();
    let mut entry = ProcessEntry32W {
        dwSize: std::mem::size_of::<ProcessEntry32W>() as u32,
        cntUsage: 0,
        th32ProcessID: 0,
        th32DefaultHeapID: 0,
        th32ModuleID: 0,
        cntThreads: 0,
        th32ParentProcessID: 0,
        pcPriClassBase: 0,
        dwFlags: 0,
        szExeFile: [0u16; 260],
    };

    let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
    while ok != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len])
            .trim()
            .to_ascii_lowercase();
        if !name.is_empty() {
            out.insert(name);
        }
        ok = unsafe { Process32NextW(snap, &mut entry) };
    }
    unsafe {
        CloseHandle(snap);
    }
    out
}

#[cfg(any(not(windows), test))]
pub fn list_running_process_names() -> HashSet<String> {
    HashSet::new()
}

/// Recursively scan `root` up to `MAX_SCAN_DEPTH` without following symlinks/junctions.
pub fn scan_provider_dir(
    root: &Path,
    scope: CacheCleanScope,
    matcher: MatchFn,
    max_depth: usize,
) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > max_depth || is_protected_path(&dir) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() || is_reparse_or_symlink(&meta) {
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let Ok(emeta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if is_reparse_or_symlink(&emeta) || is_protected_path(&p) {
                continue;
            }
            if emeta.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if !emeta.is_file() {
                continue;
            }
            let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let name_lower = fname.to_ascii_lowercase();
            if matcher(&p, &name_lower, scope) {
                out.push((p, emeta.len()));
            }
        }
    }
    out
}

struct CollectedProvider {
    preview: CacheProviderPreview,
    matched_files: Vec<(PathBuf, u64)>,
    leftover_stage_paths: Vec<String>,
}

fn collect_all_providers(
    app: &AppHandle,
    scope: CacheCleanScope,
    running_procs: &HashSet<String>,
    sandbox: bool,
    elevated: bool,
) -> Vec<CollectedProvider> {
    let mut out = Vec::new();

    // Provider 1: Fonths orphaned gdi-maps stage files + stale HKCU session registry keys.
    let stage_items = activate::list_leftover_stage_files_blocking(app.clone()).unwrap_or_default();
    let (stale_hkcu_count, _) = if sandbox {
        (0, 0)
    } else {
        activate::prune_stale_hkcu_session_entries(true)
    };
    let stage_bytes: u64 = stage_items.iter().map(|i| i.bytes).sum();
    let stage_samples: Vec<String> = stage_items
        .iter()
        .take(SAMPLE_PATH_CAP)
        .map(|i| i.path.clone())
        .collect();
    let stage_paths: Vec<String> = stage_items.iter().map(|i| i.path.clone()).collect();
    out.push(CollectedProvider {
        preview: CacheProviderPreview {
            id: "fonths-stage".into(),
            label: "Fonths Stage & HKCU Registry".into(),
            file_count: stage_paths.len(),
            bytes: stage_bytes,
            stale_hkcu_count,
            running_processes: Vec::new(),
            blocked_by_running_app: false,
            requires_admin: false,
            sample_paths: stage_samples,
        },
        matched_files: Vec::new(),
        leftover_stage_paths: stage_paths,
    });

    if sandbox {
        return out;
    }

    // Providers 2..N: External host design & productivity apps (software-agnostic).
    for spec in PROVIDERS {
        if !scope.includes(spec.min_scope) {
            continue;
        }
        let active_procs: Vec<String> = spec
            .process_names
            .iter()
            .filter(|p| running_procs.contains(**p))
            .map(|p| (*p).to_string())
            .collect();
        let mut matched: Vec<(PathBuf, u64)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut any_requires_admin = false;
        for root in provider_roots(spec.id, elevated) {
            // Note: `%TEMP%` is a shared folder so limit Adobe `%TEMP%` walk depth to 2.
            let is_temp_root = root
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("Temp"))
                .unwrap_or(false);
            let depth = if is_temp_root { 2 } else { MAX_SCAN_DEPTH };
            let hits = scan_provider_dir(&root.path, scope, spec.matcher, depth);
            if !hits.is_empty() && root.requires_admin {
                any_requires_admin = true;
            }
            for (p, sz) in hits {
                if seen.insert(path_lower(&p)) {
                    matched.push((p, sz));
                }
            }
        }
        let bytes: u64 = matched.iter().map(|(_, s)| *s).sum();
        let samples: Vec<String> = matched
            .iter()
            .take(SAMPLE_PATH_CAP)
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        out.push(CollectedProvider {
            preview: CacheProviderPreview {
                id: spec.id.into(),
                label: spec.label.into(),
                file_count: matched.len(),
                bytes,
                stale_hkcu_count: 0,
                blocked_by_running_app: !active_procs.is_empty(),
                running_processes: active_procs,
                requires_admin: any_requires_admin,
                sample_paths: samples,
            },
            matched_files: matched,
            leftover_stage_paths: Vec::new(),
        });
    }

    out
}

/// Remove a vetted list of matched cache files permanently (`Temporary Fonts Rule`),
/// returning `(deleted_files, freed_bytes, failed_files)`.
pub fn clean_matched_files(files: &[(PathBuf, u64)]) -> (usize, u64, usize) {
    let mut deleted = 0usize;
    let mut freed = 0u64;
    let mut failed = 0usize;
    for (p, sz) in files {
        if is_protected_path(p) || has_font_binary_ext(&p.to_string_lossy().to_ascii_lowercase()) {
            failed += 1;
            continue;
        }
        match crate::delete_ops::delete_temp_file_permanently(p) {
            Ok(()) => {
                deleted += 1;
                freed = freed.saturating_add(*sz);
            }
            Err(_) => failed += 1,
        }
    }
    (deleted, freed, failed)
}

pub fn scan_font_caches_blocking(
    app: AppHandle,
    scope: CacheCleanScope,
) -> Result<FontCacheScanResult, String> {
    let running = list_running_process_names();
    let sandbox = activate::sandbox_active();
    let elevated = crate::is_elevated();
    let collected = collect_all_providers(&app, scope, &running, sandbox, elevated);

    let mut total_files = 0usize;
    let mut total_bytes = 0u64;
    let mut cleanable_files = 0usize;
    let mut cleanable_bytes = 0u64;
    let mut stale_hkcu_count = 0usize;
    let mut running_blocked_providers = Vec::new();
    let mut providers = Vec::with_capacity(collected.len());

    for item in collected {
        total_files = total_files.saturating_add(item.preview.file_count);
        total_bytes = total_bytes.saturating_add(item.preview.bytes);
        stale_hkcu_count = stale_hkcu_count.saturating_add(item.preview.stale_hkcu_count);
        if item.preview.blocked_by_running_app && item.preview.file_count > 0 {
            running_blocked_providers.push(item.preview.label.clone());
        } else {
            cleanable_files = cleanable_files.saturating_add(item.preview.file_count);
            cleanable_bytes = cleanable_bytes.saturating_add(item.preview.bytes);
        }
        providers.push(item.preview);
    }

    Ok(FontCacheScanResult {
        scope,
        total_files,
        total_bytes,
        cleanable_files,
        cleanable_bytes,
        stale_hkcu_count,
        running_blocked_providers,
        providers,
    })
}

pub fn clean_font_caches_blocking(
    app: AppHandle,
    scope: CacheCleanScope,
) -> Result<FontCacheCleanResult, String> {
    let running = list_running_process_names();
    let sandbox = activate::sandbox_active();
    let elevated = crate::is_elevated();
    let collected = collect_all_providers(&app, scope, &running, sandbox, elevated);

    let mut deleted_files = 0usize;
    let mut freed_bytes = 0u64;
    let mut failed_files = 0usize;
    let mut pruned_hkcu = 0usize;
    let mut skipped_running_providers = Vec::new();

    for item in collected {
        if item.preview.id == "fonths-stage" {
            if !item.leftover_stage_paths.is_empty() {
                let stage_bytes = item.preview.bytes;
                let res = activate::recycle_leftover_stage_files_blocking(
                    app.clone(),
                    item.leftover_stage_paths,
                )?;
                deleted_files = deleted_files.saturating_add(res.recycled);
                failed_files = failed_files.saturating_add(res.failed.len());
                if res.recycled > 0 {
                    freed_bytes = freed_bytes.saturating_add(stage_bytes);
                }
            }
            if !sandbox && item.preview.stale_hkcu_count > 0 {
                let (_, removed) = activate::prune_stale_hkcu_session_entries(false);
                pruned_hkcu = pruned_hkcu.saturating_add(removed);
            }
            continue;
        }
        if item.preview.blocked_by_running_app {
            if item.preview.file_count > 0 {
                skipped_running_providers.push(item.preview.label.clone());
            }
            continue;
        }
        if !item.matched_files.is_empty() {
            let (d, b, f) = clean_matched_files(&item.matched_files);
            deleted_files = deleted_files.saturating_add(d);
            freed_bytes = freed_bytes.saturating_add(b);
            failed_files = failed_files.saturating_add(f);
        }
    }

    let mut restarted_windows_font_cache = false;
    let mut broadcast_sent = false;
    if !sandbox && scope != CacheCleanScope::Minimal && (deleted_files > 0 || pruned_hkcu > 0) {
        if elevated {
            activate::restart_font_cache_for_unlock();
            restarted_windows_font_cache = true;
        }
        #[cfg(windows)]
        {
            activate::winfont::flush_cache();
        }
        broadcast_sent = true;
    }

    Ok(FontCacheCleanResult {
        scope,
        deleted_files,
        freed_bytes,
        failed_files,
        pruned_hkcu,
        skipped_running_providers,
        restarted_windows_font_cache,
        broadcast_sent,
    })
}

pub fn get_diagnostics_snapshot_blocking(
    app: AppHandle,
    scope: CacheCleanScope,
) -> Result<DiagnosticsSnapshot, String> {
    let gdi = activate::gdi_session_state();
    let (gdi_maps_file_count, gdi_maps_bytes) = activate::gdi_maps_dir_stats();
    let scan = scan_font_caches_blocking(app, scope)?;
    Ok(DiagnosticsSnapshot {
        activation_mode: activate::activation_mode_label().into(),
        hkcu_entry_count: activate::count_hkcu_session_entries(),
        live_family_count: gdi.live_family_count,
        live_face_count: gdi.live_face_count,
        gdi_objects: gdi.gdi_objects,
        gdi_quota: gdi.gdi_quota,
        gdi_maps_file_count,
        gdi_maps_bytes,
        protocol_cache_entries: crate::font_protocol::cache_len(),
        renderer_cache_entries: crate::renderer::cache_len(),
        detected_cache_files: scan.total_files,
        detected_cache_bytes: scan.total_bytes,
        elevated: crate::is_elevated(),
    })
}

/// Scan cleanable font caches across Fonths + third-party creative/office apps without deleting.
#[tauri::command]
pub async fn scan_font_caches(
    app: AppHandle,
    scope: Option<String>,
) -> Result<FontCacheScanResult, String> {
    let parsed = CacheCleanScope::parse(scope.as_deref());
    tauri::async_runtime::spawn_blocking(move || scan_font_caches_blocking(app, parsed))
        .await
        .map_err(|e| e.to_string())?
}

/// Clean non-blocked font caches for `scope`, respecting running host apps and Windows font safety rules.
#[tauri::command]
pub async fn clean_font_caches(
    app: AppHandle,
    scope: Option<String>,
) -> Result<FontCacheCleanResult, String> {
    let parsed = CacheCleanScope::parse(scope.as_deref());
    tauri::async_runtime::spawn_blocking(move || clean_font_caches_blocking(app, parsed))
        .await
        .map_err(|e| e.to_string())?
}

/// Snapshot live Windows GDI, HKCU, gdi-maps, `font://` mmap, Fontations cache, and app-cache metrics.
#[tauri::command]
pub async fn get_diagnostics_snapshot(
    app: AppHandle,
    scope: Option<String>,
) -> Result<DiagnosticsSnapshot, String> {
    let parsed = CacheCleanScope::parse(scope.as_deref());
    tauri::async_runtime::spawn_blocking(move || get_diagnostics_snapshot_blocking(app, parsed))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_file(path: &Path) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, b"cache-bytes-123456").unwrap();
    }

    #[test]
    fn adobe_matcher_respects_minimal_standard_deep_and_preserves_fonts_and_presets() {
        let dir = std::env::temp_dir().join(format!("fm-adobe-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let lst = dir.join("TypeSupport").join("CS6").join("AdobeFnt22.lst");
        let ct_cache = dir.join("CT Font Cache").join("FntNames.db");
        let coresync = dir.join("CoreSync").join("Typekit").join("state.db-wal");
        let ttf = dir.join("CoreSync").join("livetype").join("Inter.ttf");
        let preset = dir.join("Presets").join("AdobeFntBrush.lst");

        mk_file(&lst);
        mk_file(&ct_cache);
        mk_file(&coresync);
        mk_file(&ttf);
        mk_file(&preset);

        let min_hits = scan_provider_dir(&dir, CacheCleanScope::Minimal, match_adobe_cache, 6);
        assert_eq!(min_hits.len(), 1);
        assert!(min_hits[0].0.ends_with("AdobeFnt22.lst"));

        let std_hits = scan_provider_dir(&dir, CacheCleanScope::Standard, match_adobe_cache, 6);
        assert_eq!(std_hits.len(), 2);

        let deep_hits = scan_provider_dir(&dir, CacheCleanScope::Deep, match_adobe_cache, 6);
        assert_eq!(deep_hits.len(), 3);
        assert!(!deep_hits.iter().any(|(p, _)| p == &ttf || p == &preset));

        let (deleted, freed, failed) = clean_matched_files(&deep_hits);
        assert_eq!(deleted, 3);
        assert_eq!(failed, 0);
        assert!(freed > 0);
        assert!(ttf.is_file(), "font binaries must never be removed");
        assert!(preset.is_file(), "user presets must never be removed");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn modular_other_app_matchers_filter_only_safe_font_caches() {
        let dir = std::env::temp_dir().join(format!("fm-apps-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let figma_cache = dir.join("Figma").join("FontCache").join("v1.bin");
        let figma_cookies = dir.join("Figma").join("Network").join("Cookies");
        let aff_temp = dir.join("Affinity").join("Designer").join("1.0").join("temp").join("font_enum.tmp");
        let aff_brush = dir.join("Affinity").join("Designer").join("1.0").join("user").join("raster_brushes.propcol");
        let office_fc = dir.join("Microsoft").join("FontCache").join("4").join("FontCache-S-1.dat");

        mk_file(&figma_cache);
        mk_file(&figma_cookies);
        mk_file(&aff_temp);
        mk_file(&aff_brush);
        mk_file(&office_fc);

        let fig = scan_provider_dir(&dir, CacheCleanScope::Standard, match_figma_cache, 6);
        assert_eq!(fig.len(), 1);
        assert_eq!(fig[0].0, figma_cache);

        let aff = scan_provider_dir(&dir, CacheCleanScope::Standard, match_affinity_cache, 6);
        assert_eq!(aff.len(), 1);
        assert_eq!(aff[0].0, aff_temp);

        let off = scan_provider_dir(&dir, CacheCleanScope::Standard, match_office_cache, 6);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].0, office_fc);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_hkcu_predicate_detects_missing_target_and_ignores_other_keys() {
        let existing = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("Figtree-wght.ttf");
        let existing_str = existing.to_string_lossy();
        assert!(!crate::session_stage::is_stale_hkcu_entry(
            "Arial (TrueType)",
            r"C:\missing\arial.ttf"
        ));
        assert!(!crate::session_stage::is_stale_hkcu_entry(
            "Fonths [FM-Session-abc] (TrueType)",
            &existing_str
        ));
        assert!(crate::session_stage::is_stale_hkcu_entry(
            "Fonths [FM-Session-abc] (TrueType)",
            r"C:\nonexistent\definitely-missing-font-stage.ttf"
        ));
    }
}
