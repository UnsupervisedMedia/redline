//! CPU rasterizer for burning annotations into a copy of the underlying
//! image at image-pixel resolution. Operates on primitive inputs so it can
//! run off the main thread (e.g. inside a rayon worker for agent snapshots).
//! Supports rect/oval/star outlines, line, arrow, pen, blur/pixelate, and
//! when the `agent-server` feature is on, Text via the ab_glyph rasterizer
//! in `crate::text_render`. LogicTag is rendered as a chip-bbox outline.
//! FocusRegion is intentionally skipped (UI affordance only).

use egui::{Color32, Pos2, Rect, Vec2};
use image::{Rgba, RgbaImage};

use crate::types::{star_points, Annotation, ArrowHeads, LineStyle, ShapeData};

/// Build an RgbaImage by compositing `annotations` onto a copy of
/// `color_image`. `screen_rect` is the on-screen image draw rect used for
/// screen-pixel→image-pixel thickness conversion. Returns None only when
/// there's neither a source image nor a valid canvas_size (truly nothing to
/// render onto).
pub fn flatten(
    annotations: &[Annotation],
    color_image: Option<&egui::ColorImage>,
    canvas_size: Vec2,
    screen_rect: Rect,
) -> Option<RgbaImage> {
    let mut out = match color_image {
        Some(ci) => {
            let (w, h) = (ci.size[0] as u32, ci.size[1] as u32);
            if w == 0 || h == 0 {
                return None;
            }
            let raw: Vec<u8> = ci
                .pixels
                .iter()
                .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
                .collect();
            RgbaImage::from_raw(w, h, raw)?
        }
        None => {
            // No source image — render annotations onto a white canvas of
            // canvas_size dimensions.
            let w = canvas_size.x.max(0.0) as u32;
            let h = canvas_size.y.max(0.0) as u32;
            if w == 0 || h == 0 {
                return None;
            }
            RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
        }
    };

    // Annotations are stored in image-pixel space, so no position conversion needed.
    // Thickness is in screen-pixels at fit-scale; convert to image pixels.
    let scale = if screen_rect.width() > 0.0 {
        screen_rect.width() / canvas_size.x
    } else {
        1.0
    };
    if scale <= 0.0 {
        return Some(out);
    }
    let inv = 1.0 / scale;
    let to_img_pos = |p: Pos2| p;
    let to_img_rect = |r: Rect| r;
    let img_thickness = |t: f32| (t * inv).max(1.0);

    for ann in annotations {
        let c = ann.color;
        let t = img_thickness(ann.thickness);
        match &ann.shape {
            ShapeData::Rect {
                rect,
                radius: _,
                fill,
                stroke: has_stroke,
                stroke_style,
            } => {
                // Radius is ignored here, same as the old RoundedRect rasterization
                // ("no native rounded rect in imageproc") — not a new regression.
                let img_rect = to_img_rect(*rect);
                if let Some(fill_color) = fill {
                    fill_rect(&mut out, img_rect, *fill_color);
                }
                if *has_stroke {
                    stroke_rect(&mut out, img_rect, t, c, *stroke_style);
                }
            }
            ShapeData::Oval {
                rect,
                fill,
                stroke: has_stroke,
                stroke_style,
            } => {
                let img_rect = to_img_rect(*rect);
                if let Some(fill_color) = fill {
                    fill_ellipse(&mut out, img_rect, *fill_color);
                }
                if *has_stroke {
                    stroke_ellipse(&mut out, img_rect, t, c, *stroke_style);
                }
            }
            ShapeData::Star { rect } => {
                stroke_polygon(&mut out, &star_points(to_img_rect(*rect)), t, c)
            }
            ShapeData::Line { start, end, style } => {
                stroke_line(&mut out, to_img_pos(*start), to_img_pos(*end), t, c, *style);
            }
            ShapeData::Arrow {
                start,
                end,
                style,
                heads,
            } => {
                let s = to_img_pos(*start);
                let e = to_img_pos(*end);
                stroke_line(&mut out, s, e, t, c, *style);
                stamp_arrowhead(&mut out, e, e - s, t, c);
                if *heads == ArrowHeads::Double {
                    stamp_arrowhead(&mut out, s, s - e, t, c);
                }
            }
            ShapeData::Pen {
                smoothed_points,
                raw_points,
            } => {
                let pts = if !smoothed_points.is_empty() {
                    smoothed_points
                } else {
                    raw_points
                };
                let imgs: Vec<Pos2> = pts.iter().map(|p| to_img_pos(*p)).collect();
                for w in imgs.windows(2) {
                    stroke_line(&mut out, w[0], w[1], t, c, LineStyle::Solid);
                }
            }
            ShapeData::Blur { rect, block_size } => {
                let img_rect = to_img_rect(*rect);
                let img_block = (block_size * inv).max(2.0);
                pixelate_region(&mut out, img_rect, img_block);
            }
            ShapeData::ImageLayer { rect, data, .. } => {
                composite_image(&mut out, to_img_rect(*rect), data.as_ref());
            }
            ShapeData::StepMarker {
                pos,
                number,
                radius,
            } => {
                let ip = to_img_pos(*pos);
                let ir = (radius * inv).max(4.0);
                fill_disc(&mut out, ip.x, ip.y, ir, c);
                // White rim
                stroke_ellipse(
                    &mut out,
                    Rect::from_center_size(ip, egui::vec2(ir * 2.0, ir * 2.0)),
                    1.5_f32.max(ir * 0.1),
                    Color32::WHITE,
                    LineStyle::Solid,
                );
                draw_digits(&mut out, ip.x, ip.y, *number, ir * 1.1, Color32::WHITE);
            }
            ShapeData::Callout { rect, tail } => {
                // Stroke the rounded rect outline
                stroke_rect(&mut out, to_img_rect(*rect), t, c, LineStyle::Solid);
                // Draw the tail triangle lines
                let center_x = rect.center().x;
                let base_y = rect.max.y;
                let p1 = Pos2::new(center_x - 15.0, base_y);
                let p2 = Pos2::new(center_x + 15.0, base_y);
                let tip = to_img_pos(*tail);
                stroke_line_segment(&mut out, p1, tip, t, c);
                stroke_line_segment(&mut out, p2, tip, t, c);
            }
            ShapeData::Ruler { start, end } => {
                stroke_line_segment(&mut out, to_img_pos(*start), to_img_pos(*end), t, c);
                // Perpendicular tick marks at endpoints
                let dir = *end - *start;
                let len = (dir.x * dir.x + dir.y * dir.y).sqrt();
                if len > 0.5 {
                    let d = egui::Vec2::new(dir.x / len, dir.y / len);
                    let perp = egui::Vec2::new(-d.y, d.x) * 5.0;
                    stroke_line_segment(&mut out, *start - perp, *start + perp, t, c);
                    stroke_line_segment(&mut out, *end - perp, *end + perp, t, c);
                }
                // Distance label is skipped in flatten (no font renderer available)
            }
            ShapeData::Text { pos, content, size } => {
                #[cfg(feature = "agent-server")]
                crate::text_render::rasterize_text(&mut out, *pos, content, *size, c);
                #[cfg(not(feature = "agent-server"))]
                {
                    // Fallback: outline a bounding box so non-agent builds at least
                    // know "text exists here". Width estimate = chars * size_px * 0.55.
                    let est_w = (content.chars().count() as f32) * (*size) * 0.55;
                    let est_h = *size * 1.2;
                    let bb = egui::Rect::from_min_size(*pos, egui::vec2(est_w, est_h));
                    stroke_rect(&mut out, bb, t.max(1.0), c, LineStyle::Solid);
                }
            }
            ShapeData::Question { pos, content, size } => {
                // Yellow-tinted ghost rendering for agent question bubbles.
                let yellow = egui::Color32::from_rgba_unmultiplied(255, 220, 0, c.a() / 2);
                #[cfg(feature = "agent-server")]
                crate::text_render::rasterize_text(&mut out, *pos, content, *size, yellow);
                #[cfg(not(feature = "agent-server"))]
                {
                    let est_w = (content.chars().count() as f32) * (*size) * 0.55;
                    let est_h = *size * 1.2;
                    let bb = egui::Rect::from_min_size(*pos, egui::vec2(est_w, est_h));
                    stroke_rect(&mut out, bb, t.max(1.0), yellow, LineStyle::Solid);
                }
            }
            ShapeData::LogicTag { pos, .. } => {
                // LogicTag's textual representation lives in the shape; render the tag
                // as a small box outline (the tag's chip-style UI rendering uses egui
                // and is out of scope here). Phase 5 may rasterize the chip text too.
                let rect = egui::Rect::from_center_size(*pos, egui::vec2(80.0, 24.0));
                stroke_rect(&mut out, rect, t.max(1.0), c, LineStyle::Solid);
            }
            ShapeData::FocusRegion { .. } => {
                // UI affordance, not a persisted shape; intentionally skipped.
            }
            ShapeData::BezierPath { nodes, closed, .. } => {
                let tess = crate::bezier::tessellate(nodes, *closed, 16);
                for w in tess.windows(2) {
                    stroke_line(&mut out, w[0], w[1], t, c, LineStyle::Solid);
                }
            }
        }
    }

    Some(out)
}

