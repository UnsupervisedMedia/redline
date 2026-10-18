//! Off-main-thread PNG snapshot encoding.

use base64::Engine;
use std::sync::Arc;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::agent::types::AgentError;

/// Spawn an encoding job on the rayon thread pool using the final clean shape.
/// Accepts the decoded canvas state, encodes to PNG off the main thread,
/// and fires the reply oneshot with `Result<Vec<u8>, AgentError>`.
pub fn spawn_with_reply(
    _room_id: Uuid,
    texture: Option<Arc<egui::ColorImage>>,
    annotations: Vec<crate::types::Annotation>,
    canvas_size: egui::Vec2,
    scale: f32,
    reply: oneshot::Sender<Result<Vec<u8>, AgentError>>,
) {
    rayon::spawn(move || {
        let result = render_png_from(texture, annotations, canvas_size, scale)
            .map_err(|e| AgentError::Internal { reason: e });
        let _ = reply.send(result);
    });
}

/// Encode canvas pixels to PNG bytes, compositing all annotations via `flatten`.
/// When there is no source texture (or flatten returns None), falls back to a
/// white RgbaImage at canvas_size so the tool always returns a valid PNG.
/// The output image is resampled to `canvas_size * scale` using Lanczos3.
fn render_png_from(
    texture: Option<Arc<egui::ColorImage>>,
    annotations: Vec<crate::types::Annotation>,
    canvas_size: egui::Vec2,
    scale: f32,
) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    // Clamp scale to a safe range so a bad agent input can't ask for a gigapixel.
    let scale = scale.clamp(0.1, 4.0);
    let scaled_size = canvas_size * scale;
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, scaled_size);

    let mut rgba =
        match crate::flatten::flatten(&annotations, texture.as_deref(), canvas_size, screen_rect) {
            Some(img) => img,
            None => {
                // No source image (or nothing to render onto) → fall back to a
                // white image so the tool returns *something* the agent can consume.
                let (w, h) = (canvas_size.x.max(1.0) as u32, canvas_size.y.max(1.0) as u32);
                image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
            }
        };

    // If scale != 1.0, resample to the requested output dimensions.
    if (scale - 1.0).abs() > f32::EPSILON {
        let target_w = (scaled_size.x.max(1.0) as u32).max(1);
        let target_h = (scaled_size.y.max(1.0) as u32).max(1);
        rgba = image::imageops::resize(
            &rgba,
            target_w,
            target_h,
            image::imageops::FilterType::Lanczos3,
        );
    }

    let (w, h) = (rgba.width(), rgba.height());
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out)
}

/// Encode a PNG byte vector as a base64 string suitable for MCP image content.
pub fn to_mcp_image_content(bytes: Vec<u8>) -> String {
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}
