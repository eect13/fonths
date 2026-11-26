use std::collections::{HashMap, HashSet, VecDeque};
// c253 #2: heavy commands are `async` + spawn_blocking wrappers over `*_blocking`
// bodies, because Tauri runs sync commands on the main (UI) thread.
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use rayon::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[path = "winfont.rs"]
mod winfont_host;
#[cfg(windows)]
pub use winfont_host::winfont;

mod cdn_sync;
mod disk_scan;
mod downloads;
mod session;
#[cfg(test)]
mod tests;

pub use cdn_sync::*;
pub use disk_scan::*;
pub use downloads::*;
pub use session::*;


fn sanitize(name: &str) -> String {
    let t: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let t = t.trim_matches(['.', ' ', '-']).to_string();
    if t.is_empty() {
        "font".into()
    } else {
        t
    }
}

// ---- c262 sandbox: own app ID + temp library; never the real session ----
pub const SANDBOX_IDENTIFIER: &str = "app.fonths.sandbox";
pub const LEGACY_SANDBOX_IDENTIFIER: &str = "app.fontmanager.sandbox";
static SANDBOX_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Sandbox is on when the build's identifier is the sandbox one or `FM_SANDBOX_DOCS`
/// is set. Root = `FM_SANDBOX_DOCS`, else `%TEMP%\fm-sandbox`. Production: None.
fn sandbox_root_for(identifier: &str, env: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let env = env.filter(|v| !v.is_empty()).map(PathBuf::from);
    if identifier == SANDBOX_IDENTIFIER || identifier == LEGACY_SANDBOX_IDENTIFIER || env.is_some() {
        Some(env.unwrap_or_else(|| std::env::temp_dir().join("fm-sandbox")))
    } else {
        None
    }
}
pub fn init_sandbox(identifier: &str) -> Option<PathBuf> {
    SANDBOX_ROOT
        .get_or_init(|| sandbox_root_for(identifier, std::env::var_os("FM_SANDBOX_DOCS")))
        .clone()
}
pub fn sandbox_active() -> bool {
    sandbox_root().is_some()
}
fn sandbox_root() -> Option<PathBuf> {
    SANDBOX_ROOT.get().cloned().flatten()
}

pub(crate) fn documents_root(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(sb) = sandbox_root() {
        return Ok(sb.join("Documents").join("Fonths"));
    }
    app.path()
        .document_dir()
        .map(|p| p.join("Fonths"))
        .map_err(|e| e.to_string())
}

/// Resolve `Documents\Fonths` without a Tauri `AppHandle` (used by the headless `--daemon` process).
pub(crate) fn documents_root_headless() -> Option<PathBuf> {
    if let Some(sb) = sandbox_root() {
        return Some(sb.join("Documents").join("Fonths"));
    }
    #[cfg(windows)]
    {
        // Query Windows Known Folder `FOLDERID_Documents` ({FDD39AD0-238F-46AF-ADB4-6C85480369C7})
        // so redirected OneDrive / custom Documents locations match `app.path().document_dir()`.
        #[repr(C)]
        #[allow(clippy::upper_case_acronyms)]
        struct GUID {
            data1: u32,
            data2: u16,
            data3: u16,
            data4: [u8; 8],
        }
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHGetKnownFolderPath(
                rfid: *const GUID,
                dw_flags: u32,
                h_token: isize,
                ppsz_path: *mut *mut u16,
            ) -> i32;
        }
        #[link(name = "ole32")]
        unsafe extern "system" {
            fn CoTaskMemFree(pv: *mut core::ffi::c_void);
        }
        const FOLDERID_DOCUMENTS: GUID = GUID {
            data1: 0xFDD3_9AD0,
            data2: 0x238F,
            data3: 0x46AF,
            data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
        };
        let mut raw: *mut u16 = std::ptr::null_mut();
        let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_DOCUMENTS, 0, 0, &mut raw) };
        if hr == 0 && !raw.is_null() {
            let mut len = 0usize;
            unsafe {
                while *raw.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(raw, len);
                let os = std::ffi::OsString::from_wide(slice);
                CoTaskMemFree(raw as *mut core::ffi::c_void);
                let p = PathBuf::from(os);
                if !p.as_os_str().is_empty() {
                    return Some(p.join("Fonths"));
                }
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            return Some(PathBuf::from(profile).join("Documents").join("Fonths"));
        }
        None
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents").join("Fonths"))
    }
}

#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;

const PROVIDERS_DIR: &str = "Providers";
pub(crate) const GOOGLE_CONTAINER: &str = "Google";
pub(crate) const UPLOADS_CONTAINER: &str = "Uploads";

/// FontBase-style provider layout: `Documents\Fonths\Providers\Google` (separate from `Uploads`).
pub(crate) fn google_container_path(root: &Path) -> PathBuf {
    root.join(PROVIDERS_DIR).join(GOOGLE_CONTAINER)
}

fn family_dir_for_source_in(root: &Path, family: &str, is_upload: bool) -> PathBuf {
    if is_upload {
        root.join(UPLOADS_CONTAINER).join(family_folder_name(family))
    } else {
        google_container_path(root).join(family_folder_name(family))
    }
}

fn family_dir_for_intent(app: &AppHandle, family: &str, intent: FetchIntent) -> Result<PathBuf, String> {
    let root = documents_root(app)?;
    Ok(family_dir_for_source_in(
        &root,
        family,
        matches!(intent, FetchIntent::Local),
    ))
}

fn family_dir(app: &AppHandle, family: &str) -> Result<PathBuf, String> {
    Ok(family_dir_in(&documents_root(app)?, family))
}

/// Default folder for Google/catalog downloads under `Google\<Family>` (like FontBase
/// separating provider folders from user uploads).
fn family_dir_in(root: &Path, family: &str) -> PathBuf {
    family_dir_for_source_in(root, family, false)
}

fn family_name_is_non_ascii(family: &str) -> bool {
    !family.trim().is_ascii()
}

/// On-disk folder name for `family`: `sanitize()` for ASCII names (unchanged, so the
/// catalog library is not renamed), `u-<fnv1a64>` for any name with non-ASCII.
fn family_folder_name(family: &str) -> String {
    if family_name_is_non_ascii(family) {
        unicode_family_slug(family)
    } else {
        sanitize(family)
    }
}

/// Marker in an upload folder holding the real family name (u-<hash> folders are not
/// human-readable, and the disk index / prune need folder → family).
fn family_name_marker(dir: &Path) -> PathBuf {
    dir.join(".family")
}

fn write_family_name_marker(dir: &Path, family: &str) {
    let marker = family_name_marker(dir);
    if fs::read_to_string(&marker).map(|s| s.trim() == family.trim()).unwrap_or(false) {
        return;
    }
    let _ = fs::write(marker, family.trim().as_bytes());
}

fn read_family_name_marker(dir: &Path) -> Option<String> {
    let s = fs::read_to_string(family_name_marker(dir)).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Family name exactly as the JS importer picks it (`src/lib/fonts/sfnt.ts` readName +
/// family choice): name ID 16, then 21, then 1; per ID the best record wins by score
/// (Windows 0x0409 = 5, other Windows = 4, Unicode = 3, Mac Roman = 1; first wins on
/// ties). Reads only the table directory and `name` (first face of a TTC/OTC).
fn sfnt_upload_family(path: &Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head[..12]).ok()?;
    let mut face_off: u64 = 0;
    if &head[0..4] == b"ttcf" {
        f.read_exact(&mut head[12..16]).ok()?;
        face_off = u64::from(u32::from_be_bytes([head[12], head[13], head[14], head[15]]));
        f.seek(SeekFrom::Start(face_off)).ok()?;
        f.read_exact(&mut head[..12]).ok()?;
    }
    if !matches!(&head[0..4], b"\x00\x01\x00\x00" | b"OTTO" | b"true") {
        return None;
    }
    let num_tables = usize::from(u16::from_be_bytes([head[4], head[5]]));
    if num_tables == 0 || num_tables > 512 {
        return None;
    }
    let mut dir = vec![0u8; num_tables * 16];
    f.read_exact(&mut dir).ok()?;
    let (mut name_off, mut name_len) = (0u64, 0usize);
    for rec in dir.as_chunks::<16>().0 {
        if &rec[0..4] == b"name" {
            name_off = u64::from(u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]));
            name_len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
        }
    }
    if !(6..=4 * 1024 * 1024).contains(&name_len) {
        return None;
    }
    f.seek(SeekFrom::Start(name_off)).ok()?;
    let mut name = vec![0u8; name_len];
    f.read_exact(&mut name).ok()?;
    let _ = face_off;
    sfnt_family_from_name_table(&name)
}

fn sfnt_family_from_name_table(name: &[u8]) -> Option<String> {
    let u16at = |o: usize| name.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let count = usize::from(u16at(2)?);
    let string_off = usize::from(u16at(4)?);
    let mut best: HashMap<u16, (u8, String)> = HashMap::new();
    for i in 0..count {
        let rec = 6 + i * 12;
        if rec + 12 > name.len() {
            break;
        }
        let (plat, enc, lang, id) = (u16at(rec)?, u16at(rec + 2)?, u16at(rec + 4)?, u16at(rec + 6)?);
        let (len, off) = (usize::from(u16at(rec + 8)?), usize::from(u16at(rec + 10)?));
        let start = string_off + off;
        let Some(raw) = name.get(start..start + len) else {
            continue;
        };
        let utf16 = |raw: &[u8]| {
            let units: Vec<u16> = raw
                .as_chunks::<2>().0.iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .filter(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units).trim().to_string()
        };
        let (text, score) = if plat == 3 && (enc == 1 || enc == 10) {
            (utf16(raw), if lang == 0x0409 { 5 } else { 4 })
        } else if plat == 0 {
            (utf16(raw), 3)
        } else if plat == 1 {
            (raw.iter().map(|&b| char::from(b)).collect::<String>().trim().to_string(), 1)
        } else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if best.get(&id).map(|(s, _)| score > *s).unwrap_or(true) {
            best.insert(id, (score, text));
        }
    }
    [16u16, 21, 1]
        .iter()
        .find_map(|id| best.get(id).map(|(_, t)| t.clone()))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SharedFolderMigration {
    moved: usize,
    kept_unmatched: usize,
    kept_conflict: usize,
    errors: usize,
}

const U_FOLDERS_MIGRATED_MARKER: &str = ".u-folders-v1";

/// Card 36: split legacy shared folders. Before 1.0.207+c36 every all-non-ASCII family
/// was written to `root/font/` and mixed names to their ASCII remnant (`思源 Sans` →
/// `Sans/`). Each font file whose **name table** family is non-ASCII and whose folder is
/// exactly `sanitize(family)` is renamed into that family's `u-<hash>` folder.
/// Never deletes: rename only; conflicts, locked files and unreadable / ASCII / other
/// families stay where they are. Idempotent (a second run moves nothing).
fn migrate_shared_family_folders(root: &Path) -> SharedFolderMigration {
    let mut out = SharedFolderMigration::default();
    for parent in [
        google_container_path(root),
        root.join(GOOGLE_CONTAINER),
        root.join(UPLOADS_CONTAINER),
        root.to_path_buf(),
        root.join("Activated"),
        root.join("Library"),
    ] {
        let Ok(rd) = fs::read_dir(&parent) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let folder = dir.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if folder.is_empty()
                || folder.starts_with('.')
                || folder.to_ascii_lowercase().starts_with("u-")
                || is_protected_family_dir(root, &dir)
            {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && matches!(
                            p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
                            Some("ttf" | "otf" | "ttc" | "otc")
                        )
                })
                .collect();
            files.sort();
            let mut touched: Vec<PathBuf> = Vec::new();
            for file in files {
                let Some(family) = sfnt_upload_family(&file) else {
                    out.kept_unmatched += 1;
                    eprintln!("[fm] migrate: kept {} (no readable name table)", file.display());
                    continue;
                };
                if !family_name_is_non_ascii(&family) || !sanitize(&family).eq_ignore_ascii_case(&folder) {
                    // ASCII family in its own folder, or a file the old rule did not place here.
                    continue;
                }
                let dest_dir = parent.join(unicode_family_slug(&family));
                let Some(file_name) = file.file_name() else {
                    continue;
                };
                let dest = dest_dir.join(file_name);
                if dest.exists() {
                    out.kept_conflict += 1;
                    eprintln!("[fm] migrate: kept {} ({} already exists)", file.display(), dest.display());
                    continue;
                }
                if let Err(err) = fs::create_dir_all(&dest_dir) {
                    out.errors += 1;
                    eprintln!("[fm] migrate: kept {} (mkdir {}: {err})", file.display(), dest_dir.display());
                    continue;
                }
                match fs::rename(&file, &dest) {
                    Ok(()) => {
                        out.moved += 1;
                        intact_forget(&file);
                        write_family_name_marker(&dest_dir, &family);
                        if !touched.contains(&dest_dir) {
                            touched.push(dest_dir);
                        }
                        eprintln!("[fm] migrate: {} -> {}", file.display(), dest.display());
                    }
                    Err(err) => {
                        out.errors += 1;
                        eprintln!("[fm] migrate: kept {} (rename: {err})", file.display());
                    }
                }
            }
            for dest_dir in &touched {
                let n = count_intact_faces(dest_dir);
                if n > 0 {
                    mark_family_complete(dest_dir, n);
                }
            }
            if !touched.is_empty() && dir_is_complete(&dir) {
                // Re-stamp the source's face count; never remove its markers.
                let n = count_intact_faces(&dir);
                if n > 0 {
                    mark_family_complete(&dir, n);
                }
            }
        }
    }
    out
}

fn migration_complete(result: &SharedFolderMigration) -> bool {
    result.errors == 0 && result.kept_conflict == 0
}

/// Boot hook: run the shared-folder split once per Documents root (skipped after a
/// clean run). Logs; never deletes. A kept conflict means the old file is still
/// invisible to the app, so the marker stays off and the next boot tries again.
fn migrate_shared_family_folders_on_boot(app: &AppHandle) {
    let Ok(root) = documents_root(app) else {
        return;
    };
    let marker = root.join(U_FOLDERS_MIGRATED_MARKER);
    if marker.is_file() || !root.is_dir() {
        return;
    }
    let result = migrate_shared_family_folders(&root);
    eprintln!("[fm] migrate shared family folders: {result:?}");
    if migration_complete(&result) {
        let _ = fs::write(&marker, b"1\n");
    } else if result.kept_conflict > 0 {
        eprintln!(
            "[fm] migrate: not writing {} — {} file(s) kept because the destination already exists",
            marker.display(),
            result.kept_conflict
        );
    }
}

/// Family name a folder stands for: its `.family` marker, else the folder name.
pub(crate) fn dir_family_name(dir: &Path) -> String {
    read_family_name_marker(dir).unwrap_or_else(|| {
        dir.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    })
}

fn family_locations(app: &AppHandle, family: &str) -> Vec<PathBuf> {
    let Ok(root) = documents_root(app) else {
        return Vec::new();
    };
    family_locations_in(&root, family)
}

fn family_locations_in(root: &Path, family: &str) -> Vec<PathBuf> {
    // Non-ASCII names: only their own u-<hash> folder. The lossy `sanitize()` key
    // (`font`, `Sans`, `JP`) and the ASCII-only slug belong to other families.
    let (key, slug) = if family_name_is_non_ascii(family) {
        let own = unicode_family_slug(family);
        (own.clone(), own)
    } else {
        (sanitize(family), family_folder_slug(family))
    };
    let legacy_raw = legacy_unicode_folder(family);
    let containers = [
        google_container_path(root),
        root.join(UPLOADS_CONTAINER),
        root.join(GOOGLE_CONTAINER),
        root.to_path_buf(),
        root.join("Activated"),
        root.join("Library"),
    ];
    let mut dirs = Vec::with_capacity(containers.len() * 3);
    for container in &containers {
        if container != root && !container.is_dir() {
            continue;
        }
        dirs.push(container.join(&key));
        if slug != key {
            dirs.push(container.join(&slug));
        }
        if let Some(ref raw) = legacy_raw {
            dirs.push(container.join(raw));
        }
    }
    dirs.sort();
    dirs.dedup();
    // Never hand the Font Manager root or container folders to a caller as a
    // "family folder" — callers walk, delete files in, or recycle these dirs.
    dirs.retain(|p| p.is_dir() && !is_protected_family_dir(root, p));
    if dirs.is_empty() {
        let fallback = family_dir_for_source_in(root, family, false);
        if !is_protected_family_dir(root, &fallback) {
            dirs.push(fallback);
        }
    }
    dirs
}

fn dir_is_upload_container_child(root: &Path, dir: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(root) else {
        return false;
    };
    let mut comps = rel.components();
    matches!(
        comps.next().and_then(|c| c.as_os_str().to_str()),
        Some(c) if c.eq_ignore_ascii_case(UPLOADS_CONTAINER)
    )
}

fn classify_legacy_folder_is_upload(dir: &Path) -> bool {
    if let Some(intent) = read_download_source(dir) {
        return matches!(intent, FetchIntent::Local);
    }
    if read_google_planned_keys(dir).is_some() || read_fontsource_planned_keys(dir).is_some() {
        return false;
    }
    if read_family_name_marker(dir).is_some() {
        return true;
    }
    !is_official_google_family(&dir_family_name(dir))
}

fn consolidate_dir_into(src_dir: &Path, dest_dir: &Path, moved: &mut u32) {
    if !dest_dir.exists() {
        if let Some(parent) = dest_dir.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if fs::rename(src_dir, dest_dir).is_ok() {
            *moved = moved.saturating_add(1);
        }
        return;
    }
    // Destination already exists: merge files without overwriting existing faces.
    let Ok(files) = fs::read_dir(src_dir) else {
        return;
    };
    let mut merged_any = false;
    for f in files.flatten() {
        let src_file = f.path();
        let Some(fname) = src_file.file_name() else {
            continue;
        };
        let dst_file = dest_dir.join(fname);
        if src_file.is_file() && !dst_file.exists()
            && fs::rename(&src_file, &dst_file).is_ok() {
                intact_forget(&src_file);
                merged_any = true;
            }
    }
    // Remove source dir only if now completely empty.
    if fs::read_dir(src_dir)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
        && (fs::remove_dir(src_dir).is_ok() || merged_any) {
            *moved = moved.saturating_add(1);
        }
}

/// Consolidate flat legacy folders (`root/<Family>`) and split `Activated/` /
/// `Library/` folders into FontBase-style containers:
/// - `Documents\Font Manager\Google\<Family>` (Google Fonts & catalog)
/// - `Documents\Font Manager\Uploads\<Family>` (user added/uploaded fonts)
///
/// Never overwrites existing files and stops cleanly if `cancel` is triggered.
fn consolidate_legacy_roots_in(root: &Path, cancel: Option<&AtomicBool>) -> u32 {
    let mut moved = 0u32;
    let route_dest = |src_dir: &Path, folder_name: &std::ffi::OsStr| -> PathBuf {
        let is_upload = classify_legacy_folder_is_upload(src_dir);
        if is_upload {
            root.join(UPLOADS_CONTAINER).join(folder_name)
        } else {
            google_container_path(root).join(folder_name)
        }
    };

    // 1. Migrate legacy flat `root/<Family>` directories into `Google/` or `Uploads/`.
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
                return moved;
            }
            let src_dir = entry.path();
            if !src_dir.is_dir() {
                continue;
            }
            let Some(folder_name) = src_dir.file_name() else {
                continue;
            };
            let name_str = folder_name.to_string_lossy();
            if name_str.starts_with('.') || is_protected_family_dir(root, &src_dir) {
                continue;
            }
            let dest_dir = route_dest(&src_dir, folder_name);
            consolidate_dir_into(&src_dir, &dest_dir, &mut moved);
        }
    }

    // 2. Migrate legacy `Google/<Family>`, `Activated/<Family>`, and `Library/<Family>` containers
    // into FontBase-style `Providers\Google\<Family>` (Google) and `Uploads\<Family>` (user uploads).
    for container in [GOOGLE_CONTAINER, "Activated", "Library"] {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            break;
        }
        let legacy_parent = root.join(container);
        if !legacy_parent.is_dir() {
            continue;
        }
        let Ok(rd) = fs::read_dir(&legacy_parent) else {
            continue;
        };
        for entry in rd.flatten() {
            if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
                return moved;
            }
            let src_dir = entry.path();
            if !src_dir.is_dir() {
                continue;
            }
            let Some(folder_name) = src_dir.file_name() else {
                continue;
            };
            let dest_dir = route_dest(&src_dir, folder_name);
            if is_protected_family_dir(root, &dest_dir) {
                continue;
            }
            consolidate_dir_into(&src_dir, &dest_dir, &mut moved);
        }
        // Remove container folder only when empty.
        if fs::read_dir(&legacy_parent)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(&legacy_parent);
        }
    }
    let uploads_dir = root.join(UPLOADS_CONTAINER);
    if fs::read_dir(&uploads_dir)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
    {
        let _ = fs::remove_dir(&uploads_dir);
    }
    moved
}

/// Pre-Card-36 JS upload folder (`os-activate.ts` safeSegment) for a non-ASCII family.
/// `None` for ASCII names (those use `sanitize`) or when nothing non-ASCII survives.
fn legacy_unicode_folder(family: &str) -> Option<String> {
    if !family_name_is_non_ascii(family) {
        return None;
    }
    let t: String = family
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '-' } else { c })
        .collect();
    let t = t.trim_end_matches(['.', ' ']).trim().to_string();
    (!t.is_empty() && !t.is_ascii()).then_some(t)
}

/// True for paths that must never be treated as (or deleted as) one family's folder:
/// the Documents root itself, `Activated`, `Library`, anything outside the root, or a
/// path whose last component is empty / `.` / `..`.
fn is_protected_family_dir(root: &Path, dir: &Path) -> bool {
    let reserved = |p: &Path| {
        p == root
            || p.file_name().is_none()
            || p.components().any(|c| {
                matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir)
            })
    };
    if reserved(dir) {
        return true;
    }
    let Ok(rel) = dir.strip_prefix(root) else {
        return true;
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    match parts.as_slice() {
        [] => true,
        [one] => {
            one.is_empty()
                || one.eq_ignore_ascii_case(PROVIDERS_DIR)
                || one.eq_ignore_ascii_case(GOOGLE_CONTAINER)
                || one.eq_ignore_ascii_case(UPLOADS_CONTAINER)
                || one.eq_ignore_ascii_case("Activated")
                || one.eq_ignore_ascii_case("Library")
        }
        [one, two] => {
            one.is_empty()
                || two.is_empty()
                || (one.eq_ignore_ascii_case(PROVIDERS_DIR) && two.eq_ignore_ascii_case(GOOGLE_CONTAINER))
        }
        _ => parts.iter().any(|p| p.is_empty()),
    }
}

pub(crate) fn walk_font_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_font_files(&path, out);
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2") {
            out.push(path);
        }
    }
}

fn ttf_magic(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    // SFNT only. WOFF/WOFF2 is preview — AddFontResourceW will not install it.
    matches!(
        &bytes[0..4],
        b"\x00\x01\x00\x00" | b"OTTO" | b"true" | b"typ1" | b"ttcf"
    )
}