/// Flatten `annotations` onto the source image and encode the result as PNG
/// bytes. Returns the encoded bytes together with the image dimensions, or an
/// `Err(msg)` describing why nothing could be produced (no source image and no
/// canvas size, or a PNG-encoder failure).
///
/// This is the shared entry point used by the on-disk/clipboard export path so
/// it stays in lock-step with the agent snapshot encoder. It does not depend on
/// any feature-gated code: the core rasterizer always runs, and text glyphs are
/// rendered when the `agent-server` feature is enabled (falling back to bbox
/// outlines otherwise — see `flatten`).
pub fn flatten_to_png(
    annotations: &[Annotation],
    color_image: Option<&egui::ColorImage>,
    canvas_size: Vec2,
    screen_rect: Rect,
) -> Result<(Vec<u8>, u32, u32), String> {
    use image::ImageEncoder;

    let rgba = flatten(annotations, color_image, canvas_size, screen_rect)
        .ok_or_else(|| "nothing to render (no source image and no canvas size)".to_string())?;
    let (w, h) = (rgba.width(), rgba.height());
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok((out, w, h))
}

// ── Primitives ───────────────────────────────────────────────────────────────

fn put_pixel(img: &mut RgbaImage, x: i32, y: i32, c: Color32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let a = c.a() as u32;
    if a == 255 {
        img.put_pixel(x as u32, y as u32, Rgba([c.r(), c.g(), c.b(), 255]));
        return;
    }
    // Simple alpha over
    let dst = img.get_pixel(x as u32, y as u32);
    let inv_a = 255 - a;
    let blend = |s: u32, d: u32| ((s * a + d * inv_a) / 255) as u8;
    img.put_pixel(
        x as u32,
        y as u32,
        Rgba([
            blend(c.r() as u32, dst[0] as u32),
            blend(c.g() as u32, dst[1] as u32),
            blend(c.b() as u32, dst[2] as u32),
            (a + (dst[3] as u32 * inv_a) / 255).min(255) as u8,
        ]),
    );
}

