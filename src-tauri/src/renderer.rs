//! Dedicated Skia (`skia-safe` with embedded FreeType + HarfBuzz) preview render worker
//! and LRU `Typeface` cache.
//!
//! Because `skia_safe::Typeface` and `FontMgr` are `!Send`, a single dedicated background OS
//! thread owns the `FontMgr` and `LruCache<String, Typeface>` (capacity 100) and services
//! async Tauri `render_font_preview` requests via `std::sync::mpsc` + `tokio::sync::oneshot`.

use lru::LruCache;
use skia_safe::{
    font_arguments::{variation_position::Coordinate, VariationPosition},
    surfaces,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle, TypefaceFontProvider,
    },
    AlphaType, Color, ColorType, Data, FontArguments, FontMgr, FourByteTag, ImageInfo, Typeface,
};
use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::sync::{mpsc, Mutex};
use std::thread;
use tauri::State;
use tokio::sync::oneshot;

const TYPEFACE_CACHE_CAPACITY: usize = 100;
const MAX_CANVAS_DIM: i32 = 4096;

struct RenderRequest {
    font_path: String,
    text: String,
    font_size: f32,
    width: i32,
    height: i32,
    color_hex: String,
    features: HashMap<String, i32>,
    variations: HashMap<String, f32>,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
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

