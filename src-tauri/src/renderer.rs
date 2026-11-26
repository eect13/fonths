//! Dedicated pure-Rust Google Fontations (`read-fonts` + `skrifa`) + `harfrust` + `tiny-skia`
//! preview render worker and LRU font cache.
//!
//! Zero C++ dependencies (no `skia-safe`, no GDI, no DirectWrite):
//! - **Parsing & Variable Instances:** `read_fonts::model::Font` + `memmap2` + `wuff` (`.ttf`, `.otf`, `.ttc`, `.woff2`).
//! - **OpenType Shaping:** `harfrust` (pure-Rust HarfBuzz) honors every OpenType GSUB/GPOS feature tag
//!   (`liga`, `dlig`, `calt`, `kern`, `smcp`, `c2sc`, `onum`, `tnum`, `ss01`–`ss20`, `cv01`–`cv99`, etc.)
//!   plus complex scripts and variable font variations.
//! - **Outline Scaling & Rasterization:** `skrifa::ScalerFont` extracts scaled/hinted outlines into
//!   `tiny_skia::Pixmap` for crisp anti-aliased RGBA8888 output.
//! - **Responsiveness:** latest-wins per-slot coalescing, face matching across a family's files,
//!   variable `wght`/`ital`/`slnt` support, synthetic oblique fallback, and auto-sized height.

use harfrust::{Buffer, Direction, Feature, ShapeOptions, ShaperFont};
use lru::LruCache;
use memmap2::Mmap;
use read_fonts::{
    model::{glyph::outline::PathSink, Blob, Font as FontationsFont, Source},
    types::{GlyphId, Tag},
    FileRef, TableProvider,
};
use skrifa::{DrawMemory, HintOptions, RasterTarget, ScalerFont};
use std::collections::HashMap;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::State;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};
use tokio::sync::oneshot;

const FONT_CACHE_CAPACITY: usize = 128;
const STYLE_CACHE_CAPACITY: usize = 2048;
const MAX_CANVAS_DIM: i32 = 4096;
/// Horizontal shear used for synthetic oblique when a family ships no italic face.
const SYNTHETIC_OBLIQUE_SKEW: f32 = 0.2;
/// Error string returned to callers whose request was replaced by a newer one for the same slot.
pub const SUPERSEDED: &str = "superseded";
/// Sentinel error telling the UI to switch seamlessly to native DOM color rendering for color/emoji fonts.
pub const COLOR_FONT_DOM_FALLBACK: &str = "color-font-dom-fallback";

/// Cheap per-file style facts read once with `read-fonts`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FaceStyle {
    weight: f32,
    italic: bool,
    variable: bool,
    wght_min: f32,
    wght_max: f32,
    has_ital_axis: bool,
    has_slnt_axis: bool,
    slnt_min: f32,
    slnt_max: f32,
    has_color: bool,
}

#[derive(Clone, Debug, Default)]
struct RenderParams {
    slot: String,
    font_paths: Vec<String>,
    text: String,
    font_size: f32,
    width: i32,
    height: i32,
    color_hex: String,
    features: HashMap<String, i32>,
    variations: HashMap<String, f32>,
    weight: Option<f32>,
    italic: bool,
    align: String,
    line_height: Option<f32>,
    letter_spacing: Option<f32>,
    auto_height: bool,
}

struct RenderRequest {
    params: RenderParams,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

enum WorkerMsg {
    Render(Box<RenderRequest>),
    EvictPath(PathBuf, mpsc::Sender<()>),
    EvictDir(PathBuf, mpsc::Sender<()>),
}

static WORKER_TX: OnceLock<mpsc::Sender<WorkerMsg>> = OnceLock::new();
static RENDERER_CACHE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Number of decoded Fontations font faces currently held in the render worker LRU cache.
pub fn cache_len() -> usize {
    RENDERER_CACHE_LEN.load(Ordering::Relaxed)
}

fn normalize_path_str(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_ascii_lowercase()
}

/// Evict a file from the Fontations render worker cache before deleting/moving it on disk.
pub fn evict_path(path: &Path) {
    let Some(tx) = WORKER_TX.get() else {
        return;
    };
    let (ack_tx, ack_rx) = mpsc::channel();
    if tx.send(WorkerMsg::EvictPath(path.to_path_buf(), ack_tx)).is_ok() {
        let _ = ack_rx.recv_timeout(Duration::from_millis(150));
    }
}

/// Evict all cached Fontations entries under `dir` before deleting/recycling a folder.
pub fn evict_dir(dir: &Path) {
    let Some(tx) = WORKER_TX.get() else {
        return;
    };
    let (ack_tx, ack_rx) = mpsc::channel();
    if tx.send(WorkerMsg::EvictDir(dir.to_path_buf(), ack_tx)).is_ok() {
        let _ = ack_rx.recv_timeout(Duration::from_millis(150));
    }
}

/// Output of a single render: raw unpremultiplied RGBA8888 pixels plus their dimensions.
#[derive(Debug)]
struct RenderOutput {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

/// Tauri managed state wrapping the sender channel to the dedicated Fontations render worker.
/// Kept as `SkiaRendererState` alias so existing Tauri state management and call sites stay seamless.
pub struct FontationsRendererState {
    tx: Mutex<mpsc::Sender<WorkerMsg>>,
}

pub type SkiaRendererState = FontationsRendererState;

impl FontationsRendererState {
    /// Spawn the dedicated background OS thread owning the Fontations LRU font cache.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let _ = WORKER_TX.set(tx.clone());
        thread::Builder::new()
            .name("fonths-fontations-renderer".into())
            .spawn(move || run_render_loop(rx))
            .expect("failed to spawn Fontations render worker thread");
        Self { tx: Mutex::new(tx) }
    }

    async fn submit(&self, params: RenderParams) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let guard = self
                .tx
                .lock()
                .map_err(|_| "Fontations renderer mutex poisoned".to_string())?;
            guard
                .send(WorkerMsg::Render(Box::new(RenderRequest {
                    params,
                    reply: reply_tx,
                })))
                .map_err(|_| "Fontations render worker channel closed".to_string())?;
        }
        reply_rx
            .await
            .map_err(|_| "Fontations render worker dropped response".to_string())?
    }
}