fn fill_rect(img: &mut RgbaImage, rect: Rect, c: Color32) {
    let x0 = rect.min.x.floor() as i32;
    let y0 = rect.min.y.floor() as i32;
    let x1 = rect.max.x.ceil() as i32;
    let y1 = rect.max.y.ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    for y in y0.max(0)..y1.min(ih) {
        for x in x0.max(0)..x1.min(iw) {
            put_pixel(img, x, y, c);
        }
    }
}

/// Fill a circle of radius r centered at (cx, cy). Used as a stamp for
/// thickness-aware stroking.
fn fill_disc(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, c: Color32) {
    let r2 = r * r;
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                put_pixel(img, x, y, c);
            }
        }
    }
}

/// Stamp discs along the line (a, b). Simple but produces nicely thick lines
/// with natural round caps.
fn stroke_line_segment(img: &mut RgbaImage, a: Pos2, b: Pos2, thickness: f32, c: Color32) {
    let r = (thickness * 0.5).max(0.5);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        fill_disc(img, a.x, a.y, r, c);
        return;
    }
    let step = (r * 0.6).max(0.5); // dense enough to avoid gaps
    let n = (len / step).ceil() as i32 + 1;
    for i in 0..=n {
        let t = (i as f32 / n as f32).clamp(0.0, 1.0);
        fill_disc(img, a.x + dx * t, a.y + dy * t, r, c);
    }
}

