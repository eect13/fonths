//! Headless pure-Rust Windows background daemon (`fonths.exe --daemon`).
//!
//! Runs with zero WebView2 / Chromium overhead (~6–10 MB RAM):
//! - Acquires a per-user single-instance mutex (`Local\FonthsHeadlessDaemonMutex`).
//! - On login launch (`--daemon`), immediately activates remembered `.session-families.json`
//!   fonts from `gdi-maps` in parallel via `activate::session_begin_headless()`.
//! - On UI close handoff (`--daemon --handoff`), inherits ownership of the already-live GDI
//!   session while the WebView2 UI process exits completely (freeing ~150–300 MB RAM).
//! - Owns a native Win32 `Shell_NotifyIconW` system tray icon to re-launch the on-demand UI,
//!   open `Documents\Fonths`, or quit & unload every temporary session font cleanly.

use std::path::Path;

/// CLI flag starting the pure-Rust headless tray daemon (no WebView2 initialized).
pub const DAEMON_ARG: &str = "--daemon";
/// Secondary flag indicating the UI already registered active fonts and is handing off ownership.
pub const HANDOFF_ARG: &str = "--handoff";
/// CLI flag used by NSIS installer/uninstaller hooks to cleanly unload fonts and stop a running daemon.
pub const QUIT_DAEMON_ARG: &str = "--quit-daemon";

#[cfg(windows)]
mod win_daemon {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    const DAEMON_MUTEX_NAME: &str = "Local\\FonthsHeadlessDaemonMutex";
    const DAEMON_STOP_EVENT_NAME: &str = "Local\\FonthsDaemonStopEvent";
    const DAEMON_QUIT_UNLOAD_EVENT_NAME: &str = "Local\\FonthsDaemonQuitUnloadEvent";

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_COMMAND: u32 = 0x0111;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_APP: u32 = 0x8000;
    const WM_TRAYICON: u32 = WM_APP + 1;
    const WM_DAEMON_STOP: u32 = WM_APP + 2;
    const WM_DAEMON_QUIT_UNLOAD: u32 = WM_APP + 3;

    const NIM_ADD: u32 = 0x0000_0000;
    const NIM_DELETE: u32 = 0x0000_0002;
    const NIF_MESSAGE: u32 = 0x0000_0001;
    const NIF_ICON: u32 = 0x0000_0002;
    const NIF_TIP: u32 = 0x0000_0004;

    const MF_STRING: u32 = 0x0000_0000;
    const MF_SEPARATOR: u32 = 0x0000_0800;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_NONOTIFY: u32 = 0x0080;
    const TPM_RETURNCMD: u32 = 0x0100;

    const CMD_SHOW: usize = 1001;
    const CMD_FOLDER: usize = 1002;
    const CMD_QUIT: usize = 1003;

    static DAEMON_HWND: AtomicIsize = AtomicIsize::new(0);
    static TRAY_ICON_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
    static DAEMON_QUITTING: AtomicBool = AtomicBool::new(false);
    static DAEMON_UNLOAD_ON_EXIT: AtomicBool = AtomicBool::new(false);

    #[repr(C)]
    #[allow(clippy::upper_case_acronyms)]
    struct POINT {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[allow(clippy::upper_case_acronyms)]
    struct MSG {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: POINT,
    }

    #[repr(C)]
    #[allow(clippy::upper_case_acronyms)]
    struct WNDCLASSEXW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: isize,
        h_icon: isize,
        h_cursor: isize,
        hbr_background: isize,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: isize,
    }