impl Default for FontationsRendererState {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_color_hex(hex: &str) -> Color {
    let clean = hex.trim().trim_start_matches('#');
    let byte = |s: &str, d: u8| u8::from_str_radix(s, 16).unwrap_or(d);
    let (r, g, b, a) = match clean.len() {
        3 => (
            byte(&clean[0..1], 1) * 17,
            byte(&clean[1..2], 1) * 17,
            byte(&clean[2..3], 1) * 17,
            255,
        ),
        6 => (
            byte(&clean[0..2], 0x11),
            byte(&clean[2..4], 0x11),
            byte(&clean[4..6], 0x11),
            255,
        ),
        8 => (
            byte(&clean[0..2], 0x11),
            byte(&clean[2..4], 0x11),
            byte(&clean[4..6], 0x11),
            byte(&clean[6..8], 0xFF),
        ),
        _ => (0x11, 0x11, 0x11, 255),
    };
    Color::from_rgba8(r, g, b, a)
}

fn tag_from_str(tag: &str) -> Option<Tag> {
    let bytes = tag.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(Tag::new(&[bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn bytes_have_color_tables(font: &read_fonts::FontRef<'_>) -> bool {
    font.table_data(Tag::new(b"COLR")).is_some()
        || font.table_data(Tag::new(b"CBDT")).is_some()
        || font.table_data(Tag::new(b"SVG ")).is_some()
        || font.table_data(Tag::new(b"sbix")).is_some()
}

fn font_has_color_tables(font: &FontationsFont) -> bool {
    let bytes: &[u8] = match font.source() {
        Source::Blob(b) => b.as_ref(),
        _ => return false,
    };
    let Ok(file_ref) = FileRef::new(bytes) else {
        return false;
    };
    let f = match file_ref {
        FileRef::Font(f) => f,
        FileRef::Collection(c) => match c.get(0) {
            Ok(f) => f,
            Err(_) => return false,
        },
    };
    bytes_have_color_tables(&f)
}

fn face_style_from_bytes(bytes: &[u8]) -> Option<FaceStyle> {
    let owned;
    let data = if bytes.starts_with(b"wOF2") {
        owned = wuff::decompress_woff2(bytes).ok()?;
        owned.as_slice()
    } else {
        bytes
    };
    let font = match FileRef::new(data).ok()? {
        FileRef::Font(f) => f,
        FileRef::Collection(c) => c.get(0).ok()?,
    };
    let os2 = font.os2().ok();
    let weight = os2
        .as_ref()
        .map(|o| o.us_weight_class())
        .filter(|w| *w > 0)
        .unwrap_or(400) as f32;
    // fsSelection bit 0 = ITALIC, bit 9 = OBLIQUE; fall back to head.macStyle bit 1.
    let italic = os2
        .as_ref()
        .map(|o| (o.fs_selection().bits() & 0x0201) != 0)
        .or_else(|| font.head().ok().map(|h| (h.mac_style().bits() & 0x02) != 0))
        .unwrap_or(false);
    let mut style = FaceStyle {
        weight,
        italic,
        variable: false,
        wght_min: weight,
        wght_max: weight,
        has_ital_axis: false,
        has_slnt_axis: false,
        slnt_min: 0.0,
        slnt_max: 0.0,
        has_color: bytes_have_color_tables(&font),
    };
    if let Ok(axes) = font.fvar().and_then(|f| f.axes()) {
        for axis in axes.iter() {
            style.variable = true;
            match &axis.axis_tag().to_be_bytes() {
                b"wght" => {
                    style.wght_min = axis.min_value().to_f32();
                    style.wght_max = axis.max_value().to_f32();
                }
                b"ital" => style.has_ital_axis = true,
                b"slnt" => {
                    style.has_slnt_axis = true;
                    style.slnt_min = axis.min_value().to_f32();
                    style.slnt_max = axis.max_value().to_f32();
                }
                _ => {}
            }
        }
    }
    Some(style)
}

fn read_face_style(path: &str) -> Option<FaceStyle> {
    let file = crate::fast_parse::open_shared_read(Path::new(path)).ok()?;
    if file.metadata().ok()?.len() < 12 {
        return None;
    }
    // SAFETY: read-only map of an on-disk font file opened with FILE_SHARE_DELETE; only immutable reads are performed.
    let mmap = unsafe { Mmap::map(&file) }.ok()?;
    face_style_from_bytes(&mmap)
}

/// Outcome of matching a requested weight/italic against a family's files.
#[derive(Debug, Clone, PartialEq)]
struct FacePick {
    path: String,
    style: Option<FaceStyle>,
    /// No real italic face/axis exists — draw a synthetic oblique skew.
    synthetic_oblique: bool,
}

/// Pick the best face for `weight` / `italic` (pure; unit-tested).
fn pick_face(candidates: &[(String, Option<FaceStyle>)], weight: f32, italic: bool) -> Option<FacePick> {
    if candidates.is_empty() {
        return None;
    }
    let score = |s: &FaceStyle| -> f32 {
        if s.variable && weight >= s.wght_min && weight <= s.wght_max {
            0.0
        } else if s.variable {
            (weight - weight.clamp(s.wght_min, s.wght_max)).abs()
        } else {
            (s.weight - weight).abs()
        }
    };
    let best_by = |want_italic: bool, need_axis: bool| -> Option<(String, FaceStyle)> {
        candidates
            .iter()
            .filter_map(|(p, s)| s.map(|s| (p, s)))
            .filter(|(_, s)| {
                if need_axis {
                    s.has_ital_axis || s.has_slnt_axis
                } else {
                    s.italic == want_italic
                }
            })
            .min_by(|(_, a), (_, b)| {
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.variable.cmp(&a.variable))
            })
            .map(|(p, s)| (p.clone(), s))
    };

    if let Some((path, style)) = best_by(italic, false) {
        return Some(FacePick {
            path,
            style: Some(style),
            synthetic_oblique: false,
        });
    }
    if italic {
        if let Some((path, style)) = best_by(true, true) {
            return Some(FacePick {
                path,
                style: Some(style),
                synthetic_oblique: false,
            });
        }
        if let Some((path, style)) = best_by(false, false) {
            return Some(FacePick {
                path,
                style: Some(style),
                synthetic_oblique: true,
            });
        }
    } else if let Some((path, style)) = best_by(true, false) {
        // Italic-only family: show what exists, never skew it back.
        return Some(FacePick {
            path,
            style: Some(style),
            synthetic_oblique: false,
        });
    }
    Some(FacePick {
        path: candidates[0].0.clone(),
        style: None,
        synthetic_oblique: italic,
    })
}

fn load_font_from_path(path: &str) -> Result<FontationsFont, String> {
    let mut file = crate::fast_parse::open_shared_read(Path::new(path))
        .map_err(|e| format!("Failed to read font '{path}': {e}"))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|e| format!("Failed to read font '{path}': {e}"))?;
    if raw.len() < 12 {
        return Err(format!("Font file too small: '{path}'"));
    }
    let bytes = if raw.starts_with(b"wOF2") {
        wuff::decompress_woff2(&raw)
            .map_err(|e| format!("Failed to decompress WOFF2 '{path}': {e:?}"))?
    } else {
        raw
    };
    FontationsFont::new(bytes, 0)
        .ok_or_else(|| format!("Fontations could not decode font '{path}'"))
}

/// Sink that transforms Y-up Skrifa glyph outlines directly into Y-down `tiny_skia::PathBuilder`.
struct TinySkiaSink {
    pb: PathBuilder,
    origin_x: f32,
    origin_y: f32,
    skew: f32,
}

impl TinySkiaSink {
    #[inline]
    fn tx(&self, x: f32, y: f32) -> (f32, f32) {
        // Font Y is up; canvas Y is down. Positive skew leans tops (+y) to the right.
        (self.origin_x + x + self.skew * y, self.origin_y - y)
    }
}

impl PathSink for TinySkiaSink {
    fn move_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.tx(x, y);
        self.pb.move_to(px, py);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (px, py) = self.tx(x, y);
        self.pb.line_to(px, py);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (qx, qy) = self.tx(cx0, cy0);
        let (px, py) = self.tx(x, y);
        self.pb.quad_to(qx, qy, px, py);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0x, c0y) = self.tx(cx0, cy0);
        let (c1x, c1y) = self.tx(cx1, cy1);
        let (px, py) = self.tx(x, y);
        self.pb.cubic_to(c0x, c0y, c1x, c1y, px, py);
    }

    fn close(&mut self) {
        self.pb.close();
    }
}

#[derive(Clone, Copy)]
struct ShapedGlyph {
    /// `None` = primary specimen font, `Some(i)` = `Worker::fallbacks[i]`.
    font_idx: Option<usize>,
    glyph_id: GlyphId,
    x_advance: f32,
    x_offset: f32,
    y_offset: f32,
}

struct ShapedLine {
    glyphs: Vec<ShapedGlyph>,
    width: f32,
}

/// Detect whether the first strong directional character in `text` belongs to an RTL script
/// (Hebrew, Arabic, Syriac, Thaana, or Arabic Presentation Forms).
fn first_strong_is_rtl(text: &str) -> bool {
    for ch in text.chars() {
        let cp = ch as u32;
        let is_rtl = matches!(
            cp,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF
        );
        if is_rtl {
            return true;
        }
        if ch.is_alphabetic() {
            return false;
        }
    }
    false
}

fn load_system_fallback_fonts() -> Vec<FontationsFont> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        let fonts_dir = crate::scanner::windows_system_fonts_dir();
        // Ordered broad-coverage read-only OS system fonts covering Latin/Cyrillic/Greek,
        // Arabic/Hebrew/Syriac, Indic (Nirmala), Thai (Leelawadee), Japanese, Chinese, Korean, Symbols.
        let candidates = [
            "segoeui.ttf",
            "arial.ttf",
            "tahoma.ttf",
            "seguisym.ttf",
            "LeelawUI.ttf",
            "Nirmala.ttc",
            "YuGothR.ttc",
            "msyh.ttc",
            "malgun.ttf",
        ];
        for name in candidates {
            let p: PathBuf = fonts_dir.join(name);
            let Ok(file) = crate::fast_parse::open_shared_read(&p) else {
                continue;
            };
            let Ok(meta) = file.metadata() else {
                continue;
            };
            if meta.len() < 12 {
                continue;
            }
            // SAFETY: `%SYSTEMROOT%\Fonts` files are immutable OS system fonts; read-only mmap avoids heap duplication.
            let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
                continue;
            };
            let arc: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(mmap);
            if let Some(f) = FontationsFont::new(Blob::from(arc), 0) {
                out.push(f);
            }
        }
    }
    out
}

