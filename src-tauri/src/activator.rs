//! Temporary system-wide Windows font activation (`AddFontResourceExW` / `RemoveFontResourceExW`
//! + `WM_FONTCHANGE`) without copying files into `C:\Windows\Fonts`.
//!
//! Tracks active font paths inside Tauri managed state (`ActivatorState`) and automatically
//! deactivates every tracked path on `RunEvent::Exit` in `lib.rs`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use tauri::State;

/// Managed Tauri state tracking all font file paths temporarily activated in this session.
#[derive(Default)]
pub struct ActivatorState {
    pub active_paths: Mutex<HashSet<String>>,
}

fn is_windows_system_fonts_path(path: &str) -> bool {
    let lower = path.replace('/', "\\").to_ascii_lowercase();
    lower.contains("\\windows\\fonts\\") || lower.ends_with("\\windows\\fonts")
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontResourceExW, RemoveFontResourceExW, FONT_RESOURCE_CHARACTERISTICS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SendNotifyMessageW, HWND_BROADCAST, WM_FONTCHANGE,
    };

    fn to_wide_null(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn broadcast_font_change() {
        unsafe {
            let _ = SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE, WPARAM(0), LPARAM(0));
        }
    }

    pub fn add_font_resource(font_path: &str) -> Result<i32, String> {
        let wide = to_wide_null(font_path);
        // FONT_RESOURCE_CHARACTERISTICS(0): system-wide session activation visible to
        // Photoshop, Illustrator, Figma, Word, etc. (never FR_PRIVATE, never copied to C:\Windows\Fonts).
        let added = unsafe {
            AddFontResourceExW(PCWSTR(wide.as_ptr()), FONT_RESOURCE_CHARACTERISTICS(0), None)
        };
        if added <= 0 {
            return Err(format!(
                "AddFontResourceExW returned {added} for '{font_path}'"
            ));
        }
        broadcast_font_change();
        Ok(added)
    }

    pub fn remove_font_resource(font_path: &str, broadcast: bool) -> Result<(), String> {
        let wide = to_wide_null(font_path);
        let removed = unsafe {
            RemoveFontResourceExW(PCWSTR(wide.as_ptr()), FONT_RESOURCE_CHARACTERISTICS(0).0, None)
        };
        if !removed.as_bool() {
            return Err(format!(
                "RemoveFontResourceExW failed for '{font_path}' (font may be locked or not loaded)"
            ));
        }
        if broadcast {
            broadcast_font_change();
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn broadcast_font_change() {}

    pub fn add_font_resource(_font_path: &str) -> Result<i32, String> {
        Err("Temporary font activation via Win32 GDI is only supported on Windows".to_string())
    }

    pub fn remove_font_resource(_font_path: &str, _broadcast: bool) -> Result<(), String> {
        Err("Temporary font deactivation via Win32 GDI is only supported on Windows".to_string())
    }
}

impl ActivatorState {
    /// Deactivate all temporarily activated font paths tracked in this session (called on `RunEvent::Exit`).
    pub fn deactivate_all(&self) -> usize {
        let paths: Vec<String> = match self.active_paths.lock() {
            Ok(mut guard) => guard.drain().collect(),
            Err(poisoned) => poisoned.into_inner().drain().collect(),
        };
        if paths.is_empty() {
            return 0;
        }
        let mut removed = 0usize;
        for path in &paths {
            if platform::remove_font_resource(path, false).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            platform::broadcast_font_change();
        }
        removed
    }

    pub fn is_active(&self, font_path: &str) -> bool {
        self.active_paths
            .lock()
            .map(|g| g.contains(font_path))
            .unwrap_or(false)
    }
}

/// Temporarily activate a font file system-wide for external apps without copying it to `C:\Windows\Fonts`.
#[tauri::command]
pub fn activate_font(
    state: State<'_, ActivatorState>,
    font_path: String,
) -> Result<(), String> {
    let trimmed = font_path.trim();
    if trimmed.is_empty() {
        return Err("Font path is empty".to_string());
    }
    if is_windows_system_fonts_path(trimmed) {
        return Err("Refusing to activate fonts inside C:\\Windows\\Fonts".to_string());
    }
    if !Path::new(trimmed).is_file() {
        return Err(format!("Font file does not exist: '{trimmed}'"));
    }
    {
        let guard = state
            .active_paths
            .lock()
            .map_err(|_| "Activator mutex poisoned".to_string())?;
        if guard.contains(trimmed) {
            return Ok(());
        }
    }
    platform::add_font_resource(trimmed)?;
    let mut guard = state
        .active_paths
        .lock()
        .map_err(|_| "Activator mutex poisoned".to_string())?;
    guard.insert(trimmed.to_string());
    Ok(())
}

/// Deactivate a previously activated font file and broadcast `WM_FONTCHANGE`.
#[tauri::command]
pub fn deactivate_font(
    state: State<'_, ActivatorState>,
    font_path: String,
) -> Result<(), String> {
    let trimmed = font_path.trim();
    if trimmed.is_empty() {
        return Err("Font path is empty".to_string());
    }
    if is_windows_system_fonts_path(trimmed) {
        return Err("Refusing to deactivate fonts inside C:\\Windows\\Fonts".to_string());
    }
    let res = platform::remove_font_resource(trimmed, true);
    if let Ok(mut guard) = state.active_paths.lock() {
        guard.remove(trimmed);
    }
    res
}

/// Deactivate all active paths managed by `ActivatorState` when the application exits.
pub fn cleanup_on_exit(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<ActivatorState>() {
        let _ = state.deactivate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_windows_system_fonts_directory() {
        assert!(is_windows_system_fonts_path(r"C:\Windows\Fonts\arial.ttf"));
        assert!(is_windows_system_fonts_path("c:/windows/fonts/SegoeUI.ttf"));
        assert!(!is_windows_system_fonts_path(r"C:\Users\Eric\Fonts\Inter.ttf"));
    }

    #[test]
    fn activator_state_tracks_and_drains_paths() {
        let state = ActivatorState::default();
        assert!(!state.is_active("test.ttf"));
        state
            .active_paths
            .lock()
            .unwrap()
            .insert("nonexistent-font.ttf".to_string());
        assert!(state.is_active("nonexistent-font.ttf"));
        let _ = state.deactivate_all();
        assert!(!state.is_active("nonexistent-font.ttf"));
    }
}