    #[repr(C)]
    #[allow(clippy::upper_case_acronyms)]
    struct NOTIFYICONDATAW {
        cb_size: u32,
        hwnd: isize,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: isize,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_timeout_or_version: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: [u8; 16],
        h_balloon_icon: isize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attrs: *const core::ffi::c_void, initial_owner: i32, name: *const u16) -> isize;
        fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn CreateEventW(attrs: *const core::ffi::c_void, manual_reset: i32, initial_state: i32, name: *const u16) -> isize;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn SetEvent(handle: isize) -> i32;
        fn ResetEvent(handle: isize) -> i32;
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
        fn GetModuleHandleW(name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassExW(wc: *const WNDCLASSEXW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const core::ffi::c_void,
        ) -> isize;
        fn DefWindowProcW(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize;
        fn GetMessageW(msg: *mut MSG, hwnd: isize, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const MSG) -> i32;
        fn DispatchMessageW(msg: *const MSG) -> isize;
        fn PostMessageW(hwnd: isize, msg: u32, wp: usize, lp: isize) -> i32;
        fn PostQuitMessage(code: i32);
        fn DestroyWindow(hwnd: isize) -> i32;
        fn LoadIconW(instance: isize, name: *const u16) -> isize;
        fn CreatePopupMenu() -> isize;
        fn AppendMenuW(menu: isize, flags: u32, id: usize, item: *const u16) -> i32;
        fn TrackPopupMenu(menu: isize, flags: u32, x: i32, y: i32, reserved: i32, hwnd: isize, rect: *const core::ffi::c_void) -> i32;
        fn DestroyMenu(menu: isize) -> i32;
        fn GetCursorPos(pt: *mut POINT) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn RegisterWindowMessageW(name: *const u16) -> u32;
        fn DestroyIcon(icon: isize) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn Shell_NotifyIconW(message: u32, data: *const NOTIFYICONDATAW) -> i32;
        fn ExtractIconExW(
            lpsz_file: *const u16,
            n_icon_index: i32,
            phicon_large: *mut isize,
            phicon_small: *mut isize,
            n_icons: u32,
        ) -> u32;
    }

    fn wide_z(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn is_daemon_running() -> bool {
        if crate::activate::sandbox_active() {
            return false;
        }
        let name = wide_z(DAEMON_MUTEX_NAME);
        unsafe {
            let h = OpenMutexW(SYNCHRONIZE, 0, name.as_ptr());
            if h != 0 {
                let _ = CloseHandle(h);
                true
            } else {
                false
            }
        }
    }

    pub fn spawn_daemon(handoff: bool) -> Result<(), String> {
        if crate::activate::sandbox_active() || is_daemon_running() {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg(DAEMON_ARG);
        if handoff {
            cmd.arg(HANDOFF_ARG);
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("failed to spawn headless daemon: {e}"))?;
        Ok(())
    }

    pub fn signal_daemon_stop() {
        if crate::activate::sandbox_active() {
            return;
        }
        let name = wide_z(DAEMON_STOP_EVENT_NAME);
        unsafe {
            let ev = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if ev != 0 {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
        }
    }

    /// Signal any running headless daemon to unload session fonts and exit cleanly, waiting up to `timeout_ms`.
    pub fn quit_daemon_and_wait(timeout_ms: u64) -> bool {
        if !is_daemon_running() {
            return true;
        }
        let name = wide_z(DAEMON_QUIT_UNLOAD_EVENT_NAME);
        unsafe {
            let ev = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if ev != 0 {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            } else {
                signal_daemon_stop();
            }
        }
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(200));
        while Instant::now() < deadline {
            if !is_daemon_running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        !is_daemon_running()
    }

    fn launch_ui_process() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let _ = std::process::Command::new(exe)
            .creation_flags(DETACHED_PROCESS)
            .spawn();
    }

    fn open_documents_folder() {
        if let Some(root) = crate::activate::documents_root_headless() {
            let _ = std::fs::create_dir_all(&root);
            let _ = std::process::Command::new("explorer.exe")
                .arg(root)
                .creation_flags(DETACHED_PROCESS)
                .spawn();
        }
    }

    fn make_nid(hwnd: isize, h_icon: isize, tooltip: &str) -> NOTIFYICONDATAW {
        let mut tip = [0u16; 128];
        for (i, ch) in OsStr::new(tooltip).encode_wide().take(127).enumerate() {
            tip[i] = ch;
        }
        NOTIFYICONDATAW {
            cb_size: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hwnd,
            u_id: 1,
            u_flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            u_callback_message: WM_TRAYICON,
            h_icon,
            sz_tip: tip,
            dw_state: 0,
            dw_state_mask: 0,
            sz_info: [0u16; 256],
            u_timeout_or_version: 0,
            sz_info_title: [0u16; 64],
            dw_info_flags: 0,
            guid_item: [0u8; 16],
            h_balloon_icon: 0,
        }
    }

    fn trigger_quit(hwnd: isize, unload_fonts: bool) {
        if DAEMON_QUITTING.swap(true, Ordering::SeqCst) {
            return;
        }
        DAEMON_UNLOAD_ON_EXIT.store(unload_fonts, Ordering::SeqCst);
        // Remove the tray icon immediately so Quit feels instant with zero freeze;
        // `session_end_headless()` runs right after the message loop exits cleanly.
        unsafe {
            let nid = make_nid(hwnd, 0, "");
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            let _ = DestroyWindow(hwnd);
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize {
        let tb_msg = TASKBAR_CREATED_MSG.load(Ordering::Relaxed);
        if tb_msg != 0 && msg == tb_msg && !DAEMON_QUITTING.load(Ordering::SeqCst) {
            let icon = TRAY_ICON_HANDLE.load(Ordering::Relaxed);
            let nid = make_nid(hwnd, icon, "Fonths — running in background");
            // SAFETY: Win32 window callback with valid initialized HWND and NOTIFYICONDATAW.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            }
            return 0;
        }
        match msg {
            WM_TRAYICON => {
                let ev = (lp & 0xFFFF) as u32;
                if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                    launch_ui_process();
                    return 0;
                }
                if ev == WM_RBUTTONUP {
                    // SAFETY: Win32 message thread creates, displays, and destroys transient popup menu.
                    let chosen = unsafe {
                        let menu = CreatePopupMenu();
                        if menu == 0 {
                            0usize
                        } else {
                            let show_w = wide_z("Show Fonths");
                            let folder_w = wide_z("Open Documents folder");
                            let quit_w = wide_z("Quit Fonths (unload fonts)");
                            let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW, show_w.as_ptr());
                            let _ = AppendMenuW(menu, MF_STRING, CMD_FOLDER, folder_w.as_ptr());
                            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, quit_w.as_ptr());
                            let mut pt = POINT { x: 0, y: 0 };
                            let _ = GetCursorPos(&mut pt);
                            let _ = SetForegroundWindow(hwnd);
                            let id = TrackPopupMenu(
                                menu,
                                TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_RETURNCMD,
                                pt.x,
                                pt.y,
                                0,
                                hwnd,
                                std::ptr::null(),
                            ) as usize;
                            let _ = DestroyMenu(menu);
                            id
                        }
                    };
                    match chosen {
                        CMD_SHOW => launch_ui_process(),
                        CMD_FOLDER => open_documents_folder(),
                        CMD_QUIT => trigger_quit(hwnd, true),
                        _ => {}
                    }
                    return 0;
                }
                0
            }
            WM_COMMAND => {
                match wp & 0xFFFF {
                    CMD_SHOW => launch_ui_process(),
                    CMD_FOLDER => open_documents_folder(),
                    CMD_QUIT => trigger_quit(hwnd, true),
                    _ => {}
                }
                0
            }
            WM_CLOSE => {
                trigger_quit(hwnd, true);
                0
            }
            WM_DAEMON_STOP => {
                // Stopped externally by the UI quitting (UI runs session_end itself).
                trigger_quit(hwnd, false);
                0
            }
            WM_DAEMON_QUIT_UNLOAD => {
                // Stopped externally by `--quit-daemon` (e.g., NSIS installer upgrade).
                trigger_quit(hwnd, true);
                0
            }
            WM_DESTROY => {
                // SAFETY: Terminates daemon message loop cleanly on window destruction.
                unsafe { PostQuitMessage(0) };
                0
            }
            // SAFETY: Forwards unhandled Win32 messages on valid daemon HWND.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    fn extract_small_exe_icon(h_instance: isize) -> (isize, bool) {
        if let Ok(exe) = std::env::current_exe() {
            let exe_w = wide_z(&exe.to_string_lossy());
            let mut small: isize = 0;
            let extracted = unsafe {
                ExtractIconExW(exe_w.as_ptr(), 0, std::ptr::null_mut(), &mut small, 1)
            };
            if extracted > 0 && small != 0 {
                return (small, true);
            }
        }
        let app_icon = unsafe { LoadIconW(h_instance, 32512usize as *const u16) };
        if app_icon != 0 {
            (app_icon, false)
        } else {
            (unsafe { LoadIconW(0, 32512usize as *const u16) }, false)
        }
    }

    pub fn run_headless_daemon(handoff: bool) {
        let mutex_w = wide_z(DAEMON_MUTEX_NAME);
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_w.as_ptr()) };
        if mutex == 0 {
            return;
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return;
        }

        let stop_w = wide_z(DAEMON_STOP_EVENT_NAME);
        let stop_ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, stop_w.as_ptr()) };
        if stop_ev != 0 {
            unsafe {
                let _ = ResetEvent(stop_ev);
            }
        }

