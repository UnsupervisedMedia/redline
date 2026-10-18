//! Off-thread glyph rasterizer used by `flatten` when compositing Text and
//! LogicTag annotations into snapshot images. Single weight, single font.
//!
//! On font-load failure, callers should detect via `font().is_none()` and
//! fall back to a placeholder bounding box.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use egui::{Color32, Pos2};
use image::{Rgba, RgbaImage};
use std::sync::OnceLock;

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

/// Lazily load the bundled font. Returns `None` if the TTF bytes are
/// somehow invalid (should not happen in a built binary).
pub fn font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        let bytes = include_bytes!("../assets/fonts/NotoSans-Regular.ttf").to_vec();
        match FontVec::try_from_vec(bytes) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("[text_render] failed to load bundled font: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// Rasterize `content` into `img` starting at `pos` (top-left of text bbox).
/// Multi-line via `\n`. Per-glyph alpha is composited onto `img` using
/// straight-alpha source-over. No-op when the bundled font failed to load.
pub fn rasterize_text(img: &mut RgbaImage, pos: Pos2, content: &str, size_px: f32, color: Color32) {
    let Some(font_ref) = font() else {
        return;
    };
    let scale = PxScale::from(size_px);
    let scaled = font_ref.as_scaled(scale);
    let line_height = scaled.height() + scaled.line_gap();

    let img_w = img.width() as i32;
    let img_h = img.height() as i32;

    let mut cursor_y = pos.y + scaled.ascent();
    for line in content.split('\n') {
        let mut cursor_x = pos.x;
        for ch in line.chars() {
            let glyph_id = font_ref.glyph_id(ch);
            let glyph =
                glyph_id.with_scale_and_position(scale, ab_glyph::point(cursor_x, cursor_y));
            let h_advance = scaled.h_advance(glyph_id);
            if let Some(outlined) = font_ref.outline_glyph(glyph) {
                let bb = outlined.px_bounds();
                if !bb.min.x.is_finite() || !bb.min.y.is_finite() {
                    cursor_x += h_advance;
                    continue;
                }
                outlined.draw(|gx, gy, alpha| {
                    let px = bb.min.x as i32 + gx as i32;
                    let py = bb.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= img_w || py >= img_h {
                        return;
                    }
                    if alpha <= 0.0 {
                        return;
                    }
                    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
                    let src = Rgba([color.r(), color.g(), color.b(), a]);
                    let dst = img.get_pixel_mut(px as u32, py as u32);
                    *dst = blend_over(src, *dst);
                });
            }
            cursor_x += h_advance;
        }
        cursor_y += line_height;
    }
}

/// Source-over composite: `src` painted on top of `dst`, straight alpha.
fn blend_over(src: Rgba<u8>, dst: Rgba<u8>) -> Rgba<u8> {
    let sa = src.0[3] as f32 / 255.0;
    let inv = 1.0 - sa;
    let mix = |a: u8, b: u8| ((a as f32 * sa) + (b as f32 * inv)).round() as u8;
    let out_a = ((src.0[3] as f32) + (dst.0[3] as f32 * inv))
        .round()
        .clamp(0.0, 255.0) as u8;
    Rgba([
        mix(src.0[0], dst.0[0]),
        mix(src.0[1], dst.0[1]),
        mix(src.0[2], dst.0[2]),
        out_a,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    #[test]
    fn rasterize_writes_non_background_pixels() {
        if font().is_none() {
            eprintln!("[text_render::tests] skipping: bundled font failed to load");
            return;
        }
        let mut img = RgbaImage::from_pixel(200, 80, Rgba([255, 255, 255, 255]));
        rasterize_text(&mut img, Pos2::new(10.0, 10.0), "Hi", 32.0, Color32::BLACK);
        // Sample inside the text bbox — expect at least one non-white pixel.
        let mut found_dark = false;
        for y in 10..70 {
            for x in 10..80 {
                let p = img.get_pixel(x, y);
                if p.0[0] < 250 || p.0[1] < 250 || p.0[2] < 250 {
                    found_dark = true;
                    break;
                }
            }
            if found_dark {
                break;
            }
        }
        assert!(found_dark, "expected glyph pixels in text bounding box");
    }

    #[test]
    fn rasterize_handles_newlines() {
        if font().is_none() {
            eprintln!("[text_render::tests] skipping: bundled font failed to load");
            return;
        }
        let mut img = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        rasterize_text(&mut img, Pos2::new(0.0, 0.0), "A\nB", 32.0, Color32::BLACK);
        let mut dark_above = 0u32;
        let mut dark_below = 0u32;
        for y in 0..200 {
            for x in 0..200 {
                let p = img.get_pixel(x, y);
                if p.0[0] < 200 {
                    if y < 40 {
                        dark_above += 1;
                    } else {
                        dark_below += 1;
                    }
                }
            }
        }
        assert!(
            dark_above > 0 && dark_below > 0,
            "expected glyphs on both lines (above={dark_above}, below={dark_below})"
        );
    }
}
