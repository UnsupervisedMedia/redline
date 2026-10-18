use egui::{pos2, vec2, Color32, Rect};
use redline::{
    state::CanvasState,
    svg_emit::{annotations_to_svg, combine_trace_and_annotations, shape_to_svg_element},
    types::{Annotation, ArrowHeads, LineStyle, ShapeData},
};

fn rect_annotation() -> Annotation {
    Annotation::new(
        ShapeData::plain_rect(Rect::from_min_size(pos2(10.0, 20.0), vec2(30.0, 40.0))),
        Color32::from_rgb(0xff, 0x00, 0x00),
        2.0,
    )
}

#[test]
fn rect_emits_svg_rect_element() {
    let s = shape_to_svg_element(&rect_annotation());
    assert!(s.contains("<rect"), "missing <rect: {s}");
    assert!(s.contains("x=\"10.00\""), "missing x: {s}");
    assert!(s.contains("y=\"20.00\""), "missing y: {s}");
    assert!(s.contains("width=\"30.00\""), "missing width: {s}");
    assert!(s.contains("height=\"40.00\""), "missing height: {s}");
    assert!(s.contains("stroke=\"#ff0000\""), "missing stroke: {s}");
}

#[test]
fn empty_canvas_emits_valid_empty_svg() {
    let canvas = CanvasState::default();
    let svg = annotations_to_svg(
        &canvas,
        Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)),
    );
    assert!(svg.starts_with("<svg"), "must start with <svg: {svg}");
    assert!(svg.ends_with("</svg>"), "must end with </svg>: {svg}");
    // The default document has a single empty "Layer 1"; export now nests each
    // layer in its own <g inkscape:groupmode="layer"> inside the annotations group.
    assert!(
        svg.contains("<g id=\"annotations\"><g inkscape:groupmode=\"layer\" inkscape:label=\"Layer 1\"></g></g>"),
        "must contain empty layer group: {svg}"
    );
    assert!(
        svg.contains("xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\""),
        "must declare inkscape namespace: {svg}"
    );
}

#[test]
fn blur_emits_comment_placeholder() {
    let ann = Annotation::new(
        ShapeData::Blur {
            rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)),
            block_size: 4.0,
        },
        Color32::TRANSPARENT,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("Blur"), "missing Blur keyword: {s}");
    assert!(s.starts_with("<!--"), "must be a comment: {s}");
}