        let quit_unload_w = wide_z(DAEMON_QUIT_UNLOAD_EVENT_NAME);
        let quit_unload_ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, quit_unload_w.as_ptr()) };
        if quit_unload_ev != 0 {
            unsafe {
                let _ = ResetEvent(quit_unload_ev);
            }
        }

        // Immediate headless font activation on login (zero WebView2 delay).
        if !handoff {
            std::thread::spawn(|| {
                crate::activate::session_begin_headless();
            });
        }

        unsafe {
            let h_instance = GetModuleHandleW(std::ptr::null());
            let class_name = wide_z("FonthsHeadlessDaemonWindow");
            let tb_name = wide_z("TaskbarCreated");
            TASKBAR_CREATED_MSG.store(RegisterWindowMessageW(tb_name.as_ptr()), Ordering::Relaxed);
            let (h_icon, owns_icon) = extract_small_exe_icon(h_instance);
            TRAY_ICON_HANDLE.store(h_icon, Ordering::Relaxed);

            let wc = WNDCLASSEXW {
                cb_size: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfn_wnd_proc: wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon,
                h_cursor: 0,
                hbr_background: 0,
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: h_icon,
            };
            let _ = RegisterClassExW(&wc);

            let title = wide_z("Fonths Daemon");
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                h_instance,
                std::ptr::null(),
            );
            if hwnd != 0 {
                DAEMON_HWND.store(hwnd, Ordering::SeqCst);
                let nid = make_nid(hwnd, h_icon, "Fonths — running in background");
                let _ = Shell_NotifyIconW(NIM_ADD, &nid);

                if stop_ev != 0 {
                    std::thread::spawn(move || {
                        let _ = WaitForSingleObject(stop_ev, INFINITE);
                        let target = DAEMON_HWND.load(Ordering::SeqCst);
                        if target != 0 {
                            let _ = PostMessageW(target, WM_DAEMON_STOP, 0, 0);
                        }
                    });
                }
                if quit_unload_ev != 0 {
                    std::thread::spawn(move || {
                        let _ = WaitForSingleObject(quit_unload_ev, INFINITE);
                        let target = DAEMON_HWND.load(Ordering::SeqCst);
                        if target != 0 {
                            let _ = PostMessageW(target, WM_DAEMON_QUIT_UNLOAD, 0, 0);
                        }
                    });
                }

                let mut msg = MSG {
                    hwnd: 0,
                    message: 0,
                    w_param: 0,
                    l_param: 0,
                    time: 0,
                    pt: POINT { x: 0, y: 0 },
                };
                while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            if owns_icon && h_icon != 0 {
                let _ = DestroyIcon(h_icon);
            }
            if DAEMON_UNLOAD_ON_EXIT.load(Ordering::SeqCst) {
                crate::activate::session_end_headless();
            }
            if quit_unload_ev != 0 {
                let _ = CloseHandle(quit_unload_ev);
            }
            if stop_ev != 0 {
                let _ = CloseHandle(stop_ev);
            }
            let _ = CloseHandle(mutex);
        }
    }
}