fn ttf_intact(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let len = meta.len();
    if len < 256 {
        return false;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    {
        let cache = intact_cache();
        if let Ok(g) = cache.lock()
            && let Some(&(m, l, ok)) = g.get(path)
                && m == mtime && l == len {
                    return ok;
                }
    }
    let ok = ttf_intact_read(path);
    if let Ok(mut g) = intact_cache().lock() {
        g.insert(path.to_path_buf(), (mtime, len, ok));
    }
    ok
}

fn ttf_intact_read(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    ttf_magic(&magic)
}

type IntactCacheMap = Mutex<HashMap<PathBuf, (u64, u64, bool)>>;

fn intact_cache() -> &'static IntactCacheMap {
    static C: OnceLock<IntactCacheMap> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn intact_forget(path: &Path) {
    if let Ok(mut g) = intact_cache().lock() {
        g.remove(path);
    }
}

fn dir_has_intact(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files.iter().any(|p| ttf_intact(p))
}

fn for_family_dirs(app: &AppHandle, mut visit: impl FnMut(&Path)) {
    let Ok(root) = documents_root(app) else {
        return;
    };
    for dir in [
        google_container_path(&root),
        root.join(UPLOADS_CONTAINER),
        root.join(GOOGLE_CONTAINER),
        root.clone(),
        root.join("Activated"),
        root.join("Library"),
    ] {
        // Activate's `cancel` always stops a shared walk. `docs_cancel` only
        // stops a walk while a Documents refresh is actually live — a leftover
        // flag must not hide families from the next Activate/scan.
        if bulk().cancel.load(Ordering::SeqCst) {
            return;
        }
        if bulk().docs_vf_session_live.load(Ordering::SeqCst)
            && bulk().docs_cancel.load(Ordering::SeqCst)
        {
            return;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if bulk().cancel.load(Ordering::SeqCst) {
                return;
            }
            if bulk().docs_vf_session_live.load(Ordering::SeqCst)
                && bulk().docs_cancel.load(Ordering::SeqCst)
            {
                return;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if name.eq_ignore_ascii_case(PROVIDERS_DIR)
                || name.eq_ignore_ascii_case(GOOGLE_CONTAINER)
                || name.eq_ignore_ascii_case(UPLOADS_CONTAINER)
                || name.eq_ignore_ascii_case("Activated")
                || name.eq_ignore_ascii_case("Library")
            {
                continue;
            }
            visit(&path);
        }
    }
}

/// LocalAppData GDI copies so Documents\Font Manager is never AddFontResourceExW'd.
/// Font Cache holds the mapped path after Quit; in-place Adds left family folders
/// write-locked (Open Sauce Repair could not overwrite, Explorer could not delete).
fn gdi_map_file_name(src: &Path) -> String {
    use sha2::{Digest, Sha256};
    let key = src.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    let digest = Sha256::digest(key.as_bytes());
    let mut hex = String::with_capacity(32);
    for b in &digest[..16] {
        hex.push_str(&format!("{b:02x}"));
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("ttf")
        .to_ascii_lowercase();
    let ext = if matches!(ext.as_str(), "otf" | "ttf" | "ttc" | "otc") {
        ext
    } else {
        "ttf".into()
    };
    format!("{hex}.{ext}")
}

fn is_gdi_maps_dir_name(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    lower.contains("\\fonths\\gdi-maps") || lower.contains("\\font manager\\gdi-maps")
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn gdi_maps_root() -> Option<PathBuf> {
    if let Some(sb) = sandbox_root() {
        return Some(sb.join("gdi-maps"));
    }
    Some(local_app_data()?.join("Fonths").join("gdi-maps"))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn gdi_map_dest_for(src: &Path) -> Option<PathBuf> {
    if is_gdi_maps_dir_name(src) {
        return Some(src.to_path_buf());
    }
    Some(gdi_maps_root()?.join(gdi_map_file_name(src)))
}

static STAGE_ROTATE_SEQ: AtomicU64 = AtomicU64::new(1);

fn is_transient_lock_error(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::PermissionDenied || matches!(err.raw_os_error(), Some(32) | Some(33))
}

fn copy_with_lock_retry(src: &Path, dst: &Path) -> std::io::Result<u64> {
    let mut last_err = None;
    for attempt in 0..3u8 {
        match fs::copy(src, dst) {
            Ok(n) => return Ok(n),
            Err(e) if attempt < 2 && is_transient_lock_error(&e) => {
                last_err = Some(e);
                thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("copy locked")))
}

fn rotated_stage_path(dest: &Path) -> PathBuf {
    let seq = STAGE_ROTATE_SEQ.fetch_add(1, Ordering::Relaxed);
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("font");
    let ext = dest.extension().and_then(|s| s.to_str()).unwrap_or("ttf");
    dest.with_file_name(format!("{stem}-r{}-{seq}.{ext}", std::process::id()))
}

fn commit_stage_tmp(tmp: &Path, dest: &Path) -> Option<PathBuf> {
    if fs::rename(tmp, dest).is_ok() && dest.is_file() {
        return Some(dest.to_path_buf());
    }
    if fs::copy(tmp, dest).is_ok() {
        let _ = fs::remove_file(tmp);
        if dest.is_file() {
            return Some(dest.to_path_buf());
        }
    }
    // `dest` is write-locked by an external app (Word/Photoshop/Font Cache): rotate inside `gdi-maps`
    // so activation never fails with `StageCopyFailed`.
    let rotated = rotated_stage_path(dest);
    if fs::rename(tmp, &rotated).is_ok() && rotated.is_file() {
        return Some(rotated);
    }
    if fs::copy(tmp, &rotated).is_ok() {
        let _ = fs::remove_file(tmp);
        if rotated.is_file() {
            return Some(rotated);
        }
    }
    let _ = fs::remove_file(tmp);
    None
}

/// Copy `src` into gdi-maps when missing or size-mismatched. Never deletes `src`.
/// Retries transient read locks and rotates to a fresh stage file if `dest` is write-locked.
/// Returns `None` on failure — never falls back to Documents (must_not_register).
fn ensure_gdi_session_copy_to(src: &Path, maps_root: &Path) -> Option<PathBuf> {
    guard_test_path(maps_root);
    if !src.is_file() {
        return None;
    }
    if is_gdi_maps_dir_name(src) {
        return Some(src.to_path_buf());
    }
    // Refuse to "stage" a path that is already Documents — caller must copy.
    if crate::session_stage::must_not_register_as_gdi_path(src) {
        // Still OK as *source*; dest is under maps_root.
    }
    let dest = maps_root.join(gdi_map_file_name(src));
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let src_len = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let dest_ok = dest.is_file()
        && fs::metadata(&dest)
            .map(|m| m.len() == src_len && src_len >= 256)
            .unwrap_or(false);
    if dest_ok {
        return Some(dest);
    }
    // Zero-copy NTFS hardlink fast path: creates a stage entry in microseconds with 0 bytes
    // copied when `src` and `maps_root` share the same NTFS volume. Deleting the stage entry
    // never deletes the user's original in Documents\Fonths.
    if (!dest.exists() || fs::remove_file(&dest).is_ok())
        && fs::hard_link(src, &dest).is_ok() && dest.is_file() {
            return Some(dest);
        }
    let tmp = dest.with_extension("part");
    let _ = fs::remove_file(&tmp);
    match copy_with_lock_retry(src, &tmp) {
        Ok(_) => commit_stage_tmp(&tmp, &dest),
        Err(_) => {
            let _ = fs::remove_file(&tmp);
            None
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn ensure_gdi_session_copy(src: &Path) -> Option<PathBuf> {
    let root = gdi_maps_root()?;
    ensure_gdi_session_copy_to(src, &root)
}

/// Pre-cache `bytes` into `maps_root` directly from memory on download/upload so
/// the subsequent GDI activation does not need to re-read or copy from Documents.
#[cfg_attr(not(windows), allow(dead_code))]
fn prestage_gdi_session_bytes_to(src: &Path, bytes: &[u8], maps_root: &Path) -> Option<PathBuf> {
    guard_test_path(maps_root);
    if bytes.len() < 256 || is_gdi_maps_dir_name(src) {
        return None;
    }
    let dest = maps_root.join(gdi_map_file_name(src));
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let src_len = bytes.len() as u64;
    let dest_ok = dest.is_file()
        && fs::metadata(&dest)
            .map(|m| m.len() == src_len && src_len >= 256)
            .unwrap_or(false);
    if dest_ok {
        return Some(dest);
    }
    let tmp = dest.with_extension("part");
    let _ = fs::remove_file(&tmp);
    if fs::write(&tmp, bytes).is_ok() {
        commit_stage_tmp(&tmp, &dest)
    } else {
        let _ = fs::remove_file(&tmp);
        None
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn prestage_gdi_session_bytes(src: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let root = gdi_maps_root()?;
    prestage_gdi_session_bytes_to(src, bytes, &root)
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisterFailKind {
    StageCopyFailed,
    AddReturnedZero,
    GdiQuotaExceeded,
    InvalidFontHeader,
    Unloading,
    Refused,
    NoneTried,
}

impl RegisterFailKind {
    fn label(self) -> &'static str {
        match self {
            Self::StageCopyFailed => "stage-copy failed (ensure_gdi_session_copy)",
            Self::AddReturnedZero => "AddFontResourceExW returned 0",
            Self::GdiQuotaExceeded => "paused — Windows GDI handle ceiling reached (prevented crash)",
            Self::InvalidFontHeader => "invalid or corrupt font file (skipped before GDI)",
            Self::Unloading => "unloading — register aborted",
            Self::Refused => "register refused",
            Self::NoneTried => "no intact face tried",
        }
    }

    fn worsen(self, other: Self) -> Self {
        // Prefer concrete GDI/stage failures over refused/none.
        use RegisterFailKind::*;
        let rank = |k: RegisterFailKind| match k {
            GdiQuotaExceeded => 5,
            StageCopyFailed => 4,
            InvalidFontHeader => 4,
            AddReturnedZero => 3,
            Unloading => 2,
            Refused => 1,
            NoneTried => 0,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}

#[cfg(windows)]
fn outcome_to_fail(o: winfont::RegisterOutcome) -> Option<RegisterFailKind> {
    match o {
        winfont::RegisterOutcome::Ok => None,
        winfont::RegisterOutcome::StageCopyFailed => Some(RegisterFailKind::StageCopyFailed),
        winfont::RegisterOutcome::AddReturnedZero => Some(RegisterFailKind::AddReturnedZero),
        winfont::RegisterOutcome::GdiQuotaExceeded => Some(RegisterFailKind::GdiQuotaExceeded),
        winfont::RegisterOutcome::InvalidFontHeader => Some(RegisterFailKind::InvalidFontHeader),
        winfont::RegisterOutcome::Unloading => Some(RegisterFailKind::Unloading),
        winfont::RegisterOutcome::Refused => Some(RegisterFailKind::Refused),
    }
}

fn register_path(path: &Path) -> bool {
    register_path_detailed(path).is_none()
}

fn register_path_detailed(path: &Path) -> Option<RegisterFailKind> {
    if !register_allowed_for_path(path) {
        // c260 follow-up: files-only Repair of an inactive family — no GDI load.
        return Some(RegisterFailKind::NoneTried);
    }
    register_path_detailed_fast(path, false)
}

fn register_path_detailed_fast(path: &Path, preverified_stage: bool) -> Option<RegisterFailKind> {
    if !register_allowed_for_path(path) {
        // c260 follow-up: files-only Repair of an inactive family — no GDI load.
        return Some(RegisterFailKind::NoneTried);
    }
    #[cfg(windows)]
    {
        outcome_to_fail(winfont::register_detailed_with_fast(path, preverified_stage))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, preverified_stage);
        // Desktop product is Windows; Linux unit tests never Add — treat as Add 0.
        Some(RegisterFailKind::AddReturnedZero)
    }
}

fn register_family_path(family: &str, path: &Path) -> bool {
    let _ = sanitize_on_disk_for_gdi(path);
    let fail = register_path_detailed(path);
    #[cfg(windows)]
    if fail.is_none() {
        winfont::bind(family, path);
        return true;
    }
    // Known-incapable Add=0: settle later — never auto-chase 2015 pin / Fontsource
    // on the Activate critical path (honesty: Activated only when Add>0).
    if fail == Some(RegisterFailKind::AddReturnedZero) && family_may_settle_add_zero(family) {
        note_session_gdi_refused(family);
    }
    #[cfg(not(windows))]
    {
        let _ = family;
    }
    fail.is_none()
}


#[cfg_attr(not(windows), allow(dead_code))]
fn count_family_font_filenames(app: &AppHandle, family: &str) -> usize {
    let mut n = 0usize;
    for dir in family_locations(app, family) {
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        n = n.saturating_add(files.len());
    }
    n
}

/// Skip walk+ttf_intact+Add when this process already Add'd every current face.
/// Cheap: in-memory `by_family` ∩ `loaded()`, readdir filename count, size-matched maps.
/// Maps / last-session sidecar never authorize this. New files (count mismatch) or
/// resized faces (map size mismatch) force a walk; `register_detailed` still skip-Adds
/// unchanged paths.
fn family_skip_register_this_process(app: &AppHandle, family: &str) -> Option<usize> {
    #[cfg(not(windows))]
    {
        let _ = (app, family);
        return None;
    }
    #[cfg(windows)]
    {
        let live = winfont::family_live_face_count(family)?;
        if live == 0 {
            return None;
        }
        if count_family_font_filenames(app, family) != live {
            return None;
        }
        if let Some(paths) = winfont::family_bound_paths(family) {
            for p in &paths {
                if !winfont::is_session_live_mapped(p) {
                    return None;
                }
            }
        }
        Some(live)
    }
}

/// Count intact faces already live **this process** (in `loaded()` from a real
/// Add) with a size-matched gdi-map. Map alone is never enough — that only skips
/// re-copy. `None` = at least one face still needs Add (or is not loaded).
/// Hot path uses `family_skip_register_this_process` (no ttf_intact). Kept for
/// debug — do not call from Activate All.
#[allow(dead_code)]
fn count_already_live_intact_faces(app: &AppHandle, family: &str) -> Option<usize> {
    #[cfg(not(windows))]
    {
        let _ = (app, family);
        return None;
    }
    #[cfg(windows)]
    {
        let mut n = 0usize;
        let mut any = false;
        for dir in family_locations(app, family) {
            let mut files = Vec::new();
            walk_font_files(&dir, &mut files);
            for path in files {
                if !ttf_intact(&path) {
                    continue;
                }
                any = true;
                // Skip-Add only when this process already Add'd successfully.
                if !winfont::is_loaded(&path) {
                    return None;
                }
                if !winfont::is_session_live_mapped(&path) {
                    return None;
                }
                n = n.saturating_add(1);
            }
        }
        if any && n > 0 {
            Some(n)
        } else {
            None
        }
    }
}

/// Size-matched gdi-maps for every intact face (disk only — never touches `loaded()`).
#[cfg_attr(not(windows), allow(dead_code))]
fn count_size_matched_mapped_faces(app: &AppHandle, family: &str) -> Option<usize> {
    #[cfg(not(windows))]
    {
        let _ = (app, family);
        return None;
    }
    #[cfg(windows)]
    {
        let mut n = 0usize;
        let mut any = false;
        for dir in family_locations(app, family) {
            let mut files = Vec::new();
            walk_font_files(&dir, &mut files);
            for path in files {
                if !ttf_intact(&path) {
                    continue;
                }
                any = true;
                if !winfont::is_session_live_mapped(&path) {
                    return None;
                }
                n = n.saturating_add(1);
            }
        }
        if any && n > 0 {
            Some(n)
        } else {
            None
        }
    }
}

fn family_session_maps_live(app: &AppHandle, family: &str) -> bool {
    #[cfg(windows)]
    {
        let _ = app;
        winfont::family_live_face_count(family).is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = (app, family);
        false
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn family_in_session_active(app: &AppHandle, family: &str) -> bool {
    load_session_families(app)
        .iter()
        .any(|n| n.eq_ignore_ascii_case(family))
}

/// Session-active + size-matched maps ⇒ toast exemption only (no failed_names).
/// Does **not** authorize skip-Add / activated — that needs real in-process Add.
fn family_skip_live_success(app: &AppHandle, family: &str) -> Option<usize> {
    #[cfg(windows)]
    {
        if !family_in_session_active(app, family) {
            return None;
        }
        count_size_matched_mapped_faces(app, family)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, family);
        None
    }
}


/// Product rules for unit tests (no AppHandle / GDI):
/// - size-matched gdi-map ⇒ skip re-copy only
/// - skip-Add only when this process already has the face in `loaded()` (real Add)
/// - family skip-walk: bound ∩ loaded + filename count match + maps size-matched
/// - sticky toast exemption needs session-active + maps (never maps alone)
/// - known GDI-incapable + intact ⇒ suppress failed_names (not a download fail)
/// - known GDI-incapable + intact + !undersized ⇒ disk settled (`.complete` / Scan OK);
///   still never session-Activated / markLiveActivated (Add=0 honesty)
#[cfg_attr(not(test), allow(dead_code))]
fn face_may_skip_add(in_loaded_this_process: bool) -> bool {
    in_loaded_this_process
}

/// FontBase-like family skip: never maps-only, never last-session sidecar.
#[cfg_attr(not(test), allow(dead_code))]
fn family_may_skip_add_this_process(
    bound_all_in_loaded: bool,
    on_disk_filename_count: usize,
    bound_count: usize,
    all_bound_size_matched_mapped: bool,
) -> bool {
    bound_all_in_loaded
        && bound_count > 0
        && on_disk_filename_count == bound_count
        && all_bound_size_matched_mapped
}

/// Retry of an intact Gidugu face: always attempt sanitize+Add first.
/// Settle (no refetch) only when Add is still 0 and the file is full-size.
#[cfg_attr(not(test), allow(dead_code))]
fn retry_gidugu_settle_without_refetch(
    added: usize,
    known_incapable: bool,
    intact: bool,
    undersized: bool,
) -> bool {
    added == 0 && known_incapable && intact && !undersized
}

/// Retry must not skip register for Gidugu (that was the 2099 hole).
#[cfg_attr(not(test), allow(dead_code))]
fn retry_must_attempt_register_before_settle(intact: bool) -> bool {
    intact
}

/// Honest GDI-live ceiling: library count minus settled known-incapable (Add still 0).
/// Never report Activated == Library while Gidugu Add=0 (Live 2099 · Settled 1 · Library 2100).
#[cfg_attr(not(test), allow(dead_code))]
fn catalog_expected_gdi_live(catalog_families: usize, known_incapable_intact: usize) -> usize {
    catalog_families.saturating_sub(known_incapable_intact)
}

#[cfg_attr(not(test), allow(dead_code))]
fn format_live_settled_library(live: usize, settled: usize, library: usize) -> String {
    format!("Live {live} · Settled {settled} · Library {library}")
}

/// Bar never shows done > total (1.0.181: 4044/2253 was skipped double-count).
#[cfg_attr(not(test), allow(dead_code))]
fn progress_bar_done_total(done: u32, total: u32) -> (u32, u32) {
    let t = total.max(done).max(1);
    (done.min(t), t)
}
#[cfg_attr(not(test), allow(dead_code))]
fn job_downloaded_count(done: usize, skipped: usize, failed: usize, settled: usize) -> usize {
    done.saturating_sub(skipped)
        .saturating_sub(failed)
        .saturating_sub(settled)
}

#[cfg_attr(not(test), allow(dead_code))]
fn job_toast_is_fail(failed: usize) -> bool {
    failed > 0
}

#[cfg_attr(not(test), allow(dead_code))]
fn family_toast_exempt_already_live(session_active: bool, all_faces_size_matched_mapped: bool) -> bool {
    session_active && all_faces_size_matched_mapped
}

/// Known GDI-session-incapable + intact on-disk official TTF ⇒ suppress `failed_names` /
/// DownloadBar "Couldn't load". Keep files for OT/preview; do **not** claim Activated.
#[cfg_attr(not(test), allow(dead_code))]
fn family_toast_exempt_known_gdi_incapable(known_incapable: bool, intact_on_disk: bool) -> bool {
    known_incapable && intact_on_disk
}

/// Disk settled for Scan/Repair: known GDI-incapable + intact full-size official TTF.
/// Stamp `.complete` so Incomplete/Repair-1 does not churn; never means GDI-live.
#[cfg_attr(not(test), allow(dead_code))]
fn family_disk_settled_known_gdi_incapable(
    known_incapable: bool,
    intact_on_disk: bool,
    undersized: bool,
) -> bool {
    known_incapable && intact_on_disk && !undersized
}

/// Activated / markLiveActivated only after real GDI Add — never on maps/sidecar.
/// Gidugu v2 may need sanitize/fallback; if Add actually returned >0 it is live.
#[cfg_attr(not(test), allow(dead_code))]
fn family_may_claim_session_activated(_known_incapable: bool, gdi_faces_added: usize) -> bool {
    gdi_faces_added > 0
}

fn suppress_fail_toast_known_incapable(app: &AppHandle, family: &str) -> bool {
    family_toast_exempt_known_gdi_incapable(
        family_may_settle_add_zero(family),
        family_has_intact(app, family),
    )
}

/// Quiet settle: clear JS pending without failed toast or Activated mark.
fn note_settled_quiet(family: &str) {
    if let Ok(mut p) = bulk().progress.lock()
        && !p.settled_names.iter().any(|n| n.eq_ignore_ascii_case(family)) {
            p.settled_names.push(family.to_string());
        }
}

fn register_intact_family_detailed(
    app: &AppHandle,
    family: &str,
) -> (usize, RegisterFailKind) {
    // Skip walk+Add only when this process already Add'd every current face.
    // Map-only / family_skip_live_success must never early-return here.
    if let Some(live) = family_skip_register_this_process(app, family) {
        return (live, RegisterFailKind::NoneTried);
    }
    // Known GDI-incapable already refused this session or disk-settled: no sanitize/Add churn.
    if family_early_skip_known_incapable(app, family) {
        return (0, RegisterFailKind::AddReturnedZero);
    }
    // Soft emoji: after this-session Add=0 refuse, skip re-Add (not bare .complete).
    if family_early_skip_soft_session_refused(app, family) {
        return (0, RegisterFailKind::AddReturnedZero);
    }
    // 1.0.206u VF-primary (Skye amend): when intact VF present (dual-VF italic
    // respected), Add only *-variable-* first. Purge statics **only after**
    // VarsOnly Add>0 — never before register (fallback needs statics on disk).
    let vf_primary = family_any_dir_vf_primary(app, family);
    let (n, fail) = if vf_primary {
        let (n, fail) = register_intact_family_pass(app, family, RegisterFaceMode::VarsOnly);
        if n > 0 {
            let (_del, lock) = commit_vf_primary_after_successful_add(app, family);
            if lock > 0
                && let Ok(mut p) = bulk().progress.lock() {
                    p.failed = p.failed.saturating_add(lock as u32);
                    let detail = format!(
                        "{family}: {lock} static(s) locked — deactivate fonts or quit Adobe/Word, then Repair"
                    );
                    if !p.failed_details.iter().any(|d| d == &detail) {
                        p.failed_details.push(detail);
                    }
                }
            (n, fail)
        } else {
            // Fallback: VF Add returned 0 — keep statics & compat sidecars, register all intact faces.
            register_intact_family_pass(app, family, RegisterFaceMode::AllIntactFallback)
        }
    } else {
        let (n, fail) = register_intact_family_pass(app, family, RegisterFaceMode::RespectPlan);
        if n == 0 && family_soft_try_add_then_settle(family) {
            // Soft emoji fallback: try compat-outline sidecars / all intact faces before settling.
            let (fb_n, fb_fail) =
                register_intact_family_pass(app, family, RegisterFaceMode::AllIntactFallback);
            if fb_n > 0 {
                (fb_n, fb_fail)
            } else {
                (0, fail.worsen(fb_fail))
            }
        } else {
            (n, fail)
        }
    };
    if n > 0 && family_soft_try_add_then_settle(family) {
        clear_session_gdi_refused(family);
        for dir in family_locations(app, family) {
            clear_settled_add_zero_provenance(&dir);
        }
    } else if n == 0
        && matches!(fail, RegisterFailKind::AddReturnedZero)
        && family_may_settle_add_zero(family)
    {
        note_session_gdi_refused(family);
    }
    (n, fail)
}

#[derive(Clone, Copy)]
enum RegisterFaceMode {
    /// Honor `.google-planned` / Fontsource plan via `face_allowed_for_register`.
    RespectPlan,
    /// Only `*-variable-*` faces (VF-primary Activate).
    VarsOnly,
    /// After VF Add=0: every intact non-latin face (statics allowed even if plan is vars-only).
    AllIntactFallback,
}

fn register_intact_family_pass(
    app: &AppHandle,
    family: &str,
    mode: RegisterFaceMode,
) -> (usize, RegisterFailKind) {
    let mut n = 0usize;
    let mut fail = RegisterFailKind::NoneTried;
    let fast_resume = fast_session_resume();
    for dir in family_locations(app, family) {
        let fast_dir = fast_resume
            && dir_is_complete(&dir)
            && !gfsource::dir_is_web_build(&dir)
            && !family_known_gdi_session_incapable(family);
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        sort_faces_var_first(&mut files);
        for path in files {
            let intact_ok = if fast_dir {
                path.metadata().map(|m| m.len() >= 12).unwrap_or(false)
            } else {
                ttf_intact(&path)
            };
            if !intact_ok {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let is_var = is_variable_face_filename(name);
            match mode {
                RegisterFaceMode::VarsOnly => {
                    if !is_var {
                        continue;
                    }
                }
                RegisterFaceMode::RespectPlan => {
                    if !face_allowed_for_register(&dir, &path, family) {
                        continue;
                    }
                }
                RegisterFaceMode::AllIntactFallback => {
                    // Skip Fontsource latin leftovers; allow statics even if plan is vars-only.
                    let slug = slug_family(family);
                    if !slug.is_empty() && filename_has_latin_subset(name, &slug) {
                        continue;
                    }
                    // Allow compat-outline sidecars for soft emoji families (Noto Emoji / Noto Color Emoji)
                    // when their VF returned Add=0 so Windows GDI can load the compatible outline face live.
                    if is_compat_sidecar_filename(name) && !family_soft_try_add_then_settle(family) {
                        continue;
                    }
                }
            }
            // Fast resume: when a size-matched pre-staged gdi-maps copy already exists,
            // it was already sanitized when staged — skip redundant file-open header scan.
            let skip_sanitize = fast_dir
                && gdi_map_dest_for(&path)
                    .and_then(|d| {
                        let src_len = fs::metadata(&path).ok()?.len();
                        let dst_len = fs::metadata(d).ok()?.len();
                        Some(src_len >= 256 && src_len == dst_len)
                    })
                    .unwrap_or(false);
            if !skip_sanitize {
                let _ = sanitize_on_disk_for_gdi(&path);
            }
            match register_path_detailed_fast(&path, skip_sanitize) {
                None => {
                    #[cfg(windows)]
                    winfont::bind(family, &path);
                    n += 1;
                }
                Some(kind) => {
                    fail = fail.worsen(kind);
                }
            }
        }
    }
    (n, fail)
}

fn clear_complete_markers_for_family(app: &AppHandle, family: &str) {
    for dir in family_locations(app, family) {
        clear_complete_marker(&dir);
    }
}

/// Gidugu-class: intact full-size official TTF ⇒ stamp `.complete` (disk settled).
/// Undersized remnants stay Incomplete so Repair can replace that face only.
/// Never claims Activated / GDI-live.
fn stamp_known_incapable_disk_settled(app: &AppHandle, family: &str) {
    if !family_known_gdi_session_incapable(family) {
        return;
    }
    for dir in family_locations(app, family) {
        stamp_known_incapable_dir_settled(&dir, family);
    }
}

/// After a real Add attempt returned 0: Settled honesty for hard allowlist OR soft emoji.
/// Boot seed / early-skip stay Gidugu-hard only. Soft stamps `.complete` **and**
/// `.settled-add-zero` provenance (Scan must not trust bare `.complete` from hard-emoji tips).
fn stamp_settle_after_add_zero(app: &AppHandle, family: &str) {
    // Session refuse bit so Activate All / soft early-skip do not re-Add this process.
    note_session_gdi_refused(family);
    if family_known_gdi_session_incapable(family) {
        stamp_known_incapable_disk_settled(app, family);
        return;
    }
    if !family_soft_try_add_then_settle(family) {
        return;
    }
    for dir in family_locations(app, family) {
        stamp_soft_settle_dir_after_add_zero(&dir, family);
    }
}

fn soft_emoji_full_face_ok(dir: &Path, family: &str) -> bool {
    if !family_soft_try_add_then_settle(family) {
        return true;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    // Soft emoji (color + outline): never settle a latin/CSS stub as "works".
    files.iter().any(|p| {
        ttf_intact(p) && fs::metadata(p).map(|m| m.len() >= 256 * 1024).unwrap_or(false)
    })
}

fn stamp_soft_settle_dir_after_add_zero(dir: &Path, family: &str) {
    if !family_soft_try_add_then_settle(family) {
        return;
    }
    if !dir_has_intact(dir) {
        return;
    }
    if !soft_emoji_full_face_ok(dir, family) {
        return;
    }
    if read_google_planned_keys(dir).is_none() {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        let keys: Vec<String> = files
            .iter()
            .filter(|p| ttf_intact(p))
            .filter_map(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        if !keys.is_empty() {
            write_google_planned(dir, &keys);
        }
    }
    let expected = if let Some(keys) = read_google_planned_keys(dir) {
        let intact = count_intact_planned_keys(dir, &keys);
        if intact == 0 || intact < keys.len() {
            return;
        }
        keys.len()
    } else {
        count_intact_faces(dir).max(1)
    };
    mark_family_complete(dir, expected);
    // Provenance: Soft Settled only after real Add=0 (never bare .complete from hard-emoji tips).
    stamp_settled_add_zero_provenance(dir);
}

fn stamp_known_incapable_dir_settled(dir: &Path, family: &str) {
    if !family_known_gdi_session_incapable(family) {
        return;
    }
    // 1.0.188: drop FS/latin orphans before settle stamp / undersized gate.
    let _ = purge_known_incapable_fontsource_remnants(dir, family);
    if !dir_has_intact(dir) {
        return;
    }
    if dir_has_undersized_google_static(dir, family) {
        return;
    }
    // Ensure `.google-planned` so verify trusts the stamp (official Google lie check).
    if read_google_planned_keys(dir).is_none() {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        let keys: Vec<String> = files
            .iter()
            .filter(|p| ttf_intact(p))
            .filter_map(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        if !keys.is_empty() {
            write_google_planned(dir, &keys);
        }
    }
    let expected = if let Some(keys) = read_google_planned_keys(dir) {
        let intact = count_intact_planned_keys(dir, &keys);
        if intact == 0 || intact < keys.len() {
            return;
        }
        keys.len()
    } else {
        count_intact_faces(dir).max(1)
    };
    mark_family_complete(dir, expected);
}


/// 1.0.206c: on boot, seed hard-allowlist GDI-incapable (Gidugu-class) already on
/// disk into Settled (`.complete` + session refused) so Activate All / restore
/// never queues them. Soft emoji are not seeded — they must queue and try Add.
fn seed_known_gdi_incapable_settled(app: &AppHandle) {
    for entry in KNOWN_GDI_SESSION_INCAPABLE {
        let family = entry.family;
        if !family_has_intact(app, family) {
            continue;
        }
        let undersized = family_locations(app, family)
            .iter()
            .any(|d| dir_has_undersized_google_static(d, family));
        if undersized {
            continue;
        }
        stamp_known_incapable_disk_settled(app, family);
        if family_has_complete_settled(app, family) {
            note_session_gdi_refused(family);
            note_settled_quiet(family);
        }
    }
}

struct HkcuBatchGuard(#[allow(dead_code)] std::mem::DropGuard<(), fn(())>);

fn finish_hkcu_batch_drop(_: ()) {
    #[cfg(windows)]
    winfont::finish_hkcu_batch();
}

impl HkcuBatchGuard {
    fn new() -> Self {
        #[cfg(windows)]
        winfont::begin_hkcu_batch();
        Self(std::mem::DropGuard::new((), finish_hkcu_batch_drop))
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn unregister_family_session(family: &str) -> u32 {
    #[cfg(windows)]
    {
        winfont::unregister_family(family)
    }
    #[cfg(not(windows))]
    {
        let _ = family;
        0
    }
}

fn unregister_path(path: &Path) {
    #[cfg(windows)]
    winfont::unregister(path);
}

fn gdi_flush_local() {
    #[cfg(windows)]
    winfont::flush_local();
}

fn is_lock_err(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5) | Some(32) | Some(33))
        || err.kind() == ErrorKind::PermissionDenied
}

/// Release GDI, then delete. Windows holds a session lock on AddFontResourceExW
/// files; DeleteFile fails until matching RemoveFontResourceExW + GdiFlush.
fn delete_font_file(path: &Path) -> Result<(), String> {
    guard_test_path(path);
    if !path.exists() {
        intact_forget(path);
        return Ok(());
    }
    unregister_path(path);
    crate::font_protocol::evict_path(path);
    crate::renderer::evict_path(path);
    intact_forget(path);
    gdi_flush_local();
    for attempt in 0..5u32 {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) if is_lock_err(&err) => {
                unregister_path(path);
                gdi_flush_local();
                thread::sleep(Duration::from_millis(40 * u64::from(attempt + 1)));
            }
            Err(err) => return Err(format!("could not delete {}: {err}", path.display())),
        }
    }
    // When running elevated (requireAdministrator), release stubborn Windows Font Cache
    // locks automatically via a bounded FontCache service restart and retry unlink.
    #[cfg(all(windows, not(test)))]
    if path.exists() && crate::is_elevated() {
        let _ = winfont::restart_font_cache_service(Duration::from_secs(4));
        unregister_path(path);
        gdi_flush_local();
        for attempt in 0..3u32 {
            match fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
                Err(err) if is_lock_err(&err) => {
                    thread::sleep(Duration::from_millis(80 * u64::from(attempt + 1)));
                }
                Err(err) => return Err(format!("could not delete {}: {err}", path.display())),
            }
        }
    }
    if path.exists() {
        return Err("files locked — close Word or Adobe, then Retry".into());
    }
    Ok(())
}

#[cfg(all(windows, not(test)))]
pub(crate) fn restart_font_cache_for_unlock() {
    let _ = winfont::restart_font_cache_service(Duration::from_secs(4));
}

#[cfg(not(all(windows, not(test))))]
#[allow(dead_code)]
pub(crate) fn restart_font_cache_for_unlock() {}

/// User Delete: honors global Delete Mode (Recycle Bin vs. Permanent Delete).
/// Temp `.part` / GDI maps always use `delete_font_file` (Permanent Delete).
fn recycle_user_font_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    crate::delete_ops::delete_user_dir(dir, crate::delete_ops::is_permanent_delete())
}

#[cfg(windows)]
fn recycle_bin_windows(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    struct ShFileOp {
        hwnd: isize,
        w_func: u32,
        p_from: *const u16,
        p_to: *const u16,
        f_flags: u16,
        f_any_operations_aborted: i32,
        p_name_mappings: *mut core::ffi::c_void,
        lpsz_progress_title: *const u16,
    }

    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_NOERRORUI: u16 = 0x0400;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHFileOperationW(lp_file_op: *mut ShFileOp) -> i32;
    }

    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    if from.last() == Some(&0) {
        from.pop();
    }
    from.push(0);
    from.push(0);
    let mut op = ShFileOp {
        hwnd: 0,
        w_func: FO_DELETE,
        p_from: from.as_ptr(),
        p_to: std::ptr::null(),
        f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
        f_any_operations_aborted: 0,
        p_name_mappings: std::ptr::null_mut(),
        lpsz_progress_title: std::ptr::null(),
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc == 0 && op.f_any_operations_aborted == 0 {
        return Ok(());
    }
    if path.exists() {
        if rc == 32 || rc == 5 || rc == 0x78 {
            return Err("files locked — close Word or Adobe, then Retry".into());
        }
        return Err(format!(
            "could not move to Recycle Bin (code {rc}). Close Word or Adobe, then Retry."
        ));
    }
    Ok(())
}

fn notify_fonts_changed() {
    #[cfg(windows)]
    winfont::notify();
}

fn notify_fonts_changed_maybe() {
    #[cfg(windows)]
    {
        winfont::notify_maybe(Duration::from_millis(1500));
    }
}

fn session_active_file_in(root: &Path) -> PathBuf {
    root.join(".session-active.json")
}

fn session_paths_file_in(root: &Path) -> PathBuf {
    root.join(".session-paths.txt")
}

fn parse_session_paths_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn load_session_paths_in(root: &Path) -> Vec<PathBuf> {
    let path = session_paths_file_in(root);
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_session_paths_text(&text)
}

fn save_session_paths_in(root: &Path, paths: &[PathBuf]) {
    let file = session_paths_file_in(root);
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let mut body = String::new();
    for p in paths {
        body.push_str(&p.to_string_lossy());
        body.push('\n');
    }
    let _ = fs::write(file, body);
}

fn clear_session_paths_in(root: &Path) {
    let _ = fs::remove_file(session_paths_file_in(root));
}

fn clear_session_active_in(root: &Path) {
    let _ = fs::remove_file(session_active_file_in(root));
}

/// Drop quit/crash sidecars under Documents/Font Manager.
fn clear_session_sidecars_in(root: &Path) {
    clear_session_paths_in(root);
    clear_session_active_in(root);
    crate::session_stage::clear_session_maps_in(root);
}

#[allow(dead_code)] // Windows session_end / clear_session_maps
fn clear_session_maps_in_root(root: &Path) {
    crate::session_stage::clear_session_maps_in(root);
}

#[cfg(windows)]
fn clear_session_maps(app: &AppHandle) {
    if let Ok(root) = documents_root(app) {
        clear_session_maps_in_root(&root);
    }
}

/// Persist stage-only session-paths + source↔stage maps (never Documents paths).
#[cfg(windows)]
fn persist_activation_sidecars(app: &AppHandle) {
    let Ok(root) = documents_root(app) else {
        return;
    };
    let maps = winfont::snapshot_face_maps();
    let stage_paths = winfont::snapshot_stage_paths();
    let stage_paths =
        crate::session_stage::filter_session_paths_refuse_documents(&stage_paths);
    if maps.is_empty() && stage_paths.is_empty() {
        return;
    }
    crate::session_stage::save_session_maps_in(&root, &maps);
    save_session_paths_in(&root, &stage_paths);
}

/// Rebuild/validate `.session-maps.json` against existing stage files; re-stage
/// missing (copy-only from Documents). Returns stage paths safe for session-paths.
#[cfg_attr(not(windows), allow(dead_code))]
fn rebuild_session_maps_in(root: &Path, maps_root: &Path) -> Vec<std::path::PathBuf> {
    let existing = crate::session_stage::load_session_maps_in(root);
    let need = crate::session_stage::maps_needing_restage(&existing);
    let mut restaged: Vec<(String, String)> = Vec::new();
    for m in &need {
        let src = Path::new(&m.source);
        if !src.is_file() {
            continue;
        }
        if let Some(dest) = ensure_gdi_session_copy_to(src, maps_root) {
            if crate::session_stage::must_not_register_as_gdi_path(&dest) {
                continue;
            }
            restaged.push((
                src.to_string_lossy().into_owned(),
                dest.to_string_lossy().into_owned(),
            ));
        }
    }
    // Also adopt in-memory maps from a live session.
    #[cfg(windows)]
    {
        for m in winfont::snapshot_face_maps() {
            restaged.push((m.source, m.stage));
        }
    }
    let validated = crate::session_stage::validate_session_maps(&existing, &restaged);
    crate::session_stage::save_session_maps_in(root, &validated);
    let stages = crate::session_stage::stage_paths_from_maps(&validated);
    save_session_paths_in(root, &stages);
    #[cfg(windows)]
    {
        for m in &validated {
            winfont::remember_face_map(m.clone());
        }
    }
    stages
}

fn merge_unique_paths(primary: Vec<PathBuf>, extra: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in primary.into_iter().chain(extra) {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

/// Quit watchdog budget: scales so healthy Remove can finish before exit.
/// ~15ms/path (RemoveFontResourceExW + local GdiFlush), clamped to [12s, 180s].
/// ~2k Activate All ≈ 31s; ~11k library ≈ 165s (under cap). Watchdog only —
/// worker exits when session_end completes; leftovers recover on next boot.
/// Do not restart FontCache on quit (Explorer hang risk) — prefer full Remove.
pub fn quit_unload_budget_for(path_count: usize) -> Duration {
    let ms = (path_count as u64).saturating_mul(15).clamp(12_000, 180_000);
    Duration::from_millis(ms)
}

/// SCM service names to best-effort restart after Deactivate/Quit unload.
/// `FontCache` = Windows Font Cache Service (svchost / LOCAL SERVICE).
/// `FontCache3.0.0.0` = WPF Font Cache when present.
pub fn font_cache_service_names() -> &'static [&'static str] {
    &["FontCache", "FontCache3.0.0.0"]
}

/// Hard cap for FontCache STOP+START so Quit never deadlocks Explorer.
pub fn font_cache_restart_budget() -> Duration {
    Duration::from_secs(8)
}

/// When to schedule a FontCache flush after Removes.
/// Quit and live Deactivate both flush when we attempted any unload; quit path
/// must stay inside `font_cache_restart_budget()` (no HWND_BROADCAST).
pub fn plan_font_cache_flush(attempted_removes: usize) -> bool {
    attempted_removes > 0
}

/// Toast / eprintln copy when Documents TTFs stay locked after flush attempt.
pub fn font_cache_held_message(locked: usize) -> String {
    format!(
        "Font Cache still holding {} files — retry as admin or reboot",
        locked
    )
}

/// Windows default `GDIProcessHandleQuota` (per process). Session theoretical max 65_535.
/// `AddFontResourceEx` puts files in the *font table* — it is not one GDI object per face.
/// `GetGuiResources(GR_GDIOBJECTS)` counts HFONT/HDC/bitmaps in *this* process (WebView2).
pub const GDI_OBJECT_DEFAULT_QUOTA: u32 = 10_000;
/// Soft warn only — never skip Add / never raise the quota.
pub const GDI_OBJECT_SOFT_WARN: u32 = 8_000;
/// Hard safety stop well below Windows's 10,000 per-process GDI handle wall so
/// massive font batches pause cleanly instead of crashing WebView2 / Tauri.
pub const GDI_OBJECT_HARD_STOP: u32 = 9_200;

pub fn gdi_objects_near_quota(count: u32) -> bool {
    count >= GDI_OBJECT_SOFT_WARN
}

pub fn gdi_objects_at_hard_ceiling(count: u32) -> bool {
    count >= GDI_OBJECT_HARD_STOP
}

/// Fast 4-byte SFNT/TTC magic header validation (microseconds) so corrupt,
/// truncated, or HTML-error files never reach Windows GDI (`AddFontResourceExW`).
pub fn has_valid_sfnt_magic(path: &Path) -> bool {
    let mut opened = None;
    for attempt in 0..3u8 {
        match fs::File::open(path) {
            Ok(f) => {
                opened = Some(f);
                break;
            }
            Err(e) if attempt < 2 && is_transient_lock_error(&e) => {
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => return false,
        }
    }
    let Some(mut f) = opened else {
        return false;
    };
    let Ok(meta) = f.metadata() else {
        return false;
    };
    if meta.len() < 12 {
        return false;
    }
    let mut magic = [0u8; 4];
    if std::io::Read::read_exact(&mut f, &mut magic).is_err() {
        return false;
    }
    matches!(
        &magic,
        [0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" | b"ttcf"
    )
}

/// Non-destructive session family reconciliation after boot restore.
/// Never wipes the user's saved active families when restore was cancelled,
/// paused by GDI headroom, or encountered a transient lock — only drops
/// families whose files were genuinely removed from disk.
pub fn merge_session_families_after_restore<F>(
    saved: &[String],
    ready: &[String],
    cancelled: bool,
    gdi_capped: bool,
    still_exists: F,
) -> Vec<String>
where
    F: Fn(&str) -> bool,
{
    if cancelled || gdi_capped {
        return saved.to_vec();
    }
    let mut out: Vec<String> = Vec::with_capacity(saved.len().max(ready.len()));
    let mut seen: HashSet<String> = HashSet::new();
    let ready_lower: HashSet<String> = ready.iter().map(|s| s.trim().to_lowercase()).collect();
    for fam in saved {
        let trimmed = fam.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if (ready_lower.contains(&key) || still_exists(trimmed)) && seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    for fam in ready {
        let trimmed = fam.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

pub fn gdi_pressure_message(count: u32) -> String {
    format!(
        "This window is using {count} GDI objects (Windows quota {GDI_OBJECT_DEFAULT_QUOTA}). Session fonts are the font table, not one object per face — Chromium handles count. Deactivate some families if the UI hitchs. Live marks are unchanged."
    )
}

/// Bounded workers for session_begin register_intact_family parallelism.
pub fn session_register_workers(family_count: usize) -> usize {
    const MAX: usize = 6;
    const MIN: usize = 1;
    family_count.clamp(MIN, MAX)
}

/// Gate for on-disk parallel register workers (Skye P1).
/// Cancel aborts the queue; pause waits like download drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnDiskRegisterGate {
    Run,
    WaitPaused,
    StopCancelled,
}

fn on_disk_register_gate(cancel: bool, pause: bool) -> OnDiskRegisterGate {
    if cancel {
        OnDiskRegisterGate::StopCancelled
    } else if pause {
        OnDiskRegisterGate::WaitPaused
    } else {
        OnDiskRegisterGate::Run
    }
}

/// Decision after best-effort unload: clear sidecars on success; on partial
/// failure keep remaining locked paths for next-boot recovery and fail loud.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionEndCleanup {
    clear_active: bool,
    clear_paths: bool,
    /// Successful unload must also drop `.session-maps.json`.
    clear_maps: bool,
    keep_paths: Vec<PathBuf>,
    fail_loud: Option<String>,
}

fn plan_session_end_cleanup(
    attempted: usize,
    still_locked: &[PathBuf],
) -> SessionEndCleanup {
    if still_locked.is_empty() {
        SessionEndCleanup {
            clear_active: true,
            clear_paths: true,
            clear_maps: true,
            keep_paths: Vec::new(),
            fail_loud: None,
        }
    } else {
        SessionEndCleanup {
            clear_active: true,
            clear_paths: false,
            // Keep maps so next-boot recovery can re-stage / Remove leftovers.
            clear_maps: false,
            keep_paths: still_locked.to_vec(),
            fail_loud: Some(format!(
                "Fonths: session unload incomplete — {} of {} paths still write-locked (Font Cache/svchost, fontdrvhost, or Adobe?). {} Next launch will retry RemoveFontResourceExW; or Deactivate-all as admin / reboot then Repair.",
                still_locked.len(),
                attempted.max(still_locked.len()),
                font_cache_held_message(still_locked.len())
            )),
        }
    }
}

/// Startup recover clear decision after best-effort Remove.
/// Missing stage files are not write-locked (`path_still_write_locked` → false),
/// so a naïve "still_locked.is_empty() ⇒ clear all sidecars" wipes
/// `.session-maps.json` + `.session-active.json` before rebuild can re-stage.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoverSidecarPlan {
    clear_paths: bool,
    clear_maps: bool,
    clear_active: bool,
    keep_locked_paths: Vec<PathBuf>,
}

/// `missing_before_unload`: any unload target was absent on disk before Remove.
/// When true, do not treat empty `still_locked` as successful unlock → nuke.
fn plan_recover_sidecar_clear(
    still_locked: &[PathBuf],
    missing_before_unload: bool,
) -> RecoverSidecarPlan {
    if !still_locked.is_empty() {
        return RecoverSidecarPlan {
            clear_paths: false,
            clear_maps: false,
            clear_active: false,
            keep_locked_paths: still_locked.to_vec(),
        };
    }
    if missing_before_unload {
        // Preserve maps + session-active for validate/rebuild + re-register.
        return RecoverSidecarPlan {
            clear_paths: false,
            clear_maps: false,
            clear_active: false,
            keep_locked_paths: Vec::new(),
        };
    }
    // All targets existed and are unlocked — drop the unload ledger only.
    // Keep maps + session-active so session_begin can re-register.
    RecoverSidecarPlan {
        clear_paths: true,
        clear_maps: false,
        clear_active: false,
        keep_locked_paths: Vec::new(),
    }
}

fn apply_recover_sidecar_plan(root: &Path, plan: &RecoverSidecarPlan) {
    if plan.clear_paths {
        clear_session_paths_in(root);
    }
    if plan.clear_maps {
        crate::session_stage::clear_session_maps_in(root);
    }
    if plan.clear_active {
        clear_session_active_in(root);
    }
    if !plan.keep_locked_paths.is_empty() {
        let still_stage = crate::session_stage::filter_session_paths_refuse_documents(
            &plan.keep_locked_paths,
        );
        save_session_paths_in(root, &still_stage);
    }
}

fn session_path(app: &AppHandle) -> Option<PathBuf> {
    documents_root(app).ok().map(|p| session_active_file_in(&p))
}

#[cfg(windows)]
fn load_session_paths(app: &AppHandle) -> Vec<PathBuf> {
    let Ok(root) = documents_root(app) else {
        return Vec::new();
    };
    load_session_paths_in(&root)
}

#[cfg(windows)]
fn save_session_paths(app: &AppHandle, paths: &[PathBuf]) {
    let Ok(root) = documents_root(app) else {
        return;
    };
    save_session_paths_in(&root, paths);
}

#[cfg(windows)]
fn clear_session_paths(app: &AppHandle) {
    if let Ok(root) = documents_root(app) {
        clear_session_paths_in(&root);
    }
}

#[cfg(windows)]
fn clear_session_active(app: &AppHandle) {
    if let Ok(root) = documents_root(app) {
        clear_session_active_in(&root);
    }
}

fn clear_session_sidecars(app: &AppHandle) {
    if let Ok(root) = documents_root(app) {
        clear_session_sidecars_in(&root);
    }
}

/// Trim, drop blanks, and deduplicate family names case-insensitively while preserving order.
pub fn dedupe_family_names(names: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(names.len());
    let mut seen = HashSet::with_capacity(names.len());
    for raw in names {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_ascii_lowercase()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn load_session_families(app: &AppHandle) -> Vec<String> {
    let Some(path) = session_path(app) else {
        return Vec::new();
    };
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let raw = serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default();
    dedupe_family_names(&raw)
}

fn save_session_families(app: &AppHandle, families: &[String]) {
    let Some(path) = session_path(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let clean = dedupe_family_names(families);
    let body = serde_json::to_vec_pretty(&clean).unwrap_or_else(|_| b"[]".to_vec());
    let _ = fs::write(path, body);
}

/// True when DeleteFile/rewrite would hit sharing violation (GDI/Adobe lock).
/// Uses set_len(same) — does not truncate — so Heal-style rewrite locks surface
/// without mutating font bytes.
fn path_still_write_locked(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match fs::OpenOptions::new().write(true).read(true).open(path) {
        Ok(f) => {
            let Ok(meta) = f.metadata() else {
                return false;
            };
            match f.set_len(meta.len()) {
                Ok(()) => false,
                Err(err) => is_lock_err(&err),
            }
        }
        Err(err) => is_lock_err(&err),
    }
}

fn filter_still_write_locked(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| path_still_write_locked(p))
        .cloned()
        .collect()
}

fn write_font_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    guard_test_path(path);
    if !ttf_magic(bytes) || bytes.len() < 256 {
        return Err("not an installable font".into());
    }
    let sanitized = crate::namepatch::gdi_sanitize_ttf(bytes);
    let bytes = sanitized.as_deref().unwrap_or(bytes);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        delete_font_file(path)?;
    }
    fs::write(path, bytes).map_err(|e| {
        if is_lock_err(&e) {
            "files locked — close Word or Adobe, then Retry".into()
        } else {
            e.to_string()
        }
    })?;
    let _ = prestage_gdi_session_bytes(path, bytes);
    register_path(path);
    Ok(())
}

fn slug_family(family: &str) -> String {
    let s = family
        .to_lowercase()
        .replace(['\'', '’'], "")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    s.trim_matches('-').to_string()
}

/// Folder-lookup slug that is never empty. `slug_family` stays ASCII-only (and may be
/// empty) because Fontsource / Google lookups and heal passes use `slug.is_empty()` to
/// skip names with no ASCII slug. For folder resolution an empty slug would mean
/// `root.join("")` == the Documents root, so non-ASCII names get a stable
/// `u-<fnv1a64 hex>` slug of the trimmed, lowercased name instead.
fn family_folder_slug(family: &str) -> String {
    let slug = slug_family(family);
    if !slug.is_empty() {
        return slug;
    }
    unicode_family_slug(family)
}

/// Stable `u-<fnv1a64 hex>` of the trimmed, lowercased family name.
fn unicode_family_slug(family: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in family.trim().to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("u-{hash:016x}")
}

fn host_label(url: &str) -> &'static str {
    if url.contains("gstatic") || url.contains("googleapis") {
        "google"
    } else if url.contains("jsdelivr") {
        "jsdelivr"
    } else if url.contains("unpkg") {
        "unpkg"
    } else if url.contains("github") {
        "github"
    } else {
        "cdn"
    }
}

pub struct CdnGate {
    failures: u32,
    open_until: Option<Instant>,
}

impl CdnGate {
    fn allow(&mut self) -> bool {
        if let Some(until) = self.open_until {
            if Instant::now() < until {
                return false;
            }
            self.open_until = None;
            self.failures = 0;
        }
        true
    }
    fn success(&mut self) {
        self.failures = 0;
        self.open_until = None;
    }
    fn failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 5 {
            self.open_until = Some(Instant::now() + Duration::from_secs(20));
            self.failures = 0;
        }
    }
}

fn circuit_allow(host: &'static str) -> bool {
    let Ok(mut map) = bulk().circuits.lock() else {
        return true;
    };
    map.entry(host).or_insert(CdnGate { failures: 0, open_until: None }).allow()
}

fn circuit_success(host: &'static str) {
    if let Ok(mut map) = bulk().circuits.lock() {
        map.entry(host).or_insert(CdnGate { failures: 0, open_until: None }).success();
    }
}

fn circuit_failure(host: &'static str) {
    if let Ok(mut map) = bulk().circuits.lock() {
        map.entry(host).or_insert(CdnGate { failures: 0, open_until: None }).failure();
    }
}

fn reset_circuits() {
    if let Ok(mut map) = bulk().circuits.lock() {
        map.clear();
    }
    gfsource::reset_manifest_circuit();
}

/// Google CSS2 with Mozilla / Googlebot returns one TTF per face with the full cmap,
/// but it is a **web build**: unencoded glyphs and OpenType features are stripped
/// (Barlow 571 vs 694 glyphs, no smcp/c2sc/sups/ordn/aalt). c258: css2 is the last
/// resort after the download/list manifest and the google/fonts repo, and a css2
/// face never counts as complete or as a repair (`.web-build`).
/// Safari/Chrome get WOFF2 unicode-range subsets — Activate filters those out → empty install.
const UA_DESKTOP_TTF: &str = "Mozilla/5.0";
const UA_GOOGLEBOT: &str =
    "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
/// Emoji color-compat only (SVG-in-OTF / COLRv1). Never use for normal Activate CSS.
const UA_SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
const UA_CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// Fontsource **static** `@fontsource/{slug}` TTF URLs only.
/// Never `@fontsource-variable/*` (WOFF) — not an installable desktop source.
fn ttf_urls(slug: &str, version: &str, weight: u16, italic: bool, subset: &str, bust: u128) -> Vec<String> {
    let q = if bust == 0 {
        String::new()
    } else {
        format!("?v={bust}")
    };
    let style = if italic { "italic" } else { "normal" };
    let ver = version.trim().trim_start_matches('v');
    let pin = if ver.is_empty() { "latest" } else { ver };
    // Prefer @latest, then the **npm** tag (Open Sauce is 5.3.0). Foundry
    // versions like v1.477 400 on jsDelivr and used to starve the rest of the URL list.
    let mut urls = Vec::new();
    if pin != "latest" {
        urls.push(format!(
            "https://cdn.jsdelivr.net/fontsource/fonts/{slug}@latest/{subset}-{weight}-{style}.ttf{q}"
        ));
    }
    urls.push(format!(
        "https://cdn.jsdelivr.net/fontsource/fonts/{slug}@{pin}/{subset}-{weight}-{style}.ttf{q}"
    ));
    urls.push(format!(
        "https://cdn.jsdelivr.net/npm/@fontsource/{slug}/files/{slug}-{subset}-{weight}-{style}.ttf{q}"
    ));
    urls.push(format!(
        "https://unpkg.com/@fontsource/{slug}/files/{slug}-{subset}-{weight}-{style}.ttf{q}"
    ));
    if slug == "noto-color-emoji" {
        // c253 #7: upstream moved fonts/ → 2D/fonts/ (OFL-1.1); old paths 404.
        // Try BOTH WindowsCompatible mirrors (glyf/loca tables -> Windows GDI Add=1) BEFORE
        // the CBDT-only NotoColorEmoji.ttf fallback so transient jsDelivr 429s never settle it.
        return vec![
            format!("https://cdn.jsdelivr.net/gh/googlefonts/noto-emoji@main/2D/fonts/NotoColorEmoji_WindowsCompatible.ttf{q}"),
            "https://raw.githubusercontent.com/googlefonts/noto-emoji/main/2D/fonts/NotoColorEmoji_WindowsCompatible.ttf".into(),
            format!("https://cdn.jsdelivr.net/gh/googlefonts/noto-emoji@main/2D/fonts/NotoColorEmoji.ttf{q}"),
            "https://raw.githubusercontent.com/googlefonts/noto-emoji/main/2D/fonts/NotoColorEmoji.ttf".into(),
        ];
    }
    if slug == "noto-emoji" && !italic {
        // Official monochrome Noto Emoji VF from google/fonts (OFL-1.1, GDI Add=3) FIRST —
        // before Fontsource mirrors so dead/subset mirrors never starve or trip circuits.
        let mut out = vec![
            "https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf".into(),
            "https://raw.githubusercontent.com/google/fonts/main/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf".into(),
        ];
        out.extend(urls);
        urls = out;
    }
    // Clear Sans: Fontsource CDN is WOFF / odd ~67KB TTFs (OS/2 fsType=4) that
    // AddFontResourceExW refuses. Never treat those as a GDI install source —
    // exclusive Intel clear-sans GitHub TTFs (~305KB Regular, fsType=0).
    if slug == "clear-sans" {
        return clear_sans_intel_ttf_urls(weight, italic, &q);
    }
    // Belt-and-suspenders: never hand callers a variable-package WOFF URL.
    urls.retain(|u| !u.contains("fontsource-variable"));
    urls.extend(fontsource_upstream_ttf_urls(slug, weight, italic));
    urls
}

/// Open Sauce (and similar type:other) — jsDelivr `fontsource/fonts` often 200s a
/// non-TTF body for 4/14 faces (`\x80\x01…`). Official GitHub TTFs are intact.
fn open_sauce_style_token(weight: u16, italic: bool) -> Option<&'static str> {
    let base = match weight {
        300 => "Light",
        400 => "Regular",
        500 => "Medium",
        600 => "SemiBold",
        700 => "Bold",
        800 => "ExtraBold",
        900 => "Black",
        _ => return None,
    };
    Some(if italic {
        match base {
            "Regular" => "Italic",
            "Light" => "LightItalic",
            "Medium" => "MediumItalic",
            "SemiBold" => "SemiBoldItalic",
            "Bold" => "BoldItalic",
            "ExtraBold" => "ExtraBoldItalic",
            "Black" => "BlackItalic",
            _ => return None,
        }
    } else {
        base
    })
}

fn fontsource_upstream_ttf_urls(slug: &str, weight: u16, italic: bool) -> Vec<String> {
    let (repo, prefix) = match slug {
        "open-sauce-sans" => ("marcologous/Open-Sauce-Fonts", "OpenSauceSans"),
        "open-sauce-one" => ("marcologous/Open-Sauce-Fonts", "OpenSauceOne"),
        "open-sauce-two" => ("marcologous/Open-Sauce-Fonts", "OpenSauceTwo"),
        "clear-sans" => {
            return clear_sans_intel_ttf_urls(weight, italic, "");
        }
        _ => return Vec::new(),
    };
    let Some(style) = open_sauce_style_token(weight, italic) else {
        return Vec::new();
    };
    let file = format!("{prefix}-{style}.ttf");
    vec![
        format!("https://cdn.jsdelivr.net/gh/{repo}@master/fonts/ttf/{file}"),
        format!("https://raw.githubusercontent.com/{repo}/master/fonts/ttf/{file}"),
    ]
}

/// Intel Clear Sans face token. Catalog weights: 100/300/400/500/700 (+ italic).
/// Pinned commit matches Arch AUR / proven ~305KB Regular (Add=1 on Eric's PC).
fn clear_sans_style_token(weight: u16, italic: bool) -> Option<&'static str> {
    if italic {
        return match weight {
            400 => Some("Italic"),
            500 => Some("MediumItalic"),
            700 => Some("BoldItalic"),
            _ => None,
        };
    }
    match weight {
        100 => Some("Thin"),
        300 => Some("Light"),
        400 => Some("Regular"),
        500 => Some("Medium"),
        700 => Some("Bold"),
        _ => None,
    }
}

const CLEAR_SANS_INTEL_PIN: &str = "cc22e43fc739fba9782f5e0fcd665a4933d2ba45";

fn clear_sans_intel_ttf_urls(weight: u16, italic: bool, bust_q: &str) -> Vec<String> {
    let Some(style) = clear_sans_style_token(weight, italic) else {
        return Vec::new();
    };
    let file = format!("ClearSans-{style}.ttf");
    let pin = CLEAR_SANS_INTEL_PIN;
    vec![
        format!(
            "https://cdn.jsdelivr.net/gh/intel/clear-sans@{pin}/TTF/{file}{bust_q}"
        ),
        format!(
            "https://raw.githubusercontent.com/intel/clear-sans/{pin}/TTF/{file}{bust_q}"
        ),
        format!("https://github.com/intel/clear-sans/raw/{pin}/TTF/{file}{bust_q}"),
    ]
}

/// Intel clear-sans ships exactly these 8 TTFs at CLEAR_SANS_INTEL_PIN
/// (no ThinItalic / LightItalic). Fontsource meta is weights×italic = 10 —
/// never use that matrix for planned / `.expected`.
const CLEAR_SANS_INTEL_FACES: &[(u16, bool)] = &[
    (100, false), // Thin
    (300, false), // Light
    (400, false), // Regular
    (500, false), // Medium
    (700, false), // Bold
    (400, true),  // Italic
    (500, true),  // MediumItalic
    (700, true),  // BoldItalic
];

fn clear_sans_intel_planned_count() -> usize {
    CLEAR_SANS_INTEL_FACES.len()
}

fn clear_sans_intel_face_keys() -> Vec<String> {
    CLEAR_SANS_INTEL_FACES
        .iter()
        .map(|(weight, italic)| {
            let style = if *italic { "italic" } else { "normal" };
            google_face_filename("clear-sans", &weight.to_string(), style)
        })
        .collect()
}

/// Rewrite sticky Fontsource `.expected=10` to Intel plan 8. When all 8 Intel
/// keys are intact and not Fontsource shreds, stamp `.complete` so Repair does
/// not churn jsDelivr/unpkg for faces Intel never ships.
fn heal_clear_sans_expected_plan(dir: &Path, family: &str) {
    if !is_clear_sans_family(family) {
        return;
    }
    let plan = clear_sans_intel_planned_count();
    write_expected_faces(dir, plan);
    let keys = clear_sans_intel_face_keys();
    let intact = count_intact_planned_keys(dir, &keys);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let needs_heal = files.iter().any(|p| clear_sans_face_needs_intel_heal(p));
    if intact >= plan && !needs_heal {
        mark_family_complete(dir, plan);
    } else if intact < plan || needs_heal {
        clear_complete_marker(dir);
    }
}

fn is_clear_sans_family(family: &str) -> bool {
    family.trim().eq_ignore_ascii_case("clear sans")
        || slug_family(family) == "clear-sans"
}

fn clear_sans_library_needs_heal(app: &AppHandle, family: &str) -> bool {
    if !is_clear_sans_family(family) {
        return false;
    }
    for dir in family_locations(app, family) {
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        if files.is_empty() {
            return true;
        }
        if files.iter().any(|p| clear_sans_face_needs_intel_heal(p)) {
            return true;
        }
    }
    false
}

/// Fontsource clear-sans library shreds land ~67–81KB; Intel Regular is ~305KB.
fn clear_sans_face_needs_intel_heal(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let len = meta.len();
    // Odd subset / restricted-embedding Fontsource bodies.
    (24 * 1024..=96 * 1024).contains(&len) || os2_fstype_restricted(path)
}

/// Read OS/2 fsType when present. fsType bit 1 (0x2) / bit 2 (0x4) = restricted
/// embedding — Windows AddFontResourceExW often returns 0 (Fontsource clear-sans).
/// Gidugu official TTF has fsType=0 yet still Add=0 on some PCs (GDI-incapable).
fn os2_fstype(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 12 || !ttf_magic(bytes) {
        return None;
    }
    let ntables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let mut i = 0usize;
    while i < ntables {
        let off = 12 + i * 16;
        if off + 16 > bytes.len() {
            break;
        }
        if &bytes[off..off + 4] == b"OS/2" {
            let toff = u32::from_be_bytes(bytes[off + 8..off + 12].try_into().ok()?) as usize;
            if toff + 10 > bytes.len() {
                return None;
            }
            return Some(u16::from_be_bytes([bytes[toff + 8], bytes[toff + 9]]));
        }
        i += 1;
    }
    None
}

fn os2_fstype_restricted(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    match os2_fstype(&bytes) {
        // 0x2 = restricted license, 0x4 = preview & print — not installable via GDI Add.
        Some(fs) if fs & 0x0006 != 0 => true,
        _ => false,
    }
}

/// Hard allowlist of families known to refuse session GDI Add (Add=0) despite intact TTF.
/// Seed Settled / early-skip / disk `.complete` / toast-exempt / Fontsource offer — keyed here.
/// Soft emoji are NOT listed: try Add first; settle only after Add=0 (`family_may_settle_add_zero`).
/// Append a row (+ optional FS slug override + subset plan) for the next true Add=0 class.
#[derive(Clone, Copy)]
struct KnownGdiIncapableEntry {
    family: &'static str,
    /// Fontsource package slug; `None` ⇒ `slug_family(family)`.
    fs_slug: Option<&'static str>,
    /// Subsets for opt-in Fontsource GDI offer (order = preference).
    subsets: &'static [&'static str],
}

/// Hard allowlist only: true Add=0 class (Gidugu). Seed Settled + early-skip + Activate All skip.
/// Soft emoji (`SOFT_GDI_TRY_ADD_FIRST`) try Add first; Settled only after Add=0 — never hard-skip.
const KNOWN_GDI_SESSION_INCAPABLE: &[KnownGdiIncapableEntry] = &[
    KnownGdiIncapableEntry {
        family: "Gidugu",
        fs_slug: Some("gidugu"),
        subsets: &["telugu", "latin"],
    },
];

fn known_gdi_incapable_entry(family: &str) -> Option<&'static KnownGdiIncapableEntry> {
    let key = family.trim();
    if key.is_empty() {
        return None;
    }
    KNOWN_GDI_SESSION_INCAPABLE
        .iter()
        .find(|e| e.family.eq_ignore_ascii_case(key))
}

fn family_known_gdi_session_incapable(family: &str) -> bool {
    known_gdi_incapable_entry(family).is_some()
}

/// Soft emoji allowlist — try Add first; Settled only after Add=0.
/// Mirror of TS `SOFT_GDI_TRY_ADD_FIRST` (same shape as hard table). Tip tests assert parity.
const SOFT_GDI_TRY_ADD_FIRST: &[KnownGdiIncapableEntry] = &[
    KnownGdiIncapableEntry {
        family: "Noto Color Emoji",
        fs_slug: Some("noto-color-emoji"),
        subsets: &["emoji"],
    },
    KnownGdiIncapableEntry {
        family: "Noto Emoji",
        fs_slug: Some("noto-emoji"),
        subsets: &["emoji"],
    },
];

fn soft_gdi_try_add_entry(family: &str) -> Option<&'static KnownGdiIncapableEntry> {
    let key = family.trim();
    if key.is_empty() {
        return None;
    }
    SOFT_GDI_TRY_ADD_FIRST
        .iter()
        .find(|e| e.family.eq_ignore_ascii_case(key))
}

/// Soft (emoji): try Add first; Settled honesty only after Add=0 — never boot-seed / early-skip.
fn family_soft_try_add_then_settle(family: &str) -> bool {
    soft_gdi_try_add_entry(family).is_some()
}

/// Hard allowlist OR soft emoji after an Add attempt — Settled / toast-exempt, never fake Live.
fn family_may_settle_add_zero(family: &str) -> bool {
    family_known_gdi_session_incapable(family) || family_soft_try_add_then_settle(family)
}

#[cfg_attr(not(test), allow(dead_code))]
fn fontsource_gdi_offer_slug(family: &str) -> String {
    match known_gdi_incapable_entry(family) {
        Some(e) => e
            .fs_slug
            .map(|s| s.to_string())
            .unwrap_or_else(|| slug_family(family)),
        None => slug_family(family),
    }
}

const DEFAULT_FONTSOURCE_GDI_SUBSETS: &[&str] = &["latin"];

fn fontsource_gdi_offer_subsets(family: &str) -> &'static [&'static str] {
    known_gdi_incapable_entry(family)
        .map(|e| e.subsets)
        .unwrap_or(DEFAULT_FONTSOURCE_GDI_SUBSETS)
}

/// This-process Add=0 for a known-incapable family — Activate All must not re-stage/Add.
fn session_gdi_refused() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

fn note_session_gdi_refused(family: &str) {
    // Hard Gidugu OR soft emoji after Add=0 — Activate All / early-skip via session refuse.
    if !family_may_settle_add_zero(family) {
        return;
    }
    if let Ok(mut g) = session_gdi_refused().lock() {
        g.insert(family.trim().to_ascii_lowercase());
    }
}

fn family_session_gdi_refused(family: &str) -> bool {
    session_gdi_refused()
        .lock()
        .map(|g| g.contains(&family.trim().to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Soft Power Retry: drop this-session Add=0 refuse so Add runs again.
/// Activate All leaves refuse set (soft early-skip still holds). Hard disk-settle skip unchanged.
fn clear_session_gdi_refused(family: &str) {
    if let Ok(mut g) = session_gdi_refused().lock() {
        g.remove(&family.trim().to_ascii_lowercase());
    }
}

/// Soft early-skip gate (no AppHandle): soft + this-session refuse + intact full-face.
#[cfg_attr(not(test), allow(dead_code))]
fn soft_early_skip_from_session_refuse(soft: bool, refused: bool, intact_full: bool) -> bool {
    soft && refused && intact_full
}

/// Soft Retry clears refuse ⇒ Add may run (early-skip soft no longer blocks).
#[cfg_attr(not(test), allow(dead_code))]
fn soft_retry_allows_add_after_clear(refused_before: bool, cleared_by_retry: bool) -> bool {
    !(refused_before && !cleared_by_retry)
}

fn family_has_complete_settled(app: &AppHandle, family: &str) -> bool {
    family_locations(app, family).iter().any(|dir| dir_is_complete(dir) && dir_has_intact(dir))
}

/// Early-skip Activate walk/Add: known-incapable + (this-session refused OR already `.complete` settled)
/// + intact full-size. Never claims Activated.
///
/// 1.0.188: purge FS/latin remnants first so undersized offer dest cannot block skip.
fn family_early_skip_known_incapable(app: &AppHandle, family: &str) -> bool {
    if !family_known_gdi_session_incapable(family) {
        return false;
    }
    for dir in family_locations(app, family) {
        let _ = purge_known_incapable_fontsource_remnants(&dir, family);
    }
    let intact = family_has_intact(app, family);
    if !intact {
        return false;
    }
    let undersized = family_locations(app, family)
        .iter()
        .any(|d| dir_has_undersized_google_static(d, family));
    if undersized {
        return false;
    }
    // 1.0.206c: stamp Settled before first settle scan so Activate All never queues
    // hard-allowlist (Gidugu-class) faces already intact on disk — not soft emoji.
    stamp_known_incapable_disk_settled(app, family);
    if family_has_complete_settled(app, family) {
        note_session_gdi_refused(family);
        return true;
    }
    family_session_gdi_refused(family)
}

/// Soft emoji: skip re-Add only after this-session refuse (never trust bare `.complete`).
fn family_early_skip_soft_session_refused(app: &AppHandle, family: &str) -> bool {
    if !family_soft_try_add_then_settle(family) {
        return false;
    }
    if !family_session_gdi_refused(family) {
        return false;
    }
    let intact = family_has_intact(app, family);
    if !intact {
        return false;
    }
    let undersized = family_locations(app, family)
        .iter()
        .any(|d| dir_has_undersized_google_static(d, family));
    if undersized {
        return false;
    }
    family_locations(app, family)
        .iter()
        .any(|d| soft_emoji_full_face_ok(d, family))
}

/// Settled (disk OK, Add=0) must never equal Activated.
#[cfg_attr(not(test), allow(dead_code))]
fn settled_implies_not_activated(settled: bool, gdi_faces_added: usize) -> bool {
    if settled {
        gdi_faces_added == 0
    } else {
        true
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn early_skip_after_refuse_or_settled(
    known_incapable: bool,
    session_refused: bool,
    disk_complete_settled: bool,
    intact: bool,
    undersized: bool,
) -> bool {
    known_incapable && intact && !undersized && (session_refused || disk_complete_settled)
}

fn clean_sfnt_cache() -> &'static Mutex<HashMap<PathBuf, (u64, u64)>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, (u64, u64)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn path_stamp_secs_len(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((mtime, len))
}

/// Rewrite Documents TTF in place when it still has Debg/TTFA/FFTM.
/// Fast-path checks `(mtime, len)` cache first, then reads only the 1KB SFNT table
/// directory header so normal fonts do not re-open files on every activation.
fn sanitize_on_disk_for_gdi(path: &Path) -> bool {
    let stamp = path_stamp_secs_len(path);
    if let Some((mtime, len)) = stamp
        && let Ok(g) = clean_sfnt_cache().lock()
            && g.get(path) == Some(&(mtime, len)) {
                return false;
            }
    {
        let Ok(mut f) = fs::File::open(path) else {
            return false;
        };
        let mut header = [0u8; 1036];
        let Ok(n) = f.read(&mut header) else {
            return false;
        };
        if !crate::namepatch::sfnt_header_needs_gdi_sanitize(&header[..n]) {
            if let Some((mtime, len)) = stamp
                && let Ok(mut g) = clean_sfnt_cache().lock() {
                    g.insert(path.to_path_buf(), (mtime, len));
                }
            return false;
        }
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let Some(clean) = crate::namepatch::gdi_sanitize_ttf(&bytes) else {
        if let Some((mtime, len)) = stamp
            && let Ok(mut g) = clean_sfnt_cache().lock() {
                g.insert(path.to_path_buf(), (mtime, len));
            }
        return false;
    };
    let tmp = path.with_extension("ttf.gdisan");
    if fs::write(&tmp, &clean).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
        return false;
    }
    intact_forget(path);
    if let Some((mtime, len)) = path_stamp_secs_len(path)
        && let Ok(mut g) = clean_sfnt_cache().lock() {
            g.insert(path.to_path_buf(), (mtime, len));
        }
    true
}

fn fetch_ttf(client: &reqwest::blocking::Client, slug: &str, version: &str, weight: u16, italic: bool, subset: &str) -> Result<Vec<u8>, String> {
    let bust = if bulk().bust.load(Ordering::SeqCst) {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(1)
    } else {
        0
    };
    let urls = ttf_urls(slug, version, weight, italic, subset, bust);
    let mut last = String::from("all CDNs failed");
    let mut skipped_open = 0usize;
    for url in urls.iter() {
        let host = host_label(url);
        if !circuit_allow(host) {
            last = format!("{host} paused (circuit open)");
            skipped_open += 1;
            continue;
        }
        if bulk().cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        // c260 P2: shared limiter (≤2, 250 ms, honest UA, 1–60 s backoff on 403/429/5xx).
        match gfsource::polite_fetch(client, url, &gfsource::PoliteOpts::default(), &cancel_requested) {
            Ok(bytes) if ttf_magic(&bytes) && bytes.len() >= 256 => {
                // Clear Sans Fontsource "TTF" is ~67KB + fsType restricted — skip.
                if slug == "clear-sans"
                    && (bytes.len() <= 96 * 1024
                        || os2_fstype(&bytes).map(|fs| fs & 0x0006 != 0).unwrap_or(false))
                {
                    last = format!("clear-sans reject non-Intel body from {host} ({} bytes)", bytes.len());
                    continue;
                }
                circuit_success(host);
                return Ok(bytes);
            }
            Ok(bytes) => {
                last = format!("not a TTF/OTF from {host} ({} bytes)", bytes.len());
                // 200 + wrong magic is not a CDN outage — do not trip the circuit
                // (Open Sauce jsDelivr serves 4/14 non-TTF bodies).
            }
            Err(Some(404)) => {
                last = format!("404 {host}");
                // Keep walking the list — npm @fontsource/files 404s for type:other
                // (Open Sauce) while @latest / @npmVersion still serve the TTF.
            }
            Err(Some(code)) => {
                last = format!("{code} {host}");
                if code >= 500 || code == 429 {
                    circuit_failure(host);
                }
            }
            Err(None) => {
                last = format!("{host}: network error");
                circuit_failure(host);
            }
        }
    }
    if skipped_open > 0 && skipped_open == urls.len() {
        last = "all CDNs paused (circuit open)".into();
    }
    Err(last)
}

/// Map Fontsource API `styles` strings to download flags (`false`=normal, `true`=italic).
/// Never invents normal when the package is italic-only (e.g. Syne Italic).
fn fontsource_styles_from_meta(style_names: &[String]) -> Vec<bool> {
    let has_normal = style_names.iter().any(|s| s.eq_ignore_ascii_case("normal"));
    let has_italic = style_names.iter().any(|s| s.eq_ignore_ascii_case("italic"));
    let mut out = Vec::new();
    if has_normal {
        out.push(false);
    }
    if has_italic {
        out.push(true);
    }
    if out.is_empty() {
        out.push(false);
    }
    out
}

/// Missing-package fast path: first-subset 400-normal 404, and no italic faces planned.
/// Italic-only families must not abort here — they never schedule normal, and dual-style
/// packs should keep trying italic after a normal miss.
fn fontsource_abort_on_normal_404(styles: &[bool], first_subset: bool, weight: u16, italic: bool) -> bool {
    first_subset && weight == 400 && !italic && !styles.iter().any(|s| *s)
}

/// jsDelivr `fontsource/fonts/{slug}@{tag}` wants the npm package tag (`5.3.0`),
/// not the foundry version (`v1.477`). Open Sauce / type:other 400s on the latter.
fn fontsource_jsdelivr_pin(npm_version: &str, foundry_version: &str) -> String {
    let npm = npm_version.trim().trim_start_matches('v');
    if !npm.is_empty() && !npm.eq_ignore_ascii_case("latest") {
        return npm.to_string();
    }
    foundry_version
        .trim()
        .trim_start_matches('v')
        .to_string()
}

type FontsourceMeta = (Vec<String>, Vec<u16>, Vec<bool>, String);

fn fontsource_meta(
    client: &reqwest::blocking::Client,
    slug: &str,
) -> Option<FontsourceMeta> {
    let url = format!("https://api.fontsource.org/v1/fonts/{slug}");
    let opts = gfsource::PoliteOpts { ua: None, timeout: Some(CATALOG_PROBE_TIMEOUT), cdn_lane: false };
    let text = String::from_utf8(gfsource::polite_fetch(client, &url, &opts, &cancel_requested).ok()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let subsets: Vec<String> = v
        .get("subsets")?
        .as_array()?
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.to_string()))
        .collect();
    if subsets.is_empty() {
        return None;
    }
    let weights: Vec<u16> = v
        .get("weights")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u16))
                .collect::<Vec<_>>()
        })
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| vec![400]);
    let style_names: Vec<String> = v
        .get("styles")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let styles = fontsource_styles_from_meta(&style_names);
    let version = fontsource_jsdelivr_pin(
        v.get("npmVersion").and_then(|x| x.as_str()).unwrap_or(""),
        v.get("version").and_then(|x| x.as_str()).unwrap_or(""),
    );
    Some((subsets, weights, styles, version))
}

fn is_cjk_subset(name: &str) -> bool {
    let s = name.to_ascii_lowercase();
    s.starts_with("chinese") || s == "japanese" || s == "korean" || s == "japanese-latin"
}

/// Prefer CJK / emoji / script subsets when metadata lists them. Latin-only is
/// wrong for Chiron / Noto CJK / emoji packs and must not be stamped `.complete`.
fn pick_subsets(all: &[String]) -> Vec<String> {
    let cjk: Vec<String> = all.iter().filter(|s| is_cjk_subset(s)).cloned().collect();
    if !cjk.is_empty() {
        return cjk;
    }
    let emoji: Vec<String> = all
        .iter()
        .filter(|s| s.eq_ignore_ascii_case("emoji"))
        .cloned()
        .collect();
    if !emoji.is_empty() {
        return emoji;
    }
    if all.iter().any(|s| s == "latin") {
        vec!["latin".into()]
    } else {
        all.iter().take(1).cloned().collect()
    }
}

/// Every advertised weight — never collapse to latin 400/700 as "complete".
fn pick_fontsource_weights(all: &[u16]) -> Vec<u16> {
    let mut out: Vec<u16> = all.to_vec();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        out.push(400);
    }
    out
}
/// Fontsource static faces. Empty on meta/package miss so callers fall through to Google.
/// Never treats `@fontsource-variable/*` as a desktop install source.
/// Returns (faces, version, expected face count from metadata).
fn fetch_ttf_to_file(
    client: &reqwest::blocking::Client,
    slug: &str,
    version: &str,
    weight: u16,
    italic: bool,
    subset: &str,
    dest: &Path,
) -> Result<(), String> {
    let bust = if bulk().bust.load(Ordering::SeqCst) {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(1)
    } else {
        0
    };
    let urls = ttf_urls(slug, version, weight, italic, subset, bust);
    let mut last = String::from("all CDNs failed");
    let mut skipped_open = 0usize;
    for url in urls.iter() {
        let host = host_label(url);
        if !circuit_allow(host) {
            last = format!("{host} paused (circuit open)");
            skipped_open += 1;
            continue;
        }
        if bulk().cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        // Single GET inside stream — classify from StreamFontResult (no double-GET).
        let result = stream_url_to_font_file(client, url, dest);
        if result.ok() {
            circuit_success(host);
            return Ok(());
        }
        match result {
            StreamFontResult::Cancelled => return Err("cancelled".into()),
            StreamFontResult::Http(404) => {
                last = format!("404 {host}");
                // Do not abort the URL list — Open Sauce npm files 404 while @latest works.
            }
            StreamFontResult::NotFont => {
                last = format!("not a TTF/OTF from {host}");
            }
            StreamFontResult::Http(status) if status >= 500 || status == 429 => {
                last = format!("{status} {host}");
                circuit_failure(host);
            }
            StreamFontResult::Http(status) => {
                last = format!("{status} {host}");
            }
            StreamFontResult::Failed(msg) => {
                last = format!("{host}: {msg}");
                circuit_failure(host);
            }
            StreamFontResult::Written | StreamFontResult::AlreadyIntact => {}
        }
    }
    if skipped_open > 0 && skipped_open == urls.len() {
        last = "all CDNs paused (circuit open)".into();
    }
    Err(last)
}

fn pull_fontsource_subset_to_dir(
    client: &reqwest::blocking::Client,
    slug: &str,
    version: &str,
    subsets: &[String],
    weights: &[u16],
    styles: &[bool],
    root: &Path,
) -> usize {
    let mut wrote = 0usize;
    for subset in subsets {
        for weight in weights {
            if bulk().cancel.load(Ordering::SeqCst) {
                return wrote;
            }
            for italic in styles {
                if bulk().cancel.load(Ordering::SeqCst) {
                    return wrote;
                }
                let style = if *italic { "italic" } else { "normal" };
                // Never embed Fontsource subset token `latin` in on-disk names.
                // CDN URLs still request the latin (or other) subset; filename matches Google keys.
                let name = fontsource_face_filename(slug, subset, *weight, style);
                let path = root.join(&name);
                if !bulk().bust.load(Ordering::SeqCst)
                    && ttf_intact(&path)
                    && !(slug == "clear-sans" && clear_sans_face_needs_intel_heal(&path))
                {
                    register_path(&path);
                    wrote += 1;
                    continue;
                }
                match fetch_ttf_to_file(client, slug, version, *weight, *italic, subset, &path) {
                    Ok(()) => wrote += 1,
                    Err(err) if err.starts_with("404") => {
                        let first = subset == subsets.first().map(|s| s.as_str()).unwrap_or("");
                        if fontsource_abort_on_normal_404(styles, first, *weight, *italic) {
                            return wrote;
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }
    wrote
}
fn fetch_google_css_text(client: &reqwest::blocking::Client, family: &str, ua: &str, axis: &str) -> Option<String> {
    if !google_endpoints_allowed(family) {
        return None;
    }
    let param = family.replace(' ', "+");
    let href = if axis.is_empty() {
        format!("https://fonts.googleapis.com/css2?family={param}&display=swap")
    } else {
        format!("https://fonts.googleapis.com/css2?family={param}:{axis}&display=swap")
    };
    // css2 needs a Mozilla UA to serve TTF; still paced + backed off (c260 P2).
    let opts = gfsource::PoliteOpts { ua: Some(ua), timeout: Some(CATALOG_PROBE_TIMEOUT), cdn_lane: false };
    let css = String::from_utf8(gfsource::polite_fetch(client, &href, &opts, &cancel_requested).ok()?).ok()?;
    if css.len() < 32 || !css.contains("@font-face") {
        return None;
    }
    Some(css)
}

fn css_prop<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}:");
    let rest = block.split(&key).nth(1)?;
    let end = rest.find(';').unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn css_ttf_url(block: &str) -> Option<String> {
    for token in block.split("url(").skip(1) {
        let end = token.find(')')?;
        let url = token[..end].trim().trim_matches('\'').trim_matches('"');
        if url.starts_with("http") && (url.contains(".ttf") || url.contains(".otf")) && !url.contains(".woff")
        {
            return Some(url.to_string());
        }
    }
    None
}

fn css_range_rank(block: &str) -> i32 {
    let lower = block.to_ascii_lowercase();
    if lower.contains("u+4e00")
        || lower.contains("chinese")
        || lower.contains("japanese")
        || lower.contains("korean")
    {
        return 3;
    }
    // Full desktop TTFs from Mozilla/Googlebot often omit unicode-range.
    if !block.contains("unicode-range") {
        return 2;
    }
    if block.contains("U+0000") {
        return 0;
    }
    1
}

/// One TTF/OTF per (style, weight). Prefer full / CJK ranges over latin-only shreds.
fn parse_css_faces(css: &str) -> Vec<(String, String, String)> {
    let mut best: HashMap<(String, String), (i32, String)> = HashMap::new();
    for block in css.split("@font-face") {
        let Some(url) = css_ttf_url(block) else {
            continue;
        };
        let style = css_prop(block, "font-style")
            .unwrap_or("normal")
            .trim()
            .to_ascii_lowercase();
        let weight = css_prop(block, "font-weight")
            .unwrap_or("400")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-");
        let rank = css_range_rank(block);
        let key = (style, weight);
        match best.get(&key) {
            Some((had, _)) if *had >= rank => {}
            _ => {
                best.insert(key, (rank, url));
            }
        }
    }
    let mut out: Vec<(String, String, String)> = best
        .into_iter()
        .map(|((style, weight), (_, url))| (style, weight, url))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    // Official families advertise ≤18 static faces (9 weights × italic); keep headroom.
    out.truncate(24);
    out
}

/// Hard cap for a single TTF/OTF body (google/fonts var files included).
/// CJK VFs (Chiron / Noto Serif KR·SC) are ~23–52MB — jsDelivr's 20MB limit
/// rejects them, so we fall back to GitHub raw; 32MB was still too small for
/// Chiron GoRound/Sung. Prevents unbounded RAM on a non-font payload.
const MAX_TTF_FETCH_BYTES: usize = 64 * 1024 * 1024;

fn fetch_url_ttf(client: &reqwest::blocking::Client, url: &str) -> Option<Vec<u8>> {
    if bulk().cancel.load(Ordering::SeqCst) {
        return None;
    }
    let host = host_label(url);
    if !circuit_allow(host) {
        return None;
    }
    // c258 item 5: ≤2 concurrent, ≥250 ms spacing, honest UA, backoff on 403/429/5xx.
    let bytes = match gfsource::polite_get_status(client, url, &cancel_requested) {
        Ok(b) => b,
        Err(code) => {
            if code.map(|c| c >= 500).unwrap_or(true) {
                circuit_failure(host);
            }
            return None;
        }
    };
    if bytes.len() > MAX_TTF_FETCH_BYTES {
        return None;
    }
    if ttf_magic(&bytes) && bytes.len() >= 256 {
        circuit_success(host);
        Some(bytes)
    } else {
        None
    }
}

fn static_weight_axis() -> String {
    let mut pairs = Vec::new();
    for ital in [0, 1] {
        for w in [100, 200, 300, 400, 500, 600, 700, 800, 900] {
            pairs.push(format!("{ital},{w}"));
        }
    }
    format!("ital,wght@{}", pairs.join(";"))
}

/// Higher = richer request axis. Bare family= is weakest (often Regular-400 only).
fn axis_richness(axis: &str) -> i32 {
    if axis.is_empty() {
        0
    } else if axis.starts_with("wght@") && !axis.contains("ital") {
        1
    } else if axis.contains("100..900") {
        2
    } else if axis.starts_with("ital,wght@") {
        3
    } else {
        1
    }
}

fn fetch_google_css_font(client: &reqwest::blocking::Client, family: &str, ua: &str, axis: &str) -> Option<Vec<u8>> {
    let css = fetch_google_css_text(client, family, ua, axis)?;
    for (_, _, url) in parse_css_faces(&css) {
        if let Some(bytes) = fetch_url_ttf(client, &url) {
            return Some(bytes);
        }
    }
    None
}

fn fetch_google_css_listed(
    client: &reqwest::blocking::Client,
    family: &str,
    ua: &str,
    axis: &str,
) -> Vec<(String, String, String)> {
    let Some(css) = fetch_google_css_text(client, family, ua, axis) else {
        return Vec::new();
    };
    parse_css_faces(&css)
}

/// Official fonts.google.com families (bundled directory). catalog:other skips Google CSS.
/// Accepts display names and slug-shaped folder names (`Libre Baskerville` / `libre-baskerville`).
fn register_live_google_families<I, S>(families: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    crate::google_cdn::register_live_families(families, Some(load_cached_google_revisions()));
}

fn is_official_google_family(family: &str) -> bool {
    static DIR: OnceLock<(HashSet<String>, HashSet<String>)> = OnceLock::new();
    let (by_lower, by_slug) = DIR.get_or_init(|| {
        let raw = include_str!("../../../src/lib/fonts/google-directory.json");
        let mut by_lower = HashSet::new();
        let mut by_slug = HashSet::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(arr) = v.get("families").and_then(|x| x.as_array()) {
                for name in arr {
                    if let Some(s) = name.as_str() {
                        let t = s.trim();
                        if t.is_empty() {
                            continue;
                        }
                        by_lower.insert(t.to_ascii_lowercase());
                        by_slug.insert(slug_family(t));
                    }
                }
            }
        (by_lower, by_slug)
    });
    let key = family.trim().to_ascii_lowercase();
    if key.is_empty() {
        return false;
    }
    let slug = slug_family(family);
    if by_lower.contains(&key) || by_slug.contains(&slug) {
        return true;
    }
    crate::google_cdn::is_in_live_directory(family, Some(load_cached_google_revisions()))
}


/// Activate fetch pipe: Google card = Google faces only; Fontsource card = FS only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FetchIntent {
    Google,
    Fontsource,
    Local,
}

/// c255 P3: Material Symbols come only from google/material-design-icons
/// (GitHub/jsDelivr) — never fonts.googleapis / gstatic.
fn google_endpoints_allowed(family: &str) -> bool {
    material_symbols_ttf_urls(family).is_empty()
}

/// c255 P3: Material Symbols always take the GitHub VF branch, whatever the caller sent.
fn effective_fetch_intent(family: &str, intent: FetchIntent) -> FetchIntent {
    if matches!(intent, FetchIntent::Google) && !google_endpoints_allowed(family) {
        FetchIntent::Fontsource
    } else {
        intent
    }
}

fn parse_fetch_intent(s: &str) -> Option<FetchIntent> {
    match s.trim().to_ascii_lowercase().as_str() {
        "google" => Some(FetchIntent::Google),
        "fontsource" | "other" => Some(FetchIntent::Fontsource),
        "local" => Some(FetchIntent::Local),
        _ => None,
    }
}

fn fetch_intent_label(intent: FetchIntent) -> &'static str {
    match intent {
        FetchIntent::Google => "google",
        FetchIntent::Fontsource => "fontsource",
        FetchIntent::Local => "local",
    }
}

fn infer_fetch_intent(family: &str) -> FetchIntent {
    if is_official_google_family(family) {
        FetchIntent::Google
    } else {
        FetchIntent::Fontsource
    }
}

fn remember_fetch_intent(family: &str, intent: FetchIntent) {
    let key = family.trim().to_lowercase();
    if key.is_empty() {
        return;
    }
    if let Ok(mut map) = bulk().intents.lock() {
        map.insert(key, intent);
    }
}

fn forget_fetch_intent(family: &str) {
    let key = family.trim().to_lowercase();
    if let Ok(mut map) = bulk().intents.lock() {
        map.remove(&key);
    }
}

fn intent_for_family(family: &str) -> FetchIntent {
    let key = family.trim().to_lowercase();
    if let Ok(map) = bulk().intents.lock()
        && let Some(i) = map.get(&key) {
            return *i;
        }
    infer_fetch_intent(family)
}

/// Resume / missing-store intent: stamp → planned markers → official Google catalog.
/// Returns None when ambiguous — never blind-default to Google (wrong-pipes Fontsource).
fn resolve_fetch_intent_from_disk(app: &AppHandle, family: &str) -> Option<FetchIntent> {
    let family = family.trim();
    if family.is_empty() {
        return None;
    }
    let key = family.to_lowercase();
    if let Ok(map) = bulk().intents.lock()
        && let Some(i) = map.get(&key) {
            return Some(*i);
        }
    for dir in family_locations(app, family) {
        migrate_download_source_stamp(&dir);
        if let Some(i) = read_download_source(&dir) {
            return Some(i);
        }
        if let Ok(root) = documents_root(app)
            && dir_is_upload_container_child(&root, &dir) {
                return Some(FetchIntent::Local);
            }
        if read_google_planned_keys(&dir).is_some() {
            return Some(FetchIntent::Google);
        }
        if read_fontsource_planned_keys(&dir).is_some() {
            return Some(FetchIntent::Fontsource);
        }
    }
    // Catalog: official Google directory → google. Unknown / exclusive without
    // stamp or planned markers → None (caller skips or uses catalog "other").
    // Never blind-default to Google.
    if is_official_google_family(family) {
        return Some(FetchIntent::Google);
    }
    None
}

fn family_download_source_marker(dir: &Path) -> PathBuf {
    dir.join(".download-source")
}

fn write_download_source(dir: &Path, intent: FetchIntent) {
    let _ = fs::write(
        family_download_source_marker(dir),
        fetch_intent_label(intent).as_bytes(),
    );
}

fn read_download_source(dir: &Path) -> Option<FetchIntent> {
    let s = fs::read_to_string(family_download_source_marker(dir)).ok()?;
    parse_fetch_intent(s.trim())
}

/// Boot/scan migration: stamp `.download-source` when evidence is strong.
/// google if usable `.google-planned` key list; else fontsource if
/// `.fontsource-planned` / latin-subset names dominate; else leave unset
/// (never guess wrong). Register still uses `face_allowed_for_register`.
fn migrate_download_source_stamp(dir: &Path) {
    if read_download_source(dir).is_some() {
        return;
    }
    if read_google_planned_keys(dir).is_some() {
        write_download_source(dir, FetchIntent::Google);
        return;
    }
    if read_fontsource_planned_keys(dir).is_some() {
        write_download_source(dir, FetchIntent::Fontsource);
        return;
    }
    let slug = dir_slug_hint(dir);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    if files.is_empty() {
        return;
    }
    let mut latin = 0usize;
    let mut other = 0usize;
    for path in &files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        if filename_has_latin_subset(name, &slug) {
            latin = latin.saturating_add(1);
        } else {
            other = other.saturating_add(1);
        }
    }
    // Only stamp fontsource when latin-subset names clearly dominate.
    if latin > 0 && latin > other {
        write_download_source(dir, FetchIntent::Fontsource);
    }
    // else leave unset — do not guess google vs local.
}


fn family_fontsource_planned_marker(dir: &Path) -> PathBuf {
    dir.join(".fontsource-planned")
}

fn write_fontsource_planned(dir: &Path, keys: &[String]) {
    if keys.is_empty() {
        let _ = fs::remove_file(family_fontsource_planned_marker(dir));
        return;
    }
    let body = keys.join("\n");
    let _ = fs::write(family_fontsource_planned_marker(dir), body.as_bytes());
}

fn clear_fontsource_planned(dir: &Path) {
    let _ = fs::remove_file(family_fontsource_planned_marker(dir));
}

fn read_fontsource_planned_keys(dir: &Path) -> Option<Vec<String>> {
    let s = fs::read_to_string(family_fontsource_planned_marker(dir)).ok()?;
    let keys: Vec<String> = s
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.contains('.'))
        .collect();
    if keys.is_empty() {
        None
    } else {
        Some(keys)
    }
}

/// Register only faces from the chosen download source so leftover other-source
/// TTFs in a shared family folder cannot go Live for the wrong card.
fn face_allowed_for_register(dir: &Path, path: &Path, family: &str) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let slug = slug_family(family);
    let intent = read_download_source(dir).unwrap_or_else(|| intent_for_family(family));
    match intent {
        FetchIntent::Local => true,
        FetchIntent::Google => {
            // c258 item 4: a latin-cut outlier face is never loaded into Windows.
            if !is_variable_face_filename(name) && face_is_glyph_outlier(path) {
                return false;
            }
            if let Some(keys) = read_google_planned_keys(dir) {
                return keys.iter().any(|k| k == name);
            }
            // No Google plan: never Add Fontsource latin-subset leftovers.
            if !slug.is_empty() && filename_has_latin_subset(name, &slug) {
                return false;
            }
            if let Some(fs_keys) = read_fontsource_planned_keys(dir)
                && fs_keys.iter().any(|k| k == name) {
                    return false;
                }
            true
        }
        FetchIntent::Fontsource => {
            if let Some(keys) = read_fontsource_planned_keys(dir) {
                return keys.iter().any(|k| k == name);
            }
            // No FS plan: never Add Google-planned leftovers for the Fontsource card.
            if let Some(g_keys) = read_google_planned_keys(dir)
                && g_keys.iter().any(|k| k == name) {
                    return false;
                }
            true
        }
    }
}

#[derive(Clone, Copy)]
struct GoogleCatalogMeta {
    floor: usize,
    variable: bool,
    italic: bool,
    weight_lo: u16,
    weight_hi: u16,
}

fn google_catalog_meta_maps() -> &'static (HashMap<String, GoogleCatalogMeta>, HashMap<String, GoogleCatalogMeta>) {
    static META: OnceLock<(HashMap<String, GoogleCatalogMeta>, HashMap<String, GoogleCatalogMeta>)> =
        OnceLock::new();
    META.get_or_init(|| {
        let raw = include_str!("../../../src/lib/fonts/google-catalog.json");
        let mut by_lower = HashMap::new();
        let mut by_slug = HashMap::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(arr) = v.get("families").and_then(|x| x.as_array()) {
                for row in arr {
                    let Some(row) = row.as_array() else { continue };
                    if row.len() < 5 {
                        continue;
                    }
                    let Some(name) = row[0].as_str() else { continue };
                    let weights: Vec<u16> = row[2]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_u64().map(|n| n as u16))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if weights.is_empty() {
                        continue;
                    }
                    let italic = row[3].as_bool().unwrap_or(false);
                    let variable = row[4].as_bool().unwrap_or(false);
                    let floor = weights.len().saturating_mul(if italic { 2 } else { 1 });
                    let t = name.trim();
                    if t.is_empty() || floor == 0 {
                        continue;
                    }
                    let lo = *weights.iter().min().unwrap_or(&400);
                    let hi = *weights.iter().max().unwrap_or(&400);
                    let meta = GoogleCatalogMeta {
                        floor,
                        variable,
                        italic,
                        weight_lo: lo,
                        weight_hi: hi,
                    };
                    by_lower.insert(t.to_ascii_lowercase(), meta);
                    by_slug.insert(slug_family(t), meta);
                }
            }
        (by_lower, by_slug)
    })
}

fn google_catalog_meta(family: &str) -> Option<GoogleCatalogMeta> {
    let (by_lower, by_slug) = google_catalog_meta_maps();
    let key = family.trim().to_ascii_lowercase();
    by_lower
        .get(&key)
        .or_else(|| by_slug.get(&slug_family(family)))
        .copied()
}

/// Offline floor: google-catalog `weights.len() * (2 if italic else 1)`.
/// Used to catch Fontsource latin packs stamped complete without `.google-planned`.
fn google_catalog_face_floor(family: &str) -> Option<usize> {
    google_catalog_meta(family).map(|m| m.floor)
}

fn google_catalog_is_variable(family: &str) -> bool {
    google_catalog_meta(family).map(|m| m.variable).unwrap_or(false)
}

#[derive(Clone, Copy)]
struct FontsourceOtherMeta {
    variable: bool,
}

fn fontsource_other_meta_maps() -> &'static (HashMap<String, FontsourceOtherMeta>, HashMap<String, FontsourceOtherMeta>) {
    static META: OnceLock<(HashMap<String, FontsourceOtherMeta>, HashMap<String, FontsourceOtherMeta>)> =
        OnceLock::new();
    META.get_or_init(|| {
        let raw = include_str!("../../../src/lib/fonts/fontsource-other.json");
        let mut by_lower = HashMap::new();
        let mut by_slug = HashMap::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(arr) = v.get("families").and_then(|x| x.as_array()) {
                for row in arr {
                    let Some(row) = row.as_array() else { continue };
                    if row.len() < 5 {
                        continue;
                    }
                    let Some(name) = row[0].as_str() else { continue };
                    let variable = row[4].as_bool().unwrap_or(false);
                    let t = name.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let meta = FontsourceOtherMeta { variable };
                    by_lower.insert(t.to_ascii_lowercase(), meta);
                    by_slug.insert(slug_family(t), meta);
                }
            }
        (by_lower, by_slug)
    })
}

fn fontsource_other_meta(family: &str) -> Option<FontsourceOtherMeta> {
    let (by_lower, by_slug) = fontsource_other_meta_maps();
    let key = family.trim().to_ascii_lowercase();
    by_lower
        .get(&key)
        .or_else(|| by_slug.get(&slug_family(family)))
        .copied()
}


/// Fontsource-other `variable:true` families with a public google/fonts TTF VF.
/// Folder = alphanumeric compact lower (`42dot Sans` → `42dotsans`).
/// Never WOFF2 / never `@fontsource-variable`. Material Symbols* have WOFF2-only
/// on Fontsource and no google/fonts TTF — excluded.
fn fontsource_other_is_variable(family: &str) -> bool {
    fontsource_other_meta(family).map(|m| m.variable).unwrap_or(false)
}

/// c253 #7: official Material Symbols VF TTFs (google/material-design-icons, Apache-2.0).
fn material_symbols_ttf_urls(family: &str) -> Vec<String> {
    let style = match family.trim().to_ascii_lowercase().as_str() {
        "material symbols outlined" | "material symbols" => "Outlined",
        "material symbols rounded" => "Rounded",
        "material symbols sharp" => "Sharp",
        _ => return Vec::new(),
    };
    let file = format!("MaterialSymbols{style}%5BFILL,GRAD,opsz,wght%5D.ttf");
    vec![
        format!("https://cdn.jsdelivr.net/gh/google/material-design-icons@master/variablefont/{file}"),
        format!("https://raw.githubusercontent.com/google/material-design-icons/master/variablefont/{file}"),
    ]
}

fn family_is_woff2_only_variable(family: &str) -> bool {
    let t = family.trim().to_ascii_lowercase();
    t == "material symbols outlined"
        || t == "material symbols rounded"
        || t == "material symbols sharp"
        || t == "material symbols"
}

/// google/fonts repo folder override for FS-other (and known slug fixes).
fn fs_only_google_vf_folder(family: &str) -> Option<String> {
    if family_is_woff2_only_variable(family) {
        return None;
    }
    // Known explicit folders (keep even if compact differs).
    let key = family.trim().to_ascii_lowercase();
    let fixed = match key.as_str() {
        "42dot sans" => Some("42dotsans"),
        "big shoulders display" => Some("bigshouldersdisplay"),
        "big shoulders text" => Some("bigshoulderstext"),
        "big shoulders inline display" => Some("bigshouldersinlinedisplay"),
        "big shoulders inline text" => Some("bigshouldersinlinetext"),
        "big shoulders stencil display" => Some("bigshouldersstencildisplay"),
        "big shoulders stencil text" => Some("bigshouldersstenciltext"),
        "briem hand" => Some("briemhand"),
        "finlandica text" => Some("finlandicatext"),
        "finlandica headline" => Some("finlandicaheadline"),
        _ => None,
    };
    if let Some(f) = fixed {
        return Some(f.to_string());
    }
    if fontsource_other_is_variable(family) {
        let compact: String = family
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if !compact.is_empty() {
            return Some(compact);
        }
    }
    None
}

/// True when Activate/Repair should pull a real google/fonts `*-variable-*` TTF.
/// Covers: google-catalog variable + all Fontsource-other variable with a TTF path.
fn family_ensures_google_vf(family: &str) -> bool {
    if family_has_no_public_vf(family) || family_is_woff2_only_variable(family) {
        return false;
    }
    google_catalog_is_variable(family)
        || fontsource_other_is_variable(family)
        || fs_only_google_vf_folder(family).is_some()
}


/// CSS axis strings for catalog-variable families (real `min..max` ranges).
/// Mozilla/Googlebot expand these to installable instance TTFs — never Chrome WOFF2.
fn variable_axis_specs(family: &str) -> Vec<String> {
    let meta = google_catalog_meta(family);
    let (lo, hi, italic) = meta
        .map(|m| (m.weight_lo, m.weight_hi, m.italic))
        .unwrap_or((100, 900, true));
    let mut axes = Vec::new();
    if italic {
        axes.push(format!("ital,wght@0,{lo}..{hi};1,{lo}..{hi}"));
    }
    axes.push(format!("wght@{lo}..{hi}"));
    axes
}

fn listing_is_400_swept(listed: &[(String, String, String)]) -> bool {
    !listed.is_empty() && listed.iter().all(|(_, w, _)| w == "400")
}

/// True when a CSS weight token is a range (`200-1000` from `font-weight: 200 1000`).
/// Discrete instance keys are a single integer (`400`).
fn weight_token_is_range(weight: &str) -> bool {
    let parts: Vec<&str> = weight.split('-').filter(|p| !p.is_empty()).collect();
    parts.len() >= 2 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

fn listing_has_only_range_weights(listed: &[(String, String, String)]) -> bool {
    !listed.is_empty() && listed.iter().all(|(_, w, _)| weight_token_is_range(w))
}

/// google/fonts repo folder candidates: compact (`librebaskerville`) then slug (`libre-baskerville`).
fn google_fonts_repo_folders(family: &str) -> Vec<String> {
    let compact: String = family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let slug = slug_family(family);
    let mut out = Vec::new();
    // FS-only / slug overrides first (42dot Sans → 42dotsans, not 42dot-sans miss).
    if let Some(fixed) = fs_only_google_vf_folder(family) {
        out.push(fixed);
    }
    if !compact.is_empty() && !out.iter().any(|s| s == &compact) {
        out.push(compact);
    }
    if !slug.is_empty() && !out.iter().any(|s| s == &slug) {
        out.push(slug);
    }
    out
}

fn google_fonts_pascal(family: &str) -> String {
    family.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// On-disk name for a real variable TTF from google/fonts (never latin / never WOFF).
fn variable_face_filename(slug: &str, axes: &str, italic: bool) -> String {
    let axes_tok = axes
        .replace(',', "-")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>();
    let axes_tok = axes_tok.trim_matches('-');
    if italic {
        sanitize(&format!("{slug}-variable-{axes_tok}-italic.ttf"))
    } else {
        sanitize(&format!("{slug}-variable-{axes_tok}.ttf"))
    }
}

fn parse_metadata_pb_axes_and_files(meta: &str) -> (Vec<String>, Vec<(String, bool)>) {
    // filenames like Nunito[wght].ttf / Roboto[wdth,wght].ttf / Nunito-Italic[wght].ttf
    let mut files: Vec<(String, bool)> = Vec::new();
    let mut axes_order: Vec<String> = Vec::new();
    for line in meta.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("tag:") {
            let tag = rest.trim().trim_matches('"').trim();
            if !tag.is_empty() && !axes_order.iter().any(|a| a == tag) {
                axes_order.push(tag.to_string());
            }
        }
        if let Some(rest) = t.strip_prefix("filename:") {
            let name = rest.trim().trim_matches('"').trim();
            let lower = name.to_ascii_lowercase();
            let is_ttf = lower.ends_with(".ttf") || lower.ends_with(".otf");
            // google/fonts VF names are either `Family[wght].ttf` or older
            // `Family-VariableFont_wght.ttf` — both are installable desktop files.
            let is_vf = name.contains('[')
                || lower.contains("variablefont")
                || lower.contains("-variable-");
            if is_ttf && is_vf {
                let italic = lower.contains("-italic[")
                    || lower.contains("-italic.")
                    || lower.contains("-italic-variablefont");
                files.push((name.to_string(), italic));
            }
        }
    }
    (axes_order, files)
}

fn encode_google_fonts_filename(filename: &str) -> String {
    // Bracket axes must be percent-encoded for jsDelivr / GitHub raw.
    filename
        .chars()
        .map(|c| match c {
            '[' => "%5B".to_string(),
            ']' => "%5D".to_string(),
            ' ' => "%20".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

fn jsdelivr_google_fonts_url(license: &str, folder: &str, filename: &str) -> String {
    let enc = encode_google_fonts_filename(filename);
    format!("https://cdn.jsdelivr.net/gh/google/fonts@main/{license}/{folder}/{enc}")
}

/// GitHub raw fallback — jsDelivr refuses files over ~20MB (CJK variable TTFs).
fn github_raw_google_fonts_url(license: &str, folder: &str, filename: &str) -> String {
    let enc = encode_google_fonts_filename(filename);
    format!("https://raw.githubusercontent.com/google/fonts/main/{license}/{folder}/{enc}")
}

/// Prefer jsDelivr (fast, cached) then GitHub raw (serves 20MB+ CJK VFs).
fn google_fonts_variable_cdn_urls(license: &str, folder: &str, filename: &str) -> [String; 2] {
    [
        jsdelivr_google_fonts_url(license, folder, filename),
        github_raw_google_fonts_url(license, folder, filename),
    ]
}

/// METADATA.pb: same jsDelivr → GitHub raw order as VF TTFs (jsDelivr can 403/omit).
fn google_fonts_metadata_cdn_urls(license: &str, folder: &str) -> [String; 2] {
    [
        format!("https://cdn.jsdelivr.net/gh/google/fonts@main/{license}/{folder}/METADATA.pb"),
        format!("https://raw.githubusercontent.com/google/fonts/main/{license}/{folder}/METADATA.pb"),
    ]
}

/// Catalog marks these `variable: true` but google/fonts has no public VF file
/// (Google Sans is proprietary; Edu * Hand packs ship statics only). Do not
/// invent VFs or clear `.complete` for missing `*-variable-*`.
fn family_has_no_public_vf(family: &str) -> bool {
    const NO_PUBLIC_VF: &[&str] = &[
        "Google Sans",
        "Edu NSW ACT Cursive",
        "Edu NSW ACT Hand Pre",
        "Edu QLD Hand",
        "Edu SA Hand",
        "Edu VIC WA NT Hand",
        "Edu VIC WA NT Hand Pre",
    ];
    let t = family.trim();
    NO_PUBLIC_VF.iter().any(|n| n.eq_ignore_ascii_case(t))
}

/// Catalog-variable families that should have a real `*-variable-*` / VF on disk.
fn catalog_variable_expects_public_vf(family: &str) -> bool {
    family_ensures_google_vf(family)
}

/// google/fonts ships **two** VFs (roman + italic) for these families. Ensure must
/// not early-return after roman-only planned/intact — retry italic.
fn family_expects_dual_variable(family: &str) -> bool {
    let t = family.trim();
    t.eq_ignore_ascii_case("Chiron Hei HK")
        || t.eq_ignore_ascii_case("Chiron Sung HK")
        // Catalog names (not bare "Finlandica"): each ships roman + italic VF.
        || t.eq_ignore_ascii_case("Finlandica Text")
        || t.eq_ignore_ascii_case("Finlandica Headline")
}

fn dir_has_intact_variable_italic(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files.iter().any(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .map(|n| variable_face_filename_is_italic(n) && ttf_intact(p))
            .unwrap_or(false)
    })
}

/// Download real variable TTFs from google/fonts (jsDelivr, then GitHub raw for large CJK).
/// Never `@fontsource-variable` WOFF.
/// Returns on-disk filenames that were written or already intact, plus HealStats from
/// in-place name heals on intact faces (never discard locked/healed).
fn download_google_variable_ttfs(
    client: &reqwest::blocking::Client,
    family: &str,
    slug: &str,
    root: &Path,
) -> (Vec<String>, HealStats) {
    if !family_ensures_google_vf(family) {
        return (Vec::new(), HealStats::default());
    }
    let licenses = ["ofl", "apache", "ufl"];
    let folders = google_fonts_repo_folders(family);
    let pascal = google_fonts_pascal(family);
    let mut heal = HealStats::default();

    // 1) Prefer METADATA.pb filenames + axes.
    // partial_wrote: roman landed but METADATA also listed italic (dual-VF) —
    // seed the axis-pattern fallback so we keep roman and still hunt italic.
    let mut partial_wrote: Vec<String> = Vec::new();
    'meta: for lic in licenses {
        for folder in &folders {
            if bulk().cancel.load(Ordering::SeqCst) {
                return (Vec::new(), heal);
            }
            let mut meta_text: Option<String> = None;
            for meta_url in google_fonts_metadata_cdn_urls(lic, folder) {
                if !catalog_download_allowed() {
                    return (partial_wrote, heal);
                }
                let opts = gfsource::PoliteOpts { ua: None, timeout: Some(CATALOG_PROBE_TIMEOUT), cdn_lane: false };
                let Ok(bytes) = gfsource::polite_fetch(client, &meta_url, &opts, &cancel_requested) else {
                    continue;
                };
                let Ok(text) = String::from_utf8(bytes) else { continue };
                if text.len() < 16 || !text.contains("filename:") {
                    continue;
                }
                meta_text = Some(text);
                break;
            }
            let Some(text) = meta_text else { continue };
            let (axes, files) = parse_metadata_pb_axes_and_files(&text);
            if files.is_empty() {
                continue;
            }
            let meta_wants_italic = files.iter().any(|(_, italic)| *italic);
            let mut wrote = Vec::new();
            for (fname, italic) in &files {
                let axes_label = if !axes.is_empty() {
                    axes.join(",")
                } else if let Some(start) = fname.find('[') {
                    let end = fname.find(']').unwrap_or(fname.len());
                    fname[start + 1..end].to_string()
                } else {
                    "wght".into()
                };
                let dest_name = variable_face_filename(slug, &axes_label, *italic);
                let dest = root.join(&dest_name);
                if !bulk().bust.load(Ordering::SeqCst) && ttf_intact(&dest) {
                    // Intact vars from prior installs may still mash id1
                    // ("Nunito ExtraLight"). Heal name only — keep fvar.
                    // Never call register_path here: download_family's final register
                    // respects cancel_requested() and repair_files_only().
                    heal.add(heal_google_variable_face_file(&dest, family, *italic));
                    wrote.push(dest_name);
                    continue;
                }
                // jsDelivr first; GitHub raw for >20MB CJK VFs jsDelivr rejects.
                for url in google_fonts_variable_cdn_urls(lic, folder, fname) {
                    if !catalog_download_allowed() {
                        return (wrote, heal);
                    }
                    note_catalog_current(&format!("{family} · variable"));
                    let Some(bytes) = fetch_url_ttf(client, &url) else {
                        continue;
                    };
                    // google/fonts vars often mash default-instance style into
                    // nameID 1. Rewrite name only (Regular/Italic); preserve fvar.
                    let patched = crate::namepatch::patch_variable_face(&bytes, family, *italic)
                        .unwrap_or(bytes);
                    if write_font_file(&dest, &patched).is_ok() {
                        wrote.push(dest_name);
                        break;
                    }
                }
            }
            let wrote_italic = wrote.iter().any(|w| variable_face_filename_is_italic(w));
            if !wrote.is_empty() && !(meta_wants_italic && !wrote_italic) {
                return (wrote, heal);
            }
            if !wrote.is_empty() && meta_wants_italic && !wrote_italic {
                partial_wrote = wrote;
                break 'meta;
            }
            // METADATA found but TTFs missing — try next folder/license.
        }
    }

    // 2) Fallback: try common axis filename patterns across license dirs.
    let axis_patterns = [
        "wght",
        "wdth,wght",
        "opsz,wght",
        "wght,wdth",
        "CASL,CRSV,MONO,slnt,wght",
    ];
    let mut wrote = partial_wrote;
    for lic in licenses {
        for folder in &folders {
            for axes in axis_patterns {
                if !catalog_download_allowed() {
                    return (wrote, heal);
                }
                for italic in [false, true] {
                    let dest_name = variable_face_filename(slug, axes, italic);
                    let dest = root.join(&dest_name);
                    if wrote.iter().any(|w| w == &dest_name) {
                        continue;
                    }
                    if !bulk().bust.load(Ordering::SeqCst) && ttf_intact(&dest) {
                        heal.add(heal_google_variable_face_file(&dest, family, italic));
                        register_path(&dest);
                        wrote.push(dest_name);
                        continue;
                    }
                    let axes_us = axes.replace(',', "_");
                    let remotes = if italic {
                        [
                            format!("{pascal}-Italic[{axes}].ttf"),
                            format!("{pascal}-Italic-VariableFont_{axes_us}.ttf"),
                        ]
                    } else {
                        [
                            format!("{pascal}[{axes}].ttf"),
                            format!("{pascal}-VariableFont_{axes_us}.ttf"),
                        ]
                    };
                    'outer: for remote in remotes {
                        for url in google_fonts_variable_cdn_urls(lic, folder, &remote) {
                            let Some(bytes) = fetch_url_ttf(client, &url) else {
                                continue;
                            };
                            let patched = crate::namepatch::patch_variable_face(&bytes, family, italic)
                                .unwrap_or(bytes);
                            if write_font_file(&dest, &patched).is_ok() {
                                wrote.push(dest_name);
                                break 'outer;
                            }
                        }
                    }
                }
                // If we got a roman var file for this axes pattern, stop trying other axes
                // — unless dual-VF (Hei/Sung) still needs italic.
                let has_roman = wrote
                    .iter()
                    .any(|w| w.contains("-variable-") && !variable_face_filename_is_italic(w));
                let has_italic = wrote.iter().any(|w| variable_face_filename_is_italic(w));
                if has_roman && (has_italic || !family_expects_dual_variable(family)) {
                    return (wrote, heal);
                }
            }
        }
    }
    (wrote, heal)
}

fn is_variable_face_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Dest names we write (`*-variable-*`) plus leftover google/fonts originals
    // (`Family-VariableFont_wght.ttf`, `Family[wght].ttf`) so scan/honesty counts them.
    lower.contains("-variable-")
        || lower.contains("variablefont")
        || name.contains('[')
}

/// `*-variable-*-italic.ttf` (or ends with `-italic.ttf` after the variable token).
fn variable_face_filename_is_italic(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("-variable-") && lower.contains("-italic.")
}

fn dir_has_intact_variable(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files.iter().any(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .map(|n| is_variable_face_filename(n) && ttf_intact(p))
            .unwrap_or(false)
    })
}

/// 1.0.206u: VF is primary when an intact variable face is present.
/// Dual-VF (Hei/Sung + Finlandica Text/Headline): roman + italic both required.
/// No-public-VF denylist (Poppins-class catalog statics, Google Sans, Edu hands) never VF-primary.
fn dir_vf_primary_ready(dir: &Path, family: &str) -> bool {
    if dir_static_first_blocks_vf(dir) {
        return false;
    }
    if family_has_no_public_vf(family) {
        return false;
    }
    if !dir_has_intact_variable(dir) {
        return false;
    }
    if family_expects_dual_variable(family) && !dir_has_intact_variable_italic(dir) {
        return false;
    }
    true
}

fn family_any_dir_vf_primary(app: &AppHandle, family: &str) -> bool {
    family_locations(app, family)
        .iter()
        .any(|d| dir_vf_primary_ready(d, family))
}

/// Compat emoji/color sidecars kept even under VF-primary purge.
fn is_compat_sidecar_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("-svg.") || lower.contains("-colrv1.") || lower.contains("-compat-")
}

