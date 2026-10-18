//! Transform-aware rendering for geometric shapes.
//!
//! When a leaf has a non-identity world affine, this module draws its annotation
//! through the composed `world` transform rather than the plain identity path.
//!
//! **Identity parity**: this module is ONLY called for non-identity transforms.
//! The identity path in `canvas.rs` `show()` is completely untouched.
//!
//! **Text + ImageLayer**: Task 9 — full-fidelity transform rendering implemented.
//! - ImageLayer: rendered as a 2-triangle mesh with transformed corners.
//! - Text: rendered via TextShape with rotation angle extracted from decompose.
//!   Skew and non-uniform scale are not fully honored for text (egui TextShape
//!   only supports a single rotation angle); they fall back to the
//!   rotation+uniform-scale path with a comment documenting the limitation.

use crate::{
    transform::{apply, decompose},
    types::{Annotation, ArrowHeads, LineStyle, ShapeData},
};
use egui::epaint::{Mesh, TextureId, Vertex};
use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2};
use kurbo::Affine;

// ── Core mapping helper ─────────────────────────────────────────────────────

/// Build a closure that maps a leaf-local point to screen:
/// `world` (image-local → image-world), then `project` (image → screen).
fn mapper<'a>(world: Affine, project: &'a impl Fn(Pos2) -> Pos2) -> impl Fn(Pos2) -> Pos2 + 'a {
    move |p| project(apply(world, p))
}

// ── Testable corner helper ──────────────────────────────────────────────────

/// Map a rect's four corners (TL, TR, BR, BL) through `world` then `project`.
/// Exposed for unit-testing the pure coordinate math.
pub fn corners_in_screen(r: Rect, world: Affine, project: &impl Fn(Pos2) -> Pos2) -> [Pos2; 4] {
    let m = mapper(world, project);
    [
        m(r.left_top()),
        m(r.right_top()),
        m(r.right_bottom()),
        m(r.left_bottom()),
    ]
}

// ── Image mesh helpers (Task 9) ─────────────────────────────────────────────

/// Build a 2-triangle [`Mesh`] whose 4 vertices are the screen-mapped corners
/// of `rect` (TL, TR, BR, BL) with UVs (0,0),(1,0),(1,1),(0,1).
///
/// The vertex color is white scaled by `opacity` so the image tints correctly.
///
/// # Triangle winding
/// Triangle 0: TL(0), TR(1), BR(2)
/// Triangle 1: TL(0), BR(2), BL(3)
pub fn image_mesh(
    rect: Rect,
    world: Affine,
    project: &impl Fn(Pos2) -> Pos2,
    tex_id: TextureId,
    opacity: f32,
) -> Mesh {
    let corners = corners_in_screen(rect, world, project);
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    let white = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);

    let uvs: [Pos2; 4] = [
        egui::pos2(0.0, 0.0), // TL
        egui::pos2(1.0, 0.0), // TR
        egui::pos2(1.0, 1.0), // BR
        egui::pos2(0.0, 1.0), // BL
    ];

    let mut mesh = Mesh::with_texture(tex_id);
    mesh.reserve_triangles(2);
    mesh.reserve_vertices(4);

    for (pos, uv) in corners.iter().zip(uvs.iter()) {
        mesh.vertices.push(Vertex {
            pos: *pos,
            uv: *uv,
            color: white,
        });
    }
    // Triangle 0: TL, TR, BR
    mesh.add_triangle(0, 1, 2);
    // Triangle 1: TL, BR, BL
    mesh.add_triangle(0, 2, 3);
    mesh
}

/// Testable version of [`image_mesh`]: same geometry but uses a dummy
/// [`TextureId`] so the corner math can be unit-tested without a GPU context.
pub fn image_mesh_for_test(rect: Rect, world: Affine, project: &impl Fn(Pos2) -> Pos2) -> Mesh {
    image_mesh(rect, world, project, TextureId::default(), 1.0)
}

// ── Sample an ellipse outline ───────────────────────────────────────────────

