// src/svg_emit.rs — Annotation → SVG emitter
use egui::Rect;

use crate::state::CanvasState;
use crate::types::{Annotation, ArrowHeads, LineStyle, ShapeData};

fn css_color(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn css_alpha(c: egui::Color32) -> f32 {
    c.a() as f32 / 255.0
}

/// Like `css_color`, but for colors that may carry alpha < 255 (e.g. shape
/// `fill`). `Color32` stores premultiplied components, so `.r()/.g()/.b()`
/// alone would bake the alpha into the hex; unmultiply first so the hex is
/// the straight sRGB color and `fill-opacity` (emitted separately) carries
/// the alpha.
fn css_color_unmultiplied(c: egui::Color32) -> String {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Returns ` transform="matrix(a b c d e f)"` for a non-identity affine, or
/// an empty string for the identity transform (so no stray attribute is emitted).
/// kurbo `Affine::as_coeffs()` returns `[a, b, c, d, e, f]` in the same column-
/// major convention as SVG `matrix(a b c d e f)`, so we emit them verbatim.
fn matrix_attr(a: kurbo::Affine) -> String {
    if a == kurbo::Affine::IDENTITY {
        return String::new();
    }
    let [m0, m1, m2, m3, e, f] = a.as_coeffs();
    format!(" transform=\"matrix({m0} {m1} {m2} {m3} {e} {f})\"")
}

/// Build an arrowhead polygon pointing from `from` toward `to`.
/// `head_len` is the length of the arrowhead in pixels.
fn arrow_head_polygon(
    from: egui::Pos2,
    to: egui::Pos2,
    head_len: f32,
    fill: &str,
    opacity: f32,
) -> String {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        return String::new();
    }
    let ux = dx / len;
    let uy = dy / len;
    // Perpendicular
    let px = -uy;
    let py = ux;
    let half_base = head_len * 0.4;
    // Tip at `to`, base at `to - head_len * unit`
    let base_x = to.x - head_len * ux;
    let base_y = to.y - head_len * uy;
    let p1x = base_x + half_base * px;
    let p1y = base_y + half_base * py;
    let p2x = base_x - half_base * px;
    let p2y = base_y - half_base * py;
    format!(
        "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" fill=\"{fill}\" opacity=\"{opacity:.3}\"/>",
        to.x, to.y, p1x, p1y, p2x, p2y,
    )
}

/// Compute the 5 outer and 5 inner vertices of a star inscribed in `rect`.
fn star_points(rect: Rect) -> String {
    let cx = rect.center().x;
    let cy = rect.center().y;
    let outer_r = rect.width().min(rect.height()) * 0.5;
    let inner_r = outer_r * 0.4;
    let mut pts = Vec::with_capacity(10);
    for i in 0..5 {
        // Outer
        let angle = std::f32::consts::PI * (-0.5 + 2.0 * i as f32 / 5.0);
        pts.push(format!(
            "{:.2},{:.2}",
            cx + outer_r * angle.cos(),
            cy + outer_r * angle.sin()
        ));
        // Inner
        let angle2 = std::f32::consts::PI * (-0.5 + (2.0 * i as f32 + 1.0) / 5.0);
        pts.push(format!(
            "{:.2},{:.2}",
            cx + inner_r * angle2.cos(),
            cy + inner_r * angle2.sin()
        ));
    }
    pts.join(" ")
}

/// Emit one SVG element (or small group) for the annotation. Returns an empty
/// string for variants that have no SVG equivalent. Returns a comment for Blur.
pub fn shape_to_svg_element(ann: &Annotation) -> String {
    let stroke = css_color(ann.color);
    let opacity = css_alpha(ann.color);
    let width = ann.thickness;

    match &ann.shape {
        // ── Rect-family ──────────────────────────────────────────────────────────
        ShapeData::Rect {
            rect,
            radius,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            let fill_attr = match fill {
                Some(fc) => format!(
                    "fill=\"{}\" fill-opacity=\"{:.3}\"",
                    css_color_unmultiplied(*fc),
                    css_alpha(*fc)
                ),
                None => "fill=\"none\"".to_string(),
            };
            let stroke_attr = if *has_stroke {
                let dasharray = match stroke_style {
                    LineStyle::Solid => String::new(),
                    LineStyle::Dashed => " stroke-dasharray=\"8,4\"".to_string(),
                    LineStyle::Dotted => " stroke-dasharray=\"2,4\"".to_string(),
                };
                format!("stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"{dasharray}")
            } else {
                String::new()
            };
            format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" \
                 rx=\"{radius:.2}\" ry=\"{radius:.2}\" {fill_attr} {stroke_attr}/>",
                rect.min.x,
                rect.min.y,
                rect.width(),
                rect.height(),
            )
        }

        // ── Oval-family ──────────────────────────────────────────────────────────
        ShapeData::Oval {
            rect,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            let cx = rect.center().x;
            let cy = rect.center().y;
            let rx = rect.width() * 0.5;
            let ry = rect.height() * 0.5;
            let fill_attr = match fill {
                Some(fc) => format!(
                    "fill=\"{}\" fill-opacity=\"{:.3}\"",
                    css_color_unmultiplied(*fc),
                    css_alpha(*fc)
                ),
                None => "fill=\"none\"".to_string(),
            };
            let stroke_attr = if *has_stroke {
                let dasharray = match stroke_style {
                    LineStyle::Solid => String::new(),
                    LineStyle::Dashed => " stroke-dasharray=\"8,4\"".to_string(),
                    LineStyle::Dotted => " stroke-dasharray=\"2,4\"".to_string(),
                };
                format!("stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"{dasharray}")
            } else {
                String::new()
            };
            format!(
                "<ellipse cx=\"{cx:.2}\" cy=\"{cy:.2}\" rx=\"{rx:.2}\" ry=\"{ry:.2}\" {fill_attr} {stroke_attr}/>",
            )
        }

        // ── Line-family ──────────────────────────────────────────────────────────
        ShapeData::Line { start, end, style } => {
            let dasharray = match style {
                LineStyle::Solid => String::new(),
                LineStyle::Dashed => " stroke-dasharray=\"8,4\"".to_string(),
                LineStyle::Dotted => " stroke-dasharray=\"2,4\"".to_string(),
            };
            format!(
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
                 stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"{dasharray}/>",
                start.x, start.y, end.x, end.y,
            )
        }

        ShapeData::Ruler { start, end } => format!(
            "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
             stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"/>",
            start.x, start.y, end.x, end.y,
        ),

        ShapeData::Arrow {
            start,
            end,
            style,
            heads,
        } => {
            let dasharray = match style {
                LineStyle::Solid => String::new(),
                LineStyle::Dashed => " stroke-dasharray=\"8,4\"".to_string(),
                LineStyle::Dotted => " stroke-dasharray=\"2,4\"".to_string(),
            };
            let head_len = width * 4.0;
            let line = format!(
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" \
                 stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"{dasharray}/>",
                start.x, start.y, end.x, end.y,
            );
            let head_end = arrow_head_polygon(*start, *end, head_len, &stroke, opacity);
            let head_start = match heads {
                ArrowHeads::Double => arrow_head_polygon(*end, *start, head_len, &stroke, opacity),
                ArrowHeads::Single => String::new(),
            };
            format!("{line}{head_end}{head_start}")
        }

        // ── Pen ──────────────────────────────────────────────────────────────────
        ShapeData::Pen {
            raw_points,
            smoothed_points,
        } => {
            let points = if !smoothed_points.is_empty() {
                smoothed_points
            } else {
                raw_points
            };
            if points.is_empty() {
                return String::new();
            }
            let mut d = format!("M {:.2},{:.2}", points[0].x, points[0].y);
            for pt in points.iter().skip(1) {
                d.push_str(&format!(" L {:.2},{:.2}", pt.x, pt.y));
            }
            format!(
                "<path d=\"{d}\" fill=\"none\" stroke=\"{stroke}\" \
                 stroke-width=\"{width}\" stroke-linejoin=\"round\" \
                 stroke-linecap=\"round\" opacity=\"{opacity:.3}\"/>",
            )
        }

        // ── Text-family ──────────────────────────────────────────────────────────
        ShapeData::Text { pos, content, size } => {
            let escaped = escape_xml(content);
            format!(
                "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"{size:.2}\" \
                 fill=\"{stroke}\" opacity=\"{opacity:.3}\">{escaped}</text>",
                pos.x, pos.y,
            )
        }

        ShapeData::Question { pos, content, size } => {
            let escaped = escape_xml(content);
            format!(
                "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"{size:.2}\" \
                 font-style=\"italic\" fill=\"{stroke}\" opacity=\"{opacity:.3}\">{escaped}</text>",
                pos.x, pos.y,
            )
        }

        // ── Star ──────────────────────────────────────────────────────────────────
        ShapeData::Star { rect } => {
            let pts = star_points(*rect);
            format!(
                "<polygon points=\"{pts}\" \
                 fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"/>",
            )
        }

        // ── Callout ───────────────────────────────────────────────────────────────
        ShapeData::Callout { rect, tail } => {
            let rx = 6.0_f32;
            let bubble = format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" \
                 rx=\"{rx:.2}\" ry=\"{rx:.2}\" \
                 fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"/>",
                rect.min.x, rect.min.y, rect.width(), rect.height(),
            );
            // Triangular tail: from two points near the bottom-center of the rect
            // toward the tail tip.
            let cx = rect.center().x;
            let by = rect.max.y;
            let half = (width * 3.0).max(8.0);
            let triangle = format!(
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" \
                 fill=\"{stroke}\" opacity=\"{opacity:.3}\"/>",
                cx - half,
                by,
                cx + half,
                by,
                tail.x,
                tail.y,
            );
            format!("{bubble}{triangle}")
        }

        // ── StepMarker ────────────────────────────────────────────────────────────
        ShapeData::StepMarker {
            pos,
            number,
            radius,
        } => {
            let label = number.to_string();
            let font_size = (radius * 1.0).max(8.0);
            format!(
                "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"{radius:.2}\" \
                 fill=\"{stroke}\" opacity=\"{opacity:.3}\"/>\
                 <text x=\"{:.2}\" y=\"{:.2}\" font-size=\"{font_size:.2}\" \
                 text-anchor=\"middle\" dominant-baseline=\"central\" \
                 fill=\"white\">{label}</text>",
                pos.x, pos.y, pos.x, pos.y,
            )
        }

        // ── ImageLayer ────────────────────────────────────────────────────────────
        ShapeData::ImageLayer { rect, data, .. } => {
            use base64::Engine;
            let mut png_bytes: Vec<u8> = Vec::new();
            {
                use image::ImageEncoder;
                let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
                if encoder
                    .write_image(
                        data.as_raw(),
                        data.width(),
                        data.height(),
                        image::ExtendedColorType::Rgba8,
                    )
                    .is_err()
                {
                    return String::new();
                }
            }
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
            format!(
                "<image x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" \
                 href=\"data:image/png;base64,{b64}\" opacity=\"{opacity:.3}\"/>",
                rect.min.x,
                rect.min.y,
                rect.width(),
                rect.height(),
            )
        }

        // ── Blur ─────────────────────────────────────────────────────────────────
        ShapeData::Blur { .. } => "<!-- Blur omitted: raster-only effect -->".to_string(),

        // ── Authoring helpers with no SVG equivalent ──────────────────────────────
        ShapeData::FocusRegion { .. } | ShapeData::LogicTag { .. } => String::new(),

        // ── BezierPath ────────────────────────────────────────────────────────────
        ShapeData::BezierPath {
            nodes,
            closed,
            fill,
        } => {
            if nodes.is_empty() {
                return String::new();
            }
            let mut d = format!("M {:.2},{:.2}", nodes[0].pos.x, nodes[0].pos.y);
            for i in 0..nodes.len() - 1 {
                let a = &nodes[i];
                let b = &nodes[i + 1];
                let c1 = a.pos + a.out_handle.unwrap_or(egui::Vec2::ZERO);
                let c2 = b.pos + b.in_handle.unwrap_or(egui::Vec2::ZERO);
                d.push_str(&format!(
                    " C {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                    c1.x, c1.y, c2.x, c2.y, b.pos.x, b.pos.y,
                ));
            }
            if *closed {
                let last = nodes.last().unwrap();
                let first = &nodes[0];
                let c1 = last.pos + last.out_handle.unwrap_or(egui::Vec2::ZERO);
                let c2 = first.pos + first.in_handle.unwrap_or(egui::Vec2::ZERO);
                d.push_str(&format!(
                    " C {:.2},{:.2} {:.2},{:.2} {:.2},{:.2} Z",
                    c1.x, c1.y, c2.x, c2.y, first.pos.x, first.pos.y,
                ));
            }
            let fill_attr = match fill {
                Some(c) => format!(
                    "fill=\"{}\" fill-opacity=\"{}\"",
                    css_color(*c),
                    css_alpha(*c)
                ),
                None => "fill=\"none\"".to_string(),
            };
            format!(
                "<path d=\"{d}\" {fill_attr} stroke=\"{stroke}\" stroke-width=\"{width}\" opacity=\"{opacity:.3}\"/>",
            )
        }
    }
}

/// Emit a complete SVG document containing only the annotation layer. Walks the
/// scene tree, emitting one `<g inkscape:groupmode="layer">` per layer with
/// nested group `<g>` elements. Nodes whose OWN `visible` flag is false (and
/// their subtrees) are skipped — matching the prior flat emit's `!visible` skip.
pub fn annotations_to_svg(canvas: &CanvasState, viewport: Rect) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" \
         viewBox=\"{:.2} {:.2} {:.2} {:.2}\">",
        viewport.min.x,
        viewport.min.y,
        viewport.width(),
        viewport.height(),
    ));
    out.push_str("<g id=\"annotations\">");
    let doc = &canvas.document;
    // Emit each layer (direct Layer child of root) in order.
    for layer_id in doc.layers() {
        emit_node(&mut out, doc, layer_id, true);
    }
    out.push_str("</g></svg>");
    out
}