/// Delete sibling static TTFs when VF is primary. Keeps VFs + compat sidecars.
/// Also drops matching `%LOCALAPPDATA%\Font Manager\gdi-maps` stage copies.
/// Returns `(deleted, locked)`.
fn purge_redundant_statics_in_dir(dir: &Path, family: &str) -> (usize, usize) {
    purge_redundant_statics_in_dir_cancelable(dir, family, None)
}

fn recycle_static_or_test_delete(path: &Path) -> Result<(), String> {
    guard_test_path(path);
    if !path.exists() {
        intact_forget(path);
        return Ok(());
    }
    #[cfg(test)]
    {
        delete_font_file(path)
    }
    #[cfg(not(test))]
    {
        unregister_path(path);
        intact_forget(path);
        gdi_flush_local();
        #[cfg(windows)]
        {
            recycle_bin_windows(path)
        }
        #[cfg(not(windows))]
        {
            fs::remove_file(path).map_err(|e| e.to_string())
        }
    }
}

fn file_is_variable_font(path: &Path) -> bool {
    if let Some(is_var) = crate::scanner::file_has_variations(path) {
        return is_var;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    // Real SFNT with core tables that failed Fontations is never treated as VF by filename.
    if crate::namepatch::has_table(&bytes, b"head") || crate::namepatch::has_table(&bytes, b"name") {
        return false;
    }
    path.file_name()
        .and_then(|s| s.to_str())
        .map(is_variable_face_filename)
        .unwrap_or(false)
}

/// B2/B3 guard: only purge sibling static faces when VF primary activation is safe:
/// - never purge static faces in local-uploaded folders (`FetchIntent::Local`)
/// - never purge when Win32 `nameID 1` mismatches `family` or exceeds `LF_FACESIZE - 1` (31 UTF-16)
/// - never purge when the VF has non-standard/parametric axes outside `wght`/`wdth`/`ital`/`slnt`.
fn vf_primary_purge_safe_for_dir(dir: &Path, family: &str) -> bool {
    if read_download_source(dir) == Some(FetchIntent::Local) {
        return false;
    }
    let trimmed = family.trim();
    if trimmed.is_empty() || trimmed.encode_utf16().count() > 31 {
        return false;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let mut found_safe_vf = false;
    for path in files {
        if !ttf_intact(&path) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        if let Some(axis_tags) = crate::scanner::variation_axis_tags_from_bytes(&bytes) {
            if axis_tags.is_empty() {
                continue;
            }
            const STD_AXES: &[&str] = &["wght", "wdth", "ital", "slnt"];
            if axis_tags.iter().any(|tag| !STD_AXES.contains(&tag.as_str())) {
                return false;
            }
            let info = crate::scanner::analyze_font_file(&path);
            if !info.supported || !info.is_variable {
                return false;
            }
            if let Some(ref tf) = info.typographic_family
                && !tf.trim().eq_ignore_ascii_case(trimmed) {
                    return false;
                }
            let win32_id1 = crate::namepatch::read_name_id(&bytes, 1).or(info.win32_family);
            if let Some(ref w32) = win32_id1 {
                let w32t = w32.trim();
                if w32t.encode_utf16().count() > 31 || !w32t.eq_ignore_ascii_case(trimmed) {
                    return false;
                }
            }
            found_safe_vf = true;
            continue;
        }
        if crate::namepatch::has_table(&bytes, b"head")
            || crate::namepatch::has_table(&bytes, b"name")
        {
            continue;
        }
        let is_var_stub = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(is_variable_face_filename)
            .unwrap_or(false);
        if is_var_stub {
            found_safe_vf = true;
        }
    }
    found_safe_vf
}

/// 1.0.206ad: check cancel between file deletes so mid-dir mouse Cancel can abort.
fn purge_redundant_statics_in_dir_cancelable(
    dir: &Path,
    family: &str,
    cancel: Option<&AtomicBool>,
) -> (usize, usize) {
    if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
        return (0, 0);
    }
    if !dir_vf_primary_ready(dir, family) || !vf_primary_purge_safe_for_dir(dir, family) {
        return (0, 0);
    }
    let mut deleted = 0usize;
    let mut locked = 0usize;
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    for path in files {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            break;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if file_is_variable_font(&path) || is_compat_sidecar_filename(name) {
            continue;
        }
        // Drop gdi-maps stage copy first (hash of Documents path).
        if let Some(dest) = gdi_map_dest_for(&path) {
            unregister_path(&dest);
            let _ = delete_font_file(&dest);
        }
        unregister_path(&path);
        match recycle_static_or_test_delete(&path) {
            Ok(()) => deleted = deleted.saturating_add(1),
            Err(_) => locked = locked.saturating_add(1),
        }
    }
    // Recalc planned / .complete to intact vars only.
    let mut vars = Vec::new();
    let mut again = Vec::new();
    walk_font_files(dir, &mut again);
    for p in again {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if (is_variable_face_filename(name) || file_is_variable_font(&p)) && ttf_intact(&p) {
            vars.push(name.to_string());
        }
    }
    if !vars.is_empty() {
        write_google_planned(dir, &vars);
        let intact = count_intact_planned_keys(dir, &vars);
        if intact >= vars.len() {
            mark_family_complete(dir, vars.len());
        } else {
            write_expected_faces(dir, vars.len());
            clear_complete_marker(dir);
        }
    }
    (deleted, locked)
}

#[allow(dead_code)] // available for Deactivate/Quit hooks; sync walks dirs directly
fn purge_redundant_statics_for_family(app: &AppHandle, family: &str) -> (usize, usize) {
    let mut deleted = 0usize;
    let mut locked = 0usize;
    for dir in family_locations(app, family) {
        let (d, l) = purge_redundant_statics_in_dir(&dir, family);
        deleted = deleted.saturating_add(d);
        locked = locked.saturating_add(l);
    }
    (deleted, locked)
}

/// Vars-only planned keys (after successful VarsOnly Add>0).
/// Test-only: release code commits vars-only plans elsewhere; kept for the unit tests below.
#[cfg(test)]
fn merge_variable_into_planned_keys(existing: &[String], var_files: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for v in var_files {
        if !v.is_empty() && !keys.iter().any(|k| k == v) {
            keys.push(v.clone());
        }
    }
    if !keys.is_empty() {
        return keys;
    }
    for k in existing {
        if !k.is_empty() && !keys.iter().any(|x| x == k) {
            keys.push(k.clone());
        }
    }
    keys
}

/// Pre-Add merge: vars first, **statics kept on disk/plan** until VarsOnly Add>0.
/// Skye P1: skipping static fetch / purging before Add guts AllIntactFallback.
fn merge_variable_into_planned_keys_keep_statics(
    existing: &[String],
    var_files: &[String],
) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for v in var_files {
        if !v.is_empty() && !keys.iter().any(|k| k == v) {
            keys.push(v.clone());
        }
    }
    for k in existing {
        if !k.is_empty() && !keys.iter().any(|x| x == k) {
            keys.push(k.clone());
        }
    }
    keys
}