fn pick_char_font(primary: &FontationsFont, fallbacks: &[FontationsFont], ch: char) -> Option<usize> {
    if ch.is_whitespace() || ch.is_control() || primary.charmap().map_unicode(ch).is_some() {
        return None;
    }
    for (idx, fb) in fallbacks.iter().enumerate() {
        if fb.charmap().map_unicode(ch).is_some() {
            return Some(idx);
        }
    }
    None
}

fn shape_run(
    font: &FontationsFont,
    font_idx: Option<usize>,
    text: &str,
    features: &[Feature],
    font_size: f32,
    letter_spacing_px: f32,
    out_glyphs: &mut Vec<ShapedGlyph>,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let shaper = ShaperFont::new(font);
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    if first_strong_is_rtl(text) {
        buffer.set_direction(Direction::RightToLeft);
    }
    buffer.guess_segment_properties();
    if harfrust::shape(
        &shaper,
        &mut buffer,
        ShapeOptions::new().features(features),
    )
    .is_err()
    {
        return 0.0;
    }
    let upem = font.units_per_em().max(1) as f32;
    let px_scale = font_size / upem;
    let infos = buffer.glyph_infos();
    let positions = buffer.glyph_positions();
    let len = infos.len().min(positions.len());
    let mut width = 0.0f32;
    for i in 0..len {
        let adv = positions[i].x_advance as f32 * px_scale;
        let extra = if i + 1 < len { letter_spacing_px } else { 0.0 };
        let total_adv = adv + extra;
        out_glyphs.push(ShapedGlyph {
            font_idx,
            glyph_id: GlyphId::new(infos[i].glyph_id),
            x_advance: total_adv,
            x_offset: positions[i].x_offset as f32 * px_scale,
            y_offset: positions[i].y_offset as f32 * px_scale,
        });
        width += total_adv;
    }
    width
}

fn segment_needs_fallback(primary: &FontationsFont, text: &str) -> bool {
    let charmap = primary.charmap();
    text.chars()
        .any(|ch| !ch.is_whitespace() && !ch.is_control() && charmap.map_unicode(ch).is_none())
}

