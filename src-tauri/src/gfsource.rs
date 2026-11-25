//! c258: full Google Fonts desktop files.
//!
//! Source order per family (statics first on Windows):
//! 1. `fonts.google.com/download/list?family=` manifest (the website "Download
//!    family" button). **Undocumented endpoint** — it can change or vanish, so
//!    the fallbacks below must stay solid.
//! 2. google/fonts repo (METADATA.pb) via jsDelivr, then raw.githubusercontent.
//! 3. css2 (`Mozilla/5.0`) — last resort only. Its statics are feature-stripped
//!    (Barlow 571 vs 694 glyphs, no smcp/c2sc/sups); never complete, never a repair.
//!
//! Politeness: ≤2 concurrent requests, ≥250 ms between request starts, honest UA,
//! backoff 1→60 s on 403/429/5xx, manifest cached ≤7 days, files cached by URL.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Honest UA: app name + repo URL. No invented contact address.
pub const POLITE_UA: &str = "Fonths/2.5 (+https://github.com/eect13/fonths)";
/// Metadata / manifest / API lane: polite burst-free concurrency and spacing.
pub const MAX_CONCURRENT: usize = 3;
pub const MIN_SPACING: Duration = Duration::from_millis(100);
/// Immutable font-binary CDN lane (`fonts.gstatic.com`, `cdn.jsdelivr.net`, `raw.githubusercontent.com`).
pub const CDN_MAX_CONCURRENT: usize = 6;
pub const CDN_MIN_SPACING: Duration = Duration::from_millis(35);
pub const MANIFEST_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ATTEMPTS: u32 = 4;
const MAX_FETCH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingSource {
    /// download/list manifest (gstatic files of the website download).
    Manifest,
    /// google/fonts repo statics (METADATA.pb).
    Repo,
    /// css2 web build — feature-stripped; never complete.
    Css2,
}

impl ListingSource {
    pub fn is_full(self) -> bool {
        !matches!(self, ListingSource::Css2)
    }
    pub fn label(self) -> &'static str {
        match self {
            ListingSource::Manifest => "download-list",
            ListingSource::Repo => "google-fonts-repo",
            ListingSource::Css2 => "css2-web-build",
        }
    }
}

// ---------------------------------------------------------------- source module

/// (style, weight, url) — one listed static face.
pub type Listing = Vec<(String, String, String)>;

/// c263: one Google Fonts source behind an interface. The live impl lives in
/// activate.rs (network + caches); tests use a fake. Order (`SOURCE_ORDER`):
/// 1. fonts.google.com `download/list` manifest (the site's own download, gstatic TTFs);
/// 2. google/fonts repo statics via METADATA.pb (jsDelivr, raw GitHub per face);
/// 3. css2 web build — feature-stripped, last resort, never `.complete`.
pub trait GoogleFontsSource {
    fn manifest(&self, family: &str) -> Option<Manifest>;
    /// Repo PascalCase name used by manifest filenames (`BarlowCondensed-Bold.ttf`).
    fn pascal(&self, family: &str) -> String;
    fn repo_listing(&self, family: &str, licence_dir: Option<&str>) -> Listing;
    fn css2_listing(&self, family: &str) -> Listing;
}

pub const SOURCE_ORDER: [ListingSource; 3] = [ListingSource::Manifest, ListingSource::Repo, ListingSource::Css2];

/// First non-empty listing in `SOURCE_ORDER`; later sources are only asked when needed.
pub fn choose_listing(
    manifest: Listing,
    repo: impl FnOnce() -> Listing,
    css: impl FnOnce() -> Listing,
) -> (Listing, ListingSource) {
    if !manifest.is_empty() {
        return (manifest, ListingSource::Manifest);
    }
    let r = repo();
    if !r.is_empty() {
        return (r, ListingSource::Repo);
    }
    (css(), ListingSource::Css2)
}

