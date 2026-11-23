//! Dedicated Skia (`skia-safe` with embedded FreeType + HarfBuzz) preview render worker
//! and LRU `Typeface` cache.
//!
//! Because `skia_safe::Typeface` and `FontMgr` are `!Send`, a single dedicated background OS
//! thread owns the `FontMgr` and `LruCache<String, Typeface>` (capacity 100) and services
//! async Tauri render requests via `std::sync::mpsc` + `tokio::sync::oneshot`.
//!
//! Responsiveness:
//! - **Latest-wins coalescing:** every request carries a `slot` (one per on-screen canvas).
//!   Before rendering, the worker drains its queue and answers superseded requests for the
//!   same slot with [`SUPERSEDED`], so dragging a slider never builds a render backlog.
//! - **Face matching:** `render_font_specimen` receives every file of a family and picks the
//!   face whose OS/2 weight / italic bits best match the request (cached per path), falling
//!   back to a variable `ital`/`slnt` axis or a synthetic oblique skew when no italic exists.
//! - **Auto height:** the specimen command lays out the paragraph first and returns a canvas
//!   tall enough for every line (capped by `max_height`) so the Inspector never clips text.

use lru::LruCache;
use memmap2::Mmap;
use read_fonts::{FileRef, TableProvider};
use skia_safe::{
    font_arguments::{variation_position::Coordinate, VariationPosition},
    surfaces,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle, TypefaceFontProvider,
    },
    AlphaType, Color, ColorType, Data, FontArguments, FontMgr, FourByteTag, ImageInfo, Typeface,
};
use std::collections::HashMap;
use std::fs::{self, File};
use std::num::NonZeroUsize;
use std::sync::{mpsc, Mutex};
use std::thread;
use tauri::State;
use tokio::sync::oneshot;

const TYPEFACE_CACHE_CAPACITY: usize = 100;
const STYLE_CACHE_CAPACITY: usize = 2048;
const MAX_CANVAS_DIM: i32 = 4096;
/// Horizontal shear used for synthetic oblique when a family ships no italic face.
const SYNTHETIC_OBLIQUE_SKEW: f32 = 0.2;
/// Error string returned to callers whose request was replaced by a newer one for the same slot.
pub const SUPERSEDED: &str = "superseded";

/// Cheap per-file style facts read once with `read-fonts` (never loads the face into Skia).
#[derive(Clone, Copy, Debug, PartialEq)]
struct FaceStyle {
    weight: f32,
    italic: bool,
    variable: bool,
    wght_min: f32,
    wght_max: f32,
    has_ital_axis: bool,
    has_slnt_axis: bool,
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
    auto_height: bool,
}

struct RenderRequest {
    params: RenderParams,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Output of a single render: raw unpremultiplied RGBA8888 pixels plus their dimensions.
#[derive(Debug)]
struct RenderOutput {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

/// Tauri managed state wrapping the sender channel to the dedicated Skia render OS thread.
pub struct SkiaRendererState {
    tx: Mutex<mpsc::Sender<RenderRequest>>,
}

impl SkiaRendererState {
    /// Spawn the dedicated background OS thread owning Skia's `FontMgr` and LRU `Typeface` cache.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<RenderRequest>();
        thread::Builder::new()
            .name("fonths-skia-renderer".into())
            .spawn(move || run_render_loop(rx))
            .expect("failed to spawn Skia render worker thread");
        Self { tx: Mutex::new(tx) }
    }

    async fn submit(&self, params: RenderParams) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let guard = self
                .tx
                .lock()
                .map_err(|_| "Skia renderer mutex poisoned".to_string())?;
            guard
                .send(RenderRequest {
                    params,
                    reply: reply_tx,
                })
                .map_err(|_| "Skia render worker channel closed".to_string())?;
        }
        reply_rx
            .await
            .map_err(|_| "Skia render worker dropped response".to_string())?
    }
}

impl Default for SkiaRendererState {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_color_hex(hex: &str) -> Color {
    let clean = hex.trim().trim_start_matches('#');
    let byte = |s: &str, d: u8| u8::from_str_radix(s, 16).unwrap_or(d);
    match clean.len() {
        3 => Color::from_argb(
            255,
            byte(&clean[0..1], 1) * 17,
            byte(&clean[1..2], 1) * 17,
            byte(&clean[2..3], 1) * 17,
        ),
        6 => Color::from_argb(
            255,
            byte(&clean[0..2], 0x11),
            byte(&clean[2..4], 0x11),
            byte(&clean[4..6], 0x11),
        ),
        8 => Color::from_argb(
            byte(&clean[6..8], 0xFF),
            byte(&clean[0..2], 0x11),
            byte(&clean[2..4], 0x11),
            byte(&clean[4..6], 0x11),
        ),
        _ => Color::from_argb(255, 0x11, 0x11, 0x11),
    }
}

fn tag_from_str(tag: &str) -> Option<FourByteTag> {
    let bytes = tag.as_bytes();
    if bytes.len() != 4 {
        return None;
    }
    Some(FourByteTag::from_chars(
        bytes[0] as char,
        bytes[1] as char,
        bytes[2] as char,
        bytes[3] as char,
    ))
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
                b"slnt" => style.has_slnt_axis = true,
                _ => {}
            }
        }
    }
    Some(style)
}

