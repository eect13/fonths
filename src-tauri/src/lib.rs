pub mod activator;
pub mod daemon;
pub mod font_protocol;
pub mod renderer;
pub mod scanner;

mod activate;
mod delete_ops;
mod fast_parse;
mod google_cdn;
mod meta_index;
// Some sfnt/pacer helpers are only exercised by tests and live checks.
#[cfg_attr(not(test), allow(dead_code))]
mod gfsource;
#[cfg(test)]
mod gfsource_tests;
mod namepatch;
mod parse;
mod session_stage;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

pub const DEACTIVATE_ALL_AND_EXIT_ARG: &str = "--deactivate-all-and-exit";

static QUITTING: AtomicBool = AtomicBool::new(false);
static HANDED_OFF_TO_DAEMON: AtomicBool = AtomicBool::new(false);
static PRESERVED_ON_EXIT: AtomicBool = AtomicBool::new(false);
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);
static START_MINIMIZED: AtomicBool = AtomicBool::new(false);
/// 0 = instant-zero-ram (default), 1 = daemon-tray, 2 = session-only
static ACTIVATION_MODE: AtomicU8 = AtomicU8::new(0);

fn set_tray_tooltip(app: &tauri::AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

fn show_main(app: &tauri::AppHandle) {
    if QUITTING.load(Ordering::SeqCst) {
        return;
    }
    set_tray_tooltip(app, "Fonths");
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Hide first so X feels instant. Do **not** block the event loop on unload —
/// that froze single-instance IPC (second launch hung → two processes, no window).
/// Unload runs on a worker; a path-scaled watchdog is only a hung-GDI backstop
/// (worker exits(0) when session_end finishes).
fn quit_gracefully(app: &tauri::AppHandle) {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app.remove_tray_by_id("main");
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        daemon::signal_daemon_stop();
        activator::cleanup_on_exit(&handle);
        activate::session_end(&handle);
        // Clean Tauri/WebView2 teardown so EBWebView-Admin releases its SingletonLock
        // immediately before the user reopens the app (prevents needing to click twice).
        handle.exit(0);
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        std::process::exit(0);
    });
    let budget = activate::quit_unload_budget(app);
    std::thread::spawn(move || {
        std::thread::sleep(budget);
        std::process::exit(0);
    });
}

/// Ephemeral UI exit-on-close: hand active fonts off to the ~6 MB pure-Rust headless daemon
/// and terminate the WebView2 UI process immediately without unloading any fonts.
fn handoff_to_daemon_and_exit(app: &tauri::AppHandle) {
    if HANDED_OFF_TO_DAEMON.swap(true, Ordering::SeqCst) {
        return;
    }
    activate::persist_session_handoff(app);
    let _ = daemon::spawn_daemon(true);
    let _ = app.remove_tray_by_id("main");
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        handle.exit(0);
        std::thread::sleep(std::time::Duration::from_millis(800));
        std::process::exit(0);
    });
}

/// Zero-RAM Instant exit-on-close: persist HKCU per-user font keys + `.persistent-active`,
/// stop any stale daemon, and exit the WebView2 UI process immediately (`0 MB` RAM, `0`
/// background processes) while active fonts stay live system-wide.
fn exit_preserving_active_fonts(app: &tauri::AppHandle) {
    if PRESERVED_ON_EXIT.swap(true, Ordering::SeqCst) {
        return;
    }
    activate::persist_persistent_zero_ram_session(app);
    daemon::signal_daemon_stop();
    let _ = app.remove_tray_by_id("main");
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        handle.exit(0);
        std::thread::sleep(std::time::Duration::from_millis(800));
        std::process::exit(0);
    });
}