/// Richest listing for `family` from `src`, plus the manifest (licence text).
pub fn discover_listing(src: &dyn GoogleFontsSource, family: &str) -> (Listing, ListingSource, Option<Manifest>) {
    let manifest = src.manifest(family);
    let m_list = manifest
        .as_ref()
        .map(|m| manifest_static_listing(m, &src.pascal(family)))
        .unwrap_or_default();
    let lic = manifest.as_ref().and_then(manifest_license_dir);
    let (l, s) = choose_listing(m_list, || src.repo_listing(family, lic), || src.css2_listing(family));
    (l, s, manifest)
}

// ---------------------------------------------------------------- manifest

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    /// Text files (licence, README): (filename, contents).
    pub files: Vec<(String, String)>,
    /// Font files: (filename, url).
    pub refs: Vec<(String, String)>,
}

/// Strip the XSSI guard `)]}'` that prefixes the download/list JSON.
pub fn strip_xssi(text: &str) -> &str {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    let t = t.strip_prefix(")]}'").unwrap_or(t);
    t.trim_start()
}

pub fn parse_manifest(text: &str) -> Option<Manifest> {
    let v: serde_json::Value = serde_json::from_str(strip_xssi(text)).ok()?;
    let m = v.get("manifest")?;
    let mut out = Manifest::default();
    for f in m.get("files").and_then(|x| x.as_array()).into_iter().flatten() {
        if let (Some(n), Some(c)) = (
            f.get("filename").and_then(|x| x.as_str()),
            f.get("contents").and_then(|x| x.as_str()),
        ) {
            out.files.push((n.to_string(), c.to_string()));
        }
    }
    for r in m.get("fileRefs").and_then(|x| x.as_array()).into_iter().flatten() {
        if let (Some(n), Some(u)) = (
            r.get("filename").and_then(|x| x.as_str()),
            r.get("url").and_then(|x| x.as_str()),
        )
            && u.starts_with("https://") {
                out.refs.push((n.to_string(), u.to_string()));
            }
    }
    if out.refs.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Licence file from the manifest → google/fonts licence folder.
pub fn manifest_license_dir(m: &Manifest) -> Option<&'static str> {
    for (n, _) in &m.files {
        match n.to_ascii_uppercase().as_str() {
            "OFL.TXT" => return Some("ofl"),
            "LICENSE.TXT" => return Some("apache"),
            "UFL.TXT" => return Some("ufl"),
            _ => {}
        }
    }
    None
}

/// Licence text to keep next to the family (filename, contents).
pub fn manifest_license_files(m: &Manifest) -> Vec<(String, String)> {
    m.files
        .iter()
        .filter(|(n, _)| {
            let u = n.to_ascii_uppercase();
            u == "OFL.TXT" || u == "LICENSE.TXT" || u == "UFL.TXT"
        })
        .cloned()
        .collect()
}

pub fn is_variable_ref(filename: &str) -> bool {
    let base = filename.rsplit('/').next().unwrap_or(filename);
    base.contains('[') || base.contains("VariableFont_")
}

fn style_token(tok: &str) -> Option<(u16, bool)> {
    let (core, italic) = match tok.strip_suffix("Italic") {
        Some(c) => (c, true),
        None => (tok, false),
    };
    let w = match core {
        "Thin" | "Hairline" => 100,
        "ExtraLight" | "UltraLight" => 200,
        "Light" => 300,
        "" | "Regular" | "Book" => 400,
        "Medium" => 500,
        "SemiBold" | "DemiBold" => 600,
        "Bold" => 700,
        "ExtraBold" | "UltraBold" => 800,
        "Black" | "Heavy" => 900,
        _ => return None,
    };
    Some((w, italic))
}

/// Split `static/Inter_18pt-BoldItalic.ttf` → ("Inter", Some(18), 700, true).
fn parse_static_name(filename: &str) -> Option<(String, Option<u32>, u16, bool)> {
    let base = filename.rsplit('/').next().unwrap_or(filename);
    let lower = base.to_ascii_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) || is_variable_ref(base) {
        return None;
    }
    let stem = &base[..base.len() - 4];
    let (prefix, tok) = stem.rsplit_once('-')?;
    let (w, italic) = style_token(tok)?;
    let (name, opsz) = match prefix.rsplit_once('_') {
        Some((n, pt)) if pt.ends_with("pt") && pt[..pt.len() - 2].chars().all(|c| c.is_ascii_digit()) => {
            (n.to_string(), pt[..pt.len() - 2].parse::<u32>().ok())
        }
        _ => (prefix.to_string(), None),
    };
    Some((name, opsz, w, italic))
}