fn stroke_line(
    img: &mut RgbaImage,
    a: Pos2,
    b: Pos2,
    thickness: f32,
    c: Color32,
    style: LineStyle,
) {
    match style {
        LineStyle::Solid => stroke_line_segment(img, a, b, thickness, c),
        LineStyle::Dashed => {
            let dash = (thickness * 3.0).max(6.0);
            let gap = (thickness * 2.5).max(4.0);
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let total = (dx * dx + dy * dy).sqrt();
            if total < 0.5 {
                return;
            }
            let ux = dx / total;
            let uy = dy / total;
            let mut s = 0.0_f32;
            while s < total {
                let e = (s + dash).min(total);
                let p1 = Pos2::new(a.x + ux * s, a.y + uy * s);
                let p2 = Pos2::new(a.x + ux * e, a.y + uy * e);
                stroke_line_segment(img, p1, p2, thickness, c);
                s = e + gap;
            }
        }
        LineStyle::Dotted => {
            // Dots: stamp a disc every ~2*thickness apart
            let dot = thickness.max(1.0);
            let gap = (thickness * 2.0).max(3.0);
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let total = (dx * dx + dy * dy).sqrt();
            if total < 0.5 {
                return;
            }
            let ux = dx / total;
            let uy = dy / total;
            let mut s = 0.0_f32;
            while s < total {
                fill_disc(img, a.x + ux * s, a.y + uy * s, dot * 0.5, c);
                s += dot + gap;
            }
        }
    }
}

fn stroke_rect(img: &mut RgbaImage, rect: Rect, thickness: f32, c: Color32, style: LineStyle) {
    let tl = rect.min;
    let tr = Pos2::new(rect.max.x, rect.min.y);
    let br = rect.max;
    let bl = Pos2::new(rect.min.x, rect.max.y);
    stroke_line(img, tl, tr, thickness, c, style);
    stroke_line(img, tr, br, thickness, c, style);
    stroke_line(img, br, bl, thickness, c, style);
    stroke_line(img, bl, tl, thickness, c, style);
}

