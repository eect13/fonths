fn main() {
    // c255 P3: stamp the git commit into the exe (FM_BUILD_SHA). ZIP / no-git
    // checkouts still build with "unknown" — never a made-up commit.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=FM_BUILD_SHA={sha}");
    println!("cargo:rerun-if-changed=../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=app.manifest");

    // Embed the Windows requireAdministrator manifest in release builds (and when
    // FM_EMBED_ADMIN_MANIFEST=1) so shipped binaries/installers always elevate cleanly
    // while `cargo test` and local debug sandbox runs avoid UAC error 740.
    let embed_admin = std::env::var("PROFILE").as_deref() == Ok("release")
        || std::env::var("FM_EMBED_ADMIN_MANIFEST").as_deref() == Ok("1");

    if embed_admin {
        let win = tauri_build::WindowsAttributes::new()
            .app_manifest(include_str!("app.manifest"));
        let attrs = tauri_build::Attributes::new().windows_attributes(win);
        tauri_build::try_build(attrs).expect("failed to run tauri-build with Windows admin manifest");
    } else {
        tauri_build::build();
    }
}
