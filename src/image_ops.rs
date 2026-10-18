//! Destructive image operations: rotation, crop, brightness/contrast/saturation
//! adjustments, cut/copy of masked regions.
//!
//! Operations mutate `CanvasState.color_image` (and `canvas_size`) and stash a
//! `DynamicImage` on `state.pending_texture_refresh` so the next frame uploads
//! the new texture. Annotation geometry is rotated in lock-step where relevant.

use egui::{Color32, Pos2, Rect, Vec2};
use image::{DynamicImage, Rgba, RgbaImage};

use crate::state::{AppState, SelKind, Selection};
use crate::types::ShapeData;

// ── Rotation ────────────────────────────────────────────────────────────────

/// Rotate canvas pixels and all annotation geometry by 90°.
pub fn rotate_90(state: &mut AppState, clockwise: bool) {
    let Some(ci) = state.canvas.color_image.clone() else {
        return;
    };
    let (w, h) = (ci.size[0], ci.size[1]);
    if w == 0 || h == 0 {
        return;
    }

    let mut out = vec![Color32::BLACK; w * h];
    for y in 0..h {
        for x in 0..w {
            let src = ci.pixels[y * w + x];
            let (nx, ny) = if clockwise {
                (h - 1 - y, x)
            } else {
                (y, w - 1 - x)
            };
            out[ny * h + nx] = src;
        }
    }
    let new_ci = egui::ColorImage {
        size: [h, w],
        pixels: out,
    };

    // Rotate all annotation geometry.
    let (old_w, old_h) = (w as f32, h as f32);
    let ids: Vec<uuid::Uuid> = state
        .canvas
        .document
        .leaves_in_paint_order()
        .iter()
        .map(|lv| lv.id)
        .collect();
    for id in ids {
        if let Some(leaf) = state.canvas.document.leaf_mut(id) {
            rotate_shape(&mut leaf.shape, clockwise, old_w, old_h);
        }
    }
    state.canvas.sync_annotations();
    // Rotate the selection if any.
    if let Some(sel) = state.canvas.selection {
        state.canvas.selection = Some(Selection {
            rect: rotate_rect(sel.rect, clockwise, old_w, old_h),
            ..sel
        });
    }

    let raw: Vec<u8> = new_ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    state.canvas.color_image = Some(new_ci);
    state.canvas.canvas_size = Vec2::new(h as f32, w as f32);
    state.canvas.clear_all_undo_redo();
    if let Some(buf) = RgbaImage::from_raw(h as u32, w as u32, raw) {
        state.pending_texture_refresh = Some(DynamicImage::ImageRgba8(buf));
    }
    state.status_msg = Some(format!(
        "Rotated 90° {}",
        if clockwise {
            "clockwise"
        } else {
            "counter-clockwise"
        }
    ));
}

fn rotate_pos(p: Pos2, cw: bool, w: f32, h: f32) -> Pos2 {
    if cw {
        Pos2::new(h - p.y, p.x)
    } else {
        Pos2::new(p.y, w - p.x)
    }
}

fn rotate_rect(r: Rect, cw: bool, w: f32, h: f32) -> Rect {
    Rect::from_two_pos(rotate_pos(r.min, cw, w, h), rotate_pos(r.max, cw, w, h))
}

fn rotate_shape(shape: &mut ShapeData, cw: bool, w: f32, h: f32) {
    let rp = |p: Pos2| rotate_pos(p, cw, w, h);
    let rr = |r: Rect| rotate_rect(r, cw, w, h);
    match shape {
        ShapeData::Rect { rect, .. }
        | ShapeData::Oval { rect, .. }
        | ShapeData::Star { rect }
        | ShapeData::FocusRegion { rect }
        | ShapeData::Blur { rect, .. }
        | ShapeData::ImageLayer { rect, .. } => {
            *rect = rr(*rect);
        }
        ShapeData::Arrow { start, end, .. }
        | ShapeData::Line { start, end, .. }
        | ShapeData::Ruler { start, end } => {
            *start = rp(*start);
            *end = rp(*end);
        }
        ShapeData::Pen {
            raw_points,
            smoothed_points,
        } => {
            for q in raw_points.iter_mut().chain(smoothed_points.iter_mut()) {
                *q = rp(*q);
            }
        }
        ShapeData::LogicTag { pos, .. } => {
            *pos = rp(*pos);
        }
        ShapeData::Text { pos, .. } => {
            *pos = rp(*pos);
        }
        ShapeData::Question { pos, .. } => {
            *pos = rp(*pos);
        }
        ShapeData::StepMarker { pos, .. } => {
            *pos = rp(*pos);
        }
        ShapeData::Callout { rect, tail } => {
            *rect = rr(*rect);
            *tail = rp(*tail);
        }
        ShapeData::BezierPath { nodes, .. } => {
            for n in nodes.iter_mut() {
                n.pos = rp(n.pos);
            }
        }
    }
}

