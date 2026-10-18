// These pixel helpers are consumed by the feature-gated inference backends
// (`lama_ort` / `sd_candle`) and the unit tests, so they read as dead in a
// default, non-test build. Keep the lint active under those features.
#![cfg_attr(
    not(any(feature = "native-inpaint", feature = "native-generative")),
    allow(dead_code)
)]

use image::{GenericImageView, GrayImage, Rgba, RgbaImage};

/// Inclusive bounding box `(x0, y0, x1, y1)` of the set (non-zero) mask pixels,
/// or `None` if the mask is empty.
pub fn mask_bbox(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = mask.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    let mut found = false;
    for y in 0..h {
        for x in 0..w {
            if mask.get_pixel(x, y).0[0] > 0 {
                found = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    found.then_some((x0, y0, x1, y1))
}

/// Expand an inclusive bbox by `margin` px on every side, then grow the shorter
/// axis so the region is square (LaMa infers on a square), all clamped to the
/// image. Returns `(x, y, w, h)` with `w, h >= 1`. Squaring avoids the aspect
/// distortion you'd get resizing a thin sliver to the model's square input.
pub fn padded_square_region(
    bbox: (u32, u32, u32, u32),
    img_w: u32,
    img_h: u32,
    margin: u32,
) -> (u32, u32, u32, u32) {
    let (bx0, by0, bx1, by1) = bbox;
    let x0 = bx0.saturating_sub(margin);
    let y0 = by0.saturating_sub(margin);
    let x1 = (bx1 + margin).min(img_w.saturating_sub(1));
    let y1 = (by1 + margin).min(img_h.saturating_sub(1));
    let mut w = x1 - x0 + 1;
    let mut h = y1 - y0 + 1;

    // Grow the shorter side toward a square, centred, clamped to the image.
    let side = w.max(h);
    let side = side.min(img_w).min(img_h);
    let cx = (x0 + x1) / 2;
    let cy = (y0 + y1) / 2;
    let half = side / 2;
    let mut nx0 = cx.saturating_sub(half);
    let mut ny0 = cy.saturating_sub(half);
    if nx0 + side > img_w {
        nx0 = img_w - side;
    }
    if ny0 + side > img_h {
        ny0 = img_h - side;
    }
    w = side;
    h = side;
    (nx0, ny0, w.max(1), h.max(1))
}

/// Pad bottom/right with edge-replicated pixels so dims are a multiple of `m`.
/// Returns the padded image and the original `(w, h)` for later un-padding.
#[allow(dead_code)] // used by dynamic-size/tiling path (not yet wired; LaMa is fixed 512×512)
pub fn pad_to_multiple(img: &RgbaImage, m: u32) -> (RgbaImage, (u32, u32)) {
    let (w, h) = img.dimensions();
    let pw = w.div_ceil(m) * m;
    let ph = h.div_ceil(m) * m;
    let mut out = RgbaImage::new(pw, ph);
    for y in 0..ph {
        let sy = y.min(h.saturating_sub(1));
        for x in 0..pw {
            let sx = x.min(w.saturating_sub(1));
            out.put_pixel(x, y, *img.get_pixel(sx, sy));
        }
    }
    (out, (w, h))
}

/// Crop back to the original dimensions.
#[allow(dead_code)] // pairs with `pad_to_multiple` for the not-yet-wired tiling path
pub fn unpad(img: &RgbaImage, orig: (u32, u32)) -> RgbaImage {
    img.view(0, 0, orig.0, orig.1).to_image()
}

/// Decide the (width, height) to feed the model.
/// `model_hw`: the model's required (height, width) if fixed (both > 0), or None if dynamic.
/// For dynamic models, round the source up to a multiple of 8.
/// For fixed models, use the model's size exactly.
pub fn target_infer_size(model_hw: Option<(u32, u32)>, src_w: u32, src_h: u32) -> (u32, u32) {
    match model_hw {
        Some((mh, mw)) if mh > 0 && mw > 0 => (mw, mh),
        _ => (src_w.div_ceil(8) * 8, src_h.div_ceil(8) * 8),
    }
}

/// Per-pixel blend: `out = lerp(src, infer, mask/255)`. Alpha kept from src.
pub fn composite_masked(src: &RgbaImage, infer: &RgbaImage, mask: &GrayImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let mut out = src.clone();
    for y in 0..h {
        for x in 0..w {
            let m = mask.get_pixel(x, y).0[0] as u32;
            if m == 0 {
                continue;
            }
            let s = src.get_pixel(x, y).0;
            let i = infer.get_pixel(x, y).0;
            let blend = |a: u8, b: u8| ((a as u32 * (255 - m) + b as u32 * m) / 255) as u8;
            out.put_pixel(
                x,
                y,
                Rgba([
                    blend(s[0], i[0]),
                    blend(s[1], i[1]),
                    blend(s[2], i[2]),
                    s[3],
                ]),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgba, RgbaImage};

    #[test]
    fn pad_rounds_up_to_multiple_of_8() {
        let img = RgbaImage::new(13, 17);
        let (padded, orig) = pad_to_multiple(&img, 8);
        assert_eq!(padded.width() % 8, 0);
        assert_eq!(padded.height() % 8, 0);
        assert_eq!(orig, (13, 17));
        assert!(padded.width() >= 13 && padded.height() >= 17);
    }

    #[test]
    fn unpad_is_inverse_of_pad() {
        let mut img = RgbaImage::new(10, 6);
        img.put_pixel(3, 2, Rgba([1, 2, 3, 4]));
        let (padded, orig) = pad_to_multiple(&img, 8);
        let back = unpad(&padded, orig);
        assert_eq!(back.dimensions(), (10, 6));
        assert_eq!(back.get_pixel(3, 2), &Rgba([1, 2, 3, 4]));
    }

    #[test]
    fn composite_only_touches_masked_pixels() {
        let src = RgbaImage::from_pixel(4, 4, Rgba([10, 10, 10, 255]));
        let infer = RgbaImage::from_pixel(4, 4, Rgba([200, 0, 0, 255]));
        let mut mask = GrayImage::from_pixel(4, 4, Luma([0]));
        mask.put_pixel(1, 1, Luma([255]));
        let out = composite_masked(&src, &infer, &mask);
        assert_eq!(out.get_pixel(1, 1), &Rgba([200, 0, 0, 255]));
        assert_eq!(out.get_pixel(0, 0), &Rgba([10, 10, 10, 255]));
    }

    #[test]
    fn target_size_fixed_model_uses_model_dims() {
        assert_eq!(target_infer_size(Some((512, 512)), 1000, 700), (512, 512));
    }

    #[test]
    fn target_size_dynamic_model_rounds_to_8() {
        assert_eq!(target_infer_size(None, 13, 17), (16, 24));
        assert_eq!(target_infer_size(Some((0, 0)), 13, 17), (16, 24)); // 0 dims treated as dynamic
    }

    #[test]
    fn mask_bbox_finds_set_region() {
        let mut m = GrayImage::from_pixel(20, 20, Luma([0]));
        m.put_pixel(5, 6, Luma([255]));
        m.put_pixel(9, 12, Luma([255]));
        assert_eq!(mask_bbox(&m), Some((5, 6, 9, 12)));
    }

    #[test]
    fn mask_bbox_none_when_empty() {
        let m = GrayImage::from_pixel(8, 8, Luma([0]));
        assert_eq!(mask_bbox(&m), None);
    }

    #[test]
    fn padded_region_is_square_and_clamped() {
        // bbox in the middle of a big image → square with margin, inside bounds.
        let (x, y, w, h) = padded_square_region((40, 50, 60, 56), 200, 200, 8);
        assert_eq!(w, h, "region must be square");
        assert!(x + w <= 200 && y + h <= 200, "region stays in bounds");
        // Covers the original bbox.
        assert!(x <= 40 && y <= 50 && x + w - 1 >= 60 && y + h - 1 >= 56);
    }

    #[test]
    fn padded_region_clamps_at_edges() {
        // bbox hugging the top-left corner.
        let (x, y, w, h) = padded_square_region((0, 0, 3, 3), 100, 100, 10);
        assert_eq!((x, y), (0, 0));
        assert!(w <= 100 && h <= 100);
    }

    #[test]
    fn padded_region_never_exceeds_image_on_small_canvas() {
        let (x, y, w, h) = padded_square_region((1, 1, 2, 2), 5, 5, 100);
        assert!(x + w <= 5 && y + h <= 5, "got {x},{y},{w},{h}");
    }

    #[test]
    fn composite_blends_partial_mask() {
        let src = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
        let infer = RgbaImage::from_pixel(1, 1, Rgba([100, 100, 100, 255]));
        let mask = GrayImage::from_pixel(1, 1, Luma([128]));
        let out = composite_masked(&src, &infer, &mask);
        let p = out.get_pixel(0, 0).0;
        assert!((48..=52).contains(&p[0]), "expected ~50, got {}", p[0]);
    }
}