fn shape_segment_with_fallbacks(
    primary: &FontationsFont,
    fallbacks: &[FontationsFont],
    text: &str,
    features: &[Feature],
    font_size: f32,
    letter_spacing_px: f32,
) -> ShapedLine {
    if text.is_empty() {
        return ShapedLine {
            glyphs: Vec::new(),
            width: 0.0,
        };
    }
    if fallbacks.is_empty() || !segment_needs_fallback(primary, text) {
        let mut glyphs = Vec::new();
        let width = shape_run(
            primary,
            None,
            text,
            features,
            font_size,
            letter_spacing_px,
            &mut glyphs,
        );
        return ShapedLine {
            glyphs,
            width: width.max(0.0),
        };
    }

    // Partition into contiguous runs by font index (primary vs. matched OS fallback).
    let mut runs: Vec<(Option<usize>, String)> = Vec::new();
    let mut cur_idx: Option<usize> = None;
    let mut cur_buf = String::new();
    for ch in text.chars() {
        let target_idx = if ch.is_whitespace() || ch.is_control() {
            cur_idx
        } else {
            pick_char_font(primary, fallbacks, ch)
        };
        if !cur_buf.is_empty() && target_idx != cur_idx {
            runs.push((cur_idx, std::mem::take(&mut cur_buf)));
        }
        cur_idx = target_idx;
        cur_buf.push(ch);
    }
    if !cur_buf.is_empty() {
        runs.push((cur_idx, cur_buf));
    }

    let mut glyphs = Vec::new();
    let mut total_w = 0.0f32;
    let run_count = runs.len();
    for (i, (idx_opt, seg)) in runs.into_iter().enumerate() {
        let font = idx_opt.and_then(|i| fallbacks.get(i)).unwrap_or(primary);
        let w = shape_run(
            font,
            idx_opt,
            &seg,
            features,
            font_size,
            letter_spacing_px,
            &mut glyphs,
        );
        total_w += w;
        if i + 1 < run_count {
            total_w += letter_spacing_px;
            if let Some(last) = glyphs.last_mut() {
                last.x_advance += letter_spacing_px;
            }
        }
    }
    ShapedLine {
        glyphs,
        width: total_w.max(0.0),
    }
}

/// Split `text` into word-wrapped lines using `harfrust` shaping widths so every final line
/// is shaped intact (preserving ligatures, kerning, contextual alternates, and complex scripts).
fn wrap_and_shape_lines(
    primary: &FontationsFont,
    fallbacks: &[FontationsFont],
    text: &str,
    features: &[Feature],
    font_size: f32,
    letter_spacing_px: f32,
    max_width: f32,
) -> Vec<ShapedLine> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let trimmed_para = para.trim_end_matches('\r');
        if trimmed_para.is_empty() {
            lines.push(ShapedLine {
                glyphs: Vec::new(),
                width: 0.0,
            });
            continue;
        }
        let whole = shape_segment_with_fallbacks(
            primary,
            fallbacks,
            trimmed_para,
            features,
            font_size,
            letter_spacing_px,
        );
        if whole.width <= max_width {
            lines.push(whole);
            continue;
        }

        // Greedy word-boundary wrap; re-shapes each committed line intact for exact kerning/ligatures.
        let mut current = String::new();
        for token in trimmed_para.split_inclusive(char::is_whitespace) {
            let word = token.trim_end_matches(char::is_whitespace);
            let candidate = if current.is_empty() {
                token.to_string()
            } else {
                format!("{current}{token}")
            };
            let cand_trimmed = candidate.trim_end_matches(char::is_whitespace);
            let measured = shape_segment_with_fallbacks(
                primary,
                fallbacks,
                cand_trimmed,
                features,
                font_size,
                letter_spacing_px,
            );
            if measured.width <= max_width {
                current = candidate;
                continue;
            }
            if !current.trim_end().is_empty() {
                lines.push(shape_segment_with_fallbacks(
                    primary,
                    fallbacks,
                    current.trim_end(),
                    features,
                    font_size,
                    letter_spacing_px,
                ));
                current.clear();
            }
            let single = shape_segment_with_fallbacks(
                primary,
                fallbacks,
                word,
                features,
                font_size,
                letter_spacing_px,
            );
            if single.width <= max_width {
                current = token.to_string();
            } else {
                let mut piece = String::new();
                for ch in word.chars() {
                    let mut next_piece = piece.clone();
                    next_piece.push(ch);
                    let m = shape_segment_with_fallbacks(
                        primary,
                        fallbacks,
                        &next_piece,
                        features,
                        font_size,
                        letter_spacing_px,
                    );
                    if !piece.is_empty() && m.width > max_width {
                        lines.push(shape_segment_with_fallbacks(
                            primary,
                            fallbacks,
                            &piece,
                            features,
                            font_size,
                            letter_spacing_px,
                        ));
                        piece.clear();
                    }
                    piece.push(ch);
                }
                if !piece.is_empty() {
                    current = if token.len() > word.len() {
                        format!("{piece} ")
                    } else {
                        piece
                    };
                }
            }
        }
        if !current.trim_end().is_empty() {
            lines.push(shape_segment_with_fallbacks(
                primary,
                fallbacks,
                current.trim_end(),
                features,
                font_size,
                letter_spacing_px,
            ));
        }
    }
    if lines.is_empty() {
        lines.push(ShapedLine {
            glyphs: Vec::new(),
            width: 0.0,
        });
    }
    lines
}

/// Convert `tiny-skia` premultiplied RGBA8888 pixels to unpremultiplied RGBA8888 for HTML5 Canvas `ImageData`.
fn unpremultiply_rgba(premul: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; premul.len()];
    for (src, dst) in premul.as_chunks::<4>().0.iter().zip(out.as_chunks_mut::<4>().0.iter_mut()) {
        let a = src[3];
        if a == 0 {
            dst.copy_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            dst.copy_from_slice(src);
        } else {
            let a16 = u16::from(a);
            dst[0] = ((u16::from(src[0]) * 255 + a16 / 2) / a16).min(255) as u8;
            dst[1] = ((u16::from(src[1]) * 255 + a16 / 2) / a16).min(255) as u8;
            dst[2] = ((u16::from(src[2]) * 255 + a16 / 2) / a16).min(255) as u8;
            dst[3] = a;
        }
    }
    out
}

struct Worker {
    fonts: LruCache<String, FontationsFont>,
    styles: LruCache<String, Option<FaceStyle>>,
    fallbacks: Option<Vec<FontationsFont>>,
}

impl Worker {
    fn new(font_capacity: usize) -> Self {
        Self {
            fonts: LruCache::new(NonZeroUsize::new(font_capacity.max(1)).unwrap()),
            styles: LruCache::new(NonZeroUsize::new(STYLE_CACHE_CAPACITY).unwrap()),
            fallbacks: None,
        }
    }