// ── Crop to selection ───────────────────────────────────────────────────────

/// Crop the canvas to the selection's bounding rect. Annotations are cleared
/// (matches existing crop behavior). Ellipse selections still crop to rect.
pub fn crop_to_selection(state: &mut AppState) {
    let Some(sel) = state.canvas.selection else {
        state.status_msg = Some("No selection to crop to".into());
        return;
    };
    let Some(ci) = state.canvas.color_image.clone() else {
        return;
    };
    let (iw, ih) = (ci.size[0], ci.size[1]);
    let x0 = (sel.rect.min.x.floor() as i32).max(0) as usize;
    let y0 = (sel.rect.min.y.floor() as i32).max(0) as usize;
    let x1 = (sel.rect.max.x.ceil() as i32).max(0) as usize;
    let y1 = (sel.rect.max.y.ceil() as i32).max(0) as usize;
    let x0 = x0.min(iw.saturating_sub(1));
    let y0 = y0.min(ih.saturating_sub(1));
    let x1 = x1.min(iw);
    let y1 = y1.min(ih);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        state.status_msg = Some("Selection too small to crop".into());
        return;
    }
    let (new_w, new_h) = (x1 - x0, y1 - y0);
    let mut new_pixels = Vec::with_capacity(new_w * new_h);
    for y in y0..y1 {
        let row_start = y * iw + x0;
        new_pixels.extend_from_slice(&ci.pixels[row_start..row_start + new_w]);
    }
    let new_image = egui::ColorImage {
        size: [new_w, new_h],
        pixels: new_pixels,
    };
    let raw: Vec<u8> = new_image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    state.canvas.color_image = Some(new_image);
    state.canvas.canvas_size = Vec2::new(new_w as f32, new_h as f32);
    state.canvas.document = crate::scene::Document::from_flat(Vec::new());
    state.canvas.sync_annotations();
    state.canvas.clear_all_undo_redo();
    state.canvas.selection = None;
    if let Some(buf) = RgbaImage::from_raw(new_w as u32, new_h as u32, raw) {
        state.pending_texture_refresh = Some(DynamicImage::ImageRgba8(buf));
    }
    state.status_msg = Some(format!("Cropped to selection {new_w}×{new_h}"));
}

// ── Brightness / Contrast / Saturation ──────────────────────────────────────

/// Apply BCS adjustments to `pixels` (size w*h) IN PLACE within the mask.
/// All values in -1.0..1.0: 0 = no change.
pub fn adjust_pixels(
    pixels: &mut [Color32],
    size: [usize; 2],
    brightness: f32,
    contrast: f32,
    saturation: f32,
    selection: Option<Selection>,
) {
    let (w, h) = (size[0], size[1]);
    let b = brightness.clamp(-1.0, 1.0);
    // Standard contrast factor: -1 → flatten to gray, +1 → strong contrast.
    let c = (contrast.clamp(-1.0, 1.0) + 1.0).max(0.0);
    let s = saturation.clamp(-1.0, 1.0) + 1.0;

    let adjust_channel = |v: u8| -> u8 {
        let mut f = v as f32 / 255.0;
        f = (f - 0.5) * c + 0.5; // contrast pivots around 0.5
        f += b; // brightness
        (f * 255.0).clamp(0.0, 255.0) as u8
    };

    for y in 0..h {
        for x in 0..w {
            if let Some(sel) = selection {
                if !sel.contains(x as f32 + 0.5, y as f32 + 0.5) {
                    continue;
                }
            }
            let idx = y * w + x;
            let p = pixels[idx];
            let r = adjust_channel(p.r()) as f32;
            let g = adjust_channel(p.g()) as f32;
            let b2 = adjust_channel(p.b()) as f32;
            // Saturation: mix with luminance.
            let lum = 0.299 * r + 0.587 * g + 0.114 * b2;
            let rs = (lum + (r - lum) * s).clamp(0.0, 255.0) as u8;
            let gs = (lum + (g - lum) * s).clamp(0.0, 255.0) as u8;
            let bs = (lum + (b2 - lum) * s).clamp(0.0, 255.0) as u8;
            pixels[idx] = Color32::from_rgba_unmultiplied(rs, gs, bs, p.a());
        }
    }
}

/// Commit adjustments: swap in the preview pixels, upload a fresh texture,
/// close the dialog. No undo (clears stack — consistent with crop/rotate).
pub fn commit_adjust(state: &mut AppState) {
    state.canvas.adjust_preview = None;
    // color_image is already the preview — just re-upload via pending_texture_refresh.
    refresh_texture_from_color_image(state);
    state.canvas.clear_all_undo_redo();
    state.status_msg = Some("Adjustments applied".into());
}