/// After VarsOnly Add>0: rewrite planned to vars-only and purge redundant statics.
fn commit_vf_primary_after_successful_add(app: &AppHandle, family: &str) -> (usize, usize) {
    let mut deleted = 0usize;
    let mut locked = 0usize;
    for dir in family_locations(app, family) {
        if !dir_vf_primary_ready(&dir, family) || !vf_primary_purge_safe_for_dir(&dir, family) {
            continue;
        }
        // Fast-path: already pure VF with .complete and not flagged .web-build — skip 6 directory walks.
        if fast_session_resume()
            && dir_is_pure_variable_font(&dir)
            && dir_is_complete(&dir)
            && !gfsource::dir_is_web_build(&dir)
        {
            continue;
        }
        // Collect intact vars → planned vars-only, then purge statics.
        let mut vars = Vec::new();
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        for p in &files {
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_variable_face_filename(name) && ttf_intact(p) {
                vars.push(name.to_string());
            }
        }
        if vars.is_empty() {
            continue;
        }
        write_google_planned(&dir, &vars);
        // Clear any transient .web-build marker from auxiliary CSS2 fallback statics BEFORE
        // calling mark_family_complete, because mark_family_complete refuses and deletes
        // .complete if .web-build is still set.
        gfsource::set_web_build(&dir, false);
        let intact = count_intact_planned_keys(&dir, &vars);
        if intact >= vars.len() {
            mark_family_complete(&dir, vars.len());
        } else {
            write_expected_faces(&dir, vars.len());
            clear_complete_marker(&dir);
        }
        let (d, l) = purge_redundant_statics_in_dir(&dir, family);
        deleted = deleted.saturating_add(d);
        locked = locked.saturating_add(l);
        if dir_is_pure_variable_font(&dir) {
            gfsource::set_web_build(&dir, false);
            mark_family_complete(&dir, vars.len());
        }
    }
    (deleted, locked)
}