/// One static per (weight, style) from the manifest. Width / other-family
/// statics (`Roboto_Condensed-*`) are skipped; for optical-size sets the
/// smallest size (text default, e.g. Inter 18pt) wins. Output matches the
/// css listing tuple: (style, weight, url), sorted by weight then style.
pub fn manifest_static_listing(m: &Manifest, pascal: &str) -> Vec<(String, String, String)> {
    let parsed: Vec<_> = m
        .refs
        .iter()
        .filter_map(|(f, u)| parse_static_name(f).map(|p| (p, u.clone())))
        .collect();
    let mut names: BTreeSet<String> = parsed.iter().map(|((n, ..), _)| n.clone()).collect();
    let want = if names.contains(pascal) {
        pascal.to_string()
    } else if names.len() == 1 {
        names.pop_first().unwrap_or_default()
    } else {
        return Vec::new();
    };
    let mut best: std::collections::BTreeMap<(u16, bool), (Option<u32>, String)> = Default::default();
    for ((n, opsz, w, it), url) in parsed {
        if n != want {
            continue;
        }
        let rank = opsz;
        let take = match best.get(&(w, it)) {
            None => true,
            Some((old, _)) => match (old, rank) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(a), Some(b)) => b < *a,
            },
        };
        if take {
            best.insert((w, it), (rank, url));
        }
    }
    best.into_iter()
        .map(|((w, it), (_, url))| {
            (if it { "italic" } else { "normal" }.to_string(), w.to_string(), url)
        })
        .collect()
}

/// Variable refs from the manifest: (filename, url, italic).
pub fn manifest_variable_refs(m: &Manifest) -> Vec<(String, String, bool)> {
    m.refs
        .iter()
        .filter(|(f, _)| is_variable_ref(f))
        .map(|(f, u)| {
            let base = f.rsplit('/').next().unwrap_or(f).to_string();
            let italic = base.contains("Italic");
            (base, u.clone(), italic)
        })
        .collect()
}

