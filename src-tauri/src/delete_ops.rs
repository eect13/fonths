//! Smart Deletion & Temporary Fonts Cleanup Engine.
//!
//! - User library deletions honor the global `deleteMode` preference:
//!   - `"recycle"` (default): Move to Windows Recycle Bin (`SHFileOperationW` + `FOF_ALLOWUNDO`).
//!   - `"permanent"`: Permanently unlink file/directory after releasing font handles.
//! - Temporary/preview/stage files (`gdi-maps`, `.part`, preview caches) ALWAYS bypass the
//!   Recycle Bin and are permanently deleted automatically (Temporary Fonts Rule).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static PERMANENT_DELETE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_permanent_delete(permanent: bool) {
    PERMANENT_DELETE_MODE.store(permanent, Ordering::SeqCst);
}

pub fn is_permanent_delete() -> bool {
    PERMANENT_DELETE_MODE.load(Ordering::SeqCst)
}

fn is_lock_error(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5) | Some(32) | Some(33))
        || err.kind() == ErrorKind::PermissionDenied
}

#[cfg(windows)]
fn send_to_recycle_bin_windows(path: &Path) -> Result<(), String> {
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
    extern "system" {
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
    if !path.exists() {
        return Ok(());
    }
    if rc == 32 || rc == 5 || rc == 0x78 {
        return Err("files locked — close Word or Adobe, then Retry".into());
    }
    Err(format!(
        "could not move to Recycle Bin (code {rc}). Close Word or Adobe, then Retry."
    ))
}

fn remove_dir_permanently(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    for attempt in 0..5u32 {
        match fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) if is_lock_error(&err) => {
                thread::sleep(Duration::from_millis(40 * u64::from(attempt + 1)));
            }
            Err(err) => return Err(format!("could not permanently delete {}: {err}", dir.display())),
        }
    }
    #[cfg(all(windows, not(test)))]
    if dir.exists() && crate::is_elevated() {
        crate::activate::restart_font_cache_for_unlock();
        for attempt in 0..3u32 {
            match fs::remove_dir_all(dir) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
                Err(err) if is_lock_error(&err) => {
                    thread::sleep(Duration::from_millis(80 * u64::from(attempt + 1)));
                }
                Err(err) => {
                    return Err(format!("could not permanently delete {}: {err}", dir.display()));
                }
            }
        }
    }
    if dir.exists() {
        return Err("files locked — close Word or Adobe, then Retry".into());
    }
    Ok(())
}

fn remove_file_permanently(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    for attempt in 0..5u32 {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) if is_lock_error(&err) => {
                thread::sleep(Duration::from_millis(40 * u64::from(attempt + 1)));
            }
            Err(err) => return Err(format!("could not permanently delete {}: {err}", path.display())),
        }
    }
    #[cfg(all(windows, not(test)))]
    if path.exists() && crate::is_elevated() {
        crate::activate::restart_font_cache_for_unlock();
        for attempt in 0..3u32 {
            match fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
                Err(err) if is_lock_error(&err) => {
                    thread::sleep(Duration::from_millis(80 * u64::from(attempt + 1)));
                }
                Err(err) => {
                    return Err(format!("could not permanently delete {}: {err}", path.display()));
                }
            }
        }
    }
    if path.exists() {
        return Err("files locked — close Word or Adobe, then Retry".into());
    }
    Ok(())
}

/// Delete a user font directory honoring `permanent` (Permanent Delete vs. Windows Recycle Bin).
pub fn delete_user_dir(dir: &Path, permanent: bool) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    if permanent {
        return remove_dir_permanently(dir);
    }
    #[cfg(windows)]
    {
        match send_to_recycle_bin_windows(dir) {
            Ok(()) => Ok(()),
            Err(err) => {
                #[cfg(not(test))]
                if dir.exists() && crate::is_elevated() {
                    crate::activate::restart_font_cache_for_unlock();
                    return send_to_recycle_bin_windows(dir);
                }
                Err(err)
            }
        }
    }
    #[cfg(not(windows))]
    {
        remove_dir_permanently(dir)
    }
}

/// Delete a single user font file honoring `permanent` (Permanent Delete vs. Windows Recycle Bin).
pub fn delete_user_file(path: &Path, permanent: bool) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if permanent {
        return remove_file_permanently(path);
    }
    #[cfg(windows)]
    {
        match send_to_recycle_bin_windows(path) {
            Ok(()) => Ok(()),
            Err(err) => {
                #[cfg(not(test))]
                if path.exists() && crate::is_elevated() {
                    crate::activate::restart_font_cache_for_unlock();
                    return send_to_recycle_bin_windows(path);
                }
                Err(err)
            }
        }
    }
    #[cfg(not(windows))]
    {
        remove_file_permanently(path)
    }
}

/// Temporary Fonts Rule: permanently remove temp/preview/stage files without touching Recycle Bin.
pub fn delete_temp_file_permanently(path: &Path) -> Result<(), String> {
    remove_file_permanently(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_mode_toggle_and_permanent_delete_remove_files_without_recycle_bin() {
        let dir = std::env::temp_dir().join(format!("fm-delete-ops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let f1 = dir.join("user.ttf");
        fs::write(&f1, b"font-data").unwrap();
        delete_user_file(&f1, true).expect("permanent user file delete");
        assert!(!f1.exists());

        let sub = dir.join("FamilyDir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a.ttf"), b"font-data").unwrap();
        delete_user_dir(&sub, true).expect("permanent user dir delete");
        assert!(!sub.exists());

        let tmp = dir.join("stage.part");
        fs::write(&tmp, b"temp-stage").unwrap();
        delete_temp_file_permanently(&tmp).expect("permanent temp delete");
        assert!(!tmp.exists());

        let prev = is_permanent_delete();
        set_permanent_delete(true);
        assert!(is_permanent_delete());
        set_permanent_delete(false);
        assert!(!is_permanent_delete());
        set_permanent_delete(prev);

        let _ = fs::remove_dir_all(&dir);
    }
}