    /// Submit a render job to the background Skia thread and await raw RGBA8888 pixels.
    pub async fn render(
        &self,
        font_path: String,
        text: String,
        font_size: f32,
        width: i32,
        height: i32,
        color_hex: String,
        features: HashMap<String, i32>,
        variations: HashMap<String, f32>,
    ) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let req = RenderRequest {
            font_path,
            text,
            font_size,
            width,
            height,
            color_hex,
            features,
            variations,
            reply: reply_tx,
        };
        {
            let guard = self
                .tx
                .lock()
                .map_err(|_| "Skia renderer mutex poisoned".to_string())?;
            guard
                .send(req)
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
    match clean.len() {
        3 => {
            let r = u8::from_str_radix(&clean[0..1], 16).unwrap_or(0x11) * 17;
            let g = u8::from_str_radix(&clean[1..2], 16).unwrap_or(0x11) * 17;
            let b = u8::from_str_radix(&clean[2..3], 16).unwrap_or(0x11) * 17;
            Color::from_argb(255, r, g, b)
        }
        6 => {
            let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(0x11);
            let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(0x11);
            let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(0x11);
            Color::from_argb(255, r, g, b)
        }
        8 => {
            let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(0x11);
            let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(0x11);
            let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(0x11);
            let a = u8::from_str_radix(&clean[6..8], 16).unwrap_or(0xFF);
            Color::from_argb(a, r, g, b)
        }
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

fn load_typeface_from_path(font_mgr: &FontMgr, path: &str) -> Result<Typeface, String> {
    let raw = fs::read(path).map_err(|e| format!("Failed to read font '{path}': {e}"))?;
    if raw.len() < 12 {
        return Err(format!("Font file too small: '{path}'"));
    }
    let data = Data::new_copy(&raw);
    if let Some(tf) = font_mgr.new_from_data(data, 0) {
        return Ok(tf);
    }
    // Fallback for WOFF2 if the active FreeType build did not decode the wrapper directly.
    if raw.starts_with(b"wOF2") {
        let decoded = wuff::decompress_woff2(&raw)
            .map_err(|e| format!("Failed to decompress WOFF2 '{path}': {e:?}"))?;
        let sfnt_data = Data::new_copy(&decoded);
        if let Some(tf) = font_mgr.new_from_data(sfnt_data, 0) {
            return Ok(tf);
        }
    }
    Err(format!("Skia could not decode typeface from '{path}'"))
}

fn render_once(
    font_mgr: &FontMgr,
    cache: &mut LruCache<String, Typeface>,
    req: &RenderRequest,
) -> Result<Vec<u8>, String> {
    let width = req.width.clamp(1, MAX_CANVAS_DIM);
    let height = req.height.clamp(1, MAX_CANVAS_DIM);
    let font_size = req.font_size.clamp(4.0, 512.0);

    let base_typeface = if let Some(cached) = cache.get(&req.font_path) {
        cached.clone()
    } else {
        let loaded = load_typeface_from_path(font_mgr, &req.font_path)?;
        cache.put(req.font_path.clone(), loaded.clone());
        loaded
    };

    // Apply variable font axes via FontArguments (VariationPosition).
    let active_typeface = if !req.variations.is_empty() {
        let mut coords: Vec<Coordinate> = req
            .variations
            .iter()
            .filter_map(|(tag, &value)| {
                tag_from_str(tag).map(|axis| Coordinate { axis, value })
            })
            .collect();
        coords.sort_by_key(|c| (c.axis.a(), c.axis.b(), c.axis.c(), c.axis.d()));
        if !coords.is_empty() {
            let pos = VariationPosition {
                coordinates: &coords,
            };
            let args = FontArguments::new().set_variation_design_position(pos);
            base_typeface
                .clone_with_arguments(&args)
                .unwrap_or_else(|| base_typeface.clone())
        } else {
            base_typeface
        }
    } else {
        base_typeface
    };

    // Register the active typeface in an isolated TypefaceFontProvider so Skia Paragraph + HarfBuzz
    // shapes exclusively with our exact on-disk typeface without touching OS fonts.
    const PREVIEW_FAMILY_ALIAS: &str = "FonthsPreview";
    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(active_typeface.clone(), Some(PREVIEW_FAMILY_ALIAS));

    let mut font_collection = FontCollection::new();
    font_collection.set_asset_font_manager(Some(provider.into()));
    font_collection.set_default_font_manager(font_mgr.clone(), None);

    let mut para_style = ParagraphStyle::new();
    para_style.set_text_align(TextAlign::Left);

    let mut text_style = TextStyle::new();
    text_style.set_color(parse_color_hex(&req.color_hex));
    text_style.set_font_size(font_size);
    text_style.set_font_families(&[PREVIEW_FAMILY_ALIAS]);
    text_style.set_typeface(Some(active_typeface));

    // Apply OpenType features via HarfBuzz inside Skia TextStyle.
    for (tag, &value) in &req.features {
        if tag.len() == 4 {
            text_style.add_font_feature(tag, value);
        }
    }

    let mut builder = ParagraphBuilder::new(&para_style, font_collection);
    builder.push_style(&text_style);
    builder.add_text(&req.text);
    let mut paragraph = builder.build();
    paragraph.layout(width as f32);

    let mut surface = surfaces::raster_n32_premul((width, height))
        .ok_or_else(|| format!("Failed to allocate {width}x{height} Skia raster surface"))?;
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    // Vertically center single-line specimens cleanly while keeping multi-line paragraphs top-aligned.
    let para_h = paragraph.height();
    let y_offset = if para_h > 0.0 && para_h < height as f32 {
        ((height as f32 - para_h) * 0.5).floor().max(0.0)
    } else {
        0.0
    };
    paragraph.paint(canvas, (0.0, y_offset));

    // Read pixels as unpremultiplied RGBA8888 for direct HTML5 Canvas `ImageData` consumption.
    let rgba_info = ImageInfo::new(
        (width, height),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = (width as usize) * 4;
    let mut pixels = vec![0u8; row_bytes * (height as usize)];
    if !surface.read_pixels(&rgba_info, &mut pixels, row_bytes, (0, 0)) {
        return Err("Skia read_pixels failed".to_string());
    }
    Ok(pixels)
}

fn run_render_loop(rx: mpsc::Receiver<RenderRequest>) {
    let font_mgr = FontMgr::custom_empty().unwrap_or_else(FontMgr::new);
    let mut cache: LruCache<String, Typeface> =
        LruCache::new(NonZeroUsize::new(TYPEFACE_CACHE_CAPACITY).unwrap());

    while let Ok(req) = rx.recv() {
        let res = render_once(&font_mgr, &mut cache, &req);
        let _ = req.reply.send(res);
    }
}

/// Async Tauri command rendering a font specimen via Skia (embedded FreeType + HarfBuzz)
/// and returning raw RGBA8888 pixel bytes directly over Tauri IPC (zero PNG overhead).
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
        .render(
            font_path, text, font_size, width, height, color_hex, features, variations,
        )
        .await?;
    Ok(tauri::ipc::Response::new(pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn skia_worker_renders_variable_font_with_features_to_rgba8888() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("Inter-opsz-wght.ttf");
        assert!(fixture.exists(), "Inter fixture must exist");

        let font_mgr = FontMgr::custom_empty().unwrap_or_else(FontMgr::new);
        let mut cache = LruCache::new(NonZeroUsize::new(16).unwrap());

        let mut features = HashMap::new();
        features.insert("liga".to_string(), 1);
        features.insert("smcp".to_string(), 1);

        let mut variations = HashMap::new();
        variations.insert("wght".to_string(), 700.0);

        let (tx, _rx) = oneshot::channel();
        let req = RenderRequest {
            font_path: fixture.to_string_lossy().into_owned(),
            text: "Sphinx of black quartz 0123456789".to_string(),
            font_size: 36.0,
            width: 512,
            height: 72,
            color_hex: "#111111".to_string(),
            features,
            variations,
            reply: tx,
        };

        let rgba = render_once(&font_mgr, &mut cache, &req).expect("Skia render should succeed");
        assert_eq!(rgba.len(), 512 * 72 * 4);
        let non_transparent_pixels = rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            non_transparent_pixels > 100,
            "expected rendered glyphs with non-zero alpha, got {non_transparent_pixels} pixels"
        );
    }
}