pub fn download_list_url(family: &str) -> String {
    let mut q = String::new();
    for b in family.trim().bytes() {
        match b {
            b' ' => q.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => q.push(b as char),
            _ => q.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://fonts.google.com/download/list?family={q}")
}

// ---------------------------------------------------------------- sfnt checks

fn be16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn table<'a>(b: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = be16(b, 4)? as usize;
    for i in 0..n {
        let rec = 12 + i * 16;
        if b.get(rec..rec + 4)? == tag {
            let off = be32(b, rec + 8)? as usize;
            let len = be32(b, rec + 12)? as usize;
            return b.get(off..off.checked_add(len)?);
        }
    }
    None
}

pub fn num_glyphs(b: &[u8]) -> Option<u16> {
    be16(table(b, b"maxp")?, 4)
}

pub fn has_fvar(b: &[u8]) -> bool {
    table(b, b"fvar").is_some()
}

/// GSUB + GPOS feature tags.
pub fn feature_tags(b: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for tag in [b"GSUB", b"GPOS"] {
        let Some(t) = table(b, tag) else { continue };
        let Some(fl) = be16(t, 6) else { continue };
        let fl = fl as usize;
        let Some(n) = be16(t, fl) else { continue };
        for i in 0..n as usize {
            if let Some(s) = t.get(fl + 2 + i * 6..fl + 6 + i * 6) {
                out.insert(String::from_utf8_lossy(s).into_owned());
            }
        }
    }
    out
}

/// Mapped Unicode code points (cmap format 12 preferred, else format 4).
pub fn cmap_count(b: &[u8]) -> usize {
    let Some(t) = table(b, b"cmap") else { return 0 };
    let n = be16(t, 2).unwrap_or(0) as usize;
    let (mut f4, mut f12) = (None, None);
    for i in 0..n {
        let r = 4 + i * 8;
        let (Some(pid), Some(eid), Some(off)) = (be16(t, r), be16(t, r + 2), be32(t, r + 4)) else {
            continue;
        };
        let off = off as usize;
        let fmt = be16(t, off).unwrap_or(0);
        let unicode = pid == 0 || (pid == 3 && (eid == 1 || eid == 10));
        if !unicode {
            continue;
        }
        if fmt == 12 && f12.is_none() {
            f12 = Some(off);
        } else if fmt == 4 && f4.is_none() {
            f4 = Some(off);
        }
    }
    if let Some(o) = f12 {
        let groups = be32(t, o + 12).unwrap_or(0) as usize;
        let mut cps = BTreeSet::new();
        for g in 0..groups {
            let r = o + 16 + g * 12;
            let (Some(s), Some(e), Some(gid)) = (be32(t, r), be32(t, r + 4), be32(t, r + 8)) else {
                break;
            };
            for (k, cp) in (s..=e.min(s.saturating_add(0x10FFFF))).enumerate() {
                if gid as usize + k != 0 {
                    cps.insert(cp);
                }
            }
        }
        return cps.len();
    }
    if let Some(o) = f4 {
        let segx2 = be16(t, o + 6).unwrap_or(0) as usize;
        let ends = o + 14;
        let starts = ends + segx2 + 2;
        let deltas = starts + segx2;
        let ranges = deltas + segx2;
        let mut count = 0usize;
        for s in 0..segx2 / 2 {
            let (Some(end), Some(start), Some(delta), Some(ro)) = (
                be16(t, ends + s * 2),
                be16(t, starts + s * 2),
                be16(t, deltas + s * 2),
                be16(t, ranges + s * 2),
            ) else {
                break;
            };
            if start == 0xFFFF {
                continue;
            }
            for c in start..=end {
                let gid = if ro == 0 {
                    c.wrapping_add(delta)
                } else {
                    let a = ranges + s * 2 + ro as usize + (c - start) as usize * 2;
                    match be16(t, a) {
                        Some(0) | None => 0,
                        Some(g) => g.wrapping_add(delta),
                    }
                };
                if gid != 0 {
                    count += 1;
                }
            }
        }
        return count;
    }
    0
}

/// What a face carries; used to compare a download against its expected source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceShape {
    pub glyphs: u16,
    pub features: BTreeSet<String>,
}

pub fn face_shape(b: &[u8]) -> Option<FaceShape> {
    Some(FaceShape {
        glyphs: num_glyphs(b)?,
        features: feature_tags(b),
    })
}

/// Item 3: reject a candidate that is short on glyphs or OpenType features vs
/// the reference (the expected full source, or the face already on disk).
/// Small glyph drift (≤3%) between gstatic and repo builds is allowed.
pub fn shape_not_worse(candidate: &FaceShape, reference: &FaceShape) -> bool {
    let floor = (reference.glyphs as u32 * 97) / 100;
    if (candidate.glyphs as u32) < floor {
        return false;
    }
    reference.features.is_subset(&candidate.features)
}

// ---------------------------------------------------------------- politeness

#[derive(Copy, Clone, Eq, PartialEq)]
enum PacerLane {
    Metadata,
    CdnBinary,
}

struct Pacer {
    state: Mutex<(usize, Option<Instant>)>,
    cv: Condvar,
}

fn pacer() -> &'static Pacer {
    static P: OnceLock<Pacer> = OnceLock::new();
    P.get_or_init(|| Pacer { state: Mutex::new((0, None)), cv: Condvar::new() })
}

fn cdn_pacer() -> &'static Pacer {
    static P: OnceLock<Pacer> = OnceLock::new();
    P.get_or_init(|| Pacer { state: Mutex::new((0, None)), cv: Condvar::new() })
}