/// Collect intact Google static instance filenames already on disk (for adopting
/// vars onto a legacy complete folder that lacks a usable `.google-planned`).
fn collect_intact_google_instance_keys(dir: &Path) -> Vec<String> {
    let slug = dir_slug_hint(dir);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let mut out = Vec::new();
    for p in files {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_variable_face_filename(name) || filename_has_latin_subset(name, &slug) {
            continue;
        }
        if parse_google_instance_face_name(&slug, name).is_some() && ttf_intact(&p) {
            out.push(name.to_string());
        }
    }
    out
}

/// When variable TTFs land, fold them into `.google-planned` / expected / complete.
/// **1.0.206u amend (Skye):** planned may list vars first, but **do not purge**
/// statics here — purge only after VarsOnly Add>0 (see register_intact_family_detailed).
/// Happy-path purge-before-register guts AllIntactFallback when VF Add=0.
fn adopt_variable_files_into_plan(root: &Path, var_files: &[String]) {
    if var_files.is_empty() {
        return;
    }
    let existing = read_google_planned_keys(root).unwrap_or_else(|| collect_intact_google_instance_keys(root));
    // Keep statics in planned until successful VF Add; merge lists vars first.
    let keys = merge_variable_into_planned_keys_keep_statics(&existing, var_files);
    if keys.is_empty() {
        return;
    }
    write_google_planned(root, &keys);
    let intact = count_intact_planned_keys(root, &keys);
    if intact >= keys.len() {
        mark_family_complete(root, keys.len());
    } else {
        write_expected_faces(root, keys.len());
        // Honest: planned grew to include vars that are not all intact yet.
        clear_complete_marker(root);
    }
}

fn http_download_client() -> Option<reqwest::blocking::Client> {
    static SHARED_CLIENT: std::sync::LazyLock<Option<reqwest::blocking::Client>> = std::sync::LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // CJK variable TTFs are ~25–52MB over GitHub raw; 120s was tight on slow links.
            .timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(8)
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent(gfsource::POLITE_UA)
            .build()
            .ok()
    });
    SHARED_CLIENT.as_ref().cloned()
}

/// Always pull real `*-variable-*` TTFs for catalog-variable families — including
/// when the folder is already `.complete` / statics-only. `.complete` must **not**
/// block VF fetch. `adopt_variable_files_into_plan` updates planned (statics kept
/// until VarsOnly Add>0). No-public-VF catalog families are skipped (no invent).
/// Returns (var filenames written/intact, HealStats from intact var heals).
fn ensure_catalog_variable_faces(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    family: &str,
) -> (usize, HealStats) {
    if !family_ensures_google_vf(family) {
        return (0, HealStats::default());
    }
    // c258 item 2: full-source statics present → no VF unless preferred (never both).
    if family_locations(app, family).iter().any(|d| dir_static_first_blocks_vf(d)) {
        return (0, HealStats::default());
    }
    let slug = slug_family(family);
    if slug.is_empty() {
        return (0, HealStats::default());
    }
    let Ok(root) = family_dir(app, family) else {
        return (0, HealStats::default());
    };
    if !root.is_dir() {
        let _ = fs::create_dir_all(&root);
    }

    // Fast path: planned already lists intact vars — adopt is a no-op; register only.
    // Dual-VF (Hei/Sung): roman-only planned/intact must still CDN-fetch italic.
    if let Some(keys) = read_google_planned_keys(&root) {
        let planned_vars: Vec<String> = keys
            .iter()
            .filter(|k| is_variable_face_filename(k))
            .cloned()
            .collect();
        let planned_intact = !planned_vars.is_empty()
            && planned_vars.iter().all(|k| ttf_intact(&root.join(k)));
        let dual_needs_italic = family_expects_dual_variable(family)
            && !planned_vars.iter().any(|k| variable_face_filename_is_italic(k))
            && !dir_has_intact_variable_italic(&root);
        if planned_intact && !dual_needs_italic {
            let mut heal = HealStats::default();
            for name in &planned_vars {
                let path = root.join(name);
                let italic = variable_face_filename_is_italic(name);
                heal.add(heal_google_variable_face_file(&path, family, italic));
                let _ = register_family_path(family, &path);
            }
            return (planned_vars.len(), heal);
        }
    } else if dir_has_intact_variable(&root) {
        // Vars on disk but missing from planned (legacy complete) — fold in, no CDN
        // unless dual-VF still missing italic.
        let mut files = Vec::new();
        walk_font_files(&root, &mut files);
        let var_files: Vec<String> = files
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?;
                if is_variable_face_filename(name) && ttf_intact(p) {
                    Some(name.to_string())
                } else {
                    None
                }
            })
            .collect();
        let dual_needs_italic = family_expects_dual_variable(family)
            && !var_files.iter().any(|k| variable_face_filename_is_italic(k));
        if !var_files.is_empty() && !dual_needs_italic {
            adopt_variable_files_into_plan(&root, &var_files);
            let mut heal = HealStats::default();
            for name in &var_files {
                let path = root.join(name);
                let italic = variable_face_filename_is_italic(name);
                heal.add(heal_google_variable_face_file(&path, family, italic));
                let _ = register_family_path(family, &path);
            }
            return (var_files.len(), heal);
        }
        if !var_files.is_empty() && dual_needs_italic {
            // Adopt roman now; fall through to download for italic.
            adopt_variable_files_into_plan(&root, &var_files);
        }
    }

    // Missing vars (complete statics-only Nunito, etc.) — fetch without busting statics.
    let (var_files, heal) = download_google_variable_ttfs(client, family, &slug, &root);
    if var_files.is_empty() {
        // Loud fail: catalog expects a public VF and CDN returned nothing.
        if catalog_variable_expects_public_vf(family)
            && !dir_has_intact_variable(&root)
            && !dir_has_intact(&root)
        {
            let reason = "catalog VF ensure returned 0 (jsDelivr/GitHub raw miss)";
            eprintln!("{family} — {reason}");
            remember_failed(family, reason);
            if let Ok(mut p) = bulk().progress.lock() {
                p.failed = p.failed.saturating_add(1);
            }
        }
        return (0, heal);
    }
    adopt_variable_files_into_plan(&root, &var_files);
    // 1.0.206u: register vars only here; redundant statics purged by adopt when
    // VF-primary. Intact heals already counted in download_google_variable_ttfs.
    for name in &var_files {
        let path = root.join(name);
        if ttf_intact(&path) {
            let _ = register_family_path(family, &path);
        }
    }
    (var_files.len(), heal)
}

/// Cap concurrent face streams so bulk Activate cannot buffer ~N×CJK in RAM.
const FACE_STREAM_SLOTS: usize = 4;
const MAX_IN_FLIGHT_BYTES: u64 = 96 * 1024 * 1024;
const DEFAULT_RESERVE_BYTES: u64 = 8 * 1024 * 1024;

fn face_slots() -> &'static AtomicUsize {
    static S: OnceLock<AtomicUsize> = OnceLock::new();
    S.get_or_init(|| AtomicUsize::new(FACE_STREAM_SLOTS))
}

fn in_flight_bytes() -> &'static AtomicU64 {
    static B: OnceLock<AtomicU64> = OnceLock::new();
    B.get_or_init(|| AtomicU64::new(0))
}

struct FaceStreamPermit {
    reserved: u64,
}

impl FaceStreamPermit {
    /// Unbounded cancel-aware wait. Never give up after ~30s — that silently dropped
    /// faces when DOWNLOAD_WORKERS×face streams contended on FACE_STREAM_SLOTS.
    fn acquire(reserve: u64) -> Option<Self> {
        let reserve = reserve.clamp(256, MAX_IN_FLIGHT_BYTES);
        let slots = face_slots();
        let bytes = in_flight_bytes();
        loop {
            if bulk().cancel.load(Ordering::SeqCst) {
                return None;
            }
            // Slot first.
            loop {
                let cur = slots.load(Ordering::SeqCst);
                if cur == 0 {
                    break;
                }
                if slots
                    .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    // Then byte budget.
                    loop {
                        let used = bytes.load(Ordering::SeqCst);
                        if used.saturating_add(reserve) > MAX_IN_FLIGHT_BYTES {
                            slots.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        if bytes
                            .compare_exchange(used, used + reserve, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            return Some(Self { reserved: reserve });
                        }
                    }
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for FaceStreamPermit {
    fn drop(&mut self) {
        in_flight_bytes().fetch_sub(self.reserved, Ordering::SeqCst);
        face_slots().fetch_add(1, Ordering::SeqCst);
    }
}

/// Outcome of streaming one face URL — carries HTTP status so callers need no second GET.
#[derive(Debug)]
enum StreamFontResult {
    Written,
    AlreadyIntact,
    Cancelled,
    Http(u16),
    /// HTTP 200 but not SFNT (jsDelivr Open Sauce 400/600-italic/900-italic).
    NotFont,
    Failed(String),
}

impl StreamFontResult {
    fn ok(&self) -> bool {
        matches!(self, Self::Written | Self::AlreadyIntact)
    }
}

/// Stream one TTF/OTF URL straight to disk (temp → rename). No full-file RAM buffer.
fn stream_url_to_font_file(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> StreamFontResult {
    if bulk().cancel.load(Ordering::SeqCst) {
        return StreamFontResult::Cancelled;
    }
    if !bust_needed(dest) && ttf_intact(dest) {
        register_path(dest);
        return StreamFontResult::AlreadyIntact;
    }
    // Reserve a slot before opening the body so bulk Activate cannot open dozens of CJK streams.
    // Unbounded wait (cancel-only abort) — never silently drop a face after a timed give-up.
    let Some(_permit) = FaceStreamPermit::acquire(DEFAULT_RESERVE_BYTES) else {
        return StreamFontResult::Cancelled;
    };
    // c258/c260: VF streams share the limiter (≤2, 250 ms, backoff on 403/429/5xx);
    // the slot is held until the body is streamed.
    let (mut resp, _polite) =
        match gfsource::polite_response(client, url, &gfsource::PoliteOpts::default(), &cancel_requested) {
            Ok(r) => r,
            Err(Some(code)) => return StreamFontResult::Http(code),
            Err(None) => return StreamFontResult::Failed("network error".into()),
        };
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = dest.with_extension("part");
    let _ = fs::remove_file(&tmp);
    let Ok(mut file) = fs::File::create(&tmp) else {
        return StreamFontResult::Failed("create part file".into());
    };
    let mut magic = [0u8; 4];
    if resp.read_exact(&mut magic).is_err() || !ttf_magic(&magic) {
        let _ = fs::remove_file(&tmp);
        return StreamFontResult::NotFont;
    }
    if file.write_all(&magic).is_err() {
        let _ = fs::remove_file(&tmp);
        return StreamFontResult::Failed("write magic".into());
    }
    let mut total = 4u64;
    let mut buf = [0u8; 65_536];
    loop {
        if bulk().cancel.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&tmp);
            return StreamFontResult::Cancelled;
        }
        match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if file.write_all(&buf[..n]).is_err() {
                    let _ = fs::remove_file(&tmp);
                    return StreamFontResult::Failed("write body".into());
                }
                total = total.saturating_add(n as u64);
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return StreamFontResult::Failed(err.to_string());
            }
        }
    }
    drop(file);
    if total < 256 {
        let _ = fs::remove_file(&tmp);
        return StreamFontResult::Failed("too small".into());
    }
    if dest.exists() {
        let _ = delete_font_file(dest);
    }
    if fs::rename(&tmp, dest).is_err() {
        if path_still_write_locked(dest) {
            let _ = fs::remove_file(&tmp);
            return StreamFontResult::Failed(
                "files locked — close Word or Adobe, then Retry".into(),
            );
        }
        // Cross-device fallback.
        if fs::copy(&tmp, dest).is_err() {
            let _ = fs::remove_file(&tmp);
            return StreamFontResult::Failed("rename/copy".into());
        }
        let _ = fs::remove_file(&tmp);
    }
    intact_forget(dest);
    if !ttf_intact(dest) {
        let _ = fs::remove_file(dest);
        return StreamFontResult::Failed("intact check".into());
    }
    let _ = ensure_gdi_session_copy(dest);
    register_path(dest);
    StreamFontResult::Written
}

fn bust_needed(dest: &Path) -> bool {
    bulk().bust.load(Ordering::SeqCst) || !dest.exists()
}

/// For catalog-variable families: try real axis-range CSS first. Mozilla/Googlebot
/// yield installable instance TTFs covering the range — never Chrome/Safari WOFF2
/// and never `@fontsource-variable` WOFF. Reject 400-swept listings (some CJK).
fn discover_variable_google_listing(
    client: &reqwest::blocking::Client,
    family: &str,
) -> Option<Vec<(String, String, String)>> {
    let axes = variable_axis_specs(family);
    let uas = [UA_DESKTOP_TTF, UA_GOOGLEBOT];
    let mut best: Option<Vec<(String, String, String)>> = None;
    for ua in uas {
        for axis in &axes {
            if bulk().cancel.load(Ordering::SeqCst) {
                return None;
            }
            let listed = fetch_google_css_listed(client, family, ua, axis);
            if listed.is_empty() || listing_is_400_swept(&listed) {
                continue;
            }
            let take = best
                .as_ref()
                .map(|b| listed.len() > b.len())
                .unwrap_or(true);
            if take {
                best = Some(listed);
            }
            // Prefer first rich variable-axis hit from desktop TTF UA.
            if best.as_ref().map(|b| b.len()).unwrap_or(0) >= 2 && ua == UA_DESKTOP_TTF {
                return best;
            }
        }
        if best.as_ref().map(|b| b.len()).unwrap_or(0) >= 2 {
            return best;
        }
    }
    best
}


// ---- c260 seams ----
/// P1: Repair must take the Activate download path (manifest → repo → css2, with
/// the reject checks) for a cut, css2 web-build or pre-c258 folder — never just
/// reregister the old files.
fn repair_must_download(dirs: &[PathBuf], family: &str) -> bool {
    dirs.iter()
        .any(|d| family_dir_needs_fix_before_load(d, family) || dir_has_undersized_google_static(d, family))
}

/// P1: Scan "missing variable". A static-first full-source folder is complete
/// without a VF (VF is opt-in), so it never nags.
fn dir_missing_variable(dir: &Path, family: &str, has_variable: bool, intact: usize) -> bool {
    catalog_variable_expects_public_vf(family) && !has_variable && intact > 0 && !dir_static_first_blocks_vf(dir)
}

#[derive(Debug, PartialEq, Eq)]
enum RestoreAction {
    Skip,
    Full,
    /// Load the good faces; `cut` faces are refused and the family needs Repair.
    Partial { cut: usize },
}

/// P2: what startup restore does with one family folder.
fn restore_action(dir: &Path, _family: &str) -> RestoreAction {
    if !dir_has_intact(dir) {
        return RestoreAction::Skip;
    }
    if dir_is_complete(dir) {
        return RestoreAction::Full;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let statics: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| !is_variable_face_filename(n))
                .unwrap_or(false)
                && ttf_intact(p)
        })
        .collect();
    let cut = statics.iter().filter(|p| face_is_glyph_outlier(p)).count();
    if cut > 0 && statics.len() > cut {
        RestoreAction::Partial { cut }
    } else {
        RestoreAction::Skip
    }
}

/// P2: UI-data line for a family restored with its cut faces held back.
fn restore_partial_detail(family: &str, cut: usize) -> String {
    format!("{family}: {cut} cut face(s) not loaded — Repair")
}

/// Session families that are not ready but can load their good faces.
fn restore_partial_targets(app: &AppHandle, families: &[String], ready: &[String]) -> Vec<(String, usize)> {
    families
        .iter()
        .filter(|f| !ready.iter().any(|r| r.eq_ignore_ascii_case(f)))
        .filter_map(|f| {
            family_locations(app, f).iter().find_map(|d| match restore_action(d, f) {
                RestoreAction::Partial { cut } => Some((f.clone(), cut)),
                _ => None,
            })
        })
        .collect()
}

/// P2: `%LOCALAPPDATA%`. Under `cfg(test)` this is a per-process temp dir and
/// never the real one (Skye 259: real gdi-maps mtime changed during cargo test).
#[cfg(not(test))]
fn local_app_data() -> Option<PathBuf> {
    if let Some(sb) = sandbox_root() {
        return Some(sb.join("LocalAppData"));
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(test)]
fn local_app_data() -> Option<PathBuf> {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let d = DIR.get_or_init(|| {
        sweep_stale_test_dirs(&std::env::temp_dir());
        let d = std::env::temp_dir().join(format!("fm-test-localappdata-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        d
    });
    guard_test_path(d);
    Some(d.clone())
}

/// c263: test runs left `fm-test-localappdata-<pid>` behind. Remove ones from
/// other (finished) runs older than 10 min. Only touches that prefix under %TEMP%.
#[cfg(test)]
fn sweep_stale_test_dirs(tmp: &Path) {
    sweep_stale_test_dirs_older(tmp, Duration::from_secs(600));
}

#[cfg(test)]
fn sweep_stale_test_dirs_older(tmp: &Path, age: Duration) {
    let mine = format!("fm-test-localappdata-{}", std::process::id());
    let Ok(rd) = fs::read_dir(tmp) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("fm-test-localappdata-") || name == mine {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|a| a > age)
            .unwrap_or(false);
        if old {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// P2: test guard — panics if a path resolves under the real LOCALAPPDATA or
/// the real Documents. No-op outside tests.
#[cfg(test)]
fn guard_test_path(p: &Path) {
    let norm = |x: &Path| x.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    let pn = norm(p);
    let mut real: Vec<PathBuf> = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        real.push(PathBuf::from(l));
    }
    if let Some(u) = std::env::var_os("USERPROFILE") {
        real.push(PathBuf::from(&u).join("Documents"));
        real.push(PathBuf::from(u).join("OneDrive").join("Documents"));
    }
    for r in real {
        let rn = norm(&r);
        // %TEMP% lives under LOCALAPPDATA on Windows; our temp test roots are allowed.
        let tmp = norm(&std::env::temp_dir());
        if pn.starts_with(&rn) && !pn.starts_with(&tmp) {
            panic!("test path resolves under the real user dir {}: {}", r.display(), p.display());
        }
    }
}

#[cfg(not(test))]
#[inline]
fn guard_test_path(_p: &Path) {}


// ---- c260 follow-up: Repair never changes the active set ----
fn repair_files_only_map() -> &'static Mutex<HashMap<String, Vec<PathBuf>>> {
    static M: OnceLock<Mutex<HashMap<String, Vec<PathBuf>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
/// Repair of an inactive family: replace files only (no GDI load, no session join).
fn mark_repair_files_only(family: &str, dirs: &[PathBuf]) {
    if let Ok(mut m) = repair_files_only_map().lock() {
        m.insert(family.to_ascii_lowercase(), dirs.to_vec());
    }
}
fn clear_all_repair_files_only() {
    if let Ok(mut m) = repair_files_only_map().lock() {
        m.clear();
    }
}
fn clear_repair_files_only(family: &str) {
    if let Ok(mut m) = repair_files_only_map().lock() {
        m.remove(&family.to_ascii_lowercase());
    }
}
fn repair_files_only(family: &str) -> bool {
    repair_files_only_map()
        .lock()
        .map(|m| m.contains_key(&family.to_ascii_lowercase()))
        .unwrap_or(false)
}
/// False while `path` sits in a files-only Repair folder.
fn register_allowed_for_path(path: &Path) -> bool {
    let Some(parent) = path.parent() else { return true };
    repair_files_only_map()
        .lock()
        .map(|m| !m.values().flatten().any(|d| d.as_path() == parent))
        .unwrap_or(true)
}
/// After a successful download: join the session unless it was a files-only Repair.
fn session_join_after_download(family: &str) -> bool {
    !repair_files_only(family)
}

// ---- c258 seams: full Google Fonts desktop files ----
use crate::gfsource::{self, FaceShape, ListingSource};
type GListing = Vec<(String, String, String)>;

/// Item 1: download/list manifest → google/fonts repo → css2 (last resort).
#[cfg(test)]
fn google_listing_source_order() -> [ListingSource; 3] {
    gfsource::SOURCE_ORDER
}

#[cfg(test)]
fn choose_google_listing(
    manifest: GListing,
    repo: impl FnOnce() -> GListing,
    css: impl FnOnce() -> GListing,
) -> (GListing, ListingSource) {
    debug_assert_eq!(google_listing_source_order()[2], ListingSource::Css2);
    gfsource::choose_listing(manifest, repo, css)
}

/// c263: the live Google Fonts source (network) behind `gfsource::GoogleFontsSource`.
struct LiveGoogleFonts<'a> {
    client: &'a reqwest::blocking::Client,
}

impl gfsource::GoogleFontsSource for LiveGoogleFonts<'_> {
    fn manifest(&self, family: &str) -> Option<gfsource::Manifest> {
        let revs = load_cached_google_revisions();
        let live_rev = revs.get(&family.trim().to_ascii_lowercase()).map(|s| s.as_str());
        gfsource::fetch_manifest_cached_with_rev(
            self.client,
            family,
            manifest_cache_dir().as_deref(),
            live_rev,
            &cancel_requested,
        )
    }
    fn pascal(&self, family: &str) -> String {
        google_fonts_pascal(family)
    }
    fn repo_listing(&self, family: &str, licence_dir: Option<&str>) -> GListing {
        repo_static_listing(self.client, family, licence_dir)
    }
    fn css2_listing(&self, family: &str) -> GListing {
        discover_css2_google_listing(self.client, family)
    }
}

/// `%LOCALAPPDATA%\Fonths` (same root as gdi-maps).
pub(crate) fn fm_local_root() -> Option<PathBuf> {
    local_app_data().map(|l| l.join("Fonths"))
}

fn manifest_cache_dir() -> Option<PathBuf> {
    fm_local_root().map(|p| p.join("gf-manifests"))
}

/// 1.0.206u VF-Primary default unless the user opts into Static-first
/// (`%LOCALAPPDATA%\Font Manager\prefer-statics` or `FM_STATIC_FIRST=1`).
fn prefer_variable_fonts() -> bool {
    gfsource::prefer_variable(fm_local_root().as_deref())
}

pub fn set_prefer_static_faces(prefer_statics: bool) {
    gfsource::set_prefer_statics(fm_local_root().as_deref(), prefer_statics);
}

fn fast_session_resume() -> bool {
    gfsource::fast_session_resume(fm_local_root().as_deref())
}

pub fn set_fast_session_resume(on: bool) {
    gfsource::set_fast_session_resume(fm_local_root().as_deref(), on);
}

pub fn parse_activation_mode_code(raw: &str) -> u8 {
    match raw.trim().to_ascii_lowercase().as_str() {
        "daemon-tray" | "daemon" | "tray" => 1,
        "session-only" | "temporary" | "session" => 2,
        _ => 0,
    }
}

pub fn set_activation_mode(mode: &str) -> u8 {
    let code = parse_activation_mode_code(mode);
    let label = match code {
        1 => "daemon-tray",
        2 => "session-only",
        _ => "instant-zero-ram",
    };
    if let Some(root) = fm_local_root() {
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(root.join("activation-mode.txt"), label);
    }
    code
}

#[allow(dead_code)]
pub fn get_activation_mode_code() -> u8 {
    fm_local_root()
        .and_then(|root| fs::read_to_string(root.join("activation-mode.txt")).ok())
        .map(|s| parse_activation_mode_code(&s))
        .unwrap_or(0)
}

pub fn sync_hkcu_registry_now() {
    #[cfg(windows)]
    winfont::sync_hkcu_all_active_stages();
}

pub fn clear_hkcu_registry_now() {
    #[cfg(windows)]
    winfont::clear_all_hkcu_session_entries();
}

pub fn activation_mode_label() -> &'static str {
    match get_activation_mode_code() {
        1 => "daemon-tray",
        2 => "session-only",
        _ => "instant-zero-ram",
    }
}

pub fn count_hkcu_session_entries() -> usize {
    #[cfg(windows)]
    {
        winfont::count_hkcu_session_entries()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn prune_stale_hkcu_session_entries(dry_run: bool) -> (usize, usize) {
    #[cfg(windows)]
    {
        winfont::prune_stale_hkcu_session_entries(dry_run)
    }
    #[cfg(not(windows))]
    {
        let _ = dry_run;
        (0, 0)
    }
}

pub fn gdi_maps_dir_stats() -> (usize, u64) {
    let Some(dir) = gdi_maps_root() else {
        return (0, 0);
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return (0, 0);
    };
    let mut count = 0usize;
    let mut bytes = 0u64;
    for ent in rd.flatten() {
        let p = ent.path();
        if let Ok(meta) = fs::symlink_metadata(&p)
            && meta.is_file()
        {
            count += 1;
            bytes = bytes.saturating_add(meta.len());
        }
    }
    (count, bytes)
}

/// Item 2: a folder whose statics came from a full source blocks VF-primary
/// only when the user opts into Static-first (or per-dir `.prefer-statics`).
fn dir_static_first_blocks_vf(dir: &Path) -> bool {
    gfsource::dir_has_full_source(dir) && (!prefer_variable_fonts() || gfsource::dir_prefers_statics(dir))
}

/// Item 3: accept a downloaded face only if it is readable and not short on
/// glyphs or OpenType features vs the face it would replace. css2 never repairs
/// a face unless it is strictly richer (and then the folder stays `.web-build`).
fn accept_face_shapes(cand: Option<&FaceShape>, existing: Option<&FaceShape>, source: ListingSource) -> bool {
    let Some(c) = cand else {
        return false;
    };
    match existing {
        None => true,
        Some(e) if source.is_full() => gfsource::shape_not_worse(c, e),
        Some(e) => gfsource::shape_not_worse(c, e) && c.glyphs > e.glyphs,
    }
}

fn face_shape_path(path: &Path) -> Option<FaceShape> {
    fs::read(path).ok().and_then(|b| gfsource::face_shape(&b))
}

/// True when `dir` holds >= 1 intact `*-variable-*` face and zero non-sidecar static faces.
fn dir_is_pure_variable_font(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let mut has_var = false;
    for p in files {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_compat_sidecar_filename(name) {
            continue;
        }
        if is_variable_face_filename(name) {
            if ttf_intact(&p) {
                has_var = true;
            }
        } else {
            return false;
        }
    }
    has_var
}

/// Item 4: an official Google folder that must be fixed before anything is
/// loaded into Windows: a glyph-outlier (latin cut) face, a css2 web build, or
/// statics with no full-source provenance (pre-c258 css2 downloads).
fn family_dir_needs_fix_before_load(dir: &Path, family: &str) -> bool {
    if !is_official_google_family(family) {
        return false;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let statics: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| !is_variable_face_filename(n) && !is_compat_sidecar_filename(n))
                .unwrap_or(false)
        })
        .collect();
    if statics.is_empty() {
        return false;
    }
    let slug = slug_family(family);
    if is_noto_color_emoji_family(family, &slug)
        && !gfsource::dir_is_web_build(dir)
        && read_google_planned_keys(dir).is_some()
        && !dir_has_undersized_google_static(dir, family)
    {
        return false;
    }
    if gfsource::dir_is_web_build(dir) || !gfsource::dir_has_full_source(dir) {
        return true;
    }
    statics.iter().any(|p| face_is_glyph_outlier(p))
}

/// css2 web builds never stamp `.complete`.
fn dir_may_stamp_complete(dir: &Path) -> bool {
    !gfsource::dir_is_web_build(dir)
}

/// google/fonts repo statics as a listing (jsDelivr URLs; raw is the per-face fallback).
fn repo_static_listing(client: &reqwest::blocking::Client, family: &str, lic: Option<&str>) -> GListing {
    google_fonts_repo_statics_pref(client, family, lic)
        .map(|(lic, folder, st)| {
            st.into_iter()
                .map(|(style, w, f)| (style.to_ascii_lowercase(), w, jsdelivr_google_fonts_url(&lic, &folder, &f)))
                .collect()
        })
        .unwrap_or_default()
}

fn cancel_requested() -> bool {
    bulk().cancel.load(Ordering::SeqCst)
}

/// Item 1: richest full source first — manifest → repo → css2 (last resort).
/// Returns (listing, source, manifest).
fn discover_richest_google_listing(
    client: &reqwest::blocking::Client,
    family: &str,
) -> (GListing, ListingSource, Option<gfsource::Manifest>) {
    if !google_endpoints_allowed(family) || !catalog_download_allowed() {
        return (Vec::new(), ListingSource::Css2, None);
    }
    gfsource::discover_listing(&LiveGoogleFonts { client }, family)
}

/// Keep the licence text from the manifest next to the family.
fn write_manifest_licence(root: &Path, m: &gfsource::Manifest) {
    for (name, body) in gfsource::manifest_license_files(m) {
        let p = root.join(&name);
        if fs::read_to_string(&p).map(|t| t != body).unwrap_or(true) {
            let _ = fs::write(&p, body.as_bytes());
        }
    }
}

/// css2-only: Discover the richest Google CSS listing across UA×axis **before** any face download.
/// Catalog-variable families prefer axis-range CSS (installable TTFs) over static /
/// Fontsource latin packs. Never lets bare family= Regular-400 win over a richer
/// static ital,wght listing when variable axes are unavailable or 400-swept.
fn discover_css2_google_listing(
    client: &reqwest::blocking::Client,
    family: &str,
) -> Vec<(String, String, String)> {
    if family_ensures_google_vf(family)
        && let Some(listed) = discover_variable_google_listing(client, family) {
            // Axis-range CSS sometimes yields `font-weight: 200 1000` → key `200-1000`
            // (2 faces) instead of discrete static instances. Prefer discrete static
            // multi-face listing for planned keys; real var TTFs download separately.
            if !listing_has_only_range_weights(&listed) {
                return listed;
            }
        }
    let static_axis = static_weight_axis();
    // Static ital,wght@0|1,w next. Bare family= last.
    let axes = [
        static_axis.as_str(),
        "",
    ];
    let uas = [UA_DESKTOP_TTF, UA_GOOGLEBOT];
    let mut best: Vec<(String, String, String)> = Vec::new();
    let mut best_axis_rank = -1i32;
    for ua in uas {
        for axis in axes {
            if !catalog_download_allowed() {
                return Vec::new();
            }
            let listed = fetch_google_css_listed(client, family, ua, axis);
            if listed.is_empty() {
                continue;
            }
            if listing_is_400_swept(&listed) && axis.contains("..") {
                continue;
            }
            let rank = axis_richness(axis);
            let richer = listed.len() > best.len()
                || (listed.len() == best.len() && rank > best_axis_rank);
            if richer {
                best = listed;
                best_axis_rank = rank;
            }
            // Short-circuit: Mozilla static axis already returned a usable multi-face set.
            if rank >= 3 && best.len() >= 2 && ua == UA_DESKTOP_TTF {
                return best;
            }
        }
        // After a UA finishes, if we already have a rich static listing, stop sweeping.
        if best_axis_rank >= 3 && best.len() >= 2 {
            return best;
        }
    }
    best
}


/// Parse Google instance on-disk name `{slug}-{weight}-{style}.ttf`.
/// Style is `normal` or `italic`. Skips `*-variable-*` and latin subset names.
fn parse_google_instance_face_name(slug: &str, filename: &str) -> Option<(String, String)> {
    let lower = filename.to_ascii_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
        return None;
    }
    if lower.contains("-variable-") || filename_has_latin_subset(filename, slug) {
        return None;
    }
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    let prefix = format!("{}-", slug.to_ascii_lowercase());
    let rest = stem.strip_prefix(&prefix)?;
    if let Some(w) = rest.strip_suffix("-normal")
        && !w.is_empty() {
            return Some((w.to_string(), "normal".into()));
        }
    if let Some(w) = rest.strip_suffix("-italic")
        && !w.is_empty() {
            return Some((w.to_string(), "italic".into()));
        }
    None
}