    fn ensure_fallbacks(&mut self) -> &[FontationsFont] {
        if self.fallbacks.is_none() {
            self.fallbacks = Some(load_system_fallback_fonts());
        }
        self.fallbacks.as_deref().unwrap_or(&[])
    }

    fn style_of(&mut self, path: &str) -> Option<FaceStyle> {
        if let Some(s) = self.styles.get(path) {
            return *s;
        }
        let s = read_face_style(path);
        self.styles.put(path.to_string(), s);
        s
    }

    fn font(&mut self, path: &str) -> Result<FontationsFont, String> {
        if let Some(f) = self.fonts.get(path) {
            return Ok(f.clone());
        }
        let f = load_font_from_path(path)?;
        self.fonts.put(path.to_string(), f.clone());
        RENDERER_CACHE_LEN.store(self.fonts.len(), Ordering::Relaxed);
        Ok(f)
    }

    fn evict_path(&mut self, path: &Path) {
        let target = normalize_path_str(path);
        let font_keys: Vec<String> = self
            .fonts
            .iter()
            .filter(|(k, _)| normalize_path_str(Path::new(k.as_str())) == target)
            .map(|(k, _)| k.clone())
            .collect();
        for k in font_keys {
            self.fonts.pop(&k);
        }
        let style_keys: Vec<String> = self
            .styles
            .iter()
            .filter(|(k, _)| normalize_path_str(Path::new(k.as_str())) == target)
            .map(|(k, _)| k.clone())
            .collect();
        for k in style_keys {
            self.styles.pop(&k);
        }
        RENDERER_CACHE_LEN.store(self.fonts.len(), Ordering::Relaxed);
    }