fn acquire_from(p: &'static Pacer, lane: PacerLane, max_conc: usize, min_spacing: Duration) -> Slot {
    let mut g = p.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if g.0 < max_conc {
            let wait = g.1.map(|t| min_spacing.saturating_sub(t.elapsed())).unwrap_or_default();
            if wait.is_zero() {
                g.0 += 1;
                g.1 = Some(Instant::now());
                return Slot { lane };
            }
            g = p.cv.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner()).0;
        } else {
            g = p.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// True when `url` points at an immutable font CDN host that handles higher concurrency.
pub fn is_cdn_binary_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://fonts.gstatic.com/")
        || lower.starts_with("https://cdn.jsdelivr.net/")
        || lower.starts_with("https://raw.githubusercontent.com/"))
        && !lower.contains("metadata")
}

/// RAII slot: dual-lane limiter (Metadata `MAX_CONCURRENT`/`MIN_SPACING` vs CDN binary `CDN_MAX_CONCURRENT`/`CDN_MIN_SPACING`).
pub struct Slot {
    lane: PacerLane,
}

impl Slot {
    pub fn acquire() -> Slot {
        acquire_from(pacer(), PacerLane::Metadata, MAX_CONCURRENT, MIN_SPACING)
    }
    pub fn acquire_cdn() -> Slot {
        acquire_from(cdn_pacer(), PacerLane::CdnBinary, CDN_MAX_CONCURRENT, CDN_MIN_SPACING)
    }
    pub fn in_use() -> usize {
        pacer().state.lock().map(|g| g.0).unwrap_or(0)
    }
    pub fn cdn_in_use() -> usize {
        cdn_pacer().state.lock().map(|g| g.0).unwrap_or(0)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let p = match self.lane {
            PacerLane::Metadata => pacer(),
            PacerLane::CdnBinary => cdn_pacer(),
        };
        if let Ok(mut g) = p.state.lock() {
            g.0 = g.0.saturating_sub(1);
        }
        p.cv.notify_all();
    }
}

pub fn status_backs_off(code: u16) -> bool {
    code == 403 || code == 429 || (500..=599).contains(&code)
}

/// 1, 2, 4, … capped at 60 s. `retry_after` (seconds) wins when larger, still capped.
pub fn backoff_delay(attempt: u32, retry_after: Option<u64>) -> Duration {
    let exp = 1u64 << attempt.min(6);
    let s = retry_after.unwrap_or(0).max(exp).min(60);
    Duration::from_secs(s.max(1))
}