/// True when Win nameID 1 or 16 is present and differs from the catalog family
/// (or both are missing). Intact Google faces with mashed "Nunito ExtraLight"
/// family names need an in-place rewrite (statics and vars).
fn instance_name_needs_heal(font: &[u8], family: &str) -> bool {
    let fam = family.trim();
    if fam.is_empty() {
        return false;
    }
    let id1 = crate::namepatch::read_name_id(font, 1);
    let id16 = crate::namepatch::read_name_id(font, 16);
    if let Some(ref s) = id1
        && s != fam {
            return true;
        }
    if let Some(ref s) = id16
        && s != fam {
            return true;
        }
    id1.is_none() && id16.is_none()
}

/// Counts from in-place name heal. Locked/write failures stay soft (no force
/// overwrite) but must be visible to Repair/Activate — never look like success.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HealStats {
    pub healed: usize,
    pub locked: usize,
    pub write_failed: usize,
}

impl HealStats {
    fn add(&mut self, other: Self) {
        self.healed = self.healed.saturating_add(other.healed);
        self.locked = self.locked.saturating_add(other.locked);
        self.write_failed = self.write_failed.saturating_add(other.write_failed);
    }

    fn from_write(result: Result<(), String>) -> Self {
        match result {
            Ok(()) => Self {
                healed: 1,
                ..Default::default()
            },
            Err(e) if e.contains("locked") => Self {
                locked: 1,
                ..Default::default()
            },
            Err(_) => Self {
                write_failed: 1,
                ..Default::default()
            },
        }
    }
}

fn emit_name_heal(app: &AppHandle, stats: HealStats) {
    if stats.locked > 0 || stats.write_failed > 0 || stats.healed > 0 {
        let _ = app.emit("name-heal", stats);
    }
}

/// Map write_font_file outcome after a successful patch attempt.
fn heal_write_stats(path: &Path, patched: &[u8]) -> HealStats {
    HealStats::from_write(write_font_file(path, patched))
}

/// Re-patch one intact Google CSS instance face in place when nameID 1/16 ≠ family.
/// Soft-fails on locked/unreadable files (never panics / force-overwrite) — callers
/// must surface `locked` / `write_failed` instead of treating zero healed as success.
fn heal_google_instance_face_file(
    path: &Path,
    family: &str,
    weight: &str,
    css_style: &str,
) -> HealStats {
    let Ok(bytes) = fs::read(path) else {
        return HealStats::default();
    };
    if !ttf_magic(&bytes) || bytes.len() < 256 {
        return HealStats::default();
    }
    if !instance_name_needs_heal(&bytes, family) {
        return HealStats::default();
    }
    let Some(patched) =
        crate::namepatch::patch_google_instance_face(&bytes, family, weight, css_style)
    else {
        return HealStats::default();
    };
    if patched.as_slice() == bytes.as_slice() {
        return HealStats::default();
    }
    heal_write_stats(path, &patched)
}

/// Re-patch one intact variable TTF in place when nameID 1/16 ≠ catalog family.
/// Style becomes Regular or Italic only; `fvar` is preserved by name-table rewrite.
/// Soft-fails when locked/unreadable — report via HealStats, do not swallow.
fn heal_google_variable_face_file(path: &Path, family: &str, italic: bool) -> HealStats {
    let Ok(bytes) = fs::read(path) else {
        return HealStats::default();
    };
    if !ttf_magic(&bytes) || bytes.len() < 256 {
        return HealStats::default();
    }
    if !instance_name_needs_heal(&bytes, family)
        && !crate::namepatch::variable_name_records_need_heal(&bytes, family)
    {
        return HealStats::default();
    }
    let Some(patched) = crate::namepatch::patch_variable_face(&bytes, family, italic) else {
        return HealStats::default();
    };
    if patched.as_slice() == bytes.as_slice() {
        return HealStats::default();
    }
    heal_write_stats(path, &patched)
}

/// Walk a family folder and heal mashed Google name tables on **instances and
/// vars** (id1/16 ≠ catalog family). Soft-fail per file when locked (Illustrator /
/// fontdrvhost / PID4), but return locked/write_failed counts for fail-loud UI.
/// When `include_vars` is false, skip `*-variable-*` (caller already healed via ensure).
fn heal_google_instance_names_in_dir(dir: &Path, family: &str) -> HealStats {
    heal_google_names_in_dir(dir, family, true)
}

fn heal_google_names_in_dir(dir: &Path, family: &str, include_vars: bool) -> HealStats {
    let slug = slug_family(family);
    if slug.is_empty() || !dir.is_dir() {
        return HealStats::default();
    }
    let mut stats = HealStats::default();
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    for path in files {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !ttf_intact(&path) {
            continue;
        }
        if is_variable_face_filename(name) {
            if !include_vars {
                continue;
            }
            let italic = variable_face_filename_is_italic(name);
            stats.add(heal_google_variable_face_file(&path, family, italic));
            continue;
        }
        let Some((weight, style)) = parse_google_instance_face_name(&slug, name) else {
            continue;
        };
        stats.add(heal_google_instance_face_file(&path, family, &weight, &style));
    }
    stats
}

fn heal_family_google_instance_names(app: &AppHandle, family: &str) -> HealStats {
    heal_family_google_names(app, family, true)
}

/// `include_vars: false` after `ensure_catalog_variable_faces` so var heals are
/// counted once (ensure) and static instances still get healed.
fn heal_family_google_names(app: &AppHandle, family: &str, include_vars: bool) -> HealStats {
    if !is_official_google_family(family) {
        return HealStats::default();
    }
    let mut stats = HealStats::default();
    for dir in family_locations(app, family) {
        stats.add(heal_google_names_in_dir(&dir, family, include_vars));
    }
    stats
}

/// Latin/subset shreds land ~35–60KB (CJK) or ~38KB (Gidugu); full Google TTFs are
/// typically 100KB–MBs. Tight band + allowlist — do **not** flag all official Google
/// 16–96KB faces Incomplete (collateral risk on legitimately small statics).
const TINY_CJK_FACE_MAX_BYTES: u64 = 80 * 1024;
const TINY_CJK_FACE_MIN_BYTES: u64 = 24 * 1024;
const UNDERSIZED_GOOGLE_FACE_MAX_BYTES: u64 = TINY_CJK_FACE_MAX_BYTES;
const UNDERSIZED_GOOGLE_FACE_MIN_BYTES: u64 = TINY_CJK_FACE_MIN_BYTES;

/// Families known to ship full CJK statics that Skye sometimes had replaced by
/// tiny latin-only faces (Chiron / Noto CJK / LXGW).
fn family_may_have_tiny_cjk_statics(family: &str) -> bool {
    let t = family.trim().to_ascii_lowercase();
    t.starts_with("chiron ")
        || t.starts_with("lxgw")
        || t.starts_with("noto sans jp")
        || t.starts_with("noto sans kr")
        || t.starts_with("noto sans hk")
        || t.starts_with("noto sans tc")
        || t.starts_with("noto sans sc")
        || t.starts_with("noto serif jp")
        || t.starts_with("noto serif kr")
        || t.starts_with("noto serif hk")
        || t.starts_with("noto serif tc")
        || t.starts_with("noto serif sc")
        || t == "noto sans japanese"
        || t == "noto serif japanese"
}

/// Non-CJK official Google families known to land latin-subset / undersized remnants
/// (Gidugu ~38KB vs full ~461KB). Allowlist only — never expand to all Google.
fn family_may_have_undersized_google_statics(family: &str) -> bool {
    // Same allowlist as Settled / early-skip (not a second Gidugu hardcode).
    family_known_gdi_session_incapable(family)
}

/// Intact SFNT but tiny → almost certainly a latin-subset / undersized remnant.
fn is_tiny_latin_subset_face(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let len = meta.len();
    (UNDERSIZED_GOOGLE_FACE_MIN_BYTES..=UNDERSIZED_GOOGLE_FACE_MAX_BYTES).contains(&len)
        && ttf_intact(path)
}

fn is_undersized_google_static_face(path: &Path) -> bool {
    is_tiny_latin_subset_face(path)
}

/// Allowlisted official Google face intact but << typical full TTF (Gidugu 38KB vs ~461KB).
/// Bust **this face only** on Repair/download — never whole-library wipe / never all-Google.
fn face_should_replace_undersized_google(family: &str, path: &Path) -> bool {
    is_official_google_family(family)
        && family_may_have_undersized_google_statics(family)
        && is_undersized_google_static_face(path)
}

fn face_should_replace_as_tiny_cjk(family: &str, path: &Path) -> bool {
    // Gidugu allowlist OR CJK allowlist (LXGW may not be in google-directory).
    // Compare-to-upstream still gates the write.
    face_should_replace_undersized_google(family, path)
        || (family_may_have_tiny_cjk_statics(family) && is_tiny_latin_subset_face(path))
        || (is_official_google_family(family) && face_is_glyph_outlier(path))
}

/// `maxp.numGlyphs` from SFNT bytes (TTF/OTF). None for TTC / malformed.
fn sfnt_num_glyphs_bytes(b: &[u8]) -> Option<u16> {
    if b.len() < 12 || &b[0..4] == b"ttcf" {
        return None;
    }
    let n = u16::from_be_bytes([b[4], b[5]]) as usize;
    for i in 0..n {
        let r = 12 + i * 16;
        let rec = b.get(r..r + 16)?;
        if &rec[0..4] == b"maxp" {
            let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
            let g = b.get(off + 4..off + 6)?;
            return Some(u16::from_be_bytes([g[0], g[1]]));
        }
    }
    None
}

type GlyphCountCacheMap = Mutex<HashMap<PathBuf, (u64, u64, Option<u16>)>>;

/// Cached by (mtime, len) — Scan polls, so never re-read unchanged faces.
fn sfnt_num_glyphs_path(path: &Path) -> Option<u16> {
    static C: OnceLock<GlyphCountCacheMap> = OnceLock::new();
    let meta = fs::metadata(path).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = cache.lock()
        && let Some(&(m, l, n)) = g.get(path)
            && m == mtime && l == len {
                return n;
            }
    let n = sfnt_num_glyphs_read(path);
    if let Ok(mut g) = cache.lock() {
        g.insert(path.to_path_buf(), (mtime, len, n));
    }
    n
}

/// Cheap `maxp.numGlyphs` read (table directory + 2 bytes; no full parse).
fn sfnt_num_glyphs_read(path: &Path) -> Option<u16> {
    use std::io::{Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[0..4] == b"ttcf" {
        return None;
    }
    let n = u16::from_be_bytes([head[4], head[5]]) as usize;
    let mut dir = vec![0u8; n.min(64) * 16];
    f.read_exact(&mut dir).ok()?;
    let rec = dir.as_chunks::<16>().0.iter().find(|r| &r[0..4] == b"maxp")?;
    let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as u64;
    f.seek(SeekFrom::Start(off + 4)).ok()?;
    let mut g = [0u8; 2];
    f.read_exact(&mut g).ok()?;
    Some(u16::from_be_bytes(g))
}

/// c253 #5: legacy latin-cut Regular/Bold — a static face with < 50% of the glyphs
/// of the richest static sibling in the same family folder.
fn face_is_glyph_outlier(path: &Path) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    let Some(mine) = sfnt_num_glyphs_path(path) else {
        return false;
    };
    (mine as u32) * 2 < dir_max_static_glyphs(dir) as u32
}

fn dir_max_static_glyphs(dir: &Path) -> u16 {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| !is_variable_face_filename(n))
                .unwrap_or(false)
        })
        .filter_map(|p| sfnt_num_glyphs_path(p))
        .max()
        .unwrap_or(0)
}

fn dir_has_undersized_google_static(dir: &Path, family: &str) -> bool {
    if !is_official_google_family(family) {
        return false;
    }
    let slug = slug_family(family);
    if family_may_have_undersized_google_statics(family) || family_may_have_tiny_cjk_statics(family) {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        for p in files {
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_variable_face_filename(name) || filename_has_latin_subset(name, &slug) {
                continue;
            }
            if face_should_replace_undersized_google(family, &p)
                || (family_may_have_tiny_cjk_statics(family) && is_tiny_latin_subset_face(&p))
            {
                return true;
            }
        }
    }
    dir_has_glyph_outlier_cached(dir, &slug)
}

/// c255 P2: one non-recursive listing → (name, len, mtime) signature. None when the
/// folder has subfolders (nested changes would not show) — caller computes uncached.
fn dir_listing_signature(dir: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut rows: Vec<(String, u64, u128)> = Vec::new();
    for e in fs::read_dir(dir).ok()?.flatten() {
        let md = e.metadata().ok()?;
        if md.is_dir() {
            return None;
        }
        let mt = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        rows.push((e.file_name().to_string_lossy().into_owned(), md.len(), mt));
    }
    rows.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut h);
    Some(h.finish())
}

type OutlierCacheMap = Mutex<HashMap<(PathBuf, String), (u64, bool)>>;

/// c253 #5 glyph-count outlier, cached per (dir, slug) until the listing changes.
fn dir_has_glyph_outlier_cached(dir: &Path, slug: &str) -> bool {
    static C: OnceLock<OutlierCacheMap> = OnceLock::new();
    let sig = dir_listing_signature(dir);
    let key = (dir.to_path_buf(), slug.to_string());
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(sig) = sig
        && let Ok(g) = cache.lock()
            && let Some(&(s, v)) = g.get(&key)
                && s == sig {
                    return v;
                }
    let v = dir_has_glyph_outlier_uncached(dir, slug);
    if let Some(sig) = sig
        && let Ok(mut g) = cache.lock() {
            g.insert(key, (sig, v));
        }
    v
}

fn dir_has_glyph_outlier_uncached(dir: &Path, slug: &str) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let mut counts = Vec::new();
    for p in &files {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_variable_face_filename(name) || filename_has_latin_subset(name, slug) {
            continue;
        }
        if let Some(g) = sfnt_num_glyphs_path(p) {
            counts.push(g as u32);
        }
    }
    let max = counts.iter().copied().max().unwrap_or(0);
    counts.iter().any(|g| g * 2 < max)
}

/// Result of fetching a listing into a folder.
#[derive(Default)]
struct ListedDownload {
    wrote: usize,
    heal: HealStats,
    /// Every listed face ended as a full-source file (manifest/repo) or URL-cache hit.
    all_full: bool,
    /// At least one face is a css2 web build.
    web_build: bool,
}

/// Per-face candidate URLs after the listing URL: google/fonts repo (jsDelivr,
/// raw), then css2 last.
struct FaceFallbacks<'a> {
    client: &'a reqwest::blocking::Client,
    family: &'a str,
    lic: Option<&'a str>,
    repo: Option<Option<downloads::RepoStatics>>,
    css: Option<GListing>,
}

impl FaceFallbacks<'_> {
    fn repo_urls(&mut self, style: &str, weight: &str) -> Vec<String> {
        if self.repo.is_none() {
            self.repo = Some(google_fonts_repo_statics_pref(self.client, self.family, self.lic));
        }
        let Some(Some((lic, folder, st))) = &self.repo else {
            return Vec::new();
        };
        st.iter()
            .find(|(s, w, _)| s.eq_ignore_ascii_case(style) && w == weight)
            .map(|(_, _, f)| google_fonts_variable_cdn_urls(lic, folder, f).to_vec())
            .unwrap_or_default()
    }
    fn css_url(&mut self, style: &str, weight: &str) -> Option<String> {
        if self.css.is_none() {
            self.css = Some(discover_css2_google_listing(self.client, self.family));
        }
        self.css
            .as_ref()?
            .iter()
            .find(|(s, w, _)| s.eq_ignore_ascii_case(style) && w == weight)
            .map(|(_, _, u)| u.clone())
    }
}

/// Fetch listed Google instance TTFs to `root` (c258: manifest/repo first, css2
/// last), patching name tables for Illustrator-friendly family/style split.
/// Every download is checked against the face it would replace (item 3).
fn download_listed_faces_to_dir(
    client: &reqwest::blocking::Client,
    family: &str,
    slug: &str,
    root: &Path,
    listed: Vec<(String, String, String)>,
    source: ListingSource,
    lic: Option<&str>,
) -> ListedDownload {
    let mut out = ListedDownload { all_full: source.is_full(), ..Default::default() };
    if listed.is_empty() {
        out.all_full = false;
        return out;
    }
    let mut fb = FaceFallbacks { client, family, lic, repo: None, css: None };
    for (style, weight, url) in listed {
        if !catalog_download_allowed() {
            out.all_full = false;
            break;
        }
        note_catalog_current(&format!("{family} · {weight} {style}"));
        let name = google_face_filename(slug, &weight, &style);
        let path = root.join(&name);
        let intact = !bulk().bust.load(Ordering::SeqCst) && ttf_intact(&path);
        // Undersized vs full Google (latin/subset remnant): never keep, never a reference.
        let cut = intact && face_should_replace_as_tiny_cjk(family, &path);
        if intact && !cut {
            let cached = gfsource::source_url_for(root, &name).as_deref() == Some(url.as_str());
            if cached || !source.is_full() {
                // URL cache hit (or css2-only listing): keep, re-patch names in place.
                out.heal.add(heal_google_instance_face_file(&path, family, &weight, &style));
                // c263: no per-face Add here — download_family's final register Adds
                // each face once, after sanitize (an early Add of the unsanitized file
                // was dropped and re-Added: 2-3 Adds per face per job).
                out.wrote += 1;
                if !source.is_full() {
                    out.web_build = true;
                }
                continue;
            }
            // Pre-c258 file with no full-source URL: upgrade (only if not worse).
        }
        if cut {
            intact_forget(&path);
            if let Ok(mut p) = bulk().progress.lock() {
                p.current = format!("{family} — undersized vs Google (latin/subset remnant), re-fetching face");
            }
        }
        let reference = if intact && !cut { face_shape_path(&path) } else { None };
        let mut cands: Vec<(String, ListingSource)> = vec![(url.clone(), source)];
        let mut got: Option<(Vec<u8>, String, ListingSource)> = None;
        let mut stage = 0u8; // 0 listing, 1 repo added, 2 css added
        let mut i = 0usize;
        loop {
            if i >= cands.len() {
                if stage == 0 && source.is_full() {
                    stage = 1;
                    for u in fb.repo_urls(&style, &weight) {
                        if !cands.iter().any(|(c, _)| c == &u) {
                            cands.push((u, ListingSource::Repo));
                        }
                    }
                    continue;
                }
                if stage <= 1 && source.is_full() {
                    stage = 2;
                    if let Some(u) = fb.css_url(&style, &weight) {
                        cands.push((u, ListingSource::Css2));
                        continue;
                    }
                }
                break;
            }
            let (u, src) = cands[i].clone();
            i += 1;
            if !catalog_download_allowed() {
                break;
            }
            let Some(bytes) = fetch_url_ttf(client, &u) else {
                continue;
            };
            let shape = gfsource::face_shape(&bytes);
            if accept_face_shapes(shape.as_ref(), reference.as_ref(), src) {
                got = Some((bytes, u, src));
                break;
            }
            eprintln!(
                "{family} {weight} {style}: rejected {} candidate ({:?} glyphs) — short vs on-disk face",
                src.label(),
                shape.map(|s| s.glyphs)
            );
        }
        match got {
            None => {
                if intact && !cut {
                    // Keep the existing face; nothing better was available.
                    // c263: the final register in download_family Adds it once.
                    out.wrote += 1;
                }
                out.all_full = false;
            }
            Some((bytes, u, src)) => {
                let patched = crate::namepatch::patch_google_instance_face(&bytes, family, &weight, &style)
                    .unwrap_or(bytes);
                if intact {
                    intact_forget(&path);
                }
                if write_font_file(&path, &patched).is_ok() {
                    out.wrote += 1;
                    gfsource::record_source_url(root, &name, &u);
                    if !src.is_full() {
                        out.web_build = true;
                        out.all_full = false;
                    }
                } else {
                    out.all_full = false;
                }
            }
        }
    }
    out
}

type FetchedGoogleFaces = (usize, Vec<downloads::StaticFaceSpec>, Vec<String>, HealStats);

/// Google-first install path: discover richest listing, then stream faces to disk.
/// Returns (faces written/intact, listed keys, var filenames, heal from intact heals).
fn fetch_google_family_faces_to_dir(
    client: &reqwest::blocking::Client,
    family: &str,
    slug: &str,
    root: &Path,
) -> FetchedGoogleFaces {
    let official = is_official_google_family(family);
    let ensure_vf = family_ensures_google_vf(family);
    if !catalog_download_allowed() {
        return (0, Vec::new(), Vec::new(), HealStats::default());
    }
    note_catalog_current(&format!("{family} · finding files"));
    // Official Google OR FS-only ensure (42dot → google/fonts TTF). Never empty-return
    // before VF download for ensure families.
    if !official && !ensure_vf {
        return (0, Vec::new(), Vec::new(), HealStats::default());
    }
    let mut heal = HealStats::default();
    // c258: discover the full-source statics first (manifest → repo → css2).
    let (listed, source, manifest) = discover_richest_google_listing(client, family);
    if let Some(m) = &manifest {
        write_manifest_licence(root, m);
    }
    let lic = manifest.as_ref().and_then(gfsource::manifest_license_dir);
    // Item 2: statics first on Windows; VF only when preferred or no full statics.
    let statics_full = !listed.is_empty() && source.is_full();
    let want_vf = ensure_vf && gfsource::use_variable(prefer_variable_fonts(), statics_full);
    let (var_files, var_heal) = if want_vf {
        download_google_variable_ttfs(client, family, slug, root)
    } else {
        (Vec::new(), HealStats::default())
    };
    heal.add(var_heal);
    // 1.0.206u Skye P1: when VF is used, always fetch static instances alongside VF until VarsOnly
    // Add>0 succeeds (register is VF-primary — never both live).
    let dl = if listed.is_empty() {
        ListedDownload::default()
    } else {
        download_listed_faces_to_dir(client, family, slug, root, listed.clone(), source, lic)
    };
    heal.add(dl.heal);
    if dl.all_full && !dl.web_build {
        gfsource::set_full_source(root, source);
        gfsource::set_web_build(root, false);
    } else if !var_files.is_empty() && (dl.wrote == 0 || dir_vf_primary_ready(root, family)) {
        // Real upstream variable TTFs from google/fonts landed intact; do not let auxiliary
        // CSS2 static fallback instances taint this VF-primary folder as .web-build.
        gfsource::clear_full_source(root);
        gfsource::set_web_build(root, false);
    } else {
        gfsource::clear_full_source(root);
        gfsource::set_web_build(root, dl.web_build);
    }
    let wrote = var_files.len().saturating_add(dl.wrote);
    (wrote, listed, var_files, heal)
}

fn needs_compat_pack(slug: &str) -> bool {
    slug.contains("emoji") || slug.contains("color")
}

fn install_compat_pack(client: &reqwest::blocking::Client, root: &Path, family: &str, slug: &str) {
    if cancel_requested() {
        return;
    }
    if let Some(bytes) = fetch_google_css_font(client, family, UA_SAFARI, "") {
        let path = root.join(format!("{slug}-svg.otf"));
        if !ttf_intact(&path) {
            let _ = write_font_file(&path, &bytes);
        }
    }
    if cancel_requested() {
        return;
    }
    if let Some(bytes) = fetch_google_css_font(client, family, UA_CHROME, "") {
        let path = root.join(format!("{slug}-colrv1.ttf"));
        if !ttf_intact(&path) {
            let _ = write_font_file(&path, &bytes);
        }
    }
    if cancel_requested() {
        return;
    }
    let dest = root.join(format!("{slug}-compat-outline.ttf"));
    if ttf_intact(&dest) {
        return;
    }
    // For emoji families, prefer a static outline Regular TTF from Google CSS2 first
    // (no fvar, pure glyf/loca tables — always accepted by Windows GDI AddFontResourceExW),
    // falling back to upstream fetch_ttf when offline/unavailable.
    let outline_bytes = if slug.contains("emoji") {
        fetch_google_css_font(client, "Noto Emoji", UA_DESKTOP_TTF, "wght@400")
            .or_else(|| fetch_ttf(client, "noto-emoji", "", 400, false, "latin").ok())
    } else {
        fetch_ttf(client, slug, "", 400, false, "latin").ok()
    };
    if let Some(bytes) = outline_bytes {
        let patched = crate::namepatch::patch_family_name(&bytes, family).unwrap_or(bytes);
        let _ = write_font_file(&dest, &patched);
    }
}

fn is_emoji_session_family(family: &str) -> bool {
    // Single source with `SOFT_GDI_TRY_ADD_FIRST` (mirrored in TS).
    soft_gdi_try_add_entry(family).is_some()
}

/// Noto Color Emoji only — multi-MB upstream color TTF; never Google CSS latin stubs.
fn is_noto_color_emoji_family(family: &str, slug: &str) -> bool {
    slug == "noto-color-emoji" || family.eq_ignore_ascii_case("Noto Color Emoji")
}

/// Full color-capable TTF from noto-emoji upstream (not CSS unicode-range WOFF2,
/// not latin-only stubs). Returns bytes written/intact count for the primary face.
fn pull_emoji_upstream_color_ttf(
    client: &reqwest::blocking::Client,
    family: &str,
    slug: &str,
    root: &Path,
) -> usize {
    if !is_emoji_session_family(family) && slug != "noto-color-emoji" && slug != "noto-emoji" {
        return 0;
    }
    let urls = ttf_urls(slug, "", 400, false, "emoji", 0);
    if urls.is_empty() {
        return 0;
    }
    let dest_name = if slug == "noto-color-emoji" || family.eq_ignore_ascii_case("Noto Color Emoji")
    {
        format!("{slug}.ttf")
    } else {
        format!("{slug}-400-normal.ttf")
    };
    let dest = root.join(&dest_name);
    let emoji_stub_gate = slug == "noto-color-emoji"
        || slug == "noto-emoji"
        || is_emoji_session_family(family);
    if ttf_intact(&dest) {
        // Never treat a tiny latin stub as a working emoji face (color + outline).
        if let Ok(meta) = fs::metadata(&dest) {
            if meta.len() < 256 * 1024 && emoji_stub_gate {
                let _ = delete_font_file(&dest);
            } else if slug == "noto-color-emoji" {
                // If on-disk noto-color-emoji lacks glyf/loca outline tables (the CBDT-only file that
                // Windows GDI rejects), upgrade it to the WindowsCompatible version so GDI Add succeeds.
                let needs_compat_upgrade = fs::read(&dest)
                    .map(|b| !crate::namepatch::has_table(&b, b"glyf"))
                    .unwrap_or(false);
                if needs_compat_upgrade {
                    let _ = delete_font_file(&dest);
                } else {
                    return 1;
                }
            } else {
                return 1;
            }
        } else {
            return 1;
        }
    }
    let want_glyf = slug == "noto-color-emoji" || family.eq_ignore_ascii_case("Noto Color Emoji");
    let mut fallback_cbdt: Option<Vec<u8>> = None;
    for url in &urls {
        if cancel_requested() {
            return 0;
        }
        if let Some(bytes) = fetch_url_ttf(client, url) {
            // Soft emoji TTFs are large; reject obvious latin/CSS stubs (≥256KB gate).
            if emoji_stub_gate && bytes.len() < 256 * 1024 {
                continue;
            }
            // For Noto Color Emoji, keep trying remaining WindowsCompatible mirrors if a
            // fallback URL returned a CBDT-only (no glyf) file that Windows GDI refuses.
            if want_glyf && !crate::namepatch::has_table(&bytes, b"glyf") {
                if fallback_cbdt.is_none() {
                    fallback_cbdt = Some(bytes);
                }
                continue;
            }
            if write_font_file(&dest, &bytes).is_ok() && ttf_intact(&dest) {
                return 1;
            }
        }
    }
    if let Some(bytes) = fallback_cbdt
        && write_font_file(&dest, &bytes).is_ok() && ttf_intact(&dest) {
            return 1;
        }
    0
}

/// Register intact faces. For catalog-variable families, register `*-variable-*`
/// first so Illustrator/AI can pick axes. 1.0.206u: statics are skipped/purged
/// when VF-primary — sort still puts vars ahead for mixed folders / fallback.
fn sort_faces_var_first(files: &mut [PathBuf]) {
    files.sort_by(|a, b| {
        let av = a
            .file_name()
            .and_then(|s| s.to_str())
            .map(is_variable_face_filename)
            .unwrap_or(false);
        let bv = b
            .file_name()
            .and_then(|s| s.to_str())
            .map(is_variable_face_filename)
            .unwrap_or(false);
        match (av, bv) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        }
    });
}

/// Split ready-on-disk into Add queue vs known-incapable Settled (never queue Add).
/// `family_is_ready` may stamp `.complete` for allowlisted faces; early-skip then
/// drops them from the restore Add queue (1.0.190).
fn partition_session_restore_targets(
    app: &AppHandle,
    ready: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut targets = Vec::new();
    let mut settled = Vec::new();
    let mut seen = HashSet::new();
    for family in ready {
        let trimmed = family.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_ascii_lowercase()) {
            continue;
        }
        if family_early_skip_known_incapable(app, trimmed) {
            settled.push(trimmed.to_string());
        } else {
            targets.push(trimmed.to_string());
        }
    }
    (targets, settled)
}

/// Disk-only ready check, parallel across families (no GDI). Used on boot and
/// `plan_google_activation` so a 2,000-family session does not serially stat
/// `.complete` on the invoke thread.
fn filter_ready_families_parallel(app: &AppHandle, families: &[String]) -> Vec<String> {
    let trimmed = dedupe_family_names(families);
    if trimmed.len() <= 4 {
        return trimmed
            .into_iter()
            .filter(|family| {
                family_skip_register_this_process(app, family).is_some()
                    || family_is_ready(app, family)
            })
            .collect();
    }
    trimmed
        .into_par_iter()
        .filter(|family| {
            family_skip_register_this_process(app, family).is_some()
                || family_is_ready(app, family)
        })
        .collect()
}

/// Bounded-parallel session register. `register` already skips paths in the
/// in-process loaded set (no double-Add). Workers share AppHandle; family walk /
/// file I/O stay parallel, but GDI Add/Remove is serialized on `winfont::gdi_api`
/// so Adds never overlap across families (Skye P1).
#[allow(dead_code)] // session_begin Windows path
fn register_ready_families_parallel(app: &AppHandle, ready_targets: &[String]) -> (usize, Vec<String>) {
    if ready_targets.is_empty() {
        return (0, Vec::new());
    }
    let workers = session_register_workers(ready_targets.len());
    if workers <= 1 || ready_targets.len() <= 1 {
        let mut files = 0usize;
        let mut ready = Vec::new();
        for family in ready_targets {
            let k = register_intact_family(app, family);
            if k > 0 {
                files += k;
                ready.push(family.clone());
                session_boot_push_ready(family);
            }
        }
        return (files, ready);
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(ready_targets.to_vec())));
    let results: Arc<Mutex<Vec<(String, usize)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut joins = Vec::with_capacity(workers);
    for _ in 0..workers {
        let app = app.clone();
        let queue = queue.clone();
        let results = results.clone();
        joins.push(thread::spawn(move || {
            loop {
                let next = queue
                    .lock()
                    .ok()
                    .and_then(|mut q| q.pop_front());
                let Some(family) = next else {
                    break;
                };
                let k = register_intact_family(&app, &family);
                if k > 0 {
                    session_boot_push_ready(&family);
                    if let Ok(mut g) = results.lock() {
                        g.push((family, k));
                    }
                }
            }
        }));
    }
    for j in joins {
        let _ = j.join();
    }
    let pairs = results.lock().map(|g| g.clone()).unwrap_or_default();
    let mut files = 0usize;
    let mut ready = Vec::with_capacity(pairs.len());
    for (family, k) in pairs {
        files += k;
        ready.push(family);
    }
    // Stable-ish order: match session list order when possible.
    ready.sort_by_key(|a| {
        ready_targets
            .iter()
            .position(|t| t.eq_ignore_ascii_case(a))
            .unwrap_or(usize::MAX)
    });
    (files, ready)
}

fn register_intact_family(app: &AppHandle, family: &str) -> usize {
    // Same as detailed: skip-Add only when already in loaded() this process.
    register_intact_family_detailed(app, family).0
}

fn register_intact_new(app: &AppHandle, family: &str) -> usize {
    // Same VF-primary gate as register_intact_family_detailed (session restore path).
    register_intact_family_detailed(app, family).0
}