fn read_face_style(path: &str) -> Option<FaceStyle> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() < 12 {
        return None;
    }
    // SAFETY: read-only map of an on-disk font file; only immutable reads are performed.
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
    // Unparseable styles: fall back to the first candidate (Skia may still decode it).
    Some(FacePick {
        path: candidates[0].0.clone(),
        style: None,
        synthetic_oblique: italic,
    })
}

fn load_typeface_from_path(font_mgr: &FontMgr, path: &str) -> Result<Typeface, String> {
    let raw = fs::read(path).map_err(|e| format!("Failed to read font '{path}': {e}"))?;
    if raw.len() < 12 {
        return Err(format!("Font file too small: '{path}'"));
    }
    if let Some(tf) = font_mgr.new_from_data(Data::new_copy(&raw), 0) {
        return Ok(tf);
    }
    // Fallback for WOFF2 if the active FreeType build did not decode the wrapper directly.
    if raw.starts_with(b"wOF2") {
        let decoded = wuff::decompress_woff2(&raw)
            .map_err(|e| format!("Failed to decompress WOFF2 '{path}': {e:?}"))?;
        if let Some(tf) = font_mgr.new_from_data(Data::new_copy(&decoded), 0) {
            return Ok(tf);
        }
    }
    Err(format!("Skia could not decode typeface from '{path}'"))
}

struct Worker {
    font_mgr: FontMgr,
    typefaces: LruCache<String, Typeface>,
    styles: LruCache<String, Option<FaceStyle>>,
}

impl Worker {
    fn new(typeface_capacity: usize) -> Self {
        Self {
            font_mgr: FontMgr::custom_empty().unwrap_or_else(FontMgr::new),
            typefaces: LruCache::new(NonZeroUsize::new(typeface_capacity.max(1)).unwrap()),
            styles: LruCache::new(NonZeroUsize::new(STYLE_CACHE_CAPACITY).unwrap()),
        }
    }

    fn style_of(&mut self, path: &str) -> Option<FaceStyle> {
        if let Some(s) = self.styles.get(path) {
            return *s;
        }
        let s = read_face_style(path);
        self.styles.put(path.to_string(), s);
        s
    }