#[cfg(not(windows))]
mod win_daemon {
    pub fn is_daemon_running() -> bool {
        false
    }
    pub fn spawn_daemon(_handoff: bool) -> Result<(), String> {
        Ok(())
    }
    pub fn signal_daemon_stop() {}
    pub fn quit_daemon_and_wait(_timeout_ms: u64) -> bool {
        true
    }
    pub fn run_headless_daemon(_handoff: bool) {}
}

pub fn is_daemon_running() -> bool {
    win_daemon::is_daemon_running()
}

pub fn spawn_daemon(handoff: bool) -> Result<(), String> {
    win_daemon::spawn_daemon(handoff)
}

pub fn signal_daemon_stop() {
    win_daemon::signal_daemon_stop();
}

pub fn quit_daemon_and_wait(timeout_ms: u64) -> bool {
    win_daemon::quit_daemon_and_wait(timeout_ms)
}

pub fn run_headless_daemon(handoff: bool) {
    win_daemon::run_headless_daemon(handoff);
}

/// Format startup launch command for Windows login (uses `--daemon` when Start Minimized is on).
pub fn startup_launch_args(exe_path: &Path, start_minimized: bool) -> String {
    let exe_str = exe_path.display().to_string();
    if start_minimized {
        format!("\"{exe_str}\" {DAEMON_ARG}")
    } else {
        format!("\"{exe_str}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn startup_launch_args_uses_daemon_flag_when_minimized() {
        let exe = PathBuf::from(r"C:\Program Files\Fonths\fonths.exe");
        assert_eq!(
            startup_launch_args(&exe, false),
            r#""C:\Program Files\Fonths\fonths.exe""#
        );
        assert_eq!(
            startup_launch_args(&exe, true),
            r#""C:\Program Files\Fonths\fonths.exe" --daemon"#
        );
    }

    #[test]
    fn daemon_cli_constants_stay_stable() {
        assert_eq!(DAEMON_ARG, "--daemon");
        assert_eq!(HANDOFF_ARG, "--handoff");
        assert_eq!(QUIT_DAEMON_ARG, "--quit-daemon");
    }
}