/// Unload + drop gdi-maps copy, then stage+Add again. Does not touch Documents library files.
fn reregister_intact_family(app: &AppHandle, family: &str) -> (usize, RegisterFailKind) {
    #[cfg(windows)]
    winfont::invalidate_family(family);
    // Force re-stage: unload + drop gdi-maps copy, then stage+Add. Library files untouched.
    for dir in family_locations(app, family) {
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        for path in files {
            if ttf_intact(&path) {
                unregister_path(&path);
            }
        }
    }
    gdi_flush_local();
    register_intact_family_detailed(app, family)
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct FamilyDiskHonesty {
    intact: usize,
    expected: Option<usize>,
    has_complete: bool,
    has_variable: bool,
    missing_variable: bool,
    undersized: bool,
}

fn family_disk_honesty_in(dir: &Path, family: &str) -> FamilyDiskHonesty {
    // Clear Sans: kill sticky Fontsource `.expected=10` before honesty/verify.
    heal_clear_sans_expected_plan(dir, family);
    verify_complete_marker(dir);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let intact = files.iter().filter(|p| ttf_intact(p)).count();
    let has_complete = dir_is_complete(dir);
    let has_variable = dir_has_intact_variable(dir);
    let missing_variable = dir_missing_variable(dir, family, has_variable, intact);
    let undersized = dir_has_undersized_google_static(dir, family);
    FamilyDiskHonesty {
        intact,
        expected: read_expected_faces(dir),
        has_complete,
        has_variable,
        missing_variable,
        undersized,
    }
}

fn family_disk_honesty(app: &AppHandle, family: &str) -> FamilyDiskHonesty {
    let mut best = FamilyDiskHonesty {
        intact: 0,
        expected: None,
        has_complete: false,
        has_variable: false,
        missing_variable: false,
        undersized: false,
    };
    for dir in family_locations(app, family) {
        let h = family_disk_honesty_in(&dir, family);
        if h.intact >= best.intact {
            best = h;
        }
    }
    best
}

/// Never claim `.complete` unless the marker exists. Split files / stamp / GDI cause.
#[allow(dead_code)]
fn format_register_zero_detail(app: &AppHandle, family: &str) -> String {
    format_register_zero_detail_with(app, family, RegisterFailKind::AddReturnedZero)
}

fn format_register_zero_detail_with(
    app: &AppHandle,
    family: &str,
    cause: RegisterFailKind,
) -> String {
    let h = family_disk_honesty(app, family);
    let expected = h
        .expected
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".into());
    // Re-check marker — never claim .complete when absent (Skye P0 toast).
    let complete = if h.has_complete { "yes" } else { "no" };
    let mut msg = format!(
        "{family} — files on disk {intact}/{expected}, .complete={complete}, GDI live 0 ({cause})",
        intact = h.intact,
        cause = cause.label(),
    );
    if h.missing_variable {
        msg.push_str(", catalog VF missing");
    }
    if h.undersized {
        msg.push_str(", undersized vs Google (latin/subset remnant)");
    }
    if family_known_gdi_session_incapable(family) {
        // Official TTF still Add=0 — honest refuse, not a download/size bug.
        msg.push_str(", Windows refused this face (known GDI-incapable for session install)");
    } else if matches!(cause, RegisterFailKind::AddReturnedZero) {
        msg.push_str(", Windows refused this face");
    }
    if matches!(cause, RegisterFailKind::AddReturnedZero) {
        'dw: for dir in family_locations(app, family) {
            let mut files = Vec::new();
            walk_font_files(&dir, &mut files);
            for path in files {
                if !ttf_intact(&path) {
                    continue;
                }
                let info = crate::scanner::analyze_font_file(&path);
                if let Some(reason) = info.error_reason.filter(|s| !s.is_empty()) {
                    msg.push_str(&format!("; Fontations: {reason}"));
                } else if info.supported {
                    msg.push_str(&format!(
                        "; Fontations supports ({}, faces={}, var={})",
                        info.face_type, info.face_count, info.is_variable
                    ));
                }
                break 'dw;
            }
        }
    }
    msg
}

/// Ready-path GDI 0 must not leave sticky `.complete` (Activate without bust).
fn note_register_zero(app: &AppHandle, family: &str, cause: RegisterFailKind) -> String {
    clear_complete_markers_for_family(app, family);
    format_register_zero_detail_with(app, family, cause)
}

fn alias_keys(name: &str) -> Vec<String> {
    let raw = name.trim();
    // Non-ASCII names: no lossy keys — `sanitize("思源 Sans")` is "sans", which is
    // Latin Sans's folder, and every all-non-ASCII name sanitizes to "font".
    let mut keys = if family_name_is_non_ascii(raw) {
        vec![raw.to_lowercase(), unicode_family_slug(raw)]
    } else {
        vec![
            raw.to_lowercase(),
            slug_family(raw),
            sanitize(raw).to_lowercase(),
        ]
    };
    keys.sort();
    keys.dedup();
    keys.retain(|k| !k.is_empty());
    keys
}

struct DiskIndex {
    by_key: HashMap<String, Vec<PathBuf>>,
    names: Vec<String>,
}

fn index_disk(app: &AppHandle, gc: bool) -> DiskIndex {
    let mut by_key: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut names = Vec::new();
    for_family_dirs(app, |path| {
        let name = dir_family_name(path);
        let mut files = Vec::new();
        walk_font_files(path, &mut files);
        let (intact, other): (Vec<PathBuf>, Vec<PathBuf>) =
            files.into_iter().partition(|file| ttf_intact(file));
        if gc {
            boot_tidy_family_dir(path, &other, !intact.is_empty());
        }
        if intact.is_empty() {
            return;
        }
        names.push(name.clone());
        for key in alias_keys(&name) {
            by_key.entry(key).or_default().extend(intact.iter().cloned());
        }
    });
    names.sort_by_key(|a| a.to_lowercase());
    names.dedup();
    DiskIndex { by_key, names }
}

/// Card 36: startup tidy never deletes a font file and never removes a folder that
/// still holds anything. Non-SFNT files (WOFF/WOFF2 uploads, truncated leftovers) are
/// logged and left; a folder with no SFNT face only loses its bookkeeping markers and
/// is removed with `remove_dir` — which fails (and leaves it) unless it is empty.
fn boot_tidy_family_dir(path: &Path, non_intact: &[PathBuf], has_intact: bool) {
    for file in non_intact {
        eprintln!("[fm] startup: left non-installable file {}", file.display());
    }
    if has_intact {
        return;
    }
    for marker in [".complete", ".expected", ".google-planned", ".fontsource-version"] {
        let _ = fs::remove_file(path.join(marker));
    }
    if fs::remove_dir(path).is_err() && path.exists() {
        eprintln!("[fm] startup: left folder with no installable face {}", path.display());
    }
}

fn build_disk_index(app: &AppHandle) -> DiskIndex {
    index_disk(app, false)
}

fn family_has_intact(app: &AppHandle, family: &str) -> bool {
    family_locations(app, family).iter().any(|dir| dir_has_intact(dir))
}

fn split_ready_missing(
    app: &AppHandle,
    families: Vec<String>,
    bust: bool,
) -> (Vec<String>, Vec<String>, Option<DiskIndex>) {
    // Targeted path checks only. A full Documents walk of 2,000+ folders made
    // Activate look like a download even when every family was already there.
    let ready = if bust {
        Vec::new()
    } else {
        filter_ready_families_parallel(app, &families)
    };
    let ready_set: HashSet<String> = ready.iter().map(|n| n.trim().to_ascii_lowercase()).collect();
    let missing: Vec<String> = families
        .into_iter()
        .filter(|family| {
            let t = family.trim();
            !t.is_empty() && !ready_set.contains(&t.to_ascii_lowercase())
        })
        .collect();
    (ready, missing, None)
}

#[allow(dead_code)]
fn commit_ready_families(app: &AppHandle, ready: &[String], index: Option<&DiskIndex>) {
    commit_ready_families_at(app, ready, index, 0);
}

fn commit_ready_families_at(
    app: &AppHandle,
    ready: &[String],
    index: Option<&DiskIndex>,
    done_base: usize,
) {
    if ready.is_empty() {
        return;
    }
    // Parallel walk + skip-Add (this-process loaded). Index bind is unused here —
    // register_intact_family_detailed already skip-Adds live faces. GDI stays serialized.
    let _ = index;
    let _hkcu_batch = HkcuBatchGuard::new();
    let live = register_on_disk_parallel_progress(app, ready, done_base);
    drop(_hkcu_batch);
    if !live.is_empty() {
        notify_fonts_changed();
        session_add(app, &live);
        #[cfg(windows)]
        persist_activation_sidecars(app);
    }
    emit_progress(app);
    // Idle: VF ensure + name-heal (Repair remains sync smoke path).
    let app_idle = app.clone();
    let ready_idle = ready.to_vec();
    thread::spawn(move || {
        let client = http_download_client();
        let mut heal = HealStats::default();
        for family in &ready_idle {
            if bulk().cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Some(ref c) = client {
                let (_, eh) = ensure_catalog_variable_faces(&app_idle, c, family);
                heal.add(eh);
                heal.add(heal_family_google_names(&app_idle, family, false));
            } else {
                heal.add(heal_family_google_instance_names(&app_idle, family));
            }
        }
        emit_name_heal(&app_idle, heal);
    });
}

#[allow(dead_code)] // kept: DiskIndex bind path; Activate All uses parallel register_intact
fn register_from_index(app: &AppHandle, index: &DiskIndex, family: &str) -> usize {
    let mut n = 0usize;
    let mut seen = HashSet::new();
    for key in alias_keys(family) {
        if let Some(paths) = index.by_key.get(&key) {
            for path in paths {
                if seen.insert(path.clone()) && register_family_path(family, path) {
                    n += 1;
                }
            }
        }
    }
    if n == 0 && seen.is_empty() {
        n = register_intact_new(app, family);
    }
    n
}

fn family_complete_marker(dir: &Path) -> PathBuf {
    dir.join(".complete")
}

fn family_expected_marker(dir: &Path) -> PathBuf {
    dir.join(".expected")
}

/// c253 #1: marker writes inside the watched Documents root must be no-ops when
/// unchanged — otherwise every rescan re-stamps, fires the fs watch, and rescans.
fn write_marker_if_changed(path: &Path, body: &str) {
    if fs::read(path).map(|b| b == body.as_bytes()).unwrap_or(false) {
        return;
    }
    let _ = fs::write(path, body.as_bytes());
}

fn write_expected_faces(dir: &Path, expected: usize) {
    write_marker_if_changed(&family_expected_marker(dir), &expected.to_string());
}

fn read_expected_faces(dir: &Path) -> Option<usize> {
    if let Ok(s) = fs::read_to_string(family_expected_marker(dir))
        && let Ok(n) = s.trim().parse::<usize>()
            && n > 0 {
                return Some(n);
            }
    // New stamps store the expected count in `.complete` itself.
    if let Ok(s) = fs::read_to_string(family_complete_marker(dir))
        && let Ok(n) = s.trim().parse::<usize>()
            && n > 1 {
                // Trust multi-face counts without a sidecar. Legacy stamps were bare "1".
                return Some(n);
            }
    None
}

/// Google on-disk face key: `{slug}-{weight}-{style}.ttf` (no Fontsource subset token).
fn google_face_filename(slug: &str, weight: &str, style: &str) -> String {
    sanitize(&format!("{slug}-{weight}-{style}.ttf"))
}

/// True when a file name embeds a Fontsource `latin` subset token **after** the
/// family slug. Raw `contains("-latin-")` false-positives on slugs that embed
/// the word (e.g. `m-plus-code-latin`, `anek-latin`).
fn filename_has_latin_subset(name: &str, slug: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let slug = slug.trim().to_ascii_lowercase();
    if slug.is_empty() {
        return false;
    }
    let stem = lower
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(lower.as_str());
    let Some(rest) = stem
        .strip_prefix(slug.as_str())
        .and_then(|s| s.strip_prefix('-'))
    else {
        return false;
    };
    // Subset segment after slug only: `{slug}-latin-…`, `{slug}-japanese-latin-…`,
    // or edge `{slug}-latin-latin-…`. Never match "latin" inside the slug itself.
    rest.split('-').any(|seg| seg == "latin")
}

/// Fontsource on-disk name. Never includes `latin` in the filename — even when the
/// CDN subset is latin / japanese-latin. Non-latin script subsets may keep their
/// subset token (`chinese-hongkong`, …) so CJK packs do not collide.
fn fontsource_face_filename(slug: &str, subset: &str, weight: u16, style: &str) -> String {
    let sub = subset.trim().to_ascii_lowercase();
    if sub.is_empty()
        || sub == "latin"
        || sub.starts_with("latin-")
        || sub.contains("latin")
        || filename_has_latin_subset(&format!("{slug}-{sub}-{weight}-{style}.ttf"), slug)
    {
        return google_face_filename(slug, &weight.to_string(), style);
    }
    let name = sanitize(&format!("{slug}-{subset}-{weight}-{style}.ttf"));
    debug_assert!(
        !filename_has_latin_subset(&name, slug),
        "fontsource_face_filename must never emit latin-named files"
    );
    name
}

/// 1.0.188: For known-GDI-incapable folders, delete Fontsource/latin orphans that are
/// not the intact full Google face — especially undersized `*-400-normal.ttf` written by
/// legacy `try_fontsource_gdi_offer` (subset label "latin" → google-shaped dest). Keeps
/// full-size Google TTFs so early-skip / Settled can succeed.
fn purge_known_incapable_fontsource_remnants(dir: &Path, family: &str) -> usize {
    if !family_known_gdi_session_incapable(family) {
        return 0;
    }
    let slug = slug_family(family);
    if slug.is_empty() {
        return 0;
    }
    let subsets = fontsource_gdi_offer_subsets(family);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    let mut removed = 0usize;
    for path in files {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_variable_face_filename(name) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let latin_named = filename_has_latin_subset(name, &slug);
        // FS offer dest collision: google-shaped name + undersized bytes.
        let undersized_collision =
            is_undersized_google_static_face(&path) && is_google_face_key(name, &slug);
        // Non-latin subset packs from allowlist plan (`gidugu-telugu-…`).
        let subset_named = subsets.iter().any(|sub| {
            let sub = sub.trim().to_ascii_lowercase();
            !sub.is_empty()
                && sub != "latin"
                && !sub.starts_with("latin-")
                && lower.starts_with(&format!("{slug}-{sub}-"))
        });
        if !(latin_named || undersized_collision || subset_named) {
            continue;
        }
        if delete_font_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

/// Strip any leftover `*-latin-*` files (legacy Fontsource packs).
fn purge_latin_named_files(dir: &Path) {
    let slug = dir_slug_hint(dir);
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if filename_has_latin_subset(name, &slug) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// True when `file_name` matches a Google face key for `slug`.
/// Fontsource names embed a subset (`{slug}-latin-400-normal.ttf`) — never count those
/// toward Google planned / `.complete`.
fn is_google_face_key(file_name: &str, slug: &str) -> bool {
    let stem = match file_name.rsplit_once('.') {
        Some((s, ext)) if ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf") => s,
        _ => return false,
    };
    let Some(rest) = stem
        .strip_prefix(slug)
        .and_then(|s| s.strip_prefix('-'))
    else {
        return false;
    };
    // Explicit: never count Fontsource latin subset packs toward Google planned.
    if filename_has_latin_subset(file_name, slug) {
        return false;
    }
    let mut parts: Vec<&str> = rest.split('-').collect();
    let Some(style) = parts.pop() else {
        return false;
    };
    if style != "normal" && style != "italic" {
        return false;
    }
    if parts.is_empty() {
        return false;
    }
    // Weight tokens are digits only (`400` or range `100-900`). Letter tokens = subset.
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn dir_slug_hint(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .map(slug_family)
        .unwrap_or_default()
}

fn family_google_planned_marker(dir: &Path) -> PathBuf {
    dir.join(".google-planned")
}

/// Persist the exact Google face-key list (one filename per line) so verify can
/// match stamp — never a bare count that accepts any google-shaped / latin pad.
fn write_google_planned(dir: &Path, keys: &[String]) {
    if keys.is_empty() {
        let _ = fs::remove_file(family_google_planned_marker(dir));
        return;
    }
    let body = keys.join("\n");
    let _ = fs::write(family_google_planned_marker(dir), body.as_bytes());
}

fn clear_google_planned(dir: &Path) {
    let _ = fs::remove_file(family_google_planned_marker(dir));
}

/// After Google lists face keys, drop **all** leftovers not in that exact key list:
/// Fontsource `*-latin-*`, other subset packs, and duplicate google-shaped files
/// from prior static/partial installs. Never keep a `*-latin-*` name even if it
/// somehow appears in `planned_keys` (latin must never be a Google face key).
/// Compat emoji/color sidecars (`-svg.`, `-colrv1.`, `-compat-`) are retained.
fn purge_unplanned_font_files(dir: &Path, planned_keys: &[String]) {
    if planned_keys.is_empty() {
        return;
    }
    let slug = dir_slug_hint(dir);
    let planned: HashSet<&str> = planned_keys
        .iter()
        .map(|s| s.as_str())
        .filter(|n| {
            // Belt: planned keys must never include Fontsource latin subset names.
            // Slug-aware — do not drop real Google faces for families like
            // m-plus-code-latin / anek-latin.
            !filename_has_latin_subset(n, &slug)
        })
        .collect();
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    for path in files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        // Keep emoji/color compat sidecars. Variable TTFs stay only when planned
        // (do not keep forever-unplanned `*-variable-*` leftovers).
        if lower.contains("-svg.")
            || lower.contains("-colrv1.")
            || lower.contains("-compat-")
        {
            continue;
        }
        // Always strip Fontsource latin subset packs on Google re-download.
        let is_latin = filename_has_latin_subset(name, &slug);
        if !is_latin && planned.contains(name) {
            continue;
        }
        unregister_path(&path);
        intact_forget(&path);
        let _ = delete_font_file(&path);
    }
}

/// Read planned Google face keys from `.google-planned`. Rejects legacy bare-count
/// bodies (`"18"`) — those cannot prove stamp/verify key parity.
fn read_google_planned_keys(dir: &Path) -> Option<Vec<String>> {
    let s = fs::read_to_string(family_google_planned_marker(dir)).ok()?;
    let keys: Vec<String> = s
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect();
    if keys.is_empty() {
        return None;
    }
    // Legacy body was a single integer count — not a key list.
    if keys.len() == 1 && keys[0].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(keys)
}

fn count_intact_google_face_keys(dir: &Path, slug: &str) -> usize {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| is_google_face_key(n, slug) && ttf_intact(p))
                .unwrap_or(false)
        })
        .count()
}

fn count_intact_google_listed_keys(
    dir: &Path,
    slug: &str,
    listed: &[(String, String, String)],
) -> usize {
    listed
        .iter()
        .filter(|(style, weight, _)| {
            ttf_intact(&dir.join(google_face_filename(slug, weight, style)))
        })
        .count()
}

fn count_intact_planned_keys(dir: &Path, keys: &[String]) -> usize {
    keys.iter().filter(|k| ttf_intact(&dir.join(k))).count()
}

/// Faces that count toward `.complete`. When Google planned the set, only the
/// listed keys in `.google-planned` count — never any-google-shaped or latin pad.
fn count_intact_toward_expected(dir: &Path) -> usize {
    if family_google_planned_marker(dir).is_file() {
        if let Some(keys) = read_google_planned_keys(dir) {
            return count_intact_planned_keys(dir, &keys);
        }
        // Marker present but body unreadable / legacy bare count: do not accept
        // latin padding or arbitrary google-shaped files as satisfying the plan.
        return 0;
    }
    count_intact_faces(dir)
}

fn count_intact_faces(dir: &Path) -> usize {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files.iter().filter(|p| ttf_intact(p)).count()
}

fn clear_complete_marker(dir: &Path) {
    let _ = fs::remove_file(family_complete_marker(dir));
    clear_settled_add_zero_provenance(dir);
}

/// Soft Settled provenance — written only after real Add=0 (`stamp_soft_settle_dir_after_add_zero`).
fn family_settled_add_zero_marker(dir: &Path) -> PathBuf {
    dir.join(".settled-add-zero")
}

fn dir_has_settled_add_zero_provenance(dir: &Path) -> bool {
    family_settled_add_zero_marker(dir).is_file()
}

fn stamp_settled_add_zero_provenance(dir: &Path) {
    let _ = fs::write(family_settled_add_zero_marker(dir), b"1");
}

fn clear_settled_add_zero_provenance(dir: &Path) {
    let _ = fs::remove_file(family_settled_add_zero_marker(dir));
}

/// One-shot upgrade: soft `.complete` from hard-emoji tips lacks provenance — wipe so Scan
/// does not treat bare `.complete` as Settled. Full-face soft stays not-Incomplete (try-Add).
fn wipe_soft_complete_lacking_provenance(dir: &Path, family: &str) {
    if !family_soft_try_add_then_settle(family) {
        return;
    }
    if dir_is_complete(dir) && !dir_has_settled_add_zero_provenance(dir) {
        clear_complete_marker(dir);
    }
}

/// Soft + full-face size OK + no provenance ⇒ not Incomplete / not Repair (try-Add first).
#[cfg_attr(not(test), allow(dead_code))]
fn soft_full_face_exclude_repair(dir: &Path, family: &str) -> bool {
    if !family_soft_try_add_then_settle(family) {
        return false;
    }
    if dir_has_settled_add_zero_provenance(dir) {
        // Provenanced Settled uses .complete path — not a Repair exclude special-case.
        return false;
    }
    dir_has_intact(dir)
        && soft_emoji_full_face_ok(dir, family)
        && !dir_has_undersized_google_static(dir, family)
}

/// Scan/Repair classification for soft emoji (unit-tested).
#[cfg_attr(not(test), allow(dead_code))]
fn soft_scan_settled_from_provenance(
    has_provenance: bool,
    full_face_ok: bool,
    intact: bool,
    undersized: bool,
) -> bool {
    has_provenance && full_face_ok && intact && !undersized
}

#[cfg_attr(not(test), allow(dead_code))]
fn soft_scan_incomplete(
    settled: bool,
    soft: bool,
    full_face_ok: bool,
    intact: bool,
    has_complete: bool,
    missing_variable: bool,
    undersized: bool,
) -> bool {
    if settled {
        return false;
    }
    // Soft full-face without provenance: not Incomplete (Activate try-Add; no huge Repair).
    if soft && full_face_ok && intact && !undersized {
        return false;
    }
    (intact && !has_complete) || missing_variable || undersized
}

fn family_soft_full_face_exclude_repair(app: &AppHandle, family: &str) -> bool {
    if !family_soft_try_add_then_settle(family) {
        return false;
    }
    family_locations(app, family)
        .iter()
        .any(|d| soft_full_face_exclude_repair(d, family))
}

/// Stamp `.complete` only for a full face set. Body + `.expected` store the count.
fn mark_family_complete(root: &Path, expected: usize) {
    if expected == 0 {
        return;
    }
    if !dir_may_stamp_complete(root) {
        // css2 web build: never complete.
        write_expected_faces(root, expected);
        clear_complete_marker(root);
        return;
    }
    write_expected_faces(root, expected);
    write_marker_if_changed(&family_complete_marker(root), &expected.to_string());
    if let Some(fam) = read_family_name_marker(root)
        .or_else(|| root.file_name().and_then(|n| n.to_str()).map(str::to_string))
    {
        stamp_cdn_revision_if_known(root, &fam);
    }
}

fn dir_is_complete(dir: &Path) -> bool {
    family_complete_marker(dir).is_file() && dir_may_stamp_complete(dir)
}

/// True when every installable file embeds a Fontsource `-latin-` subset token.
fn dir_only_latin_fontsource_names(dir: &Path) -> bool {
    let slug = dir_slug_hint(dir);
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    if files.is_empty() {
        return false;
    }
    files.iter().all(|p| {
        p.file_name()
            .and_then(|s| s.to_str())
            .map(|n| filename_has_latin_subset(n, &slug))
            .unwrap_or(false)
    })
}

/// Official Google family whose `.complete` cannot be trusted: no `.google-planned`
/// key list, only Fontsource `*-latin-*` names, or (no-planned / latin-lie path)
/// intact Google face keys below the google-catalog weights×italic floor.
/// When `read_google_planned_keys` succeeds, trust `keys.len()` only — do not
/// apply the catalog floor (Google CSS often omits edge weights 1/1000, so
/// planned can be ≪ floor for Sofia Sans / Ysabeau / DM Sans / Nunito / …
/// without being a latin lie; floor-when-planned caused permanent Repair↔retry).
///
/// Missing catalog VFs are **not** cleared here: clearing `.complete` would push
/// families through Repair/bust and risk touching statics. Ensure/backfill pulls
/// VFs while `.complete` stays; `adopt_variable_files_into_plan` updates the stamp
/// after vars land without deleting/renaming/rewriting static faces.
fn official_google_complete_is_lie(dir: &Path) -> bool {
    let family = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim();
    if family.is_empty() || !is_official_google_family(family) {
        return false;
    }
    // Hard Gidugu or soft-settled emoji: intact full-size — Add=0 does not
    // make `.complete` a lie; Scan must not churn Repair.
    if family_disk_settled_known_gdi_incapable(
        family_may_settle_add_zero(family),
        dir_has_intact(dir),
        dir_has_undersized_google_static(dir, family),
    ) {
        return false;
    }
    // Honest plan present → verify_complete_marker uses keys.len() only.
    if read_google_planned_keys(dir).is_some() {
        return dir_only_latin_fontsource_names(dir);
    }
    // No usable `.google-planned` key list: latin-lie / understated-pack heuristics.
    if dir_only_latin_fontsource_names(dir) {
        return true;
    }
    let slug = dir_slug_hint(dir);
    if let Some(floor) = google_catalog_face_floor(family)
        && count_intact_google_face_keys(dir, &slug) < floor {
            return true;
        }
    // Official Google without a planned key list cannot prove stamp honesty.
    true
}

/// Drop lying `.complete` when intact faces are below the expected full set,
/// or when there is no usable expected face count (legacy bare `"1"` body,
/// missing/empty/unparsable `.expected`). For official Google families, also
/// clear when there is no `.google-planned`, only `*-latin-*` names, or (when
/// there is no planned key list) intact Google keys are below the catalog
/// weights×italic floor — understated `.expected` must not keep Activate/Repair
/// from running. When `.google-planned` is a real key list, expected =
/// `keys.len()` only (no catalog floor). Keeps Documents files intact — only
/// the sentinel is removed so Repair appears.
fn verify_complete_marker(dir: &Path) {
    if !dir_is_complete(dir) {
        return;
    }
    if official_google_complete_is_lie(dir) {
        clear_complete_marker(dir);
        return;
    }
    // Gidugu-class: .complete + undersized planned face must not block Repair.
    let family_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if !family_name.is_empty() && dir_has_undersized_google_static(dir, family_name) {
        clear_complete_marker(dir);
        return;
    }
    // When `.google-planned` is a real key list, expected = keys.len() — never an
    // understated `.expected` that could keep a partial Google stamp.
    let expected = if let Some(keys) = read_google_planned_keys(dir) {
        keys.len()
    } else {
        let Some(n) = read_expected_faces(dir) else {
            // Legacy bare "1" / unknown expected — untrusted; clear so Repair shows.
            clear_complete_marker(dir);
            return;
        };
        n
    };
    if count_intact_toward_expected(dir) < expected {
        clear_complete_marker(dir);
    }
}

/// Adopt pre-existing intact variable or full-source Google static files on disk when
/// `.google-planned` / `.complete` was missing so already-downloaded folders activate
/// without requiring a redundant network round-trip.
fn heal_preexisting_google_plan(dir: &Path, family: &str) {
    if dir_is_complete(dir) && read_google_planned_keys(dir).is_some() {
        return;
    }
    if !is_official_google_family(family)
        || dir_only_latin_fontsource_names(dir)
        || dir_has_undersized_google_static(dir, family)
    {
        return;
    }
    // Pure variable-font folders whose optional static fallback had left `.web-build`
    // behind (after VF-Primary purged statics) are full variable fonts, not css2 cuts.
    if gfsource::dir_is_web_build(dir) {
        if dir_is_pure_variable_font(dir) {
            gfsource::set_web_build(dir, false);
        } else {
            return;
        }
    }
    if dir_vf_primary_ready(dir, family)
        && (read_google_planned_keys(dir).is_none() || !dir_is_complete(dir))
    {
        let mut files = Vec::new();
        walk_font_files(dir, &mut files);
        let var_files: Vec<String> = files
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?;
                if is_variable_face_filename(name) && ttf_intact(p) {
                    Some(name.to_string())
                } else {
                    None
                }
            })
            .collect();
        if !var_files.is_empty() {
            adopt_variable_files_into_plan(dir, &var_files);
            return;
        }
    }
    if let Some(keys) = read_google_planned_keys(dir) {
        if !dir_is_complete(dir)
            && !family_dir_needs_fix_before_load(dir, family)
            && !keys.is_empty()
            && count_intact_planned_keys(dir, &keys) >= keys.len()
        {
            mark_family_complete(dir, keys.len());
        }
        return;
    }
    if !family_dir_needs_fix_before_load(dir, family) {
        let inst_keys = collect_intact_google_instance_keys(dir);
        let floor_ok = google_catalog_face_floor(family)
            .map(|fl| inst_keys.len() >= fl)
            .unwrap_or(true);
        if !inst_keys.is_empty() && floor_ok {
            write_google_planned(dir, &inst_keys);
            mark_family_complete(dir, inst_keys.len());
        }
    }
}

/// If an existing Noto Color Emoji folder has a legacy bitmap-only (`CBDT` without `glyf`)
/// TTF from an older download, remove it and clear `.complete` / `.settled-add-zero` so
/// the next Activate downloads `NotoColorEmoji_WindowsCompatible.ttf` and loads live in GDI.
fn upgrade_legacy_cbdt_color_emoji_dir(dir: &Path, family: &str) {
    let is_color_emoji = family.trim().eq_ignore_ascii_case("Noto Color Emoji");
    let is_mono_emoji = family.trim().eq_ignore_ascii_case("Noto Emoji");
    if !is_color_emoji && !is_mono_emoji {
        return;
    }
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    if is_mono_emoji {
        // Older versions stamped `.settled-add-zero` on Noto Emoji because AllIntactFallback
        // skipped compat-outline sidecars or transient bulk Add=0 settled it. If a full-size
        // intact face (>=256KB) or compat-outline is on disk, clear stale `.settled-add-zero`
        // so Windows GDI can load it live.
        let has_loadable = files.iter().any(|p| {
            let is_compat = p
                .file_name()
                .and_then(|s| s.to_str())
                .map(is_compat_sidecar_filename)
                .unwrap_or(false);
            let full_size = fs::metadata(p).map(|m| m.len() >= 256 * 1024).unwrap_or(false);
            (is_compat || full_size) && ttf_intact(p)
        });
        if has_loadable && dir_has_settled_add_zero_provenance(dir) {
            clear_settled_add_zero_provenance(dir);
            clear_session_gdi_refused(family);
        }
        return;
    }
    let mut purged_legacy = false;
    for path in files {
        let Ok(mut f) = fs::File::open(&path) else {
            continue;
        };
        let mut header = [0u8; 1036];
        let Ok(n) = f.read(&mut header) else {
            continue;
        };
        drop(f);
        let slice = &header[..n];
        if crate::namepatch::has_table(slice, b"CBDT")
            && !crate::namepatch::has_table(slice, b"glyf")
        {
            unregister_path(&path);
            intact_forget(&path);
            let _ = delete_font_file(&path);
            if let Some(maps) = gdi_maps_root() {
                let _ = fs::remove_file(maps.join(gdi_map_file_name(&path)));
            }
            purged_legacy = true;
        }
    }
    if purged_legacy {
        clear_complete_marker(dir);
        clear_settled_add_zero_provenance(dir);
        clear_session_gdi_refused(family);
    }
}

/// Fast-resume metadata check: at least one non-empty font file without opening SFNT headers.
fn dir_has_font_file_fast(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk_font_files(dir, &mut files);
    files.iter().any(|p| {
        let is_sidecar = p
            .file_name()
            .and_then(|s| s.to_str())
            .map(is_compat_sidecar_filename)
            .unwrap_or(false);
        !is_sidecar && p.metadata().map(|m| m.len() >= 12).unwrap_or(false)
    })
}

/// Fast O(1) stamp-validated cache hit against `scan-index.json` so unchanged complete folders
/// skip redundant SFNT file opens on cold boot (automatically invalidates on any file/marker change).
fn scan_index_cached_dir_ready(dir: &Path) -> bool {
    let Some(stamp) = dir_scan_stamp(dir) else {
        return false;
    };
    let key = dir.to_string_lossy().to_ascii_lowercase();
    let Ok(mut guard) = scan_index_mem().lock() else {
        return false;
    };
    if guard.is_none() {
        let loaded = scan_index_file_path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<HashMap<String, DiskScanCacheEntry>>(&b).ok())
            .unwrap_or_default();
        *guard = Some(loaded);
    }
    let Some(map) = guard.as_ref() else {
        return false;
    };
    let Some(entry) = map.get(&key) else {
        return false;
    };
    if entry.stamp != stamp {
        return false;
    }
    let Some(ref row) = entry.row else {
        return false;
    };
    row.has_complete && !row.incomplete && row.corrupt == 0 && row.files > 0
}

fn family_dir_is_ready_with_mode(dir: &Path, family: &str, fast_resume: bool) -> bool {
    let in_uploads = dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case(UPLOADS_CONTAINER))
        .unwrap_or(false);
    if in_uploads
        || matches!(read_download_source(dir), Some(FetchIntent::Local))
        || read_family_name_marker(dir).is_some()
    {
        return if fast_resume {
            dir_has_font_file_fast(dir)
        } else {
            dir_has_intact(dir)
        };
    }
    if (fast_resume || scan_index_cached_dir_ready(dir))
        && dir_is_complete(dir)
        && !gfsource::dir_is_web_build(dir)
        && !family_known_gdi_session_incapable(family)
    {
        return dir_has_font_file_fast(dir);
    }
    upgrade_legacy_cbdt_color_emoji_dir(dir, family);
    heal_clear_sans_expected_plan(dir, family);
    heal_preexisting_google_plan(dir, family);
    verify_complete_marker(dir);
    // Known-incapable + intact full-size ⇒ disk settled (Scan/Repair honesty).
    stamp_known_incapable_dir_settled(dir, family);
    dir_is_complete(dir) && dir_has_intact(dir)
}

/// Ready = honest `.complete` plus at least one intact TTF/OTF (full face set).
/// Any one TTF without `.complete` is incomplete — Activate must Repair, not skip.
fn family_is_ready(app: &AppHandle, family: &str) -> bool {
    let fast_resume = fast_session_resume();
    family_locations(app, family)
        .iter()
        .any(|dir| family_dir_is_ready_with_mode(dir, family, fast_resume))
}

fn family_is_incomplete(app: &AppHandle, family: &str) -> bool {
    // Soft full-face without provenance: not Incomplete / not Repair target.
    if family_soft_full_face_exclude_repair(app, family) {
        return false;
    }
    !family_is_ready(app, family) && family_has_intact(app, family)
}

fn purge_family_files(app: &AppHandle, family: &str) {
    let _ = purge_family_files_result(app, family);
}

fn purge_family_files_result(app: &AppHandle, family: &str) -> Result<(), String> {
    let root = documents_root(app)?;
    purge_family_dirs_in(&root, family)
}

fn purge_family_dirs_in(root: &Path, family: &str) -> Result<(), String> {
    if family.trim().is_empty() || family_folder_slug(family).is_empty() {
        return Err("refusing to delete: empty family name".into());
    }
    let mut last_err = String::new();
    for dir in family_locations_in(root, family) {
        if is_protected_family_dir(root, &dir) {
            return Err(format!(
                "refusing to delete {}: not a single family folder",
                dir.display()
            ));
        }
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        for path in &files {
            unregister_path(path);
            crate::font_protocol::evict_path(path);
            crate::renderer::evict_path(path);
            intact_forget(path);
        }
        crate::font_protocol::evict_dir(&dir);
        crate::renderer::evict_dir(&dir);
        gdi_flush_local();
        if let Err(err) = recycle_user_font_dir(&dir) {
            last_err = err;
        }
    }
    if last_err.is_empty() {
        Ok(())
    } else {
        Err(last_err)
    }
}