/// Cancel adjustments: restore the original pixels from the preview snapshot.
pub fn cancel_adjust(state: &mut AppState) {
    if let Some(prev) = state.canvas.adjust_preview.take() {
        if let Some(ci) = state.canvas.color_image.as_mut() {
            if ci.size == prev.original_size && ci.pixels.len() == prev.original_pixels.len() {
                ci.pixels = prev.original_pixels;
                refresh_texture_from_color_image(state);
            }
        }
        state.status_msg = Some("Adjustments cancelled".into());
    }
}

/// Re-run the BCS transform on the preview's original pixels and write them
/// into `canvas.color_image`. Called every frame the dialog is open so sliders
/// update live.
pub fn rerun_adjust_preview(state: &mut AppState) {
    let Some(ap) = &state.canvas.adjust_preview else {
        return;
    };
    let Some(ci) = state.canvas.color_image.as_mut() else {
        return;
    };
    if ci.size != ap.original_size {
        return;
    }
    ci.pixels.copy_from_slice(&ap.original_pixels);
    adjust_pixels(
        &mut ci.pixels,
        ap.original_size,
        ap.brightness,
        ap.contrast,
        ap.saturation,
        ap.selection,
    );
    refresh_texture_from_color_image(state);
}

fn refresh_texture_from_color_image(state: &mut AppState) {
    let Some(ci) = state.canvas.color_image.as_ref() else {
        return;
    };
    let (w, h) = (ci.size[0] as u32, ci.size[1] as u32);
    let raw: Vec<u8> = ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    if let Some(buf) = RgbaImage::from_raw(w, h, raw) {
        state.pending_texture_refresh = Some(DynamicImage::ImageRgba8(buf));
    }
}

// ── New blank canvas / resize canvas ─────────────────────────────────────────

/// Create a new white blank canvas of the given size and open it as a new tab.
/// Create a new blank canvas filled with `fill` (RGBA). A fully-transparent
/// fill (`[_, _, _, 0]`) yields a transparent canvas.
pub fn new_blank_canvas_with_fill(
    width: u32,
    height: u32,
    fill: [u8; 4],
    state: &mut AppState,
    ctx: &egui::Context,
) {
    if width == 0 || height == 0 {
        return;
    }
    state.push_current_as_tab_and_reset(format!("Canvas {}×{}", width, height));
    crate::loader::apply_image(
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, Rgba(fill))),
        state,
        ctx,
    );
    state.canvas.selection = None;
    state.canvas.adjust_preview = None;
    state.canvas.marquee_state = crate::state::MarqueeState::Idle;
    state.tool_state = crate::state::ToolState::Idle;
    state.canvas.pdf_page = 0;
    state.canvas.pdf_pages = 0;
    state.status_msg = Some(format!(
        "New {width}×{height} canvas created — paste with Ctrl+V"
    ));
}

/// Resize the canvas (crop or white-pad) without clearing annotations.
/// Annotations remain in image-pixel space; those outside the new bounds are
/// simply clipped during rendering/export.
pub fn resize_canvas(new_w: u32, new_h: u32, state: &mut AppState) {
    if new_w == 0 || new_h == 0 {
        return;
    }
    let Some(ci) = state.canvas.color_image.as_ref() else {
        return;
    };
    let (old_w, old_h) = (ci.size[0] as u32, ci.size[1] as u32);
    if old_w == new_w && old_h == new_h {
        return;
    }

    let copy_w = old_w.min(new_w) as usize;
    let copy_h = old_h.min(new_h) as usize;
    let mut new_pixels = vec![Color32::WHITE; (new_w * new_h) as usize];
    for y in 0..copy_h {
        for x in 0..copy_w {
            new_pixels[y * new_w as usize + x] = ci.pixels[y * old_w as usize + x];
        }
    }

    let new_ci = egui::ColorImage {
        size: [new_w as usize, new_h as usize],
        pixels: new_pixels,
    };
    let raw: Vec<u8> = new_ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    state.canvas.color_image = Some(new_ci);
    state.canvas.canvas_size = Vec2::new(new_w as f32, new_h as f32);
    // Annotations are intentionally kept; undo history is cleared (destructive op).
    state.canvas.clear_all_undo_redo();
    if let Some(buf) = RgbaImage::from_raw(new_w, new_h, raw) {
        state.pending_texture_refresh = Some(DynamicImage::ImageRgba8(buf));
    }
    state.status_msg = Some(format!("Canvas resized to {new_w}×{new_h}"));
}