/// Polite GET → body bytes on 2xx. Backs off on 403/429/5xx; `cancel` stops waits.
pub fn polite_get(
    client: &reqwest::blocking::Client,
    url: &str,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<u8>> {
    polite_get_status(client, url, cancel).ok()
}

/// Like `polite_get`; `Err(None)` = network error, `Err(Some(code))` = final HTTP status.
pub fn polite_get_status(
    client: &reqwest::blocking::Client,
    url: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<u8>, Option<u16>> {
    polite_fetch(client, url, &PoliteOpts::default(), cancel)
}

/// Per-request options for the shared limiter.
#[derive(Default, Clone)]
pub struct PoliteOpts<'a> {
    /// Override the honest UA (css2 needs `Mozilla/5.0` to serve TTF).
    pub ua: Option<&'a str>,
    pub timeout: Option<Duration>,
    /// Route through the higher-throughput CDN binary lane.
    pub cdn_lane: bool,
}

/// Bytes of a 2xx body through the limiter (slot held through the body read).
pub fn polite_fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    opts: &PoliteOpts,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<u8>, Option<u16>> {
    let (resp, _slot) = polite_response(client, url, opts, cancel)?;
    let code = resp.status().as_u16();
    if resp.content_length().map(|n| n as usize > MAX_FETCH_BYTES).unwrap_or(false) {
        return Err(Some(code));
    }
    let b = resp.bytes().map_err(|_| None)?;
    if b.len() <= MAX_FETCH_BYTES {
        Ok(b.to_vec())
    } else {
        Err(Some(code))
    }
}

/// A 2xx response plus the limiter slot it holds (drop both when the body is read).
/// Backs off 1→60 s on 403/429/5xx and network errors (Retry-After honoured, capped);
/// 404 and other 4xx return at once. `cancel` cuts any wait short.
pub fn polite_response(
    client: &reqwest::blocking::Client,
    url: &str,
    opts: &PoliteOpts,
    cancel: &dyn Fn() -> bool,
) -> Result<(reqwest::blocking::Response, Slot), Option<u16>> {
    let mut last = None;
    for attempt in 0..MAX_ATTEMPTS {
        if cancel() {
            return Err(last);
        }
        let slot = if opts.cdn_lane || is_cdn_binary_url(url) {
            Slot::acquire_cdn()
        } else {
            Slot::acquire()
        };
        let mut req = client.get(url).header(reqwest::header::USER_AGENT, opts.ua.unwrap_or(POLITE_UA));
        if let Some(t) = opts.timeout {
            req = req.timeout(t);
        }
        let resp = match req.send() {
            Ok(r) => r,
            Err(_) => {
                drop(slot);
                last = None;
                if attempt + 1 < MAX_ATTEMPTS {
                    sleep_cancelable(backoff_delay(attempt, None), cancel);
                }
                continue;
            }
        };
        let code = resp.status().as_u16();
        if resp.status().is_success() {
            return Ok((resp, slot));
        }
        last = Some(code);
        if !status_backs_off(code) || attempt + 1 == MAX_ATTEMPTS {
            return Err(last);
        }
        drop(slot);
        let ra = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        sleep_cancelable(backoff_delay(attempt, ra), cancel);
    }
    Err(last)
}

fn sleep_cancelable(d: Duration, cancel: &dyn Fn() -> bool) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if cancel() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
    }
}

// ---------------------------------------------------------------- caches

static MANIFEST_FAIL_STREAK: AtomicUsize = AtomicUsize::new(0);
static MANIFEST_OPEN_UNTIL: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static MANIFEST_MISS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn manifest_open_until() -> &'static Mutex<Option<Instant>> {
    MANIFEST_OPEN_UNTIL.get_or_init(|| Mutex::new(None))
}

fn manifest_miss_set() -> &'static Mutex<HashSet<String>> {
    MANIFEST_MISS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Reset manifest circuit breaker at the start of a fresh download job.
/// Keeps session negative-cache (`MANIFEST_MISS`) so families that 404/SPA on
/// `download/list` are not retried within the same process.
pub fn reset_manifest_circuit() {
    MANIFEST_FAIL_STREAK.store(0, Ordering::SeqCst);
    if let Ok(mut g) = manifest_open_until().lock() {
        *g = None;
    }
}

pub fn clear_manifest_negative_cache() {
    if let Ok(mut g) = manifest_miss_set().lock() {
        g.clear();
    }
}

pub fn manifest_circuit_allow() -> bool {
    let Ok(mut g) = manifest_open_until().lock() else {
        return true;
    };
    if let Some(until) = *g {
        if Instant::now() < until {
            return false;
        }
        *g = None;
        MANIFEST_FAIL_STREAK.store(0, Ordering::SeqCst);
    }
    true
}

pub fn note_manifest_success() {
    MANIFEST_FAIL_STREAK.store(0, Ordering::SeqCst);
    if let Ok(mut g) = manifest_open_until().lock() {
        *g = None;
    }
}

pub fn note_manifest_failure() {
    let streak = MANIFEST_FAIL_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    if streak >= 5 {
        MANIFEST_FAIL_STREAK.store(0, Ordering::SeqCst);
        if let Ok(mut g) = manifest_open_until().lock() {
            *g = Some(Instant::now() + Duration::from_secs(30));
        }
    }
}

pub fn manifest_missed_this_session(family: &str) -> bool {
    let key = cache_key(family);
    manifest_miss_set()
        .lock()
        .map(|g| g.contains(&key))
        .unwrap_or(false)
}