#[test]
fn line_solid_emits_no_dasharray() {
    let ann = Annotation::new(
        ShapeData::Line {
            start: pos2(0.0, 0.0),
            end: pos2(10.0, 10.0),
            style: LineStyle::Solid,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("<line"), "missing <line: {s}");
    assert!(
        !s.contains("stroke-dasharray"),
        "solid line must not have dasharray: {s}"
    );
}

#[test]
fn line_dashed_emits_dasharray() {
    let ann = Annotation::new(
        ShapeData::Line {
            start: pos2(0.0, 0.0),
            end: pos2(10.0, 10.0),
            style: LineStyle::Dashed,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(
        s.contains("stroke-dasharray=\"8,4\""),
        "dashed line must have dasharray: {s}"
    );
}

#[test]
fn arrow_single_head_emits_one_polygon() {
    let ann = Annotation::new(
        ShapeData::Arrow {
            start: pos2(0.0, 0.0),
            end: pos2(100.0, 0.0),
            style: LineStyle::Solid,
            heads: ArrowHeads::Single,
        },
        Color32::BLACK,
        2.0,
    );
    let s = shape_to_svg_element(&ann);
    assert_eq!(
        s.matches("<polygon").count(),
        1,
        "single-head arrow must have exactly 1 polygon: {s}"
    );
    assert!(s.contains("<line"), "must have a line element: {s}");
}

#[test]
fn arrow_double_head_emits_two_polygons() {
    let ann = Annotation::new(
        ShapeData::Arrow {
            start: pos2(0.0, 0.0),
            end: pos2(100.0, 0.0),
            style: LineStyle::Solid,
            heads: ArrowHeads::Double,
        },
        Color32::BLACK,
        2.0,
    );
    let s = shape_to_svg_element(&ann);
    assert_eq!(
        s.matches("<polygon").count(),
        2,
        "double-head arrow must have exactly 2 polygons: {s}"
    );
}

#[test]
fn text_escapes_xml_special_chars() {
    let ann = Annotation::new(
        ShapeData::Text {
            pos: pos2(0.0, 0.0),
            content: "<a&b>".to_string(),
            size: 12.0,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(
        s.contains("&lt;a&amp;b&gt;"),
        "expected escaped XML, got: {s}"
    );
}

#[test]
fn combine_trace_and_annotations_inserts_group_before_closing_svg() {
    let mut canvas = CanvasState::default();
    canvas.canvas_size = vec2(100.0, 100.0);
    // Route through the document (authoritative store) — annotations_to_svg now
    // walks the scene tree, so pushing to the mirror alone would not export.
    canvas.add_annotation(rect_annotation());
    let traced = "<svg viewBox=\"0 0 100 100\"><path d=\"M 0,0\"/></svg>";
    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
    let combined = combine_trace_and_annotations(traced, &canvas, viewport);
    let g_idx = combined
        .find("<g id=\"annotations\"")
        .expect("group must be present");
    let close_idx = combined.find("</svg>").expect("svg close must be present");
    assert!(
        g_idx < close_idx,
        "group must come before closing </svg>: {combined}"
    );
    assert!(
        combined.contains("<path d=\"M 0,0\"/>"),
        "original path must be preserved: {combined}"
    );
    assert!(
        combined.contains("<rect"),
        "annotation rect must be present: {combined}"
    );
}

#[test]
fn invisible_annotation_is_skipped() {
    let mut canvas = CanvasState::default();
    let mut ann = rect_annotation();
    ann.visible = false;
    // Route through the document so annotations_to_svg (tree walk) sees it; the
    // leaf's own visible=false must cause it to be skipped.
    canvas.add_annotation(ann);
    let svg = annotations_to_svg(
        &canvas,
        Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)),
    );
    assert!(
        !svg.contains("<rect"),
        "invisible annotation must be omitted: {svg}"
    );
}

#[test]
fn rounded_rect_emits_rx_ry() {
    let ann = Annotation::new(
        ShapeData::Rect {
            rect: Rect::from_min_size(pos2(5.0, 5.0), vec2(20.0, 20.0)),
            radius: 4.0,
            fill: Some(Color32::from_rgba_unmultiplied(0, 0, 0, 20)),
            stroke: true,
            stroke_style: LineStyle::Solid,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("rx=\"4.00\""), "must have rx: {s}");
    assert!(s.contains("ry=\"4.00\""), "must have ry: {s}");
}

#[test]
fn redact_emits_filled_rect() {
    let ann = Annotation::new(
        ShapeData::Rect {
            rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)),
            radius: 0.0,
            fill: Some(Color32::from_rgba_unmultiplied(0, 0, 0, 255)),
            stroke: false,
            stroke_style: LineStyle::Solid,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("fill=\"#000000\""), "redact must use fill: {s}");
    assert!(
        !s.contains("fill=\"none\""),
        "redact must not be empty fill: {s}"
    );
    assert!(
        !s.contains("stroke=\"#"),
        "redact must have no stroke attribute: {s}"
    );
}

#[test]
fn step_marker_emits_circle_and_text() {
    let ann = Annotation::new(
        ShapeData::StepMarker {
            pos: pos2(50.0, 50.0),
            number: 3,
            radius: 16.0,
        },
        Color32::from_rgb(0, 120, 255),
        2.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("<circle"), "must have circle: {s}");
    assert!(s.contains("<text"), "must have text: {s}");
    assert!(s.contains("3"), "must include step number: {s}");
}

#[test]
fn pen_emits_path_element() {
    let ann = Annotation::new(
        ShapeData::Pen {
            raw_points: vec![pos2(0.0, 0.0), pos2(10.0, 10.0), pos2(20.0, 5.0)],
            smoothed_points: vec![],
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("<path"), "pen must emit a path: {s}");
    assert!(s.contains("M "), "path must start with M: {s}");
    assert!(s.contains(" L "), "path must have L segments: {s}");
}

#[test]
fn pen_prefers_smoothed_points() {
    let ann = Annotation::new(
        ShapeData::Pen {
            raw_points: vec![pos2(0.0, 0.0), pos2(999.0, 999.0)],
            smoothed_points: vec![pos2(1.0, 2.0), pos2(3.0, 4.0)],
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("1.00,2.00"), "must use smoothed points: {s}");
    assert!(
        !s.contains("999.00"),
        "must not use raw points when smoothed present: {s}"
    );
}

#[test]
fn bezier_open_emits_path_without_z() {
    use redline::types::{BezierNode, NodeType};
    let nodes = vec![
        BezierNode {
            pos: pos2(0.0, 0.0),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        },
        BezierNode {
            pos: pos2(10.0, 0.0),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        },
    ];
    let ann = Annotation::new(
        ShapeData::BezierPath {
            nodes,
            closed: false,
            fill: None,
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.contains("M 0.00,0.00"));
    assert!(s.contains("C "));
    assert!(!s.contains(" Z"));
    assert!(s.contains("fill=\"none\""));
}

#[test]
fn bezier_closed_emits_path_with_z() {
    use redline::types::{BezierNode, NodeType};
    let nodes = vec![
        BezierNode {
            pos: pos2(0.0, 0.0),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        },
        BezierNode {
            pos: pos2(10.0, 0.0),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        },
        BezierNode {
            pos: pos2(0.0, 10.0),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        },
    ];
    let ann = Annotation::new(
        ShapeData::BezierPath {
            nodes,
            closed: true,
            fill: Some(Color32::from_rgb(0x10, 0x20, 0x30)),
        },
        Color32::BLACK,
        1.0,
    );
    let s = shape_to_svg_element(&ann);
    assert!(s.ends_with("/>"));
    assert!(s.contains(" Z"));
    assert!(s.contains("fill=\"#102030\""));
}