/// Sample `n` equidistant points around an axis-aligned ellipse defined by
/// `rect`. Points are in image-local space; callers map them through `m`.
fn sample_ellipse(rect: Rect, n: usize) -> Vec<Pos2> {
    let cx = rect.center().x;
    let cy = rect.center().y;
    let rx = rect.width() * 0.5;
    let ry = rect.height() * 0.5;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            Pos2::new(cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

/// Sample `n` points around a rounded-rect outline (rect with corner radius).
/// Uses straight edges + arc segments at each corner.
fn sample_rounded_rect(rect: Rect, radius: f32, n: usize) -> Vec<Pos2> {
    let r = radius
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);
    if r < 0.5 {
        // Degenerate: treat as normal rect.
        return vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ];
    }

    // Approximate: sample via parametric; 4 corner arcs of PI/2 each + 4 edges.
    // Distribute n points across the total perimeter.
    let corner_arc = std::f32::consts::FRAC_PI_2;
    let pts_per_corner = (n / 4).max(4);
    let mut pts = Vec::with_capacity(n + 4);

    struct Corner {
        cx: f32,
        cy: f32,
        start_angle: f32,
    }
    let corners = [
        Corner {
            cx: rect.max.x - r,
            cy: rect.min.y + r,
            start_angle: -corner_arc,
        }, // TR
        Corner {
            cx: rect.max.x - r,
            cy: rect.max.y - r,
            start_angle: 0.0,
        }, // BR
        Corner {
            cx: rect.min.x + r,
            cy: rect.max.y - r,
            start_angle: corner_arc,
        }, // BL
        Corner {
            cx: rect.min.x + r,
            cy: rect.min.y + r,
            start_angle: std::f32::consts::PI,
        }, // TL
    ];

    for c in &corners {
        for i in 0..pts_per_corner {
            let t = i as f32 / pts_per_corner as f32 * corner_arc + c.start_angle;
            pts.push(Pos2::new(c.cx + r * t.cos(), c.cy + r * t.sin()));
        }
    }
    pts
}

// ── Star point generation (transform-aware) ──────────────────────────────────

fn star_points_mapped(rect: Rect, m: &impl Fn(Pos2) -> Pos2) -> Vec<Pos2> {
    crate::types::star_points(rect).into_iter().map(m).collect()
}

// ── Fill-color helper (premultiplied-alpha correctness) ─────────────────────