pub fn note_manifest_miss(family: &str) {
    let key = cache_key(family);
    if !key.is_empty()
        && let Ok(mut g) = manifest_miss_set().lock() {
            g.insert(key);
        }
}

/// Manifest cache file is fresh when younger than 7 days.
pub fn cache_fresh(path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .map(|age| age < MANIFEST_MAX_AGE)
        .unwrap_or(false)
}

/// Revision-aware manifest cache freshness: rejects cached manifests whose `.rev`
/// sidecar differs from the live Google catalog `lastModified` revision.
pub fn cache_fresh_for_revision(path: &Path, now: SystemTime, live_revision: Option<&str>) -> bool {
    if !cache_fresh(path, now) {
        return false;
    }
    let Some(live_rev) = live_revision.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let rev_path = path.with_extension("rev");
    match std::fs::read_to_string(&rev_path) {
        Ok(stored) => stored.trim() == live_rev,
        Err(_) => false,
    }
}

/// Manifest for `family`: cached text in `cache_dir` (≤7 days), else fetched
/// with the 20s probe timeout, session negative-cache, and circuit breaker.
/// A failed refetch falls back to a stale cache rather than nothing.
pub fn fetch_manifest_cached(
    client: &reqwest::blocking::Client,
    family: &str,
    cache_dir: Option<&Path>,
    cancel: &dyn Fn() -> bool,
) -> Option<Manifest> {
    fetch_manifest_cached_with_rev(client, family, cache_dir, None, cancel)
}

/// Manifest fetch with optional live `lastModified` revision cache invalidation.
pub fn fetch_manifest_cached_with_rev(
    client: &reqwest::blocking::Client,
    family: &str,
    cache_dir: Option<&Path>,
    live_revision: Option<&str>,
    cancel: &dyn Fn() -> bool,
) -> Option<Manifest> {
    let cache = cache_dir.map(|d| d.join(format!("{}.json", cache_key(family))));
    if let Some(p) = &cache
        && cache_fresh_for_revision(p, SystemTime::now(), live_revision)
            && let Some(m) = std::fs::read_to_string(p).ok().and_then(|t| parse_manifest(&t)) {
                return Some(m);
            }
    if manifest_missed_this_session(family) || !manifest_circuit_allow() {
        return cache
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| parse_manifest(&t));
    }
    let opts = PoliteOpts {
        ua: None,
        timeout: Some(PROBE_TIMEOUT),
        cdn_lane: false,
    };
    let fetched = polite_fetch(client, &download_list_url(family), &opts, cancel)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| parse_manifest(&t).map(|m| (t, m)));
    match fetched {
        Some((text, m)) => {
            note_manifest_success();
            if let Some(p) = &cache {
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(p, text.as_bytes());
                if let Some(rev) = live_revision.map(str::trim).filter(|s| !s.is_empty()) {
                    let _ = std::fs::write(p.with_extension("rev"), rev.as_bytes());
                }
            }
            Some(m)
        }
        None => {
            note_manifest_miss(family);
            note_manifest_failure();
            cache
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| parse_manifest(&t))
        }
    }
}

pub fn cache_key(family: &str) -> String {
    family
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Per-folder URL cache: `.source-urls` lines `filename\turl`.
pub const SOURCE_URLS_MARKER: &str = ".source-urls";

pub fn read_source_urls(dir: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(dir.join(SOURCE_URLS_MARKER))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string())))
        .collect()
}

pub fn source_url_for(dir: &Path, filename: &str) -> Option<String> {
    read_source_urls(dir).into_iter().find(|(f, _)| f == filename).map(|(_, u)| u)
}

pub fn record_source_url(dir: &Path, filename: &str, url: &str) {
    let mut rows: Vec<_> = read_source_urls(dir).into_iter().filter(|(f, _)| f != filename).collect();
    rows.push((filename.to_string(), url.to_string()));
    rows.sort();
    let body: String = rows.iter().map(|(f, u)| format!("{f}\t{u}\n")).collect();
    let _ = std::fs::write(dir.join(SOURCE_URLS_MARKER), body);
}