    fn evict_dir(&mut self, dir: &Path) {
        let mut prefix = normalize_path_str(dir);
        if !prefix.ends_with('\\') {
            prefix.push('\\');
        }
        let trimmed = prefix.trim_end_matches('\\');
        let font_keys: Vec<String> = self
            .fonts
            .iter()
            .filter(|(k, _)| {
                let norm = normalize_path_str(Path::new(k.as_str()));
                norm.starts_with(&prefix) || norm == trimmed
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in font_keys {
            self.fonts.pop(&k);
        }
        let style_keys: Vec<String> = self
            .styles
            .iter()
            .filter(|(k, _)| {
                let norm = normalize_path_str(Path::new(k.as_str()));
                norm.starts_with(&prefix) || norm == trimmed
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in style_keys {
            self.styles.pop(&k);
        }
        RENDERER_CACHE_LEN.store(self.fonts.len(), Ordering::Relaxed);
    }

    fn render(&mut self, p: &RenderParams) -> Result<RenderOutput, String> {
        let width = p.width.clamp(1, MAX_CANVAS_DIM);
        let max_height = p.height.clamp(1, MAX_CANVAS_DIM);
        let font_size = p.font_size.clamp(4.0, 512.0);

        let paths: Vec<String> = p
            .font_paths
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if paths.is_empty() {
            return Err("No font file to render".to_string());
        }
        let pick = if paths.len() == 1 && p.weight.is_none() && !p.italic {
            FacePick {
                path: paths[0].clone(),
                style: self.style_of(&paths[0]),
                synthetic_oblique: false,
            }
        } else {
            let candidates: Vec<(String, Option<FaceStyle>)> = paths
                .iter()
                .map(|path| (path.clone(), self.style_of(path)))
                .collect();
            pick_face(&candidates, p.weight.unwrap_or(400.0), p.italic)
                .ok_or_else(|| "No font file to render".to_string())?
        };

        let base = self.font(&pick.path)?;
        if pick.style.map(|s| s.has_color).unwrap_or(false) || font_has_color_tables(&base) {
            return Err(COLOR_FONT_DOM_FALLBACK.to_string());
        }

        // Variable font coordinates: combine explicit variations with requested weight/italic axes.
        let mut variations = p.variations.clone();
        if let Some(style) = pick.style
            && style.variable {
                if let Some(w) = p.weight
                    && !variations.contains_key("wght") && style.wght_max > style.wght_min {
                        variations.insert("wght".into(), w.clamp(style.wght_min, style.wght_max));
                    }
                if p.italic {
                    if style.has_ital_axis && !variations.contains_key("ital") {
                        variations.insert("ital".into(), 1.0);
                    } else if style.has_slnt_axis && !variations.contains_key("slnt") {
                        let slnt_val = if style.slnt_min < 0.0 {
                            style.slnt_min.max(-12.0)
                        } else {
                            style.slnt_max.min(12.0)
                        };
                        variations.insert("slnt".into(), slnt_val);
                    }
                }
            }

        let active = if variations.is_empty() {
            base
        } else {
            let mut pairs: Vec<(Tag, f32)> = variations
                .iter()
                .filter_map(|(tag, &val)| tag_from_str(tag).map(|t| (t, val)))
                .collect();
            pairs.sort_by_key(|(t, _)| t.to_be_bytes());
            if pairs.is_empty() {
                base
            } else {
                base.instance_builder().variations(pairs).build()
            }
        };

        let units_per_em = active.units_per_em().max(1);
        let px_scale = font_size / (units_per_em as f32);

        // Build OpenType feature list for HarfRust (every 4-char tag: liga, dlig, smcp, ss01..ss20, etc.).
        let mut ot_features: Vec<Feature> = p
            .features
            .iter()
            .filter_map(|(tag, &val)| {
                let t = tag_from_str(tag)?;
                Some(Feature::new(t, val.max(0) as u32, ..))
            })
            .collect();
        ot_features.sort_by_key(|f| f.tag.to_be_bytes());

        let letter_spacing_px = p
            .letter_spacing
            .filter(|v| v.is_finite() && v.abs() > 0.0001 && v.abs() <= 2.0)
            .map(|ls| ls * font_size)
            .unwrap_or(0.0);

        let text = if p.text.trim().is_empty() {
            "The quick brown fox jumps over the lazy dog 0123456789"
        } else {
            p.text.as_str()
        };

        // Only load OS fallback fonts if the primary font actually lacks glyphs for `text`.
        let fallbacks: Vec<FontationsFont> = if segment_needs_fallback(&active, text) {
            self.ensure_fallbacks().to_vec()
        } else {
            Vec::new()
        };

        // Use light grayscale hinting at smaller sizes for crisp UI legibility, unhinted at display sizes.
        let make_scaler = |font: FontationsFont| {
            if font_size <= 48.0 {
                let hint_opts = HintOptions::auto().with_target(RasterTarget::Light);
                ScalerFont::hinted(font, font_size, hint_opts)
            } else {
                ScalerFont::unhinted(font, Some(font_size))
            }
        };
        let scaler = make_scaler(active.clone());

        let metrics = scaler.metrics();
        let line_box = metrics.h_line();
        let raw_asc = line_box
            .map(|lb| lb.ascender)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(font_size * 0.82);
        let raw_desc = line_box
            .map(|lb| (-lb.descender).max(0.0))
            .filter(|v| v.is_finite())
            .unwrap_or(font_size * 0.22);
        let natural_line_h = (raw_asc + raw_desc).max(font_size);
        let line_step = p
            .line_height
            .filter(|v| v.is_finite() && *v > 0.5 && *v < 4.0)
            .map(|lh| (lh * font_size).max(font_size * 0.6))
            .unwrap_or(natural_line_h * 1.12);

        let skew = if pick.synthetic_oblique {
            SYNTHETIC_OBLIQUE_SKEW
        } else {
            0.0
        };
        let pad_x = (font_size * 0.1).ceil();
        let pad_y = (font_size * 0.22).ceil();
        let skew_margin = (skew * raw_asc).ceil();
        let layout_w = (width as f32 - pad_x * 2.0 - skew_margin).max(font_size.min(width as f32));

        let lines = wrap_and_shape_lines(
            &active,
            &fallbacks,
            text,
            &ot_features,
            font_size,
            letter_spacing_px,
            layout_w,
        );

        let _ = (active.units_per_em(), px_scale);

        // Total visual ink height from top of first line's ascender to bottom of last line's descender.
        let para_h = if lines.len() <= 1 {
            raw_asc + raw_desc
        } else {
            raw_asc + (lines.len() - 1) as f32 * line_step + raw_desc
        };

        let height = if p.auto_height {
            ((para_h + pad_y * 2.0).ceil() as i32).clamp(1, max_height)
        } else {
            max_height
        };

        let mut pixmap = Pixmap::new(width as u32, height as u32)
            .ok_or_else(|| format!("Failed to allocate {width}x{height} tiny-skia pixmap"))?;

        let mut paint = Paint::default();
        paint.set_color(parse_color_hex(&p.color_hex));
        paint.anti_alias = true;

        let top_y = if p.auto_height {
            pad_y
        } else if para_h > 0.0 && para_h < height as f32 {
            ((height as f32 - para_h) * 0.5).floor().max(0.0)
        } else {
            0.0
        };

        let effective_align = if p.align.trim().is_empty() && first_strong_is_rtl(text) {
            "right"
        } else {
            p.align.as_str()
        };

        let mut fallback_scalers: Vec<Option<ScalerFont>> =
            (0..fallbacks.len()).map(|_| None).collect();

        let mut pb = PathBuilder::new();
        for (line_idx, line) in lines.iter().enumerate() {
            let baseline_y = top_y + raw_asc + (line_idx as f32) * line_step;
            if baseline_y - raw_asc > height as f32 + 4.0 {
                break;
            }
            let start_x = match effective_align {
                "center" => pad_x + ((layout_w - line.width) * 0.5).max(0.0),
                "right" => pad_x + (layout_w - line.width).max(0.0),
                _ => pad_x,
            };
            let mut pen_x = start_x;
            for g in &line.glyphs {
                let outline_opt = match g.font_idx {
                    None => scaler.outline(g.glyph_id),
                    Some(idx) => {
                        if let Some(slot) = fallback_scalers.get_mut(idx) {
                            let fb_scaler = slot.get_or_insert_with(|| make_scaler(fallbacks[idx].clone()));
                            fb_scaler.outline(g.glyph_id)
                        } else {
                            None
                        }
                    }
                };
                if let Some(outline) = outline_opt {
                    let mut sink = TinySkiaSink {
                        pb: std::mem::take(&mut pb),
                        origin_x: pen_x + g.x_offset,
                        origin_y: baseline_y - g.y_offset,
                        skew,
                    };
                    let _ = outline.draw(DrawMemory::Scratch, &mut sink);
                    pb = sink.pb;
                }
                pen_x += g.x_advance;
            }
        }

        if let Some(path) = pb.finish() {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }

        Ok(RenderOutput {
            width,
            height,
            pixels: unpremultiply_rgba(pixmap.data()),
        })
    }
}

/// Keep only the newest request per slot; answer the rest with [`SUPERSEDED`].
fn coalesce_latest(batch: Vec<RenderRequest>) -> Vec<RenderRequest> {
    let mut latest: HashMap<String, usize> = HashMap::new();
    for (i, req) in batch.iter().enumerate() {
        if !req.params.slot.is_empty() {
            latest.insert(req.params.slot.clone(), i);
        }
    }
    let mut keep = Vec::with_capacity(batch.len());
    for (i, req) in batch.into_iter().enumerate() {
        let stale = !req.params.slot.is_empty() && latest.get(&req.params.slot) != Some(&i);
        if stale {
            let _ = req.reply.send(Err(SUPERSEDED.to_string()));
        } else {
            keep.push(req);
        }
    }
    keep
}

fn encode_output(out: RenderOutput, with_header: bool) -> Vec<u8> {
    if !with_header {
        return out.pixels;
    }
    let mut buf = Vec::with_capacity(out.pixels.len() + 8);
    buf.extend_from_slice(&(out.width as u32).to_le_bytes());
    buf.extend_from_slice(&(out.height as u32).to_le_bytes());
    buf.extend_from_slice(&out.pixels);
    buf
}

fn run_render_loop(rx: mpsc::Receiver<WorkerMsg>) {
    let mut worker = Worker::new(FONT_CACHE_CAPACITY);
    while let Ok(first) = rx.recv() {
        let mut batch: Vec<RenderRequest> = Vec::new();
        let mut evictions: Vec<WorkerMsg> = Vec::new();
        match first {
            WorkerMsg::Render(req) => batch.push(*req),
            ev => evictions.push(ev),
        }
        while let Ok(more) = rx.try_recv() {
            match more {
                WorkerMsg::Render(req) => batch.push(*req),
                ev => evictions.push(ev),
            }
        }
        for req in coalesce_latest(batch) {
            let with_header = req.params.auto_height;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.render(&req.params)))
                .unwrap_or_else(|_| Err("Fontations render panicked".to_string()))
                .map(|out| encode_output(out, with_header));
            let _ = req.reply.send(res);
        }
        for ev in evictions {
            match ev {
                WorkerMsg::EvictPath(p, ack) => {
                    worker.evict_path(&p);
                    let _ = ack.send(());
                }
                WorkerMsg::EvictDir(d, ack) => {
                    worker.evict_dir(&d);
                    let _ = ack.send(());
                }
                WorkerMsg::Render(_) => {}
            }
        }
    }
}

/// Fixed-size Fontations render returning raw RGBA8888 pixel bytes (`width * height * 4`, no header).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn render_font_preview(
    state: State<'_, FontationsRendererState>,
    font_path: String,
    text: String,
    font_size: f32,
    width: i32,
    height: i32,
    color_hex: String,
    features: HashMap<String, i32>,
    variations: HashMap<String, f32>,
) -> Result<tauri::ipc::Response, String> {
    let pixels = state
        .submit(RenderParams {
            font_paths: vec![font_path],
            text,
            font_size,
            width,
            height,
            color_hex,
            features,
            variations,
            ..Default::default()
        })
        .await?;
    Ok(tauri::ipc::Response::new(pixels))
}

/// Live Inspector specimen: picks the best face of a family for `weight` / `italic`,
/// shapes with HarfRust (full OpenType GSUB/GPOS features), scales with Skrifa,
/// auto-sizes height to the laid-out paragraph (capped at `max_height`), and coalesces
/// rapid updates per `slot`. Returns `[u32 LE width][u32 LE height]` + RGBA8888 pixels.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn render_font_specimen(
    state: State<'_, FontationsRendererState>,
    slot: String,
    font_paths: Vec<String>,
    text: String,
    font_size: f32,
    width: i32,
    max_height: i32,
    color_hex: String,
    features: HashMap<String, i32>,
    variations: HashMap<String, f32>,
    weight: Option<f32>,
    italic: Option<bool>,
    align: Option<String>,
    line_height: Option<f32>,
    letter_spacing: Option<f32>,
) -> Result<tauri::ipc::Response, String> {
    let bytes = state
        .submit(RenderParams {
            slot,
            font_paths,
            text,
            font_size,
            width,
            height: max_height,
            color_hex,
            features,
            variations,
            weight,
            italic: italic.unwrap_or(false),
            align: align.unwrap_or_default(),
            line_height,
            letter_spacing,
            auto_height: true,
        })
        .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join(name);
        assert!(p.exists(), "fixture {name} must exist");
        p.to_string_lossy().into_owned()
    }

    fn alpha_count(px: &[u8]) -> usize {
        px.as_chunks::<4>().0.iter().filter(|c| c[3] > 0).count()
    }

    #[test]
    fn fontations_worker_renders_variable_font_with_features_to_rgba8888() {
        let mut worker = Worker::new(16);
        let mut features = HashMap::new();
        features.insert("liga".to_string(), 1);
        features.insert("zero".to_string(), 1);
        let mut variations = HashMap::new();
        variations.insert("wght".to_string(), 700.0);
        let out = worker
            .render(&RenderParams {
                font_paths: vec![fixture("Inter-opsz-wght.ttf")],
                text: "Sphinx of black quartz 0123456789".into(),
                font_size: 36.0,
                width: 512,
                height: 72,
                color_hex: "#111111".into(),
                features,
                variations,
                ..Default::default()
            })
            .expect("Fontations render should succeed");
        assert_eq!(out.pixels.len(), 512 * 72 * 4);
        assert!(alpha_count(&out.pixels) > 100);
    }

    #[test]
    fn opentype_features_actually_change_shaped_glyphs_in_harfrust() {
        let mut worker = Worker::new(8);
        let base = RenderParams {
            font_paths: vec![fixture("Inter-opsz-wght.ttf")],
            text: "0123456789 -> fi fl".into(),
            font_size: 44.0,
            width: 520,
            height: 160,
            color_hex: "#000000".into(),
            auto_height: true,
            ..Default::default()
        };
        let default_px = worker.render(&base).unwrap().pixels;

        let mut with_ot = base.clone();
        with_ot.features.insert("zero".to_string(), 1);
        with_ot.features.insert("tnum".to_string(), 1);
        with_ot.features.insert("ss01".to_string(), 1);
        with_ot.features.insert("calt".to_string(), 1);
        let ot_px = worker.render(&with_ot).unwrap().pixels;
        assert_ne!(
            default_px, ot_px,
            "HarfRust OpenType features (zero/tnum/ss01/calt) must alter rendered pixels"
        );
    }

    #[test]
    fn auto_height_grows_for_wrapped_text_and_never_clips() {
        let mut worker = Worker::new(4);
        let base = RenderParams {
            font_paths: vec![fixture("Figtree-wght.ttf")],
            text: "The quick brown fox jumps over the lazy dog. ".repeat(4),
            font_size: 40.0,
            width: 300,
            height: 2000,
            color_hex: "#000".into(),
            auto_height: true,
            ..Default::default()
        };
        let out = worker.render(&base).unwrap();
        assert!(out.height > 200, "wrapped paragraph should be tall, got {}", out.height);
        assert!(out.height < 2000);
        // Last row stays transparent (bottom padding) so descenders are not cut.
        let last_row = &out.pixels[(out.height as usize - 1) * 300 * 4..];
        assert_eq!(alpha_count(last_row), 0);
        let enc = encode_output(out, true);
        assert_eq!(u32::from_le_bytes(enc[0..4].try_into().unwrap()), 300);
    }

    #[test]
    fn synthetic_italic_changes_pixels_for_upright_only_family() {
        let mut worker = Worker::new(4);
        let mut p = RenderParams {
            font_paths: vec![fixture("Figtree-wght.ttf")],
            text: "Italic Il".into(),
            font_size: 48.0,
            width: 400,
            height: 400,
            color_hex: "#000".into(),
            auto_height: true,
            weight: Some(400.0),
            ..Default::default()
        };
        let upright = worker.render(&p).unwrap();
        p.italic = true;
        let slanted = worker.render(&p).unwrap();
        assert_ne!(upright.pixels, slanted.pixels, "italic toggle must change the render");
    }

    #[test]
    fn variable_weight_follows_requested_weight() {
        let mut worker = Worker::new(4);
        let mut p = RenderParams {
            font_paths: vec![fixture("Inter-opsz-wght.ttf")],
            text: "Weight".into(),
            font_size: 48.0,
            width: 400,
            height: 200,
            color_hex: "#000".into(),
            auto_height: true,
            weight: Some(100.0),
            ..Default::default()
        };
        let thin = alpha_count(&worker.render(&p).unwrap().pixels);
        p.weight = Some(900.0);
        let black = alpha_count(&worker.render(&p).unwrap().pixels);
        assert!(black > thin, "wght 900 ({black}) must ink more than wght 100 ({thin})");
    }

    #[test]
    fn pick_face_matches_weight_italic_and_falls_back() {
        let st = |w: f32, italic: bool| FaceStyle {
            weight: w,
            italic,
            variable: false,
            wght_min: w,
            wght_max: w,
            has_ital_axis: false,
            has_slnt_axis: false,
            slnt_min: 0.0,
            slnt_max: 0.0,
            has_color: false,
        };
        let c = vec![
            ("r400".to_string(), Some(st(400.0, false))),
            ("r700".to_string(), Some(st(700.0, false))),
            ("i400".to_string(), Some(st(400.0, true))),
        ];
        assert_eq!(pick_face(&c, 650.0, false).unwrap().path, "r700");
        let it = pick_face(&c, 700.0, true).unwrap();
        assert_eq!((it.path.as_str(), it.synthetic_oblique), ("i400", false));
        let upright_only = vec![c[0].clone(), c[1].clone()];
        let synth = pick_face(&upright_only, 400.0, true).unwrap();
        assert_eq!((synth.path.as_str(), synth.synthetic_oblique), ("r400", true));
        let mut vf = st(400.0, false);
        vf.variable = true;
        vf.wght_min = 100.0;
        vf.wght_max = 900.0;
        vf.has_slnt_axis = true;
        let axis = pick_face(&[("vf".to_string(), Some(vf))], 400.0, true).unwrap();
        assert_eq!((axis.path.as_str(), axis.synthetic_oblique), ("vf", false));
    }

    #[test]
    fn coalesce_keeps_latest_per_slot_and_answers_superseded() {
        let mk = |slot: &str, text: &str| {
            let (tx, rx) = oneshot::channel();
            (
                RenderRequest {
                    params: RenderParams {
                        slot: slot.into(),
                        text: text.into(),
                        ..Default::default()
                    },
                    reply: tx,
                },
                rx,
            )
        };
        let (a1, mut r1) = mk("inspector", "1");
        let (b1, _rb) = mk("other", "b");
        let (a2, _r2) = mk("inspector", "2");
        let kept = coalesce_latest(vec![a1, b1, a2]);
        let texts: Vec<_> = kept.iter().map(|r| r.params.text.clone()).collect();
        assert_eq!(texts, vec!["b", "2"]);
        assert_eq!(r1.try_recv().unwrap(), Err(SUPERSEDED.to_string()));
    }

    #[test]
    fn letter_spacing_changes_rendered_pixels() {
        let mut worker = Worker::new(4);
        let mut p = RenderParams {
            font_paths: vec![fixture("Figtree-wght.ttf")],
            text: "Tracking".into(),
            font_size: 40.0,
            width: 480,
            height: 200,
            color_hex: "#000".into(),
            auto_height: true,
            ..Default::default()
        };
        let tight = worker.render(&p).unwrap();
        p.letter_spacing = Some(0.18);
        let wide = worker.render(&p).unwrap();
        assert_ne!(tight.pixels, wide.pixels, "letter_spacing must change rendered Fontations output");
    }

    #[test]
    fn rtl_arabic_text_shapes_and_renders_pixels() {
        assert!(first_strong_is_rtl("مرحبا بكم"));
        assert!(!first_strong_is_rtl("Hello مرحبا"));
        let mut worker = Worker::new(4);
        let out = worker
            .render(&RenderParams {
                font_paths: vec![fixture("Inter-opsz-wght.ttf")],
                text: "مرحبا بالعالم".into(),
                font_size: 40.0,
                width: 480,
                height: 160,
                color_hex: "#000000".into(),
                auto_height: true,
                ..Default::default()
            })
            .expect("RTL Arabic specimen should shape via primary or OS fallback");
        #[cfg(windows)]
        assert!(alpha_count(&out.pixels) > 80, "RTL Arabic must render inked pixels via Windows fallback");
        let _ = out;
    }

    #[test]
    fn missing_glyphs_fall_back_to_windows_system_fonts() {
        let mut worker = Worker::new(4);
        let ascii = worker
            .render(&RenderParams {
                font_paths: vec![fixture("Inter-opsz-wght.ttf")],
                text: "Hello ".into(),
                font_size: 40.0,
                width: 600,
                height: 160,
                color_hex: "#000000".into(),
                auto_height: true,
                ..Default::default()
            })
            .unwrap();
        let mixed = worker
            .render(&RenderParams {
                font_paths: vec![fixture("Inter-opsz-wght.ttf")],
                text: "Hello 日本語 한글 العربية".into(),
                font_size: 40.0,
                width: 600,
                height: 160,
                color_hex: "#000000".into(),
                auto_height: true,
                ..Default::default()
            })
            .unwrap();
        #[cfg(windows)]
        assert!(
            alpha_count(&mixed.pixels) > alpha_count(&ascii.pixels),
            "mixed CJK/Hangul/Arabic fallback must ink more pixels than ASCII alone"
        );
        let _ = (ascii, mixed);
    }

    #[test]
    fn color_emoji_font_signals_dom_color_fallback() {
        let emj = crate::scanner::windows_system_fonts_dir().join("seguiemj.ttf");
        if !emj.is_file() {
            return;
        }
        let mut worker = Worker::new(4);
        let res = worker.render(&RenderParams {
            font_paths: vec![emj.to_string_lossy().into_owned()],
            text: "🎨🚀✨".into(),
            font_size: 40.0,
            width: 320,
            height: 120,
            color_hex: "#000000".into(),
            auto_height: true,
            ..Default::default()
        });
        assert_eq!(res.unwrap_err(), COLOR_FONT_DOM_FALLBACK);
    }

    #[test]
    fn worker_evict_path_and_dir_clear_font_and_style_caches() {
        let mut worker = Worker::new(8);
        let p = fixture("Figtree-wght.ttf");
        let _ = worker.font(&p).expect("load fixture");
        assert!(worker.style_of(&p).is_some());
        assert_eq!(worker.fonts.len(), 1);
        assert!(cache_len() >= 1);

        worker.evict_path(Path::new(&p));
        assert_eq!(worker.fonts.len(), 0);
        assert_eq!(worker.styles.len(), 0);

        let _ = worker.font(&p).expect("reload fixture");
        let dir = Path::new(&p).parent().unwrap();
        worker.evict_dir(dir);
        assert_eq!(worker.fonts.len(), 0);
        assert_eq!(cache_len(), 0);
    }
}
