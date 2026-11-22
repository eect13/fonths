//! Native Windows DirectWrite font inspection & read-only system font enumeration.
//!
//! Architecture contract (Font Manager 2.0.0):
//! - System Fonts drawer uses DirectWrite's cached system font collection
//!   (`IDWriteFactory::GetSystemFontCollection`) filtered strictly to `%SYSTEMROOT%\Fonts`
//!   so reading System Fonts is instant, read-only, and never mixes in session-activated
//!   fonts from `gdi-maps` or `Documents\Font Manager`.
//! - Uploaded / downloaded font validation and variable-axis detection use DirectWrite
//!   (`CreateFontFileReference` + `Analyze` + `IDWriteFontFace5::HasVariations`) without
//!   ever calling GDI `AddFontResourceExW` or mutating system fonts.
//! - Session-only OS activation for external apps remains in `activate.rs` (`winfont`),
//!   with batched HKCU `[FM-Session-*]` metadata so DirectWrite-aware apps see active faces
//!   while keeping zero permanent OS changes after Quit.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DWriteInspectResult {
    pub supported: bool,
    pub face_count: u32,
    pub file_type: String,
    pub face_type: String,
    pub is_variable: bool,
    pub typographic_family: Option<String>,
    pub win32_family: Option<String>,
    pub full_name: Option<String>,
    pub weight: u16,
    pub italic: bool,
    pub hresult_code: Option<i32>,
    pub error_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DWriteEngineInfo {
    pub engine: String,
    pub available: bool,
    pub activation_mode: String,
    pub system_fonts_read_only: bool,
    pub system_font_count: usize,
    pub platform: String,
    pub elevated: bool,
}

fn normalize_win_path_str(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    s
}

/// Resolve the read-only OS system fonts directory (`C:\Windows\Fonts`).
pub fn windows_system_fonts_dir() -> PathBuf {
    let windir = std::env::var("SYSTEMROOT")
        .or_else(|_| std::env::var("WINDIR"))
        .unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(windir).join("Fonts")
}

/// Strict guard: true ONLY for font files physically located inside `%SYSTEMROOT%\Fonts`.
/// Explicitly rejects `gdi-maps`, `Documents\Font Manager`, and legacy per-user
/// `Microsoft\Windows\Fonts\FontManager` paths so session-activated fonts never leak into
/// the read-only System Fonts drawer.
#[allow(dead_code)]
pub fn is_system_windows_fonts_dir(path: &Path) -> bool {
    is_system_windows_fonts_dir_with_root(path, &windows_system_fonts_dir())
}