/// css2 web-build faces in this folder: never complete, never a repair.
pub const WEB_BUILD_MARKER: &str = ".web-build";

pub fn dir_is_web_build(dir: &Path) -> bool {
    dir.join(WEB_BUILD_MARKER).exists()
}

pub fn set_web_build(dir: &Path, on: bool) {
    let p = dir.join(WEB_BUILD_MARKER);
    if on {
        let _ = std::fs::write(&p, b"css2 web build: feature-stripped; Repair fetches the full files\n");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

/// Full-source provenance: this folder's statics came from the manifest or repo.
pub const FULL_SOURCE_MARKER: &str = ".full-source";

pub fn dir_has_full_source(dir: &Path) -> bool {
    dir.join(FULL_SOURCE_MARKER).exists()
}

pub fn set_full_source(dir: &Path, src: ListingSource) {
    let _ = std::fs::write(dir.join(FULL_SOURCE_MARKER), src.label().as_bytes());
}

pub fn clear_full_source(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FULL_SOURCE_MARKER));
}

// ---------------------------------------------------------------- VF policy

pub const PREFER_STATICS_MARKER: &str = "prefer-statics";
pub const FAST_RESUME_MARKER: &str = "fast-resume";
pub const NO_FAST_RESUME_MARKER: &str = "no-fast-resume";
pub const DIR_PREFER_STATICS_MARKER: &str = ".prefer-statics";

/// 1.0.206u VF-Primary default: prefer variable fonts unless the user opts into
/// Static-first (`prefer-statics` file in the app config dir, `FM_STATIC_FIRST=1`,
/// or `FM_PREFER_VARIABLE=0`).
pub fn prefer_variable(config_dir: Option<&Path>) -> bool {
    if let Ok(v) = std::env::var("FM_PREFER_VARIABLE") {
        if v == "0" || v.eq_ignore_ascii_case("false") {
            return false;
        }
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return true;
        }
    }
    if std::env::var("FM_STATIC_FIRST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }
    if config_dir.map(|d| d.join(PREFER_STATICS_MARKER).exists()).unwrap_or(false) {
        return false;
    }
    true
}

fn set_config_marker(config_dir: Option<&Path>, marker: &str, enabled: bool) {
    let Some(dir) = config_dir else {
        return;
    };
    let p = dir.join(marker);
    if enabled {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&p, b"1\n");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

pub fn set_prefer_statics(config_dir: Option<&Path>, prefer_statics: bool) {
    set_config_marker(config_dir, PREFER_STATICS_MARKER, prefer_statics);
}

/// Automatic Fast Session Resume (enabled by default): bypass deep header
/// re-verification on `.complete` non-web folders during boot/activation unless
/// explicitly disabled (`no-fast-resume` marker or `FM_FAST_RESUME=0`).
pub fn fast_session_resume(config_dir: Option<&Path>) -> bool {
    if let Ok(v) = std::env::var("FM_FAST_RESUME") {
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return true;
        }
        if v == "0" || v.eq_ignore_ascii_case("false") {
            return false;
        }
    }
    !config_dir
        .map(|d| d.join(NO_FAST_RESUME_MARKER).exists())
        .unwrap_or(false)
}

pub fn set_fast_session_resume(config_dir: Option<&Path>, on: bool) {
    set_config_marker(config_dir, FAST_RESUME_MARKER, on);
    set_config_marker(config_dir, NO_FAST_RESUME_MARKER, !on);
}

pub fn dir_prefers_statics(dir: &Path) -> bool {
    dir.join(DIR_PREFER_STATICS_MARKER).exists()
}

pub fn set_dir_prefer_statics(dir: &Path, on: bool) {
    let p = dir.join(DIR_PREFER_STATICS_MARKER);
    if on {
        let _ = std::fs::write(&p, b"1\n");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

/// Never both for one family: VF only when preferred, or when no statics exist.
pub fn use_variable(prefer_vf: bool, statics_available: bool) -> bool {
    prefer_vf || !statics_available
}