#[tauri::command]
fn daemon_status() -> bool {
    daemon::is_daemon_running()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn set_desktop_prefs(
    close_to_tray: bool,
    start_with_windows: bool,
    start_minimized: Option<bool>,
    prefer_static_faces: Option<bool>,
    fast_session_resume: Option<bool>,
    delete_mode: Option<String>,
    hkcu_registry_sync: Option<bool>,
    activation_mode: Option<String>,
) {
    CLOSE_TO_TRAY.store(close_to_tray, Ordering::SeqCst);
    let minimized = start_minimized.unwrap_or(false);
    START_MINIMIZED.store(minimized, Ordering::SeqCst);
    if let Some(mode) = activation_mode {
        ACTIVATION_MODE.store(activate::set_activation_mode(&mode), Ordering::SeqCst);
    }
    let _ = apply_start_with_windows(start_with_windows, minimized);
    activate::set_prefer_static_faces(prefer_static_faces.unwrap_or(false));
    activate::set_fast_session_resume(fast_session_resume.unwrap_or(true));
    let hkcu = hkcu_registry_sync.unwrap_or(true);
    let prev_hkcu = session_stage::hkcu_sync_enabled();
    session_stage::set_hkcu_sync_enabled(hkcu);
    if prev_hkcu != hkcu {
        std::thread::spawn(move || {
            if hkcu {
                activate::sync_hkcu_registry_now();
            } else {
                activate::clear_hkcu_registry_now();
            }
        });
    }
    delete_ops::set_permanent_delete(
        delete_mode
            .as_deref()
            .map(|m| m.eq_ignore_ascii_case("permanent"))
            .unwrap_or(false),
    );
}

/// c255 P3: does this Startup `.cmd` body start `exe`? (case-insensitive, Windows paths)
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn startup_cmd_is_ours(body: &str, exe: &str) -> bool {
    let norm = |s: &str| s.to_ascii_lowercase().replace('/', "\\");
    let exe = norm(exe.trim());
    !exe.is_empty() && norm(body).contains(&format!("\"{exe}\""))
}

/// True when the current Windows process holds an Administrator token (manifest `requireAdministrator`).
pub(crate) fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        win_admin::is_elevated()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn apply_start_with_windows(on: bool, start_minimized: bool) -> Result<(), String> {
    // c263: a sandbox build never touches the real Startup folder or Task Scheduler.
    if activate::sandbox_active() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const TASK_NAME: &str = "FonthsAutoStart";
        const LEGACY_TASK_NAME: &str = "FontManagerAutoStart";

        let appdata = std::env::var("APPDATA").map_err(|e| e.to_string())?;
        let dir = std::path::PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup");
        let link = dir.join("Fonths.cmd");
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe_str = exe.display().to_string();
        let launch_cmd = daemon::startup_launch_args(&exe, start_minimized);

        if win_admin::is_elevated() {
            let _ = std::process::Command::new("schtasks.exe")
                .args(["/Delete", "/TN", LEGACY_TASK_NAME, "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            if on {
                let status = std::process::Command::new("schtasks.exe")
                    .args([
                        "/Create",
                        "/TN",
                        TASK_NAME,
                        "/SC",
                        "ONLOGON",
                        "/RL",
                        "HIGHEST",
                        "/TR",
                        &launch_cmd,
                        "/F",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .status()
                    .map_err(|e| format!("schtasks create failed: {e}"))?;
                if !status.success() {
                    return Err(format!("schtasks create exited with {status}"));
                }
            } else {
                let _ = std::process::Command::new("schtasks.exe")
                    .args(["/Delete", "/TN", TASK_NAME, "/F"])
                    .creation_flags(CREATE_NO_WINDOW)
                    .status();
            }
            if link.exists() {
                let body = std::fs::read_to_string(&link).unwrap_or_default();
                if startup_cmd_is_ours(&body, &exe_str) {
                    let _ = std::fs::remove_file(&link);
                }
            }
        } else if on {
            let body = format!("@echo off\r\nstart \"\" {launch_cmd}\r\n");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&link, body).map_err(|e| e.to_string())?;
        } else if link.exists() {
            let body = std::fs::read_to_string(&link).unwrap_or_default();
            if startup_cmd_is_ours(&body, &exe_str) {
                let _ = std::fs::remove_file(&link);
            }
        }
    }
    let _ = (on, start_minimized);
    Ok(())
}

/// c255 P3: git commit this exe was built from (build.rs).
const BUILD_SHA: &str = env!("FM_BUILD_SHA");

#[cfg(windows)]
mod win_admin {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn IsUserAnAdmin() -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ChangeWindowMessageFilter(message: u32, dw_flag: u32) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegOpenKeyExW(
            h_key: isize,
            lp_sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut isize,
        ) -> i32;
        fn RegDeleteValueW(h_key: isize, lp_value_name: *const u16) -> i32;
        fn RegCloseKey(h_key: isize) -> i32;
    }

    const HKEY_CURRENT_USER: isize = -2147483647i32 as isize; // 0x80000001
    const KEY_SET_VALUE: u32 = 0x0002;
    const MSGFLT_ADD: u32 = 1;
    const WM_DROPFILES: u32 = 0x0233;
    const WM_COPYDATA: u32 = 0x004A;
    const WM_COPYGLOBALDATA: u32 = 0x0049;

    fn wide_z(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn clear_runasadmin_layer(exe: &std::path::Path) {
        let subkey = wide_z(std::ffi::OsStr::new(
            r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers",
        ));
        let mut hkey: isize = 0;
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if rc == 0 && hkey != 0 {
            let wide = wide_z(exe.as_os_str());
            unsafe {
                let _ = RegDeleteValueW(hkey, wide.as_ptr());
                if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                    for (dir_name, bin_name) in [
                        ("Fonths", "fonths.exe"),
                        ("Font Manager", "font-manager.exe"),
                    ] {
                        let installed = std::path::PathBuf::from(&local)
                            .join(dir_name)
                            .join(bin_name);
                        let inst_w = wide_z(installed.as_os_str());
                        let _ = RegDeleteValueW(hkey, inst_w.as_ptr());
                    }
                }
                let _ = RegCloseKey(hkey);
            }
        }
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        fn WaitForSingleObject(h_handle: isize, dw_milliseconds: u32) -> u32;
        fn CloseHandle(h_object: isize) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;

    pub fn wait_for_relaunch_parent_pid() {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--relaunch-after-pid" {
                if let Some(pid_str) = args.next()
                    && let Ok(pid) = pid_str.trim().parse::<u32>()
                        && pid != 0 && pid != std::process::id() {
                            unsafe {
                                let handle = OpenProcess(SYNCHRONIZE, 0, pid);
                                if handle != 0 {
                                    let _ = WaitForSingleObject(handle, 10_000);
                                    let _ = CloseHandle(handle);
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(120));
                        }
                break;
            }
        }
    }

    pub fn is_elevated() -> bool {
        unsafe { IsUserAnAdmin() != 0 }
    }

    pub fn prepare_windows_session() {
        // SAFETY: Runs on single-threaded startup before background workers spawn.
        unsafe {
            std::env::remove_var("__COMPAT_LAYER");
        }
        if let Ok(exe) = std::env::current_exe() {
            clear_runasadmin_layer(&exe);
        }
        let is_admin = unsafe { IsUserAnAdmin() != 0 };
        if is_admin {
            if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none()
                && let Some(local) = std::env::var_os("LOCALAPPDATA") {
                    let admin_udf = std::path::PathBuf::from(local)
                        .join("app.fonths.desktop")
                        .join("EBWebView-Admin");
                    // SAFETY: Runs before WebView2 initialization on single-threaded startup.
                    unsafe {
                        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", admin_udf);
                    }
                }
            unsafe {
                let _ = ChangeWindowMessageFilter(WM_DROPFILES, MSGFLT_ADD);
                let _ = ChangeWindowMessageFilter(WM_COPYDATA, MSGFLT_ADD);
                let _ = ChangeWindowMessageFilter(WM_COPYGLOBALDATA, MSGFLT_ADD);
            }
        }
    }
}

fn spawn_delayed_relaunch_on_quitting() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let pid = std::process::id().to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        let _ = std::process::Command::new(&exe)
            .args(["--relaunch-after-pid", &pid])
            .creation_flags(DETACHED_PROCESS)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new(exe)
            .args(["--relaunch-after-pid", &pid])
            .spawn();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    eprintln!("Fonths {} (commit {BUILD_SHA})", env!("CARGO_PKG_VERSION"));
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("Fonths {} (commit {BUILD_SHA})", env!("CARGO_PKG_VERSION"));
        return;
    }
    // Pure-Rust headless tray daemon & clean installer/uninstaller quit commands (exit before WebView2).
    if std::env::args().any(|a| a == DEACTIVATE_ALL_AND_EXIT_ARG) {
        let _ = daemon::quit_daemon_and_wait(4_000);
        activate::deactivate_all_and_exit_headless();
        return;
    }
    if std::env::args().any(|a| a == daemon::QUIT_DAEMON_ARG) {
        let _ = daemon::quit_daemon_and_wait(6_000);
        return;
    }
    if std::env::args().any(|a| a == daemon::DAEMON_ARG) {
        let handoff = std::env::args().any(|a| a == daemon::HANDOFF_ARG);
        daemon::run_headless_daemon(handoff);
        return;
    }
    ACTIVATION_MODE.store(activate::get_activation_mode_code(), Ordering::SeqCst);
    #[cfg(windows)]
    {
        win_admin::wait_for_relaunch_parent_pid();
        win_admin::prepare_windows_session();
    }
    tauri::Builder::default()
        .manage(renderer::FontationsRendererState::new())
        .manage(activator::ActivatorState::default())
        .register_asynchronous_uri_scheme_protocol("font", |_ctx, request, responder| {
            font_protocol::handle_request(request, responder);
        })
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if QUITTING.load(Ordering::SeqCst) {
                spawn_delayed_relaunch_on_quitting();
                std::process::exit(0);
            }
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            if let Some(sb) = activate::init_sandbox(&app.config().identifier) {
                eprintln!("Fonths SANDBOX root {}", sb.display());
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                activate::session_begin(&handle);
            });

            if !daemon::is_daemon_running() {
                let show = MenuItem::with_id(app, "show", "Show Fonths", true, None::<&str>)?;
                let folder = MenuItem::with_id(app, "folder", "Open Documents folder", true, None::<&str>)?;
                let cancel_docs = MenuItem::with_id(
                    app,
                    "cancel_docs_refresh",
                    "Cancel Documents refresh",
                    true,
                    None::<&str>,
                )?;
                let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &folder, &cancel_docs, &quit])?;

                let icon = app
                    .default_window_icon()
                    .cloned()
                    .expect("app icon");

                TrayIconBuilder::with_id("main")
                    .icon(icon)
                    .tooltip("Fonths")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => show_main(app),
                        "folder" => {
                            let _ = activate::open_activation_folder(app.clone(), None);
                        }
                        "cancel_docs_refresh" => {
                            activate::request_docs_vf_cancel_native(app.clone());
                        }
                        "quit" => quit_gracefully(app),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            show_main(tray.app_handle());
                        }
                    })
                    .build(app)?;
            }

            if std::env::args().any(|a| a == "--minimized") {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.hide();
                }
                set_tray_tooltip(app.handle(), "Fonths — running in background");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_desktop_prefs,
            daemon_status,
            scanner::scan_font_directory,
            renderer::render_font_preview,
            renderer::render_font_specimen,
            activator::activate_font,
            activator::deactivate_font,
            activate::activation_folder,
            activate::install_font_file,
            activate::unload_font_family,
            activate::unload_font_families,
            activate::uninstall_font_family,
            activate::font_family_installed,
            activate::list_activated_families,
            activate::open_activation_folder,
            activate::register_font_path,
            activate::flush_font_cache,
            activate::save_library_file,
            activate::remove_library_file,
            activate::resolve_family_fetch_intent,
            activate::start_google_downloads,
            activate::retry_google_downloads,
            activate::clear_session_gdi_refused_family,
            activate::try_fontsource_gdi_offer,
            activate::repair_incomplete_families,
            activate::skip_google_failures,
            activate::cancel_google_downloads,
            activate::cancel_documents_refresh,
            activate::drop_google_download_families,
            activate::pause_google_downloads,
            activate::resume_google_downloads,
            activate::google_download_progress,
            activate::register_existing_on_disk,
            activate::activate_families_on_disk,
            activate::plan_google_activation,
            activate::set_session_families,
            activate::session_families,
            activate::session_boot_state,
            activate::gdi_session_state,
            activate::read_family_font,
            activate::list_family_font_files,
            activate::scan_disk_families,
            activate::sync_documents_vf_policy,
            activate::prune_unknown_folders,
            activate::purge_non_google_fonts,
            activate::list_leftover_stage_files,
            activate::recycle_leftover_stage_files,
            activate::cleanup_temp_stage_files,
            activate::fetch_live_google_catalog,
            activate::fetch_live_google_catalog_conditional,
            activate::check_google_cdn_updates,
            activate::sync_google_cdn_updates,
            scanner::dwrite_engine_info,
            scanner::dwrite_inspect_font,
            meta_index::index_downloaded_metadata,
            parse::parse_family_cmap,
            parse::parse_family_layout,
            parse::parse_font_layout,
            parse::parse_font_layouts,
            parse::parse_font_cmap,
            parse::read_family_preview_info,
            parse::hash_bytes,
            parse::hash_font_path,
            parse::hash_font_paths,
            parse::index_font_paths,
            parse::diff_font_bytes,
            parse::list_system_fonts,
            parse::open_system_fonts_folder,
        ])
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                    if activate::sandbox_active() {
                        let _ = window.hide();
                        set_tray_tooltip(window.app_handle(), "Fonths — running in background");
                    } else {
                        match ACTIVATION_MODE.load(Ordering::SeqCst) {
                            1 => handoff_to_daemon_and_exit(window.app_handle()),
                            2 => quit_gracefully(window.app_handle()),
                            _ => exit_preserving_active_fonts(window.app_handle()),
                        }
                    }
                    return;
                }
                quit_gracefully(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Fonths")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if HANDED_OFF_TO_DAEMON.load(Ordering::SeqCst)
                    || PRESERVED_ON_EXIT.load(Ordering::SeqCst)
                {
                    return;
                }
                activator::cleanup_on_exit(app);
                if !QUITTING.load(Ordering::SeqCst) {
                    daemon::signal_daemon_stop();
                    activate::session_end(app);
                }
            }
        });
}