    fn typeface(&mut self, path: &str) -> Result<Typeface, String> {
        if let Some(tf) = self.typefaces.get(path) {
            return Ok(tf.clone());
        }
        let tf = load_typeface_from_path(&self.font_mgr, path)?;
        self.typefaces.put(path.to_string(), tf.clone());
        Ok(tf)
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
                style: None,
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

        let base = self.typeface(&pick.path)?;

        // Variable axes via FontArguments (VariationPosition). For a variable face picked by
        // weight, make sure `wght` follows the requested weight even if the caller omitted it.
        let mut variations = p.variations.clone();
        if let (Some(style), Some(w)) = (pick.style, p.weight) {
            if style.variable && !variations.contains_key("wght") && style.wght_max > style.wght_min {
                variations.insert("wght".into(), w.clamp(style.wght_min, style.wght_max));
            }
        }
        let active = if variations.is_empty() {
            base
        } else {
            let mut coords: Vec<Coordinate> = variations
                .iter()
                .filter_map(|(tag, &value)| tag_from_str(tag).map(|axis| Coordinate { axis, value }))
                .collect();
            coords.sort_by_key(|c| (c.axis.a(), c.axis.b(), c.axis.c(), c.axis.d()));
            if coords.is_empty() {
                base
            } else {
                let args = FontArguments::new().set_variation_design_position(VariationPosition {
                    coordinates: &coords,
                });
                base.clone_with_arguments(&args).unwrap_or(base)
            }
        };

        // Isolated provider: Skia Paragraph + HarfBuzz shape only with the exact on-disk face.
        const PREVIEW_FAMILY_ALIAS: &str = "FonthsPreview";
        let mut provider = TypefaceFontProvider::new();
        provider.register_typeface(active.clone(), Some(PREVIEW_FAMILY_ALIAS));
        let mut font_collection = FontCollection::new();
        font_collection.set_asset_font_manager(Some(provider.into()));
        font_collection.set_default_font_manager(self.font_mgr.clone(), None);

        let mut para_style = ParagraphStyle::new();
        para_style.set_text_align(match p.align.as_str() {
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            _ => TextAlign::Left,
        });

        let mut text_style = TextStyle::new();
        text_style.set_color(parse_color_hex(&p.color_hex));
        text_style.set_font_size(font_size);
        text_style.set_font_families(&[PREVIEW_FAMILY_ALIAS]);
        text_style.set_typeface(Some(active));
        if let Some(lh) = p.line_height.filter(|v| v.is_finite() && *v > 0.5 && *v < 4.0) {
            text_style.set_height(lh);
            text_style.set_height_override(true);
        }
        for (tag, &value) in &p.features {
            if tag.len() == 4 {
                text_style.add_font_feature(tag, value);
            }
        }

        let text = if p.text.trim().is_empty() {
            "The quick brown fox jumps over the lazy dog 0123456789"
        } else {
            p.text.as_str()
        };
        let pad_x = (font_size * 0.08).ceil();
        let pad_y = (font_size * 0.18).ceil();
        let build = |layout_w: f32| {
            let mut builder = ParagraphBuilder::new(&para_style, font_collection.clone());
            builder.push_style(&text_style);
            builder.add_text(text);
            let mut paragraph = builder.build();
            paragraph.layout(layout_w.max(1.0));
            paragraph
        };
        let mut layout_w = (width as f32 - pad_x * 2.0).max(1.0);
        let mut paragraph = build(layout_w);
        let skew = if pick.synthetic_oblique { SYNTHETIC_OBLIQUE_SKEW } else { 0.0 };
        if skew > 0.0 {
            // Reserve the shear overhang so the slanted top-right never clips.
            layout_w = (layout_w - skew * paragraph.height()).max(font_size);
            paragraph = build(layout_w);
        }
        let para_h = paragraph.height();

        let height = if p.auto_height {
            ((para_h + pad_y * 2.0).ceil() as i32).clamp(1, max_height)
        } else {
            max_height
        };

        let mut surface = surfaces::raster_n32_premul((width, height))
            .ok_or_else(|| format!("Failed to allocate {width}x{height} Skia raster surface"))?;
        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        let y = if p.auto_height {
            pad_y
        } else if para_h > 0.0 && para_h < height as f32 {
            ((height as f32 - para_h) * 0.5).floor().max(0.0)
        } else {
            0.0
        };
        canvas.save();
        if skew > 0.0 {
            // x' = x - skew * (y - baseline_top): lean right, anchored at the paragraph bottom.
            canvas.translate((pad_x + skew * (y + para_h), 0.0));
            canvas.skew((-skew, 0.0));
            paragraph.paint(canvas, (0.0, y));
        } else {
            paragraph.paint(canvas, (pad_x, y));
        }
        canvas.restore();

        // Unpremultiplied RGBA8888 for direct HTML5 Canvas `ImageData` consumption.
        let rgba_info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let row_bytes = (width as usize) * 4;
        let mut pixels = vec![0u8; row_bytes * (height as usize)];
        if !surface.read_pixels(&rgba_info, &mut pixels, row_bytes, (0, 0)) {
            return Err("Skia read_pixels failed".to_string());
        }
        Ok(RenderOutput {
            width,
            height,
            pixels,
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

fn run_render_loop(rx: mpsc::Receiver<RenderRequest>) {
    let mut worker = Worker::new(TYPEFACE_CACHE_CAPACITY);
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for req in coalesce_latest(batch) {
            let with_header = req.params.auto_height;
            // A panic inside Skia must not kill the worker thread for the rest of the session.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.render(&req.params)))
                .unwrap_or_else(|_| Err("Skia render panicked".to_string()))
                .map(|out| encode_output(out, with_header));
            let _ = req.reply.send(res);
        }
    }
}

/// Fixed-size Skia render returning raw RGBA8888 pixel bytes (`width * height * 4`, no header).
#[tauri::command]
pub async fn render_font_preview(
    state: State<'_, SkiaRendererState>,
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
/// auto-sizes height to the laid-out paragraph (capped at `max_height`), and coalesces
/// rapid updates per `slot`. Returns `[u32 LE width][u32 LE height]` + RGBA8888 pixels.
#[tauri::command]
pub async fn render_font_specimen(
    state: State<'_, SkiaRendererState>,
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
        px.chunks_exact(4).filter(|c| c[3] > 0).count()
    }

    #[test]
    fn skia_worker_renders_variable_font_with_features_to_rgba8888() {
        let mut worker = Worker::new(16);
        let mut features = HashMap::new();
        features.insert("liga".to_string(), 1);
        features.insert("smcp".to_string(), 1);
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
            .expect("Skia render should succeed");
        assert_eq!(out.pixels.len(), 512 * 72 * 4);
        assert!(alpha_count(&out.pixels) > 100);
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
}