fn fill_ellipse(img: &mut RgbaImage, rect: Rect, c: Color32) {
    let cx = rect.center().x;
    let cy = rect.center().y;
    let rx = (rect.width() * 0.5).abs();
    let ry = (rect.height() * 0.5).abs();
    if rx < 0.5 || ry < 0.5 {
        return;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let y0 = (cy - ry).floor() as i32;
    let y1 = (cy + ry).ceil() as i32;
    for y in y0.max(0)..y1.min(ih) {
        let dy = (y as f32 + 0.5 - cy) / ry;
        if dy.abs() > 1.0 {
            continue;
        }
        let dx = rx * (1.0 - dy * dy).max(0.0).sqrt();
        let x0 = (cx - dx).floor() as i32;
        let x1 = (cx + dx).ceil() as i32;
        for x in x0.max(0)..x1.min(iw) {
            put_pixel(img, x, y, c);
        }
    }
}

fn stroke_ellipse(img: &mut RgbaImage, rect: Rect, thickness: f32, c: Color32, style: LineStyle) {
    let cx = rect.center().x;
    let cy = rect.center().y;
    let rx = (rect.width() * 0.5).abs();
    let ry = (rect.height() * 0.5).abs();
    if rx < 0.5 || ry < 0.5 {
        return;
    }
    let circumference = std::f32::consts::PI * (rx + ry); // approx
    let steps = (circumference / 2.0).max(48.0) as i32;
    let mut prev = Pos2::new(cx + rx, cy);
    for i in 1..=steps {
        let t = (i as f32 / steps as f32) * std::f32::consts::TAU;
        let p = Pos2::new(cx + rx * t.cos(), cy + ry * t.sin());
        stroke_line(img, prev, p, thickness, c, style);
        prev = p;
    }
}

fn stroke_polygon(img: &mut RgbaImage, points: &[Pos2], thickness: f32, c: Color32) {
    if points.len() < 2 {
        return;
    }
    for window in points.windows(2) {
        stroke_line_segment(img, window[0], window[1], thickness, c);
    }
    stroke_line_segment(img, *points.last().unwrap(), points[0], thickness, c);
}

fn stamp_arrowhead(img: &mut RgbaImage, tip: Pos2, dir: Vec2, thickness: f32, c: Color32) {
    let n = dir.length();
    if n < 0.5 {
        return;
    }
    let len = (thickness * 4.0).clamp(10.0, 40.0);
    let d = dir / n;
    let perp = Vec2::new(-d.y, d.x);
    let base = tip - d * len;
    let left = base + perp * (len * 0.5);
    let right = base - perp * (len * 0.5);
    stroke_line_segment(img, tip, left, thickness, c);
    stroke_line_segment(img, tip, right, thickness, c);
}

/// Paint `src` into `out` at `dst_rect` (image-pixel coords) with nearest-
/// neighbor sampling and alpha-over blending. Clips to the `out` bounds.
fn composite_image(out: &mut RgbaImage, dst_rect: Rect, src: &RgbaImage) {
    let (ow, oh) = (out.width() as i32, out.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    if sw == 0 || sh == 0 {
        return;
    }
    let dx0 = dst_rect.min.x.floor() as i32;
    let dy0 = dst_rect.min.y.floor() as i32;
    let dx1 = dst_rect.max.x.ceil() as i32;
    let dy1 = dst_rect.max.y.ceil() as i32;
    let dw = (dx1 - dx0).max(1);
    let dh = (dy1 - dy0).max(1);

    let y_start = dy0.max(0);
    let y_end = dy1.min(oh);
    let x_start = dx0.max(0);
    let x_end = dx1.min(ow);
    for y in y_start..y_end {
        let sy = ((y - dy0) as f32 / dh as f32 * sh as f32) as i32;
        let sy = sy.clamp(0, sh - 1);
        for x in x_start..x_end {
            let sx = ((x - dx0) as f32 / dw as f32 * sw as f32) as i32;
            let sx = sx.clamp(0, sw - 1);
            let s = src.get_pixel(sx as u32, sy as u32).0;
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                out.put_pixel(x as u32, y as u32, Rgba([s[0], s[1], s[2], 255]));
                continue;
            }
            let d = out.get_pixel(x as u32, y as u32).0;
            let inv_a = 255 - a;
            let blend = |sc: u32, dc: u32| ((sc * a + dc * inv_a) / 255) as u8;
            out.put_pixel(
                x as u32,
                y as u32,
                Rgba([
                    blend(s[0] as u32, d[0] as u32),
                    blend(s[1] as u32, d[1] as u32),
                    blend(s[2] as u32, d[2] as u32),
                    (a + (d[3] as u32 * inv_a) / 255).min(255) as u8,
                ]),
            );
        }
    }
}

fn pixelate_region(img: &mut RgbaImage, rect: Rect, block: f32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let x0 = (rect.min.x.floor() as i32).max(0);
    let y0 = (rect.min.y.floor() as i32).max(0);
    let x1 = (rect.max.x.ceil() as i32).min(w);
    let y1 = (rect.max.y.ceil() as i32).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let block = block.max(2.0);

    let mut by = y0 as f32;
    while (by as i32) < y1 {
        let mut bx = x0 as f32;
        let bh = (block).min((y1 as f32) - by);
        while (bx as i32) < x1 {
            let bw = (block).min((x1 as f32) - bx);
            // Average pixels in the block
            let (mut r, mut g, mut b, mut cnt) = (0u64, 0u64, 0u64, 0u64);
            let ex = ((bx + bw) as i32).min(x1);
            let ey = ((by + bh) as i32).min(y1);
            for y in (by as i32)..ey {
                for x in (bx as i32)..ex {
                    let px = img.get_pixel(x as u32, y as u32);
                    r += px[0] as u64;
                    g += px[1] as u64;
                    b += px[2] as u64;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                let avg = Rgba([(r / cnt) as u8, (g / cnt) as u8, (b / cnt) as u8, 255]);
                for y in (by as i32)..ey {
                    for x in (bx as i32)..ex {
                        img.put_pixel(x as u32, y as u32, avg);
                    }
                }
            }
            bx += block;
        }
        by += block;
    }
}

// ── Digit font (5x7, digits 0-9 only) ────────────────────────────────────────
//
// Used to burn numbers inside step-marker circles when the canvas is flattened.
// Each row is 5 bits, top-to-bottom. One byte per row, 7 rows per glyph.

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

#[rustfmt::skip]
const DIGITS: [[u8; GLYPH_H]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

/// Draw `number` centered at (cx, cy) at a pixel height of approximately
/// `target_h`. Each "on" bit becomes a filled disc so the digits stay legible
/// at any scale.
fn draw_digits(img: &mut RgbaImage, cx: f32, cy: f32, number: u32, target_h: f32, color: Color32) {
    let s = number.to_string();
    let chars: Vec<u8> = s.bytes().collect();
    let glyph_h = target_h.max(5.0);
    let px = (glyph_h / GLYPH_H as f32).max(1.0);
    let glyph_w = GLYPH_W as f32 * px;
    let spacing = px;
    let total_w = chars.len() as f32 * glyph_w + (chars.len().saturating_sub(1)) as f32 * spacing;
    let mut x0 = cx - total_w * 0.5;
    let y0 = cy - glyph_h * 0.5;
    for ch in chars {
        if !ch.is_ascii_digit() {
            x0 += glyph_w + spacing;
            continue;
        }
        let digit = (ch - b'0') as usize;
        let glyph = &DIGITS[digit];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..GLYPH_W {
                if (bits >> (GLYPH_W - 1 - col)) & 1 == 1 {
                    let cxp = x0 + col as f32 * px + px * 0.5;
                    let cyp = y0 + row as f32 * px + px * 0.5;
                    fill_disc(img, cxp, cyp, px * 0.6, color);
                }
            }
        }
        x0 += glyph_w + spacing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_returns_none_without_image_or_canvas_size() {
        assert!(flatten(&[], None, egui::Vec2::ZERO, egui::Rect::ZERO).is_none());
    }

    #[test]
    fn flatten_no_image_with_canvas_size_renders_on_white() {
        use crate::types::{Annotation, ShapeData};
        use egui::{vec2, Color32, Pos2, Rect};

        let ann = Annotation::new_authored(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::new(10.0, 10.0), vec2(20.0, 20.0))),
            Color32::RED,
            2.0,
            crate::types::ParticipantId::HUMAN_LOCAL,
        );

        let img = flatten(
            &[ann],
            None, // no source image
            vec2(100.0, 100.0),
            Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0)),
        )
        .expect("flatten should produce an image when canvas_size is set");

        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);

        // Verify the rect outline made it onto the white background.
        // Sample a pixel near the rect's edge — should be red, not white.
        let edge_pixel = img.get_pixel(10, 15);
        assert!(
            edge_pixel.0[0] > 200 && edge_pixel.0[1] < 100,
            "expected red pixel near rect edge, got {edge_pixel:?}"
        );
    }

    #[test]
    fn pixelate_averages_block() {
        let mut img = RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = Rgba([200, 0, 0, 255]);
        }
        pixelate_region(
            &mut img,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(4.0, 4.0)),
            4.0,
        );
        let px = img.get_pixel(1, 1);
        assert_eq!(px[0], 200);
        assert_eq!(px[1], 0);
    }

    #[test]
    fn flatten_to_png_produces_valid_png_bytes() {
        use crate::types::{Annotation, ShapeData};
        use egui::{vec2, Color32, Pos2, Rect};

        // Small synthetic canvas with one red rectangle annotation, no source image.
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::new(8.0, 8.0), vec2(32.0, 24.0))),
            Color32::RED,
            2.0,
            crate::types::ParticipantId::HUMAN_LOCAL,
        );
        let canvas = vec2(64.0, 48.0);
        let screen = Rect::from_min_size(Pos2::ZERO, canvas);

        let (bytes, w, h) =
            flatten_to_png(&[ann], None, canvas, screen).expect("flatten_to_png should succeed");

        assert_eq!((w, h), (64, 48));
        assert!(!bytes.is_empty(), "PNG bytes should be non-empty");
        // PNG magic number: 0x89 'P' 'N' 'G' \r \n 0x1a \n
        assert_eq!(
            &bytes[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "output should start with the PNG signature"
        );
        // Round-trips through the decoder at the expected dimensions.
        let decoded = image::load_from_memory(&bytes)
            .expect("decode PNG")
            .to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (64, 48));
    }

    #[test]
    fn flatten_to_png_errors_when_nothing_to_render() {
        let err = flatten_to_png(&[], None, egui::Vec2::ZERO, egui::Rect::ZERO);
        assert!(
            err.is_err(),
            "no image and no canvas size should be an error"
        );
    }

    #[cfg(feature = "agent-server")]
    #[test]
    fn flatten_with_text_writes_glyphs() {
        use crate::types::{Annotation, ShapeData};
        use egui::{vec2, Color32, Pos2, Rect};

        let mut ann = Annotation::new_authored(
            ShapeData::Text {
                pos: Pos2::new(10.0, 10.0),
                content: "Hi".into(),
                size: 32.0,
            },
            Color32::BLACK,
            1.0,
            crate::types::ParticipantId::HUMAN_LOCAL,
        );
        ann.label = None;

        let white = egui::Color32::WHITE;
        let ci = egui::ColorImage {
            size: [200, 100],
            pixels: vec![white; 200 * 100],
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0));
        let img = flatten(&[ann], Some(&ci), vec2(200.0, 100.0), screen).expect("flatten");

        // At least one non-white pixel inside the text bbox.
        let mut found = false;
        for y in 10..70 {
            for x in 10..80 {
                let p = img.get_pixel(x, y);
                if p.0[0] < 250 || p.0[1] < 250 || p.0[2] < 250 {
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        assert!(found, "expected glyph pixels in text bbox");
    }
}