/// Recursively emit a node. `is_layer` controls the inkscape layer attributes.
fn emit_node(
    out: &mut String,
    doc: &crate::scene::Document,
    id: crate::scene::NodeId,
    is_layer: bool,
) {
    let Some(node) = doc.get(id) else { return };
    if !node.visible {
        return; // hidden container/leaf: skip subtree (matches flat emit's !visible skip)
    }
    match &node.kind {
        crate::scene::NodeKind::Shape(_) => {
            if let Some(ann) = doc.leaf_to_annotation(id) {
                let xform = matrix_attr(node.transform);
                if xform.is_empty() {
                    out.push_str(&shape_to_svg_element(&ann));
                } else {
                    out.push_str(&format!("<g{xform}>"));
                    out.push_str(&shape_to_svg_element(&ann));
                    out.push_str("</g>");
                }
            }
        }
        crate::scene::NodeKind::Layer | crate::scene::NodeKind::Group => {
            out.push_str("<g");
            if is_layer {
                out.push_str(&format!(
                    " inkscape:groupmode=\"layer\" inkscape:label=\"{}\"",
                    escape_xml(&node.name)
                ));
            }
            if node.opacity < 1.0 {
                out.push_str(&format!(" opacity=\"{:.3}\"", node.opacity));
            }
            out.push_str(&matrix_attr(node.transform));
            out.push('>');
            for child in &node.children {
                emit_node(out, doc, *child, false);
            }
            out.push_str("</g>");
        }
    }
}