fn is_system_windows_fonts_dir_with_root(path: &Path, sys_fonts_dir: &Path) -> bool {
    let norm = normalize_win_path_str(path);
    if norm.contains(r"\gdi-maps\")
        || norm.ends_with(r"\gdi-maps")
        || norm.contains(r"\documents\fonths\")
        || norm.ends_with(r"\documents\fonths")
        || norm.contains(r"\documents\font manager\")
        || norm.ends_with(r"\documents\font manager")
        || norm.contains(r"\microsoft\windows\fonts\fontmanager\")
        || norm.ends_with(r"\microsoft\windows\fonts\fontmanager")
    {
        return false;
    }
    let root = normalize_win_path_str(sys_fonts_dir);
    if root.is_empty() {
        return false;
    }
    norm.starts_with(&format!("{root}\\"))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::HashSet;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory, IDWriteFont, IDWriteFontCollection, IDWriteFontFace,
        IDWriteFontFace3, IDWriteFontFace5, IDWriteFontFamily, IDWriteFontFile,
        IDWriteLocalFontFileLoader, IDWriteLocalizedStrings, DWRITE_FACTORY_TYPE_SHARED,
        DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FACE_TYPE_BITMAP, DWRITE_FONT_FACE_TYPE_CFF,
        DWRITE_FONT_FACE_TYPE_OPENTYPE_COLLECTION, DWRITE_FONT_FACE_TYPE_RAW_CFF,
        DWRITE_FONT_FACE_TYPE_TRUETYPE, DWRITE_FONT_FACE_TYPE_TYPE1,
        DWRITE_FONT_FACE_TYPE_UNKNOWN, DWRITE_FONT_FACE_TYPE_VECTOR, DWRITE_FONT_FILE_TYPE,
        DWRITE_FONT_FILE_TYPE_BITMAP, DWRITE_FONT_FILE_TYPE_CFF,
        DWRITE_FONT_FILE_TYPE_OPENTYPE_COLLECTION, DWRITE_FONT_FILE_TYPE_TRUETYPE,
        DWRITE_FONT_FILE_TYPE_TYPE1_PFB, DWRITE_FONT_FILE_TYPE_TYPE1_PFM,
        DWRITE_FONT_FILE_TYPE_UNKNOWN, DWRITE_FONT_FILE_TYPE_VECTOR, DWRITE_FONT_SIMULATIONS_NONE,
        DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STYLE_OBLIQUE, DWRITE_FONT_WEIGHT_REGULAR,
        DWRITE_INFORMATIONAL_STRING_FULL_NAME, DWRITE_INFORMATIONAL_STRING_ID,
        DWRITE_INFORMATIONAL_STRING_PREFERRED_FAMILY_NAMES,
        DWRITE_INFORMATIONAL_STRING_WIN32_FAMILY_NAMES,
    };
    use windows_core::{Interface, BOOL, PCWSTR};

    fn wide_z(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_str(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn create_factory() -> Result<IDWriteFactory, String> {
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .map_err(|e| format!("DWriteCreateFactory failed: {e}"))
    }

    fn file_type_label(t: DWRITE_FONT_FILE_TYPE) -> String {
        match t {
            x if x == DWRITE_FONT_FILE_TYPE_CFF => "cff",
            x if x == DWRITE_FONT_FILE_TYPE_TRUETYPE => "truetype",
            x if x == DWRITE_FONT_FILE_TYPE_OPENTYPE_COLLECTION => "collection",
            x if x == DWRITE_FONT_FILE_TYPE_TYPE1_PFM => "type1-pfm",
            x if x == DWRITE_FONT_FILE_TYPE_TYPE1_PFB => "type1-pfb",
            x if x == DWRITE_FONT_FILE_TYPE_VECTOR => "vector",
            x if x == DWRITE_FONT_FILE_TYPE_BITMAP => "bitmap",
            x if x == DWRITE_FONT_FILE_TYPE_UNKNOWN => "unknown",
            _ => "other",
        }
        .to_string()
    }

    fn face_type_label(t: DWRITE_FONT_FACE_TYPE) -> String {
        match t {
            x if x == DWRITE_FONT_FACE_TYPE_CFF => "cff",
            x if x == DWRITE_FONT_FACE_TYPE_TRUETYPE => "truetype",
            x if x == DWRITE_FONT_FACE_TYPE_OPENTYPE_COLLECTION => "collection",
            x if x == DWRITE_FONT_FACE_TYPE_TYPE1 => "type1",
            x if x == DWRITE_FONT_FACE_TYPE_VECTOR => "vector",
            x if x == DWRITE_FONT_FACE_TYPE_BITMAP => "bitmap",
            x if x == DWRITE_FONT_FACE_TYPE_RAW_CFF => "raw-cff",
            x if x == DWRITE_FONT_FACE_TYPE_UNKNOWN => "unknown",
            _ => "other",
        }
        .to_string()
    }

    pub fn hresult_reason(hr: i32) -> &'static str {
        match hr as u32 {
            0x88985000 => "invalid or unsupported font file format (DWRITE_E_FILEFORMAT)",
            0x88985001 => "unexpected DirectWrite font error (DWRITE_E_UNEXPECTED)",
            0x88985002 => "no valid font face found in file (DWRITE_E_NOFONT)",
            0x88985003 => "font file not found on disk (DWRITE_E_FILENOTFOUND)",
            0x88985004 => "font file access denied or locked (DWRITE_E_FILEACCESS)",
            0x88985009 => "unsupported DirectWrite font operation (DWRITE_E_UNSUPPORTEDOPERATION)",
            0x8898500A => {
                "incompatible font outline or renderer format (DWRITE_E_TEXTRENDERERINCOMPATIBLE)"
            }
            0x80070002 => "font file not found (ERROR_FILE_NOT_FOUND)",
            0x80070005 => "permission denied reading font file (E_ACCESSDENIED)",
            _ => "DirectWrite rejected font file",
        }
    }

    fn read_localized_string(strings: &IDWriteLocalizedStrings) -> Option<String> {
        let count = unsafe { strings.GetCount() };
        if count == 0 {
            return None;
        }
        let mut idx = 0u32;
        let mut exists = BOOL(0);
        for locale in ["en-US", "en-us"] {
            let loc_w = wide_str(locale);
            if unsafe { strings.FindLocaleName(PCWSTR(loc_w.as_ptr()), &mut idx, &mut exists) }
                .is_ok()
                && exists.as_bool()
            {
                break;
            }
            idx = 0;
        }
        let len = unsafe { strings.GetStringLength(idx) }.ok()?;
        let mut buf = vec![0u16; (len as usize).saturating_add(1)];
        unsafe { strings.GetString(idx, &mut buf) }.ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let text = String::from_utf16_lossy(&buf[..end]).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    fn font_info_string(font: &IDWriteFont, id: DWRITE_INFORMATIONAL_STRING_ID) -> Option<String> {
        let mut strings: Option<IDWriteLocalizedStrings> = None;
        let mut exists = BOOL(0);
        if unsafe { font.GetInformationalStrings(id, &mut strings, &mut exists) }.is_ok()
            && exists.as_bool()
        {
            if let Some(s) = strings {
                return read_localized_string(&s);
            }
        }
        None
    }

    fn face3_info_string(
        face3: &IDWriteFontFace3,
        id: DWRITE_INFORMATIONAL_STRING_ID,
    ) -> Option<String> {
        let mut strings: Option<IDWriteLocalizedStrings> = None;
        let mut exists = BOOL(0);
        if unsafe { face3.GetInformationalStrings(id, &mut strings, &mut exists) }.is_ok()
            && exists.as_bool()
        {
            if let Some(s) = strings {
                return read_localized_string(&s);
            }
        }
        None
    }

    fn font_face_local_path(face: &IDWriteFontFace) -> Option<PathBuf> {
        let mut count = 0u32;
        let _ = unsafe { face.GetFiles(&mut count, None) };
        if count == 0 {
            return None;
        }
        let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
        unsafe { face.GetFiles(&mut count, Some(files.as_mut_ptr())) }.ok()?;
        for maybe_file in files.into_iter().take(count as usize) {
            let Some(file) = maybe_file else {
                continue;
            };
            let Ok(loader) = (unsafe { file.GetLoader() }) else {
                continue;
            };
            let Ok(local) = loader.cast::<IDWriteLocalFontFileLoader>() else {
                continue;
            };
            let mut key_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut key_size = 0u32;
            if unsafe { file.GetReferenceKey(&mut key_ptr, &mut key_size) }.is_err()
                || key_ptr.is_null()
                || key_size == 0
            {
                continue;
            }
            let Ok(len) = (unsafe { local.GetFilePathLengthFromKey(key_ptr, key_size) }) else {
                continue;
            };
            let mut buf = vec![0u16; (len as usize).saturating_add(1)];
            if unsafe { local.GetFilePathFromKey(key_ptr, key_size, &mut buf) }.is_err() {
                continue;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let raw = String::from_utf16_lossy(&buf[..end]).trim().to_string();
            if !raw.is_empty() {
                let p = PathBuf::from(raw.strip_prefix(r"\\?\").unwrap_or(&raw));
                return Some(p);
            }
        }
        None
    }

    fn face_has_variations(face: &IDWriteFontFace) -> bool {
        face.cast::<IDWriteFontFace5>()
            .map(|f5| unsafe { f5.HasVariations().as_bool() })
            .unwrap_or(false)
    }

    fn collect_candidate_fonts(family: &IDWriteFontFamily) -> Vec<IDWriteFont> {
        let mut out = Vec::new();
        if let Ok(primary) = unsafe {
            family.GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT_REGULAR,
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
        } {
            if unsafe { primary.GetSimulations() } == DWRITE_FONT_SIMULATIONS_NONE {
                out.push(primary);
            }
        }
        let count = unsafe { family.GetFontCount() };
        let mut extras: Vec<(i32, IDWriteFont)> = Vec::new();
        for idx in 0..count {
            let Ok(font) = (unsafe { family.GetFont(idx) }) else {
                continue;
            };
            if unsafe { font.GetSimulations() } != DWRITE_FONT_SIMULATIONS_NONE {
                continue;
            }
            let style_rank = if unsafe { font.GetStyle() } == DWRITE_FONT_STYLE_NORMAL {
                0
            } else {
                10_000
            };
            let weight_dist = (unsafe { font.GetWeight() }.0 - 400).abs();
            extras.push((style_rank + weight_dist, font));
        }
        extras.sort_by_key(|(rank, _)| *rank);
        for (_, font) in extras {
            out.push(font);
        }
        if out.is_empty() && count > 0 {
            if let Ok(fallback) = unsafe { family.GetFont(0) } {
                out.push(fallback);
            }
        }
        out
    }

    pub fn list_system_fonts_dwrite() -> Result<Vec<crate::parse::SystemFontOut>, String> {
        let factory = create_factory()?;
        let mut collection: Option<IDWriteFontCollection> = None;
        unsafe { factory.GetSystemFontCollection(&mut collection, false) }
            .map_err(|e| format!("GetSystemFontCollection failed: {e}"))?;
        let collection =
            collection.ok_or_else(|| "DirectWrite returned null system font collection".to_string())?;

        let sys_fonts_dir = super::windows_system_fonts_dir();
        let family_count = unsafe { collection.GetFontFamilyCount() };
        let mut out: Vec<crate::parse::SystemFontOut> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for idx in 0..family_count {
            if out.len() >= 800 {
                break;
            }
            let Ok(dw_family) = (unsafe { collection.GetFontFamily(idx) }) else {
                continue;
            };
            let wws_family = unsafe { dw_family.GetFamilyNames() }
                .ok()
                .and_then(|s| read_localized_string(&s));

            let candidates = collect_candidate_fonts(&dw_family);
            let mut picked: Option<(IDWriteFont, IDWriteFontFace, PathBuf)> = None;
            for font in candidates {
                let Ok(face) = (unsafe { font.CreateFontFace() }) else {
                    continue;
                };
                let Some(path) = font_face_local_path(&face) else {
                    continue;
                };
                if super::is_system_windows_fonts_dir_with_root(&path, &sys_fonts_dir)
                    && path.is_file()
                {
                    picked = Some((font, face, path));
                    break;
                }
            }
            let Some((font, face, path)) = picked else {
                continue;
            };

            let family_name = font_info_string(&font, DWRITE_INFORMATIONAL_STRING_PREFERRED_FAMILY_NAMES)
                .or(wws_family)
                .unwrap_or_default()
                .trim()
                .to_string();
            if family_name.is_empty() {
                continue;
            }
            let key = family_name.to_ascii_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let full_raw = font_info_string(&font, DWRITE_INFORMATIONAL_STRING_FULL_NAME)
                .unwrap_or_else(|| family_name.clone());
            let full_name = if full_raw.trim().is_empty()
                || full_raw.trim().eq_ignore_ascii_case(&family_name)
            {
                family_name.clone()
            } else {
                full_raw.trim().to_string()
            };

            let weight = unsafe { font.GetWeight() }.0.clamp(1, 1000) as u16;
            let style = unsafe { font.GetStyle() };
            let italic = style == DWRITE_FONT_STYLE_ITALIC || style == DWRITE_FONT_STYLE_OBLIQUE;
            let variable = face_has_variations(&face);
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("font.ttf")
                .to_string();
            let path_s = path.to_string_lossy().into_owned();

            out.push(crate::parse::SystemFontOut {
                family: family_name,
                full_name,
                path: path_s,
                file_name,
                italic,
                variable,
                weight,
            });
        }

        out.sort_by_key(|a| a.family.to_lowercase());
        Ok(out)
    }

    pub fn analyze_font_file(path: &Path) -> DWriteInspectResult {
        if !path.is_file() {
            return DWriteInspectResult {
                supported: false,
                face_count: 0,
                file_type: "missing".into(),
                face_type: "unknown".into(),
                is_variable: false,
                typographic_family: None,
                win32_family: None,
                full_name: None,
                weight: 400,
                italic: false,
                hresult_code: None,
                error_reason: Some("font file missing on disk".into()),
            };
        }
        let factory = match create_factory() {
            Ok(f) => f,
            Err(err) => {
                return DWriteInspectResult {
                    supported: false,
                    face_count: 0,
                    file_type: "unknown".into(),
                    face_type: "unknown".into(),
                    is_variable: false,
                    typographic_family: None,
                    win32_family: None,
                    full_name: None,
                    weight: 400,
                    italic: false,
                    hresult_code: None,
                    error_reason: Some(err),
                };
            }
        };
        let path_w = wide_z(path.as_os_str());
        let font_file = match unsafe { factory.CreateFontFileReference(PCWSTR(path_w.as_ptr()), None) }
        {
            Ok(f) => f,
            Err(err) => {
                let hr = err.code().0;
                return DWriteInspectResult {
                    supported: false,
                    face_count: 0,
                    file_type: "unknown".into(),
                    face_type: "unknown".into(),
                    is_variable: false,
                    typographic_family: None,
                    win32_family: None,
                    full_name: None,
                    weight: 400,
                    italic: false,
                    hresult_code: Some(hr),
                    error_reason: Some(hresult_reason(hr).into()),
                };
            }
        };

        let mut supported = BOOL(0);
        let mut file_type = DWRITE_FONT_FILE_TYPE_UNKNOWN;
        let mut face_type = DWRITE_FONT_FACE_TYPE_UNKNOWN;
        let mut face_count = 0u32;
        if let Err(err) = unsafe {
            font_file.Analyze(
                &mut supported,
                &mut file_type,
                Some(&mut face_type),
                &mut face_count,
            )
        } {
            let hr = err.code().0;
            return DWriteInspectResult {
                supported: false,
                face_count: 0,
                file_type: file_type_label(file_type),
                face_type: face_type_label(face_type),
                is_variable: false,
                typographic_family: None,
                win32_family: None,
                full_name: None,
                weight: 400,
                italic: false,
                hresult_code: Some(hr),
                error_reason: Some(hresult_reason(hr).into()),
            };
        }

        if !supported.as_bool() || face_count == 0 {
            return DWriteInspectResult {
                supported: false,
                face_count,
                file_type: file_type_label(file_type),
                face_type: face_type_label(face_type),
                is_variable: false,
                typographic_family: None,
                win32_family: None,
                full_name: None,
                weight: 400,
                italic: false,
                hresult_code: None,
                error_reason: Some(format!(
                    "DirectWrite does not support font format ({}/{})",
                    file_type_label(file_type),
                    face_type_label(face_type)
                )),
            };
        }

        let files = [Some(font_file)];
        let face = match unsafe {
            factory.CreateFontFace(face_type, &files, 0, DWRITE_FONT_SIMULATIONS_NONE)
        } {
            Ok(f) => f,
            Err(err) => {
                let hr = err.code().0;
                return DWriteInspectResult {
                    supported: false,
                    face_count,
                    file_type: file_type_label(file_type),
                    face_type: face_type_label(face_type),
                    is_variable: false,
                    typographic_family: None,
                    win32_family: None,
                    full_name: None,
                    weight: 400,
                    italic: false,
                    hresult_code: Some(hr),
                    error_reason: Some(hresult_reason(hr).into()),
                };
            }
        };

        let is_variable = face_has_variations(&face);
        let mut typographic_family = None;
        let mut win32_family = None;
        let mut full_name = None;
        let mut weight = 400u16;
        let mut italic = false;

        if let Ok(face3) = face.cast::<IDWriteFontFace3>() {
            typographic_family =
                face3_info_string(&face3, DWRITE_INFORMATIONAL_STRING_PREFERRED_FAMILY_NAMES)
                    .or_else(|| unsafe { face3.GetFamilyNames() }.ok().and_then(|s| read_localized_string(&s)));
            win32_family = face3_info_string(&face3, DWRITE_INFORMATIONAL_STRING_WIN32_FAMILY_NAMES)
                .or_else(|| typographic_family.clone());
            full_name = face3_info_string(&face3, DWRITE_INFORMATIONAL_STRING_FULL_NAME);
            weight = unsafe { face3.GetWeight() }.0.clamp(1, 1000) as u16;
            let st = unsafe { face3.GetStyle() };
            italic = st == DWRITE_FONT_STYLE_ITALIC || st == DWRITE_FONT_STYLE_OBLIQUE;
        }

        DWriteInspectResult {
            supported: true,
            face_count,
            file_type: file_type_label(file_type),
            face_type: face_type_label(face_type),
            is_variable,
            typographic_family,
            win32_family,
            full_name,
            weight,
            italic,
            hresult_code: None,
            error_reason: None,
        }
    }

    pub fn engine_available() -> bool {
        create_factory().is_ok()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn list_system_fonts_dwrite() -> Result<Vec<crate::parse::SystemFontOut>, String> {
        Err("DirectWrite is Windows-only".into())
    }

    pub fn analyze_font_file(_path: &Path) -> DWriteInspectResult {
        DWriteInspectResult {
            supported: false,
            face_count: 0,
            file_type: "unavailable".into(),
            face_type: "unavailable".into(),
            is_variable: false,
            typographic_family: None,
            win32_family: None,
            full_name: None,
            weight: 400,
            italic: false,
            hresult_code: None,
            error_reason: Some("DirectWrite is only available on Windows".into()),
        }
    }

    pub fn engine_available() -> bool {
        false
    }
}

pub fn list_system_fonts_dwrite() -> Result<Vec<crate::parse::SystemFontOut>, String> {
    imp::list_system_fonts_dwrite()
}

pub fn analyze_font_file(path: &Path) -> DWriteInspectResult {
    imp::analyze_font_file(path)
}

#[allow(dead_code)]
pub fn file_has_variations_dwrite(path: &Path) -> Option<bool> {
    let res = analyze_font_file(path);
    res.supported.then_some(res.is_variable)
}

pub fn win32_family_name_dwrite(path: &Path) -> Option<String> {
    let res = analyze_font_file(path);
    if !res.supported {
        return None;
    }
    res.win32_family.or(res.typographic_family)
}

pub fn dwrite_engine_info_blocking() -> Result<DWriteEngineInfo, String> {
    let available = imp::engine_available();
    let system_font_count = crate::parse::list_system_fonts_blocking()
        .map(|rows| rows.len())
        .unwrap_or(0);
    Ok(DWriteEngineInfo {
        engine: if available {
            "DirectWrite".into()
        } else {
            "ttf-parser".into()
        },
        available,
        activation_mode: "session-only".into(),
        system_fonts_read_only: true,
        system_font_count,
        platform: std::env::consts::OS.to_string(),
        elevated: crate::is_elevated(),
    })
}

#[tauri::command]
pub async fn dwrite_engine_info() -> Result<DWriteEngineInfo, String> {
    tauri::async_runtime::spawn_blocking(dwrite_engine_info_blocking)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn dwrite_inspect_font(path: String) -> Result<DWriteInspectResult, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(analyze_font_file(Path::new(&path))))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_dir_guard_rejects_session_paths() {
        let sys = Path::new(r"C:\Windows\Fonts");
        assert!(is_system_windows_fonts_dir_with_root(
            Path::new(r"C:\Windows\Fonts\arial.ttf"),
            sys
        ));
        assert!(is_system_windows_fonts_dir_with_root(
            Path::new(r"\\?\c:\windows\fonts\seguiemj.ttf"),
            sys
        ));
        assert!(!is_system_windows_fonts_dir_with_root(
            Path::new(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\abcd.ttf"),
            sys
        ));
        assert!(!is_system_windows_fonts_dir_with_root(
            Path::new(r"C:\Users\Eric\Documents\Font Manager\Inter\inter-variable.ttf"),
            sys
        ));
        assert!(!is_system_windows_fonts_dir_with_root(
            Path::new(r"C:\Users\Eric\AppData\Local\Microsoft\Windows\Fonts\FontManager\a.ttf"),
            sys
        ));
    }

    #[cfg(windows)]
    #[test]
    fn dwrite_analyzes_real_vf_fixtures() {
        let dir = std::env::temp_dir().join(format!("fm-dwrite-vf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let fig = dir.join("Figtree-wght.ttf");
        std::fs::write(&fig, include_bytes!("../../tests/fixtures/Figtree-wght.ttf")).unwrap();
        let fig_res = analyze_font_file(&fig);
        assert!(fig_res.supported, "Figtree must pass DirectWrite Analyze: {fig_res:?}");
        assert!(fig_res.is_variable, "Figtree must report variable=true");
        assert_eq!(fig_res.typographic_family.as_deref(), Some("Figtree"));

        let inter = dir.join("Inter-opsz-wght.ttf");
        std::fs::write(&inter, include_bytes!("../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();
        let inter_res = analyze_font_file(&inter);
        assert!(inter_res.supported, "Inter must pass DirectWrite Analyze: {inter_res:?}");
        assert!(inter_res.is_variable, "Inter must report variable=true");
        assert_eq!(inter_res.typographic_family.as_deref(), Some("Inter"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn dwrite_rejects_corrupt_font_with_named_reason() {
        let dir = std::env::temp_dir().join(format!("fm-dwrite-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let bad = dir.join("corrupt.ttf");
        std::fs::write(&bad, b"not a valid font payload at all 1234567890").unwrap();
        let res = analyze_font_file(&bad);
        assert!(!res.supported);
        assert!(res.error_reason.as_deref().map(|s| !s.is_empty()).unwrap_or(false));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn dwrite_system_fonts_are_strictly_windows_fonts_only() {
        let t0 = std::time::Instant::now();
        let rows = list_system_fonts_dwrite().expect("DirectWrite system font collection");
        let elapsed_ms = t0.elapsed().as_millis();
        eprintln!(
            "[dwrite] list_system_fonts_dwrite returned {} families in {} ms",
            rows.len(),
            elapsed_ms
        );
        assert!(!rows.is_empty(), "Windows machine must have system fonts");
        for row in &rows {
            let p = Path::new(&row.path);
            assert!(
                is_system_windows_fonts_dir(p),
                "System font leaked outside Windows\\Fonts: {} ({})",
                row.family,
                row.path
            );
            let lower = row.path.to_ascii_lowercase();
            assert!(!lower.contains("gdi-maps"));
            assert!(!lower.contains("font manager"));
        }
    }
}