// ── Cut / Copy of masked selection ───────────────────────────────────────────

/// Extract the masked pixels within the selection's bounding rect into a new
/// RgbaImage (masked-out pixels become transparent). None if no selection /
/// selection is empty.
pub fn copy_selection_pixels(state: &AppState) -> Option<RgbaImage> {
    let sel = state.canvas.selection?;
    let ci = state.canvas.color_image.as_ref()?;
    let (iw, ih) = (ci.size[0], ci.size[1]);
    let x0 = (sel.rect.min.x.floor() as i32).max(0) as usize;
    let y0 = (sel.rect.min.y.floor() as i32).max(0) as usize;
    let x1 = (sel.rect.max.x.ceil() as i32).max(0) as usize;
    let y1 = (sel.rect.max.y.ceil() as i32).max(0) as usize;
    let x0 = x0.min(iw);
    let y0 = y0.min(ih);
    let x1 = x1.min(iw);
    let y1 = y1.min(ih);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = RgbaImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let ix = x0 + x;
            let iy = y0 + y;
            let fx = ix as f32 + 0.5;
            let fy = iy as f32 + 0.5;
            let keep = if sel.kind == SelKind::Rect && !sel.inverted {
                true
            } else {
                sel.contains(fx, fy)
            };
            if !keep {
                out.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 0]));
                continue;
            }
            let p = ci.pixels[iy * iw + ix];
            out.put_pixel(x as u32, y as u32, Rgba([p.r(), p.g(), p.b(), p.a()]));
        }
    }
    Some(out)
}

/// Clear masked pixels (fill transparent) and return the extracted bitmap.
pub fn cut_selection(state: &mut AppState) -> Option<RgbaImage> {
    let copied = copy_selection_pixels(state)?;
    let sel = state.canvas.selection?;
    let ci = state.canvas.color_image.as_mut()?;
    let (iw, ih) = (ci.size[0], ci.size[1]);
    let x0 = (sel.rect.min.x.floor() as i32).max(0) as usize;
    let y0 = (sel.rect.min.y.floor() as i32).max(0) as usize;
    let x1 = (sel.rect.max.x.ceil() as i32).max(0) as usize;
    let y1 = (sel.rect.max.y.ceil() as i32).max(0) as usize;
    let x0 = x0.min(iw);
    let y0 = y0.min(ih);
    let x1 = x1.min(iw);
    let y1 = y1.min(ih);
    for y in y0..y1 {
        for x in x0..x1 {
            if sel.contains(x as f32 + 0.5, y as f32 + 0.5) {
                ci.pixels[y * iw + x] = Color32::from_rgba_unmultiplied(0, 0, 0, 0);
            }
        }
    }
    refresh_texture_from_color_image(state);
    state.canvas.clear_all_undo_redo();
    Some(copied)
}

#[cfg(test)]
mod tests {
    use super::new_blank_canvas_with_fill;
    use crate::state::{AppState, MarqueeState, Selection, ToolState};
    use egui::{pos2, vec2, Color32, Context, Rect};

    #[test]
    fn new_blank_canvas_resets_state_and_loads_drawable_image() {
        let ctx = Context::default();
        let mut state = AppState::default();
        state.canvas.selection = Some(Selection {
            kind: crate::state::SelKind::Rect,
            rect: Rect::from_two_pos(pos2(1.0, 1.0), pos2(10.0, 10.0)),
            inverted: false,
        });
        state.canvas.marquee_state = MarqueeState::Drawing {
            start: pos2(4.0, 4.0),
        };
        state.tool_state = ToolState::Drawing {
            start: pos2(2.0, 2.0),
            current: pos2(8.0, 8.0),
        };
        state.canvas.canvas_size = vec2(50.0, 50.0);
        state.canvas.adjust_preview = Some(crate::state::AdjustPreview {
            brightness: 0.1,
            contrast: 0.2,
            saturation: 0.3,
            original_pixels: vec![Color32::WHITE],
            original_size: [1, 1],
            selection: None,
        });

        new_blank_canvas_with_fill(640, 480, [255, 255, 255, 255], &mut state, &ctx);

        assert_eq!(state.canvas.canvas_size, vec2(640.0, 480.0));
        assert!(state.canvas.texture.is_some());
        assert!(state.canvas.color_image.is_some());
        assert!(state.canvas.selection.is_none());
        assert!(state.canvas.adjust_preview.is_none());
        assert!(matches!(state.canvas.marquee_state, MarqueeState::Idle));
        assert!(matches!(state.tool_state, ToolState::Idle));
        assert_eq!(state.canvas.pdf_page, 0);
        assert_eq!(state.canvas.pdf_pages, 0);
    }
}
