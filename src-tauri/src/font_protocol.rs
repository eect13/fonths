//! Zero-copy asynchronous `font://` Tauri custom URI scheme protocol backed by an LRU `memmap2`
//! cache and `rayon` background I/O pool.
//!
//! Serves local `.ttf`, `.otf`, `.ttc`, `.otc`, `.woff`, and `.woff2` files directly to WebView2
//! CSS `@font-face` / `FontFace` without base64 or JSON IPC serialization overhead.

use lru::LruCache;
use memmap2::Mmap;
use std::fs::File;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::UriSchemeResponder;

const MMAP_CACHE_CAPACITY: usize = 128;

fn mmap_cache() -> &'static Mutex<LruCache<PathBuf, Arc<Mmap>>> {
    static CACHE: LazyLock<Mutex<LruCache<PathBuf, Arc<Mmap>>>> = LazyLock::new(|| {
        Mutex::new(LruCache::new(NonZeroUsize::new(MMAP_CACHE_CAPACITY).unwrap()))
    });
    &CACHE
}

/// Percent-decode a URI path segment (UTF-8 safe).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Extract a local filesystem path from a `font://localhost/<encoded-path>` or `http://font.localhost/<encoded-path>` URI.
pub fn decode_font_uri_path(uri_path: &str) -> Option<PathBuf> {
    let trimmed = uri_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let decoded = percent_decode(trimmed);
    #[cfg(windows)]
    let normalized = {
        let stripped = decoded.strip_prefix('/').unwrap_or(&decoded);
        stripped.replace('/', "\\")
    };
    #[cfg(not(windows))]
    let normalized = if decoded.starts_with('/') {
        decoded
    } else {
        format!("/{decoded}")
    };
    let path = PathBuf::from(normalized);
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    if !is_allowed_font_extension(&path) {
        return None;
    }
    Some(path)
}

fn is_allowed_font_extension(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2")
}

fn mime_for_font(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "otf" => "font/otf",
        "ttc" | "otc" => "font/collection",
        _ => "font/ttf",
    }
}

/// Evict a file from the mmap cache (call before deleting/replacing a font file on disk
/// so Windows releases the memory-mapped section handle and never hits ERROR_SHARING_VIOLATION).
pub fn evict_path(path: &Path) {
    let target = path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if let Ok(mut guard) = mmap_cache().lock() {
        let keys: Vec<PathBuf> = guard
            .iter()
            .filter_map(|(k, _)| {
                let norm = k.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
                (norm == target).then(|| k.clone())
            })
            .collect();
        for k in keys {
            guard.pop(&k);
        }
    }
}

/// Evict every cached mmap under `dir` before deleting/recycling a family folder.
pub fn evict_dir(dir: &Path) {
    let mut prefix = dir.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    if !prefix.ends_with('\\') {
        prefix.push('\\');
    }
    if let Ok(mut guard) = mmap_cache().lock() {
        let keys: Vec<PathBuf> = guard
            .iter()
            .filter_map(|(k, _)| {
                let norm = k.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
                (norm.starts_with(&prefix) || norm == prefix.trim_end_matches('\\')).then(|| k.clone())
            })
            .collect();
        for k in keys {
            guard.pop(&k);
        }
    }
}

fn read_cached_font_bytes(path: &Path) -> Result<Vec<u8>, String> {
    if let Ok(mut cache) = mmap_cache().lock()
        && let Some(mapped) = cache.get(path) {
            return Ok(mapped.as_ref().to_vec());
        }
    let file = File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if !(12..=128 * 1024 * 1024).contains(&len) {
        return Err("font file size out of bounds".into());
    }
    // SAFETY: read-only mapping of an immutable font file; cache is bounded and evictable.
    let mmap = Arc::new(unsafe { Mmap::map(&file) }.map_err(|e| format!("mmap failed: {e}"))?);
    let bytes = mmap.as_ref().to_vec();
    if let Ok(mut cache) = mmap_cache().lock() {
        cache.put(path.to_path_buf(), mmap);
    }
    Ok(bytes)
}

fn build_error(status: StatusCode, msg: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(msg.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Handle an asynchronous `font://` protocol request on the `rayon` threadpool.
pub fn handle_request(request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let uri_path = request.uri().path().to_string();
    rayon::spawn(move || {
        let Some(path) = decode_font_uri_path(&uri_path) else {
            responder.respond(build_error(StatusCode::BAD_REQUEST, "invalid font path"));
            return;
        };
        if !path.is_file() {
            responder.respond(build_error(StatusCode::NOT_FOUND, "font file not found"));
            return;
        }
        match read_cached_font_bytes(&path) {
            Ok(bytes) => {
                let resp = Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime_for_font(&path))
                    .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                    .body(bytes)
                    .unwrap_or_else(|_| Response::new(Vec::new()));
                responder.respond(resp);
            }
            Err(err) => {
                responder.respond(build_error(StatusCode::INTERNAL_SERVER_ERROR, &err));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_encoded_font_uri_and_rejects_non_fonts() {
        #[cfg(windows)]
        {
            let p = decode_font_uri_path("/C%3A%5CUsers%5CEric%5CDocuments%5CFonths%5CInter%20Variable.ttf")
                .expect("valid .ttf URI must decode");
            assert_eq!(p, PathBuf::from(r"C:\Users\Eric\Documents\Fonths\Inter Variable.ttf"));
        }
        assert!(decode_font_uri_path("/C%3A%5CWindows%5CSystem32%5Ccmd.exe").is_none());
        assert!(decode_font_uri_path("/secret.txt").is_none());
    }

    #[test]
    fn rejects_parent_traversal_in_font_uri() {
        assert!(decode_font_uri_path("/C%3A%5CUsers%5C..%5CWindows%5CFonts%5Carial.ttf").is_none());
        assert!(decode_font_uri_path("/C:/Users/%2e%2e/Fonts/arial.ttf").is_none());
        assert!(decode_font_uri_path("/../etc/passwd.ttf").is_none());
    }

    #[test]
    fn reads_fixture_through_mmap_cache() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("Figtree-wght.ttf");
        let a = read_cached_font_bytes(&fixture).expect("first mmap read");
        let b = read_cached_font_bytes(&fixture).expect("cached mmap read");
        assert!(a.len() > 1000);
        assert_eq!(a.len(), b.len());
    }
}
