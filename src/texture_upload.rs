//! Safe texture uploads.
//!
//! egui panics outright when handed a texture larger than the backend's
//! `max_texture_side` ("Texture "canvas" has size 3037x1820, but the maximum
//! texture side is 2048"), which is reachable with an ordinary screenshot on a
//! software/limited GL context — including on session restore, before the user
//! can do anything about it. Every upload in the app goes through here, which
//! downscales for *display only*: `CanvasState::color_image` keeps the
//! full-resolution source pixels, so exports, flatten and pixel-space tools are
//! unaffected. Annotations live in canvas-size coordinates and the texture is
//! drawn UV 0..1 into the image rect, so a downscaled texture needs no
//! coordinate fixups either.

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

/// Scale factor that fits `size` within `max` per side. `1.0` when it already
/// fits (the overwhelmingly common case — no work, no copy).
pub fn fit_scale(size: [usize; 2], max: usize) -> f32 {
    let max = max.max(1);
    let longest = size[0].max(size[1]);
    if longest <= max || longest == 0 {
        1.0
    } else {
        max as f32 / longest as f32
    }
}

/// Downscale `ci` so neither side exceeds `max`. Returns the input untouched
/// when it already fits.
pub fn fit_for_upload(ci: ColorImage, max: usize) -> ColorImage {
    let scale = fit_scale(ci.size, max);
    if scale >= 1.0 {
        return ci;
    }
    let (w, h) = (ci.size[0], ci.size[1]);
    let (nw, nh) = (
        ((w as f32 * scale).round() as usize).max(1),
        ((h as f32 * scale).round() as usize).max(1),
    );
    let raw: Vec<u8> = ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, raw) else {
        return ci;
    };
    let resized = image::imageops::resize(
        &buf,
        nw as u32,
        nh as u32,
        image::imageops::FilterType::Triangle,
    );
    ColorImage::from_rgba_unmultiplied([nw, nh], resized.as_raw())
}

/// `Context::load_texture`, clamped to what the backend can actually hold.
pub fn load_texture_fitted(
    ctx: &Context,
    name: impl Into<String>,
    ci: ColorImage,
    options: TextureOptions,
) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side);
    let original = ci.size;
    let fitted = fit_for_upload(ci, max);
    if fitted.size != original {
        eprintln!(
            "texture: {}x{} exceeds max texture side {max}; displaying at {}x{} \
             (source pixels kept at full resolution)",
            original[0], original[1], fitted.size[0], fitted.size[1]
        );
    }
    ctx.load_texture(name, fitted, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    fn img(w: usize, h: usize) -> ColorImage {
        ColorImage::new([w, h], Color32::from_rgb(10, 20, 30))
    }

    #[test]
    fn small_images_are_untouched() {
        assert_eq!(fit_scale([1024, 768], 2048), 1.0);
        let out = fit_for_upload(img(1024, 768), 2048);
        assert_eq!(out.size, [1024, 768]);
    }

    #[test]
    fn oversize_is_clamped_to_max_side_preserving_aspect() {
        // The exact shape of the reported crash: 3037x1820 on a 2048 backend.
        let out = fit_for_upload(img(3037, 1820), 2048);
        assert_eq!(out.size[0], 2048);
        assert_eq!(out.size[1], 1227); // 1820 * (2048/3037), rounded
        assert!(out.size.iter().all(|&s| s <= 2048));
        assert_eq!(out.pixels.len(), out.size[0] * out.size[1]);
    }

    #[test]
    fn tall_images_clamp_on_the_long_side() {
        let out = fit_for_upload(img(400, 9000), 2048);
        assert_eq!(out.size[1], 2048);
        assert!(out.size[0] >= 1 && out.size[0] <= 2048);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert_eq!(fit_scale([0, 0], 2048), 1.0);
        let out = fit_for_upload(img(1, 5000), 2048);
        assert!(out.size[0] >= 1 && out.size[1] <= 2048);
    }
}