#[cfg(test)]
mod c255_main_tests {
    use super::*;

    #[test]
    fn c255_disable_startup_keeps_other_exes_cmd() {
        let mine = r"C:\Program Files\Font Manager\font-manager.exe";
        let ours = format!("@echo off\r\nstart \"\" \"{mine}\"\r\n");
        let other = "@echo off\r\nstart \"\" \"D:\\Portable\\font-manager.exe\"\r\n";
        assert!(startup_cmd_is_ours(&ours, mine));
        assert!(startup_cmd_is_ours(&ours.to_uppercase(), mine));
        assert!(!startup_cmd_is_ours(other, mine), "another install's Startup entry");
        assert!(!startup_cmd_is_ours("", mine));
    }

    #[test]
    fn windows_relaunch_and_webview_session_isolation_contracts() {
        let src = include_str!("lib.rs").replace('\r', "");
        assert!(
            src.contains("std::env::remove_var(\"__COMPAT_LAYER\")"),
            "prepare_windows_session must strip __COMPAT_LAYER so WebView2 sandbox never fails on relaunch"
        );
        assert!(
            src.contains("RegDeleteValueW(hkey, wide.as_ptr())"),
            "prepare_windows_session must clear stale RUNASADMIN AppCompatFlags layers"
        );
        assert!(
            src.contains("EBWebView-Admin"),
            "elevated runs must isolate WEBVIEW2_USER_DATA_FOLDER from normal user runs"
        );
        assert!(
            src.contains("spawn_delayed_relaunch_on_quitting()"),
            "single_instance callback must relaunch when clicked while previous instance is quitting"
        );
    }
}