/// Combine a shape's own translucent fill color with an additional alpha
/// multiplier (from provisional-state / layer opacity), correctly
/// un-premultiplying first.
///
/// `egui::Color32` stores premultiplied-alpha components, so reading
/// `.r()/.g()/.b()` directly off a translucent fill bakes the alpha into the
/// RGB bytes instead of returning straight sRGB. This mirrors the fix applied
/// in `export.rs::fill_color_to_hex` and `svg_emit.rs::css_color_unmultiplied`:
/// unmultiply first via `to_srgba_unmultiplied()`, then re-premultiply once
/// with the final combined alpha.
pub fn blended_fill(fill_color: Color32, alpha_mul: f32) -> Color32 {
    let [r, g, b, base_a] = fill_color.to_srgba_unmultiplied();
    let a = (base_a as f32 * alpha_mul).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

// ── Line drawing helpers (private, mirroring canvas.rs) ─────────────────────

fn draw_line_styled_xf(
    painter: &Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    style: LineStyle,
) {
    match style {
        LineStyle::Solid => {
            painter.line_segment([start, end], stroke);
        }
        LineStyle::Dashed => {
            let dash_len = (stroke.width * 3.0).max(6.0);
            let gap_len = (stroke.width * 2.5).max(4.0);
            let vec = end - start;
            let total = vec.length();
            if total < 0.5 {
                return;
            }
            let dir = vec / total;
            let mut t = 0.0_f32;
            while t < total {
                let t2 = (t + dash_len).min(total);
                painter.line_segment([start + dir * t, start + dir * t2], stroke);
                t = t2 + gap_len;
            }
        }
        LineStyle::Dotted => {
            let vec = end - start;
            let len = vec.length();
            if len < 0.5 {
                return;
            }
            let dir = vec / len;
            let gap = stroke.width * 3.0;
            let mut t = 0.0_f32;
            while t < len {
                painter.circle_filled(start + dir * t, stroke.width * 0.6, stroke.color);
                t += gap;
            }
        }
    }
}

fn draw_arrowhead_xf(painter: &Painter, tip: Pos2, dir: Vec2, stroke: Stroke, len: f32) {
    let n = dir.length();
    if n < 0.5 {
        return;
    }
    let d = dir / n;
    let perp = Vec2::new(-d.y, d.x);
    let base = tip - d * len;
    let left = base + perp * (len * 0.5);
    let right = base - perp * (len * 0.5);
    painter.line_segment([tip, left], stroke);
    painter.line_segment([tip, right], stroke);
}

fn draw_arrow_xf(
    painter: &Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    style: LineStyle,
    heads: ArrowHeads,
) {
    draw_line_styled_xf(painter, start, end, stroke, style);
    let head_len = (stroke.width * 4.0).clamp(10.0, 28.0);
    draw_arrowhead_xf(painter, end, end - start, stroke, head_len);
    if heads == ArrowHeads::Double {
        draw_arrowhead_xf(painter, start, start - end, stroke, head_len);
    }
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Draw `ann` through `world` (leaf-local → image-world) then `project`
/// (image → screen), applying zoom-scaled thickness and opacity.
///
/// Text and ImageLayer are routed to the axis-aligned fallback (Task 9).
#[allow(clippy::too_many_arguments)]
pub fn draw_transformed(
    painter: &Painter,
    ann: &Annotation,
    world: Affine,
    project: &impl Fn(Pos2) -> Pos2,
    view_zoom: f32,
    color: Color32,
    layer_opacity: f32,
    accent: Color32,
) {
    // Derive effective alpha (same logic as draw_annotation_full in canvas.rs).
    let provisional_alpha = match ann.provisional_until {
        Some(deadline) if deadline > std::time::SystemTime::now() => 128u8,
        _ => 255u8,
    };
    let alpha_mul = (provisional_alpha as f32 / 255.0) * layer_opacity.clamp(0.0, 1.0);
    let ann_color = {
        let c = color;
        Color32::from_rgba_unmultiplied(
            c.r(),
            c.g(),
            c.b(),
            (c.a() as f32 * alpha_mul).round() as u8,
        )
    };

    let thickness = ann.thickness * view_zoom;
    let stroke = Stroke::new(thickness, ann_color);
    let m = mapper(world, project);

    match &ann.shape {
        // ── Rect-based: render as a transformed quad ────────────────────────
        ShapeData::Rect {
            rect,
            radius,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            let pts: Vec<Pos2> = sample_rounded_rect(*rect, *radius, 64)
                .into_iter()
                .map(&m)
                .collect();
            if let Some(fill_color) = fill {
                let fc = blended_fill(*fill_color, alpha_mul);
                painter.add(egui::Shape::convex_polygon(pts.clone(), fc, Stroke::NONE));
            }
            if *has_stroke {
                match stroke_style {
                    LineStyle::Solid => {
                        painter.add(egui::Shape::closed_line(pts, stroke));
                    }
                    LineStyle::Dashed | LineStyle::Dotted => {
                        let n = pts.len();
                        for i in 0..n {
                            draw_line_styled_xf(
                                painter,
                                pts[i],
                                pts[(i + 1) % n],
                                stroke,
                                *stroke_style,
                            );
                        }
                    }
                }
            }
        }

        ShapeData::Oval {
            rect,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            const N: usize = 64;
            let pts: Vec<Pos2> = sample_ellipse(*rect, N).into_iter().map(&m).collect();
            if let Some(fill_color) = fill {
                let fc = blended_fill(*fill_color, alpha_mul);
                painter.add(egui::Shape::convex_polygon(pts.clone(), fc, Stroke::NONE));
            }
            if *has_stroke {
                match stroke_style {
                    LineStyle::Solid => {
                        painter.add(egui::Shape::closed_line(pts, stroke));
                    }
                    LineStyle::Dashed | LineStyle::Dotted => {
                        let n = pts.len();
                        for i in 0..n {
                            draw_line_styled_xf(
                                painter,
                                pts[i],
                                pts[(i + 1) % n],
                                stroke,
                                *stroke_style,
                            );
                        }
                    }
                }
            }
        }

        ShapeData::Blur { rect, .. } => {
            // Blur needs pixel sampling which is screen-space; for transformed
            // leaves fall back to a dark fill placeholder (same as no-image path).
            let c = corners_in_screen(*rect, world, project);
            painter.add(egui::Shape::convex_polygon(
                c.to_vec(),
                Color32::from_rgba_unmultiplied(30, 30, 30, 230),
                Stroke::NONE,
            ));
        }

        ShapeData::FocusRegion { rect } => {
            let c = corners_in_screen(*rect, world, project);
            painter.add(egui::Shape::closed_line(
                c.to_vec(),
                Stroke::new(2.0, accent),
            ));
        }

        ShapeData::Star { rect } => {
            let pts = star_points_mapped(*rect, &m);
            painter.add(egui::Shape::closed_line(pts, stroke));
        }

        // ── Point-based ─────────────────────────────────────────────────────
        ShapeData::Line { start, end, style } => {
            draw_line_styled_xf(painter, m(*start), m(*end), stroke, *style);
        }

        ShapeData::Arrow {
            start,
            end,
            style,
            heads,
        } => {
            draw_arrow_xf(painter, m(*start), m(*end), stroke, *style, *heads);
        }

        ShapeData::Ruler { start, end } => {
            let s = m(*start);
            let e = m(*end);
            painter.line_segment([s, e], stroke);
            let dir = e - s;
            let len = dir.length();
            if len > 0.5 {
                let d = dir / len;
                let perp = Vec2::new(-d.y, d.x) * 5.0;
                painter.line_segment([s - perp, s + perp], stroke);
                painter.line_segment([e - perp, e + perp], stroke);
            }
            let dist = len;
            let mid = Pos2::new((s.x + e.x) * 0.5, (s.y + e.y) * 0.5);
            let perp_offset = if len > 0.5 {
                let d = dir / len;
                Vec2::new(-d.y, d.x) * 12.0
            } else {
                Vec2::new(0.0, -12.0)
            };
            painter.text(
                mid + perp_offset,
                egui::Align2::CENTER_CENTER,
                format!("{:.0}px", dist),
                egui::FontId::proportional(12.0),
                ann_color,
            );
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
            let mapped: Vec<Pos2> = pts.iter().copied().map(&m).collect();
            if mapped.len() >= 2 {
                painter.add(egui::Shape::line(mapped, stroke));
            }
        }

        ShapeData::LogicTag { pos, tag } => {
            // LogicTag is text-positioned; use the transformed origin but
            // render axis-aligned (text layout can't be affinely transformed).
            let screen_pos = m(*pos);
            let amber = accent;
            let label = tag.label();
            let galley = painter.layout_no_wrap(
                label.to_string(),
                egui::FontId::proportional(12.0),
                Color32::BLACK,
            );
            let bg = Rect::from_min_size(screen_pos, galley.size() + egui::vec2(8.0, 4.0));
            painter.rect_filled(bg, 3.0, amber);
            painter.galley(screen_pos + egui::vec2(4.0, 2.0), galley, Color32::BLACK);
        }

        ShapeData::StepMarker {
            pos,
            number,
            radius,
        } => {
            let screen_pos = m(*pos);
            let scaled_radius = *radius * view_zoom;
            painter.circle_filled(screen_pos, scaled_radius, ann_color);
            painter.circle_stroke(screen_pos, scaled_radius, Stroke::new(1.5, Color32::WHITE));
            let font_size = (scaled_radius * 1.1).max(10.0);
            painter.text(
                screen_pos,
                egui::Align2::CENTER_CENTER,
                number.to_string(),
                egui::FontId::proportional(font_size),
                Color32::WHITE,
            );
        }

        ShapeData::Callout { rect, tail } => {
            // Map the four bubble corners and the tail tip through `m`.
            let c = corners_in_screen(*rect, world, project);
            let tail_screen = m(*tail);

            // Fill quad (convex_polygon approximation; fine for bubble shape).
            let fill_color =
                Color32::from_rgba_unmultiplied(ann_color.r(), ann_color.g(), ann_color.b(), 40);
            painter.add(egui::Shape::convex_polygon(c.to_vec(), fill_color, stroke));

            // Tail triangle: map base points from bubble-bottom, then tip.
            // Original logic anchors base at center-bottom ±15px.
            let orig_cx = rect.center().x;
            let orig_base_y = rect.max.y;
            let p1 = m(Pos2::new(orig_cx - 15.0, orig_base_y));
            let p2 = m(Pos2::new(orig_cx + 15.0, orig_base_y));
            painter.add(egui::Shape::convex_polygon(
                vec![p1, p2, tail_screen],
                ann_color,
                Stroke::NONE,
            ));
        }

        ShapeData::BezierPath {
            nodes,
            closed,
            fill,
        } => {
            // Map each node position; handles are relative offsets — scale them
            // by view_zoom (same as project_annotation does) after world-mapping
            // the anchor. For the tessellation we build a temporary node list
            // with screen positions + scaled handles.
            let mapped_nodes: Vec<crate::types::BezierNode> = nodes
                .iter()
                .map(|n| crate::types::BezierNode {
                    pos: m(n.pos),
                    in_handle: n.in_handle.map(|h| h * view_zoom),
                    out_handle: n.out_handle.map(|h| h * view_zoom),
                    node_type: n.node_type,
                })
                .collect();
            let tess = crate::bezier::tessellate(&mapped_nodes, *closed, 12);
            if tess.len() < 2 {
                return;
            }
            if let Some(fill_color) = fill {
                if *closed {
                    // NOTE: convex_polygon matches the identity rendering path
                    // in canvas.rs draw_annotation (line ~976), which also uses
                    // egui::Shape::convex_polygon for closed BezierPath fill.
                    // Both paths share the same convex-hull limitation for
                    // non-convex bezier shapes; this is NOT a divergence.
                    painter.add(egui::Shape::convex_polygon(
                        tess.clone(),
                        *fill_color,
                        egui::Stroke::NONE,
                    ));
                }
            }
            for w in tess.windows(2) {
                painter.line_segment([w[0], w[1]], stroke);
            }
        }

        // ── ImageLayer: Task 9 — mesh-based affine rendering ────────────────
        ShapeData::ImageLayer { rect, texture, .. } => {
            if let Some(tex) = &texture.0 {
                let mesh = image_mesh(*rect, world, project, tex.id(), layer_opacity);
                painter.add(egui::Shape::mesh(mesh));
            } else {
                // Texture not yet uploaded — render a dark placeholder quad.
                let c = corners_in_screen(*rect, world, project);
                painter.add(egui::Shape::convex_polygon(
                    c.to_vec(),
                    Color32::from_gray(40),
                    Stroke::NONE,
                ));
            }
        }

        // ── Text: Task 9 — rotation-aware TextShape rendering ───────────────
        ShapeData::Text { pos, content, size } => {
            if content.is_empty() {
                return;
            }
            // Map the text origin through the world transform.
            let screen_pos = project(apply(world, *pos));

            // Decompose to extract rotation and scale.
            let d = decompose(world);
            // Scale the font size by scale_x (use average of sx/sy for non-uniform).
            // TextShape only supports a single rotation angle — skew and non-uniform
            // scale are NOT fully honored (known limitation; vector shapes get full
            // affine, but egui text rasterisation cannot be sheared). The rotation
            // and uniform-scale portion of the transform is applied faithfully.
            let scale = ((d.scale_x.abs() + d.scale_y.abs()) * 0.5) as f32;
            let scaled_size = (*size * view_zoom * scale).max(1.0);
            let font_id = egui::FontId::proportional(scaled_size);

            let galley = painter
                .ctx()
                .fonts(|f| f.layout_no_wrap(content.clone(), font_id, ann_color));

            let text_shape = egui::epaint::TextShape::new(screen_pos, galley, ann_color)
                .with_angle(d.rotation as f32);
            painter.add(egui::Shape::Text(text_shape));
        }

        ShapeData::Question { pos, content, size } => {
            // Question bubbles are text-anchored; use transformed origin,
            // render axis-aligned.
            let screen_pos = m(*pos);
            let scaled_size = *size * view_zoom;
            if !content.is_empty() {
                let est_w = content.chars().count() as f32 * scaled_size * 0.6 + 8.0;
                let bb = Rect::from_min_size(screen_pos, egui::Vec2::new(est_w, scaled_size + 6.0));
                let outline = Color32::from_rgba_unmultiplied(
                    ann_color.r(),
                    ann_color.g(),
                    ann_color.b(),
                    ann_color.a() / 2,
                );
                painter.rect_stroke(bb, 3.0, egui::Stroke::new(1.0, outline));
                painter.text(
                    screen_pos + egui::vec2(3.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    content,
                    egui::FontId::proportional(scaled_size),
                    ann_color,
                );
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Affine;

    /// Verify the image mesh has the right shape and that vertex positions match
    /// the transformed corners as computed by `corners_in_screen`.
    #[test]
    fn image_mesh_uses_transformed_corners() {
        let project = |p: egui::Pos2| p;
        let r = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        // Rotate 90° clockwise about origin.
        let world = Affine::rotate(std::f64::consts::FRAC_PI_2);
        let mesh = image_mesh_for_test(r, world, &project);

        // Must have exactly 4 vertices and 6 indices (2 triangles).
        assert_eq!(mesh.vertices.len(), 4, "expected 4 vertices");
        assert_eq!(mesh.indices.len(), 6, "expected 6 indices (2 triangles)");

        // Compute expected corners via `corners_in_screen` and compare.
        let expected = corners_in_screen(r, world, &project);
        for (i, (v, e)) in mesh.vertices.iter().zip(expected.iter()).enumerate() {
            let dx = (v.pos.x - e.x).abs();
            let dy = (v.pos.y - e.y).abs();
            assert!(
                dx < 1e-3 && dy < 1e-3,
                "vertex {i}: got {:?}, expected {:?}",
                v.pos,
                e
            );
        }

        // Verify UV assignments: TL=(0,0), TR=(1,0), BR=(1,1), BL=(0,1).
        let uvs = [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for (i, (v, (u, vv))) in mesh.vertices.iter().zip(uvs.iter()).enumerate() {
            assert!(
                (v.uv.x - u).abs() < 1e-6 && (v.uv.y - vv).abs() < 1e-6,
                "vertex {i} UV: got ({}, {}), expected ({u}, {vv})",
                v.uv.x,
                v.uv.y
            );
        }
    }

    #[test]
    fn maps_rect_corners_through_world_then_project() {
        // project = identity for the test
        let project = |p: egui::Pos2| p;
        let world = Affine::translate((50.0, 0.0));
        let r = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0));
        let corners = corners_in_screen(r, world, &project);
        assert_eq!(corners[0], egui::pos2(50.0, 0.0)); // TL
        assert_eq!(corners[2], egui::pos2(60.0, 10.0)); // BR
    }

    #[test]
    fn identity_world_leaves_corners_unchanged() {
        let project = |p: egui::Pos2| p;
        let world = Affine::IDENTITY;
        let r = egui::Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(15.0, 20.0));
        let corners = corners_in_screen(r, world, &project);
        assert_eq!(corners[0], egui::pos2(5.0, 5.0)); // TL
        assert_eq!(corners[1], egui::pos2(15.0, 5.0)); // TR
        assert_eq!(corners[2], egui::pos2(15.0, 20.0)); // BR
        assert_eq!(corners[3], egui::pos2(5.0, 20.0)); // BL
    }

    #[test]
    fn project_is_applied_after_world() {
        // project offsets x by 100
        let project = |p: egui::Pos2| egui::pos2(p.x + 100.0, p.y);
        let world = Affine::translate((10.0, 0.0));
        let r = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(5.0, 5.0));
        let corners = corners_in_screen(r, world, &project);
        // TL: world(0,0)=(10,0), project=(110,0)
        assert_eq!(corners[0], egui::pos2(110.0, 0.0));
        // BR: world(5,5)=(15,5), project=(115,5)
        assert_eq!(corners[2], egui::pos2(115.0, 5.0));
    }

    /// Regression test for the premultiplied-alpha bug: a translucent shape
    /// fill (e.g. the "Round" preset: straight yellow 255,204,0 @ alpha 20)
    /// must be blended by un-premultiplying first, not by reading `.r()/.g()/.b()`
    /// directly off the stored (premultiplied) `Color32`. Reading the raw
    /// channels would return roughly (20,16,0) — near-black — instead of the
    /// straight (255,204,0) yellow.
    #[test]
    fn blended_fill_unmultiplies_before_recombining_alpha() {
        let straight = Color32::from_rgba_unmultiplied(255, 204, 0, 20);

        // Sanity: reading the raw (premultiplied) channels off `straight`
        // does NOT reproduce the straight sRGB values — this is exactly the
        // trap the old code fell into.
        assert_ne!(straight.r(), 255);
        assert_ne!(straight.g(), 204);

        // `Color32::from_rgba_unmultiplied` bakes its inputs into premultiplied
        // storage immediately (gamma-correct, lossy for alpha < 255), so even
        // the color's own construction literals aren't recoverable byte-exact.
        // Use the color's own `to_srgba_unmultiplied()` view as the oracle
        // (same pattern as export.rs's `new_rect_export_round_trips_fill_and_stroke`).
        let [exp_r, exp_g, exp_b, exp_a] = straight.to_srgba_unmultiplied();

        // No additional attenuation (alpha_mul = 1.0): RGB must match the
        // straight sRGB view exactly, and alpha must be unchanged.
        let fc = blended_fill(straight, 1.0);
        let [r, g, b, a] = fc.to_srgba_unmultiplied();
        assert_eq!((r, g, b, a), (exp_r, exp_g, exp_b, exp_a));

        // With attenuation (e.g. layer_opacity/provisional alpha combined),
        // RGB must still match the straight view (within 2 units — an extra
        // premultiply/unmultiply round trip at low alpha loses a little
        // precision to rounding); only alpha scales.
        let fc_half = blended_fill(straight, 0.5);
        let [r2, g2, b2, a2] = fc_half.to_srgba_unmultiplied();
        assert!(
            (r2 as i16 - exp_r as i16).abs() <= 2,
            "r2={r2} exp_r={exp_r}"
        );
        assert!(
            (g2 as i16 - exp_g as i16).abs() <= 2,
            "g2={g2} exp_g={exp_g}"
        );
        assert!(
            (b2 as i16 - exp_b as i16).abs() <= 2,
            "b2={b2} exp_b={exp_b}"
        );
        assert_eq!(a2, (exp_a as f32 * 0.5).round() as u8);

        // Buggy-code sentinel: the old implementation read `.r()/.g()/.b()`
        // straight off the premultiplied `Color32`, darkening green from 204
        // down to roughly 16. Confirm the fixed helper stays near-straight.
        assert!(
            g > 150,
            "green channel should stay near-straight (204), got {g}"
        );
    }
}