/// Insert the annotation `<g>` group from `annotations_to_svg` into the
/// traced SVG just before the closing `</svg>` tag.
pub fn combine_trace_and_annotations(
    traced_svg: &str,
    canvas: &CanvasState,
    viewport: Rect,
) -> String {
    let ann_doc = annotations_to_svg(canvas, viewport);
    let g_start = ann_doc.find("<g id=\"annotations\"").unwrap_or(0);
    let g_end = ann_doc
        .rfind("</g>")
        .map(|i| i + 4)
        .unwrap_or(ann_doc.len());
    let group = &ann_doc[g_start..g_end];
    if let Some(close_idx) = traced_svg.rfind("</svg>") {
        format!(
            "{}{group}{}",
            &traced_svg[..close_idx],
            &traced_svg[close_idx..]
        )
    } else {
        format!("{traced_svg}{group}</svg>")
    }
}

#[cfg(test)]
mod layer_svg_tests {
    use crate::command::CommandExecutor;
    use crate::state::CanvasState;
    use crate::types::{Annotation, Command, ParticipantId, ShapeData};
    use egui::{Color32, Pos2, Rect, Vec2};

    #[test]
    fn export_emits_layer_group_with_inkscape_attrs() {
        let mut c = CanvasState::default();
        // Rename Layer 1, set opacity, add a rect, then group it.
        let layer = c.document.default_layer();
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id: layer,
                prop: crate::types::NodeProp::Name("Sketch".into()),
            },
        );
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id: layer,
                prop: crate::types::NodeProp::Opacity(0.5),
            },
        );
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        let aid = a.id;
        c.add_annotation(a);
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::Group {
                ids: vec![aid],
                name: "G".into(),
            },
        );

        let svg = crate::svg_emit::annotations_to_svg(
            &c,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
        );
        assert!(svg.contains("xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\""));
        assert!(svg.contains("inkscape:groupmode=\"layer\""));
        assert!(svg.contains("inkscape:label=\"Sketch\""));
        assert!(svg.contains("opacity=\"0.500\"")); // layer opacity
                                                    // The rect lives inside a nested (group) <g> inside the layer <g>.
        let layer_g = svg.find("inkscape:groupmode=\"layer\"").unwrap();
        let rect_at = svg.find("<rect").unwrap();
        assert!(
            rect_at > layer_g,
            "rect must be nested inside the layer group"
        );
    }

    #[test]
    fn export_skips_invisible_layers() {
        let mut c = CanvasState::default();
        let layer = c.document.default_layer();
        let a = Annotation::new(ShapeData::plain_rect(Rect::ZERO), Color32::RED, 2.0);
        c.add_annotation(a);
        c.document.get_mut(layer).unwrap().visible = false;
        c.sync_annotations();
        let svg = crate::svg_emit::annotations_to_svg(
            &c,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(10.0, 10.0)),
        );
        assert!(!svg.contains("<rect"));
    }

    // ── transform="matrix(...)" tests ─────────────────────────────────────────

    #[test]
    fn rotated_leaf_emits_matrix_transform() {
        use kurbo::Affine;
        let mut c = CanvasState::default();
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0))),
            Color32::BLUE,
            1.0,
        );
        let leaf_id = c.add_annotation(a);
        // Apply a rotation to the leaf node's own transform.
        c.document.get_mut(leaf_id).unwrap().transform = Affine::rotate(0.5);
        let svg = crate::svg_emit::annotations_to_svg(
            &c,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
        );
        assert!(
            svg.contains("matrix("),
            "expected a matrix transform in:\n{svg}"
        );
    }

    #[test]
    fn identity_node_emits_no_transform() {
        let c = CanvasState::default();
        // Default canvas has identity transforms on all nodes.
        let svg = crate::svg_emit::annotations_to_svg(
            &c,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
        );
        assert!(
            !svg.contains("transform=\"matrix"),
            "unexpected matrix attr in:\n{svg}"
        );
    }

    #[test]
    fn scaled_group_emits_matrix_transform() {
        use crate::command::CommandExecutor;
        use crate::types::{Command, ParticipantId};
        use kurbo::Affine;
        let mut c = CanvasState::default();
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            1.0,
        );
        let aid = a.id;
        c.add_annotation(a);
        // Wrap the leaf in a group.
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::Group {
                ids: vec![aid],
                name: "ScaledGroup".into(),
            },
        );
        // Find the group node and apply a scale transform.
        let group_id = c
            .document
            .leaves_in_paint_order()
            .first()
            .map(|lv| {
                // Walk up to the parent group.
                c.document.get(lv.id).and_then(|n| n.parent)
            })
            .flatten()
            .expect("group parent");
        c.document.get_mut(group_id).unwrap().transform = Affine::scale(2.0);
        let svg = crate::svg_emit::annotations_to_svg(
            &c,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0)),
        );
        assert!(
            svg.contains("matrix("),
            "expected matrix on scaled group in:\n{svg}"
        );
    }

    // ── translucent fill → straight-sRGB hex (premultiplied-alpha regression) ──

    #[test]
    fn translucent_rect_fill_emits_straight_srgb_hex() {
        // `Color32::from_rgba_unmultiplied` immediately bakes its inputs into
        // premultiplied storage, so the *exact* round-tripped straight-sRGB
        // value can differ from the input by rounding (e.g. 255 -> 254).
        // Assert against the color's own `to_srgba_unmultiplied()` view
        // (what a correct unmultiply must produce) rather than a hardcoded
        // "#ff0000", and separately confirm it's nowhere near the darkened,
        // alpha-baked value a premultiplied `.r()/.g()/.b()` read would give.
        let fill_color = Color32::from_rgba_unmultiplied(255, 0, 0, 120);
        let [exp_r, exp_g, exp_b, _] = fill_color.to_srgba_unmultiplied();
        let expected_hex = format!("#{exp_r:02x}{exp_g:02x}{exp_b:02x}");

        let ann = Annotation::new(
            ShapeData::Rect {
                rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0)),
                radius: 0.0,
                fill: Some(fill_color),
                stroke: false,
                stroke_style: crate::types::LineStyle::Solid,
            },
            Color32::BLACK,
            1.0,
        );
        let svg = crate::svg_emit::shape_to_svg_element(&ann);
        assert!(
            svg.contains(&format!("fill=\"{expected_hex}\"")),
            "expected straight-sRGB fill=\"{expected_hex}\", got:\n{svg}"
        );
        // Guard against the premultiplied-alpha regression this test protects
        // against: a darkened hex like #780000 must never appear.
        assert!(
            !svg.contains("#780000"),
            "fill hex was baked with alpha (premultiplied) in:\n{svg}"
        );
    }
}