#[tauri::command]
pub fn activation_folder(app: AppHandle) -> Result<String, String> {
    let dir = documents_root(&app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn open_activation_folder(app: AppHandle, sub: Option<String>) -> Result<(), String> {
    let dir = documents_root(&app)?;
    let google_dir = google_container_path(&dir);
    let uploads_dir = dir.join(UPLOADS_CONTAINER);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = fs::create_dir_all(&google_dir);
    if fs::read_dir(&uploads_dir)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
    {
        let _ = fs::remove_dir(&uploads_dir);
    }
    let target = match sub.as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("google") => google_dir,
        Some("uploads" | "local") if uploads_dir.is_dir() => uploads_dir,
        _ => dir,
    };
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(&target)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&target)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(&target).spawn();
    }
    Ok(())
}

#[tauri::command]
pub fn install_font_file(app: AppHandle, family: String, file_name: String, bytes: Vec<u8>) -> Result<(), String> {
    install_font_file_in(&documents_root(&app)?, &family, &file_name, &bytes)?;
    notify_fonts_changed_maybe();
    Ok(())
}

/// Upload write under an explicit Documents root (testable without an AppHandle).
/// User added/uploaded files live under `Uploads\<Family>` separately from `Google\<Family>`.
fn install_font_file_in(root: &Path, family: &str, file_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = family_dir_for_source_in(root, family, true);
    let path = dir.join(sanitize(file_name));
    write_font_file(&path, bytes)?;
    write_family_name_marker(&dir, family);
    write_download_source(&dir, FetchIntent::Local);
    let intact = count_intact_faces(&dir);
    if intact > 0 {
        // User/upload stamp — clear Google key list so verify does not require
        // listed Google faces against a non-Google intact count.
        clear_google_planned(&dir);
        mark_family_complete(&dir, intact);
    }
    Ok(path)
}

#[tauri::command]
pub fn save_library_file(app: AppHandle, family: String, file_name: String, bytes: Vec<u8>) -> Result<(), String> {
    install_font_file(app, family, file_name, bytes)
}

#[tauri::command]
pub fn remove_library_file(app: AppHandle, family: String, file_name: String) -> Result<(), String> {
    let fname = sanitize(&file_name);
    let path = family_locations(&app, &family)
        .into_iter()
        .map(|d| d.join(&fname))
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            documents_root(&app)
                .map(|r| family_dir_for_source_in(&r, &family, true).join(&fname))
                .unwrap_or_else(|_| PathBuf::from(&fname))
        });
    unregister_path(&path);
    intact_forget(&path);
    gdi_flush_local();
    crate::delete_ops::delete_user_file(&path, crate::delete_ops::is_permanent_delete())?;
    // Drop empty family folder so Delete does not leave orphans; otherwise update expected count.
    if let Some(dir) = path.parent() {
        let mut left = Vec::new();
        walk_font_files(dir, &mut left);
        if left.is_empty() {
            let _ = fs::remove_file(family_complete_marker(dir));
            let _ = fs::remove_file(family_expected_marker(dir));
            let _ = fs::remove_file(family_name_marker(dir));
            let _ = fs::remove_file(family_download_source_marker(dir));
            clear_google_planned(dir);
            let _ = fs::remove_file(dir.join(".fontsource-version"));
            let _ = fs::remove_dir(dir);
        } else if read_google_planned_keys(dir).is_none() {
            let intact = count_intact_faces(dir);
            if intact > 0 {
                mark_family_complete(dir, intact);
            }
        }
    }
    notify_fonts_changed();
    let _ = app;
    Ok(())
}

fn resolve_register_font_family(p: &Path, family_hint: Option<&str>) -> Option<String> {
    family_hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| sfnt_upload_family(p))
        .or_else(|| {
            p.parent().and_then(|dir| {
                read_family_name_marker(dir).or_else(|| {
                    dir.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
            })
        })
}

fn register_font_path_in(p: &Path, family_hint: Option<&str>) -> Result<(bool, Option<String>), String> {
    let lower = p.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    if lower.contains("\\windows\\fonts") {
        return Err("refusing to register C:\\Windows\\Fonts".into());
    }
    if !ttf_intact(p) {
        return Err("file is not an intact font".into());
    }
    let resolved_family = resolve_register_font_family(p, family_hint);
    let registered = if let Some(ref fam) = resolved_family {
        register_family_path(fam, p)
    } else {
        register_path(p)
    };
    Ok((registered, resolved_family))
}

#[tauri::command]
pub fn register_font_path(app: AppHandle, path: String, family: Option<String>) -> Result<(), String> {
    let p = PathBuf::from(path);
    let (registered, resolved_family) = register_font_path_in(&p, family.as_deref())?;
    if registered {
        if let Some(ref fam) = resolved_family {
            session_add(&app, std::slice::from_ref(fam));
        }
        #[cfg(windows)]
        persist_activation_sidecars(&app);
        notify_fonts_changed_maybe();
    }
    Ok(())
}

#[tauri::command]
pub async fn flush_font_cache() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(flush_font_cache_blocking)
        .await
        .map_err(|e| e.to_string())?
}

pub fn flush_font_cache_blocking() -> Result<(), String> {
    #[cfg(windows)]
    {
        winfont::flush_local();
        let _ = winfont::restart_font_cache_service(font_cache_restart_budget());
        winfont::flush_cache();
    }
    Ok(())
}

/// c255 P2: drop this family's GDI faces left by a failed Activate (per-family Deactivate).
#[cfg(windows)]
fn sweep_family_leftovers(family: &str, unloaded_paths: &mut Vec<PathBuf>) -> u32 {
    if winfont::loaded_is_empty() {
        return 0;
    }
    let maps: Vec<(PathBuf, PathBuf)> = winfont::snapshot_face_maps()
        .into_iter()
        .map(|m| (PathBuf::from(m.source), PathBuf::from(m.stage)))
        .collect();
    let now = winfont::snapshot_loaded();
    let mut k = 0u32;
    for p in leftover_loaded_for_family(&alias_keys(family), &now, &maps) {
        winfont::unregister_keep_stage(&p);
        unloaded_paths.push(p);
        k += 1;
    }
    k
}

fn unload_now(app: &AppHandle, families: &[String], report: bool) -> u32 {
    // Session HashSet only. Walking Documents here was the Deactivate hang:
    // thousands of RemoveFontResourceExW on files that were never Add'ed,
    // including anything that looked like a System family name.
    let mut n = 0u32;
    let total_n = families.len() as u32;
    if report {
        if let Ok(mut p) = bulk().progress.lock() {
            p.running = true;
            p.paused = false;
            p.kind = "remove".into();
            p.done = 0;
            p.total = total_n;
            p.failed = 0;
            p.skipped = 0;
            p.current = families.first().cloned().unwrap_or_default();
            p.ready_names.clear();
            p.failed_names.clear();
            p.failed_details.clear();
            p.settled_names.clear();
        }
        emit_progress(app);
    }
    #[cfg(windows)]
    let loaded = winfont::snapshot_loaded();
    #[cfg(windows)]
    let mut unloaded_paths: Vec<PathBuf> = Vec::new();
    let mut last_emit = Instant::now();
    let mut cancelled = false;
    for (i, family) in families.iter().enumerate() {
        // 1.0.206i: bulk Deactivate / unload_now honor Cancel + Pause (same gate as on-disk register).
        // Cancel → stop further Removes (already-unloaded stay Off). Pause → wait; Resume continues.
        let state = bulk();
        match on_disk_register_gate(
            state.cancel.load(Ordering::SeqCst),
            state.pause.load(Ordering::SeqCst),
        ) {
            OnDiskRegisterGate::StopCancelled => {
                cancelled = true;
                if report {
                    if let Ok(mut p) = state.progress.lock() {
                        p.kind = "remove".into();
                        p.running = false;
                        p.paused = false;
                        p.current = "Cancelled".into();
                        // Keep done at families already processed — do not jump to total.
                    }
                    emit_progress(app);
                }
                break;
            }
            OnDiskRegisterGate::WaitPaused => {
                if report {
                    if let Ok(mut p) = state.progress.lock() {
                        p.paused = true;
                        p.running = true;
                        p.kind = "remove".into();
                    }
                    emit_progress(app);
                }
                thread::sleep(Duration::from_millis(200));
                // Re-process same index after pause/cancel check.
                // `continue` would skip without advancing — use a nested wait loop instead.
                loop {
                    let st = bulk();
                    match on_disk_register_gate(
                        st.cancel.load(Ordering::SeqCst),
                        st.pause.load(Ordering::SeqCst),
                    ) {
                        OnDiskRegisterGate::StopCancelled => {
                            cancelled = true;
                            break;
                        }
                        OnDiskRegisterGate::WaitPaused => {
                            if report {
                                if let Ok(mut p) = st.progress.lock() {
                                    p.paused = true;
                                    p.running = true;
                                    p.kind = "remove".into();
                                }
                                emit_progress(app);
                            }
                            thread::sleep(Duration::from_millis(200));
                        }
                        OnDiskRegisterGate::Run => {
                            if report
                                && let Ok(mut p) = st.progress.lock() {
                                    p.paused = false;
                                }
                            break;
                        }
                    }
                }
                if cancelled {
                    if report {
                        if let Ok(mut p) = bulk().progress.lock() {
                            p.kind = "remove".into();
                            p.running = false;
                            p.paused = false;
                            p.current = "Cancelled".into();
                        }
                        emit_progress(app);
                    }
                    break;
                }
            }
            OnDiskRegisterGate::Run => {
                if report
                    && let Ok(mut p) = state.progress.lock() {
                        p.paused = false;
                    }
            }
        }
        let t = family.trim();
        if t.is_empty() {
            continue;
        }
        #[cfg(windows)]
        {
            // Unload bound family paths directly without O(L) before/after loaded() clones,
            // and keep pre-cached gdi-maps files so re-activation skips copying.
            let mut k = winfont::unregister_family_keep_stage(t, &mut unloaded_paths);
            if k == 0 && !winfont::loaded_is_empty() {
                let keys: Vec<String> = alias_keys(t);
                for path in &loaded {
                    if winfont::is_windows_fonts_path(path) {
                        continue;
                    }
                    let parent = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if keys.iter().any(|akey| parent.eq_ignore_ascii_case(akey)) {
                        unloaded_paths.push(path.clone());
                        winfont::unregister_keep_stage(path);
                        k += 1;
                    }
                }
                k += sweep_family_leftovers(t, &mut unloaded_paths); // c255 P2
            }
            n += k;
        }
        #[cfg(not(windows))]
        {
            let k = unregister_family_session(t);
            n += k;
        }
        forget_queued(t);
        if let Ok(mut denied) = bulk().denied.lock() {
            denied.insert(t.to_lowercase());
        }
        if report {
            if let Ok(mut p) = bulk().progress.lock() {
                p.kind = "remove".into();
                p.running = true;
                p.done = (i + 1) as u32;
                p.total = total_n;
                p.current = t.to_string();
                // 1.0.206j: ready_names = unloaded prefix so UI confirms Off only for these
                // (never confirmDeactivated(all) at unload_font_families spawn).
                if !p.ready_names.iter().any(|n| n.eq_ignore_ascii_case(t)) {
                    p.ready_names.push(t.to_string());
                }
            }
            let last = i + 1 == families.len();
            if i == 0 || last || (i + 1) % 4 == 0 || last_emit.elapsed() >= Duration::from_millis(150) {
                emit_progress(app);
                last_emit = Instant::now();
            }
        }
    }
    // On cancel, only drop session entries for families actually processed (done count).
    // Full cancel mid-bulk: still remove session for unloaded prefix via per-family forget_queued;
    // session_remove of the whole list would mark still-Live faces Off in sidecar — skip when cancelled.
    if !cancelled {
        session_remove(app, families);
        // c253 #6: Deactivate that empties the session also drops GDI faces Added by a
        // failed Activate that never entered the session list (leak vs baseline).
        #[cfg(windows)]
        {
            let remaining = load_session_families(app);
            if remaining.is_empty() {
                winfont::clear_all_hkcu_session_entries();
                if let Ok(root) = documents_root(app) {
                    crate::session_stage::clear_persistent_active_marker_in(&root);
                }
            }
            let now = winfont::snapshot_loaded();
            for p in leftover_loaded_after_unload(&remaining, &now, winfont::is_windows_fonts_path) {
                unregister_path(&p);
                unloaded_paths.push(p);
                n += 1;
            }
        }
    } else {
        // Persist whatever unregister already did.
        let done_n = bulk()
            .progress
            .lock()
            .map(|p| p.done as usize)
            .unwrap_or(0);
        let prefix: Vec<String> = families.iter().take(done_n).cloned().collect();
        if !prefix.is_empty() {
            session_remove(app, &prefix);
        }
    }
    if report && !cancelled {
        if let Ok(mut p) = bulk().progress.lock() {
            p.kind = "remove".into();
            p.running = true;
            p.done = total_n;
            p.total = total_n;
            p.current = "Updating Windows…".into();
        }
        emit_progress(app);
    }
    if n > 0 {
        gdi_flush_local();
        #[cfg(windows)]
        {
            notify_fonts_changed();
            let mut still = filter_still_write_locked(&unloaded_paths);
            let access_denied = if !still.is_empty() && plan_font_cache_flush(still.len()) {
                let outcome = winfont::restart_font_cache_service(font_cache_restart_budget());
                still = filter_still_write_locked(&still);
                matches!(outcome, winfont::FontCacheRestartOutcome::AccessDenied)
            } else {
                false
            };
            if font_cache_toast_needed(still.len(), access_denied) {
                eprintln!("Fonths: {}", font_cache_held_message(still.len()));
                emit_font_cache_held_toast(app, &still, access_denied);
            } else if access_denied {
                eprintln!(
                    "Fonths: FontCache restart skipped (not elevated); 0 files locked — Deactivate complete"
                );
            }
            persist_activation_sidecars(app);
        }
        #[cfg(not(windows))]
        {
            notify_fonts_changed();
        }
    }
    if report {
        if let Ok(mut p) = bulk().progress.lock() {
            p.kind = "remove".into();
            p.running = false;
            p.paused = false;
            if !cancelled {
                p.done = total_n;
                p.total = total_n;
                p.current.clear();
            }
            // cancelled: keep done at processed count; current already "Cancelled"
        }
        emit_progress(app);
    }
    n
}

#[tauri::command]
pub fn unload_font_family(app: AppHandle, family: String) -> Result<u32, String> {
    // Sync — callers that delete next must finish Remove before DeleteFile.
    Ok(unload_now(&app, &[family], false))
}

#[tauri::command]
pub async fn unload_font_families(app: AppHandle, families: Vec<String>) -> Result<u32, String> {
    tauri::async_runtime::spawn_blocking(move || unload_font_families_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn unload_font_families_blocking(app: AppHandle, families: Vec<String>) -> Result<u32, String> {
    let n = families.len() as u32;
    if n == 0 {
        return Ok(0);
    }
    // Seed the bar on this thread so the first JS poll is not a leftover idle
    // snapshot (that used to toast “done” while GDI was still running).
    let downloading = bulk().running.load(Ordering::SeqCst);
    if !downloading {
        // Fresh remove job — clear leftover cancel/pause from a prior Cancel click (1.0.206i).
        {
            let state = bulk();
            state.cancel.store(false, Ordering::SeqCst);
            state.pause.store(false, Ordering::SeqCst);
        }
        if let Ok(mut p) = bulk().progress.lock() {
            p.running = true;
            p.paused = false;
            p.kind = "remove".into();
            p.done = 0;
            p.total = n;
            p.failed = 0;
            p.skipped = 0;
            p.current = families.first().cloned().unwrap_or_default();
            p.ready_names.clear();
            p.failed_names.clear();
            p.failed_details.clear();
            p.settled_names.clear();
        }
        emit_progress(&app);
    }
    // Bulk deactivate stays background so Activate-all off does not freeze UI.
    thread::spawn(move || {
        let _ = unload_now(&app, &families, !downloading);
    });
    Ok(n)
}

#[tauri::command]
pub async fn uninstall_font_family(app: AppHandle, family: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || uninstall_font_family_blocking(app, family))
        .await
        .map_err(|e| e.to_string())?
}

pub fn uninstall_font_family_blocking(app: AppHandle, family: String) -> Result<(), String> {
    // Await unload on this thread before DeleteFile — do not race GDI.
    let _ = unload_now(&app, std::slice::from_ref(&family), false);
    gdi_flush_local();
    purge_family_files_result(&app, &family)?;
    // Empty after Explorer-delete is success (missing = already gone).
    Ok(())
}

#[tauri::command]
pub async fn font_family_installed(app: AppHandle, family: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || font_family_installed_blocking(app, family))
        .await
        .map_err(|e| e.to_string())?
}

pub fn font_family_installed_blocking(app: AppHandle, family: String) -> Result<bool, String> {
    Ok(family_is_ready(&app, &family))
}

#[tauri::command]
pub async fn list_activated_families(app: AppHandle) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || list_activated_families_blocking(app))
        .await
        .map_err(|e| e.to_string())?
}

pub fn list_activated_families_blocking(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(build_disk_index(&app).names)
}

#[tauri::command]
pub async fn register_existing_on_disk(app: AppHandle, families: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || register_existing_on_disk_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn register_existing_on_disk_blocking(app: AppHandle, families: Vec<String>) -> Result<usize, String> {
    if families.is_empty() {
        return Ok(0);
    }
    let index = build_disk_index(&app);
    let mut n = 0usize;
    let mut ready = Vec::new();
    for family in &families {
        let added = register_from_index(&app, &index, family);
        if added > 0 {
            n += added;
            ready.push(family.clone());
        }
    }
    if n > 0 {
        #[cfg(windows)]
        winfont::sync_hkcu_all_active_stages();
        notify_fonts_changed();
        session_add(&app, &ready);
    }
    Ok(n)
}

/// After session register: pull real `*-variable-*` TTFs for catalog-variable
/// families that only have static instances on disk. Must not run on the boot
/// invoke thread — CDN METADATA for ~550 families would freeze startup.
/// Need VF backfill when no intact var, OR dual-VF family missing italic.
fn dir_needs_variable_backfill(dir: &Path, family: &str) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let has_var = dir_has_intact_variable(dir);
    let dual_needs_italic = family_expects_dual_variable(family)
        && !dir_has_intact_variable_italic(dir);
    !has_var || dual_needs_italic
}

fn family_needs_variable_backfill(app: &AppHandle, family: &str) -> bool {
    // Include FS-only google/fonts VF ensure (42dot etc.) — not google-catalog alone.
    family_ensures_google_vf(family)
        && family_locations(app, family)
            .iter()
            .any(|dir| dir_needs_variable_backfill(dir, family))
}

#[allow(dead_code)]
fn backfill_missing_variable_faces(app: &AppHandle, families: &[String]) {
    let need: Vec<String> = families
        .iter()
        .filter(|family| family_needs_variable_backfill(app, family))
        .cloned()
        .collect();
    if need.is_empty() {
        return;
    }
    let Some(client) = http_download_client() else {
        return;
    };
    let mut wrote = 0usize;
    for family in &need {
        if bulk().cancel.load(Ordering::SeqCst) {
            break;
        }
        let (n, _) = ensure_catalog_variable_faces(app, &client, family);
        if n > 0 {
            wrote = wrote.saturating_add(n);
            let _ = register_intact_family(app, family);
        }
    }
    if wrote > 0 {
        notify_fonts_changed();
        #[cfg(windows)]
        persist_activation_sidecars(app);
    }
}

#[tauri::command]
pub async fn activate_families_on_disk(app: AppHandle, families: Vec<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || activate_families_on_disk_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn activate_families_on_disk_blocking(app: AppHandle, families: Vec<String>) -> Result<Vec<String>, String> {
    let families = dedupe_family_names(&families);
    if families.is_empty() {
        return Ok(Vec::new());
    }
    // P0 (1.0.165): never run the GDI register loop on the invoke thread —
    // Activate All of ~2k on-disk families made the window "Not Responding"
    // even while mid-flight progress events fired. Filter + ≤6 parallel
    // register_intact_family run on a worker; callers wait on progress
    // (running=false + ready_names) via poll/event — Ok([]) here means
    // "accepted / started", NOT "registered none" (invoke fail still catch→[]).
    // Honesty unchanged: ready_names / session only after successful Add.
    let state = bulk();
    let own_progress = !state.running.load(Ordering::SeqCst);
    if own_progress {
        // Fresh job — clear leftover cancel/pause from a prior Cancel click.
        state.cancel.store(false, Ordering::SeqCst);
        state.pause.store(false, Ordering::SeqCst);
        if let Ok(mut p) = state.progress.lock() {
            p.running = true;
            p.paused = false;
            // 1.0.205: on-disk Activate All is register, not download (split owners).
            p.kind = "register".into();
            p.done = 0;
            p.total = families.len() as u32;
            p.failed = 0;
            p.skipped = 0;
            p.current = format!("Checking {} on disk…", families.len());
            p.ready_names.clear();
            p.failed_names.clear();
            p.failed_details.clear();
            p.settled_names.clear();
        }
        state.running.store(true, Ordering::SeqCst);
        emit_progress(&app);
    } else if let Ok(mut p) = state.progress.lock() {
        p.kind = "download".into();
        p.running = true;
        if p.total < families.len() as u32 {
            p.total = families.len() as u32;
        }
        p.current = format!("Checking {} on disk…", families.len());
    }
    if !own_progress {
        emit_progress(&app);
    }

    let app2 = app.clone();
    thread::spawn(move || {
        activate_on_disk_worker(app2, families, own_progress);
    });
    Ok(Vec::new())
}

/// Worker: lock-free `rayon` parallel classify (already-live / settled / ready-on-disk) in one pass,
/// then parallel intact register with O(1) progress tracking.
fn activate_on_disk_worker(app: AppHandle, families: Vec<String>, own_progress: bool) {
    let families = dedupe_family_names(&families);
    let mut rows: Vec<(usize, u8, String)> = families
        .par_iter()
        .enumerate()
        .filter_map(|(idx, t)| {
            if bulk().cancel.load(Ordering::SeqCst) {
                return None;
            }
            for dir in family_locations(&app, t) {
                migrate_download_source_stamp(&dir);
            }
            let bucket = if family_skip_register_this_process(&app, t).is_some() {
                Some(0u8)
            } else if family_early_skip_known_incapable(&app, t) {
                Some(1u8)
            } else if family_is_ready(&app, t) {
                Some(2u8)
            } else {
                None
            };
            bucket.map(|b| (idx, b, t.clone()))
        })
        .collect();
    rows.sort_by_key(|(idx, _, _)| *idx);
    let mut already: Vec<String> = Vec::new();
    let mut settled_skip: Vec<String> = Vec::new();
    let mut ready: Vec<String> = Vec::new();
    for (_, b, name) in rows {
        match b {
            0 => already.push(name),
            1 => settled_skip.push(name),
            _ => ready.push(name),
        }
    }
    let state = bulk();
    if ready.is_empty() && already.is_empty() && settled_skip.is_empty() {
        if own_progress {
            if let Ok(mut p) = state.progress.lock() {
                p.failed = 0;
                p.failed_names.clear();
                p.failed_details.clear();
                p.settled_names.clear();
                // Nothing intact — surface as failed so JS clears pending.
                for family in &families {
                    let t = family.trim();
                    if t.is_empty() {
                        continue;
                    }
                    p.failed = p.failed.saturating_add(1);
                    if !p.failed_names.iter().any(|n| n.eq_ignore_ascii_case(t)) {
                        p.failed_names.push(t.to_string());
                        let detail = if resolve_fetch_intent_from_disk(&app, t).is_none() {
                            format!("{t} — Not found (no file on disk and no catalog source)")
                        } else {
                            format!("{t} — not intact on disk (.complete missing or incomplete)")
                        };
                        p.failed_details.push(detail);
                    }
                }
                let n = p.failed_names.len() as u32;
                p.running = false;
                p.current.clear();
                p.done = n;
                p.total = n.max(1);
            }
            state.running.store(false, Ordering::SeqCst);
            emit_progress(&app);
        }
        return;
    }

    if let Ok(mut p) = state.progress.lock() {
        // 1.0.205: on-disk Activate All is register, not download (split owners).
        p.kind = "register".into();
        p.running = true;
        p.done = already.len() as u32;
        p.total = (already.len() + ready.len()) as u32;
        p.current = if ready.is_empty() && settled_skip.is_empty() {
            format!("Already registered — {} typefaces", already.len())
        } else if ready.is_empty() {
            format!(
                "Live {} · Settled {} · Library on disk",
                already.len(),
                settled_skip.len()
            )
        } else {
            format!("Registering {} already on disk…", ready.len())
        };
        if own_progress {
            p.failed = 0;
            p.skipped = 0;
            // Keep ready_names if merging into a parent job; own job starts clean.
            p.ready_names.clear();
            p.failed_names.clear();
            p.failed_details.clear();
            p.settled_names.clear();
        }
        let mut seen_ready: HashSet<String> = p
            .ready_names
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .collect();
        for f in &already {
            if seen_ready.insert(f.trim().to_ascii_lowercase()) {
                p.ready_names.push(f.clone());
                p.skipped = p.skipped.saturating_add(1);
            }
        }
        let mut seen_settled: HashSet<String> = p
            .settled_names
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .collect();
        for f in &settled_skip {
            stamp_known_incapable_disk_settled(&app, f);
            note_session_gdi_refused(f);
            if seen_settled.insert(f.trim().to_ascii_lowercase()) {
                p.settled_names.push(f.clone());
            }
        }
        // Include settled in done/total so Activate All does not look stuck at 2099/2100.
        let handled = already.len() + settled_skip.len();
        p.done = handled as u32;
        p.total = (handled + ready.len()) as u32;
    }
    emit_progress(&app);

    let mut registered = already.clone();
    if !ready.is_empty() {
        let _hkcu_batch = HkcuBatchGuard::new();
        registered.extend(register_on_disk_parallel_progress(
            &app,
            &ready,
            already.len() + settled_skip.len(),
        ));
    }
    let cancelled = state.cancel.load(Ordering::SeqCst);
    if !registered.is_empty() {
        session_add(&app, &registered);
    }
    if !registered.is_empty() {
        notify_fonts_changed();
        #[cfg(windows)]
        persist_activation_sidecars(&app);
    }
    emit_gdi_pressure_if_high(&app);

    // Keep cached on-disk activation 100% local and instant — do not spawn background
    // network VF backfill sweeps automatically after activating already-downloaded fonts.

    // Requested but not intact: clear pending via failed_names (poll finalize).
    // On cancel: running=false + emit; keep ready_names for completed Adds only —
    // do not mark remaining queue items as ready (or pretend they finished).
    if own_progress {
        if let Ok(mut p) = state.progress.lock() {
            if !cancelled {
                let handled: HashSet<String> = ready
                    .iter()
                    .chain(already.iter())
                    .chain(settled_skip.iter())
                    .map(|n| n.trim().to_lowercase())
                    .collect();
                for family in &families {
                    let t = family.trim();
                    if t.is_empty() || handled.contains(&t.to_lowercase()) {
                        continue;
                    }
                    p.failed = p.failed.saturating_add(1);
                    if !p.failed_names.iter().any(|n| n.eq_ignore_ascii_case(t)) {
                        p.failed_names.push(t.to_string());
                        let detail = if resolve_fetch_intent_from_disk(&app, t).is_none() {
                            format!("{t} — Not found (no file on disk and no catalog source)")
                        } else {
                            format!("{t} — not intact on disk (.complete missing or incomplete)")
                        };
                        p.failed_details.push(detail);
                    }
                }
                p.done = (already.len() + settled_skip.len() + ready.len()) as u32;
                p.total = p.done;
            }
            p.running = false;
            p.paused = false;
            p.current.clear();
        }
        state.running.store(false, Ordering::SeqCst);
        emit_progress(&app);
    } else {
        emit_progress(&app);
    }
}

/// ≤6 workers walk/register in parallel; GDI Add stays serialized in gdi_api.
/// Progress done/total ticks per family; ready_names only when Add returned >0.
/// Skye P1: honor bulk cancel/pause — Cancel clears the queue so workers stop;
/// Pause waits like download drain (does not drain GDI while held).
/// `done_base` is families already counted (this-process live skip) so percent
/// does not restart at 0 after seeding already-registered names.
fn register_on_disk_parallel_progress(app: &AppHandle, ready: &[String], done_base: usize) -> Vec<String> {
    let ready = dedupe_family_names(ready);
    if ready.is_empty() {
        return Vec::new();
    }
    let workers = session_register_workers(ready.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(ready.clone())));
    let registered: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let processed = Arc::new(AtomicUsize::new(0));
    let last_emit = Arc::new(Mutex::new(Instant::now()));
    // O(1) dedupe sets so registering 2,000+ families never does O(N^2) scans under the progress mutex.
    let (init_ready, init_settled, init_failed) = bulk()
        .progress
        .lock()
        .map(|p| {
            (
                p.ready_names
                    .iter()
                    .map(|n| n.trim().to_ascii_lowercase())
                    .collect::<HashSet<String>>(),
                p.settled_names
                    .iter()
                    .map(|n| n.trim().to_ascii_lowercase())
                    .collect::<HashSet<String>>(),
                p.failed_names
                    .iter()
                    .map(|n| n.trim().to_ascii_lowercase())
                    .collect::<HashSet<String>>(),
            )
        })
        .unwrap_or_default();
    let seen_ready = Arc::new(Mutex::new(init_ready));
    let seen_settled = Arc::new(Mutex::new(init_settled));
    let seen_failed = Arc::new(Mutex::new(init_failed));
    let total = ready.len();
    let mut joins = Vec::with_capacity(workers);

    for _ in 0..workers {
        let app = app.clone();
        let queue = queue.clone();
        let registered = registered.clone();
        let processed = processed.clone();
        let last_emit = last_emit.clone();
        let seen_ready = seen_ready.clone();
        let seen_settled = seen_settled.clone();
        let seen_failed = seen_failed.clone();
        joins.push(thread::spawn(move || {
            loop {
                let state = bulk();
                match on_disk_register_gate(
                    state.cancel.load(Ordering::SeqCst),
                    state.pause.load(Ordering::SeqCst),
                ) {
                    OnDiskRegisterGate::StopCancelled => {
                        if let Ok(mut q) = queue.lock() {
                            q.clear();
                        }
                        break;
                    }
                    OnDiskRegisterGate::WaitPaused => {
                        if let Ok(mut p) = state.progress.lock() {
                            p.paused = true;
                            p.running = true;
                        }
                        emit_progress(&app);
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                    OnDiskRegisterGate::Run => {
                        if let Ok(mut p) = state.progress.lock() {
                            p.paused = false;
                        }
                    }
                }
                let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                let Some(family) = next else {
                    break;
                };
                // Re-check after pop: Cancel between pop and Add must not leave
                // the family half-handled as "ready" if we skip Add — put back
                // only if we have not started register; here we abort without Add.
                if state.cancel.load(Ordering::SeqCst) {
                    if let Ok(mut q) = queue.lock() {
                        q.clear();
                    }
                    break;
                }
                while matches!(
                    on_disk_register_gate(
                        state.cancel.load(Ordering::SeqCst),
                        state.pause.load(Ordering::SeqCst),
                    ),
                    OnDiskRegisterGate::WaitPaused
                ) {
                    if let Ok(mut p) = state.progress.lock() {
                        p.paused = true;
                        p.running = true;
                    }
                    emit_progress(&app);
                    thread::sleep(Duration::from_millis(200));
                }
                if state.cancel.load(Ordering::SeqCst) {
                    if let Ok(mut q) = queue.lock() {
                        q.clear();
                    }
                    break;
                }
                let (k, cause) = register_intact_family_detailed(&app, &family);
                forget_queued(&family);
                if matches!(cause, RegisterFailKind::GdiQuotaExceeded) {
                    if let Ok(mut q) = queue.lock() {
                        q.clear();
                    }
                    emit_gdi_pressure_if_high(&app);
                    break;
                }
                if let Ok(mut denied) = bulk().denied.lock() {
                    denied.remove(&family.trim().to_lowercase());
                }
                let done_n = processed.fetch_add(1, Ordering::SeqCst) + 1;
                if done_n.is_multiple_of(64) {
                    thread::yield_now();
                }
                let restoring = session_boot().running.load(Ordering::SeqCst);
                if let Ok(mut p) = bulk().progress.lock() {
                    if p.kind.is_empty() {
                        p.kind = "register".into();
                    }
                    p.running = true;
                    let want_total = (done_base + total) as u32;
                    let step_done = (done_base + done_n).min(done_base + total) as u32;
                    if restoring {
                        // Session GDI restore total is strictly bounded by unique ready targets
                        // and done advances monotonically without exceeding total.
                        p.total = want_total;
                        p.done = p.done.max(step_done).min(want_total);
                    } else {
                        if p.total < want_total {
                            p.total = want_total;
                        }
                        p.done = p.done.max(step_done).min(p.total);
                    }
                    // Calm Restoring N/T during session_begin — not per-face download chrome.
                    p.current = if restoring {
                        format!("Restoring {}/{}", p.done.min(want_total), want_total)
                    } else {
                        format!("Registering {family}")
                    };
                    let fam_key = family.trim().to_ascii_lowercase();
                    if k > 0 {
                        // Honesty: this family did Add successfully — count it even
                        // if Cancel arrived mid-flight after Add returned.
                        let is_new = seen_ready
                            .lock()
                            .map(|mut s| s.insert(fam_key.clone()))
                            .unwrap_or(false);
                        if is_new {
                            p.ready_names.push(family.clone());
                        }
                        session_boot_push_ready(&family);
                        if let Ok(mut g) = registered.lock() {
                            g.push(family.clone());
                        }
                    } else if family_session_maps_live(&app, &family) {
                        // Already in loaded() from real Add this process.
                        let is_new = seen_ready
                            .lock()
                            .map(|mut s| s.insert(fam_key.clone()))
                            .unwrap_or(false);
                        if is_new {
                            p.ready_names.push(family.clone());
                            p.skipped = p.skipped.saturating_add(1);
                        }
                        session_boot_push_ready(&family);
                        if let Ok(mut g) = registered.lock() {
                            g.push(family.clone());
                        }
                    } else if family_skip_live_success(&app, &family).is_some() {
                        // Toast only: session-active + maps — suppress failed_names,
                        // do not claim activated / ready_names.
                    } else if suppress_fail_toast_known_incapable(&app, &family) {
                        // Hard Gidugu OR soft emoji after Add=0 — disk settled + quiet.
                        // Push settled while holding progress (avoid note_settled_quiet deadlock).
                        stamp_settle_after_add_zero(&app, &family);
                        let is_new = seen_settled
                            .lock()
                            .map(|mut s| s.insert(fam_key.clone()))
                            .unwrap_or(false);
                        if is_new {
                            p.settled_names.push(family.clone());
                        }
                    } else {
                        let detail = note_register_zero(&app, &family, cause);
                        let is_new = seen_failed
                            .lock()
                            .map(|mut s| s.insert(fam_key))
                            .unwrap_or(false);
                        if is_new {
                            p.failed = p.failed.saturating_add(1);
                            p.failed_names.push(family.clone());
                            p.failed_details.push(detail);
                        }
                    }
                } else if k > 0
                    && let Ok(mut g) = registered.lock() {
                        g.push(family.clone());
                    }
                // After register: Cancel → clear remaining queue (do not mark rest ready).
                if bulk().cancel.load(Ordering::SeqCst) {
                    if let Ok(mut q) = queue.lock() {
                        q.clear();
                    }
                    emit_progress(&app);
                    break;
                }
                let last = done_n >= total;
                // Pair with emit_progress 350ms throttle; force first/last so bar starts/clears.
                let should_emit = last
                    || done_n == 1
                    || last_emit
                        .lock()
                        .map(|t| t.elapsed() >= Duration::from_millis(emit_progress_throttle_ms()))
                        .unwrap_or(true);
                if should_emit {
                    if last || done_n == 1 {
                        emit_progress_force(&app);
                    } else {
                        emit_progress(&app);
                    }
                    if let Ok(mut t) = last_emit.lock() {
                        *t = Instant::now();
                    }
                }
            }
        }));
    }
    for j in joins {
        let _ = j.join();
    }
    let mut out = registered.lock().map(|g| g.clone()).unwrap_or_default();
    out.sort_by_key(|a| {
        ready
            .iter()
            .position(|t| t.eq_ignore_ascii_case(a))
            .unwrap_or(usize::MAX)
    });
    out
}
