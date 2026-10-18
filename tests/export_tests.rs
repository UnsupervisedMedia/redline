use egui::{pos2, vec2, Color32, Rect};
use redline::{
    export::{build_sidecar, export_to_annotation},
    state::CanvasState,
    types::{Annotation, ShapeData, SidecarExport, TextureCell},
};
use std::sync::Arc;

fn canvas_with_rect() -> CanvasState {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    c.annotations.push(Annotation::new(
        ShapeData::plain_rect(Rect::from_min_size(pos2(100.0, 200.0), vec2(300.0, 150.0))),
        Color32::RED,
        2.0,
    ));
    c
}

#[test]
fn sidecar_has_correct_canvas_size() {
    let canvas = canvas_with_rect();
    let export = build_sidecar(&canvas);
    assert_eq!(export.canvas_size, [1000.0, 1000.0]);
}

#[test]
fn sidecar_annotation_normalized_coords() {
    let canvas = canvas_with_rect();
    let export = build_sidecar(&canvas);
    let ann = &export.annotations[0];
    let coords = ann.coords.as_ref().unwrap();
    assert_eq!(coords.x, 100.0);
    assert_eq!(coords.y, 200.0);
    assert_eq!(coords.w, 300.0);
    assert_eq!(coords.h, 150.0);
}

#[test]
fn sidecar_roundtrips_json() {
    let canvas = canvas_with_rect();
    let export = build_sidecar(&canvas);
    let json = serde_json::to_string(&export).unwrap();
    let back: SidecarExport = serde_json::from_str(&json).unwrap();
    assert_eq!(back.canvas_size, export.canvas_size);
    assert_eq!(back.annotations.len(), export.annotations.len());
}

#[test]
fn clipboard_text_mentions_rect() {
    let canvas = canvas_with_rect();
    let export = build_sidecar(&canvas);
    assert!(
        export.clipboard_text.contains("rect"),
        "text: {}",
        export.clipboard_text
    );
}

#[test]
fn sidecar_roundtrips_rect_annotation() {
    let canvas = canvas_with_rect();
    let exported = build_sidecar(&canvas);
    let ae = &exported.annotations[0];
    let ann = export_to_annotation(ae).expect("roundtrip");
    let r = ann.shape.bounding_rect();
    assert!((r.min.x - 100.0).abs() < 0.001);
    assert!((r.width() - 300.0).abs() < 0.001);
}

#[test]
fn sidecar_roundtrips_text_annotation() {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    c.annotations.push(Annotation::new(
        ShapeData::Text {
            pos: pos2(50.0, 60.0),
            content: "hello".into(),
            size: 20.0,
        },
        Color32::WHITE,
        1.0,
    ));
    let exported = build_sidecar(&c);
    let ae = &exported.annotations[0];
    assert_eq!(ae.kind, "text");
    assert_eq!(ae.content.as_deref(), Some("hello"));
    let ann = export_to_annotation(ae).expect("roundtrip");
    match ann.shape {
        ShapeData::Text { content, size, .. } => {
            assert_eq!(content, "hello");
            assert!((size - 20.0).abs() < 0.001);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn sidecar_roundtrips_blur_annotation() {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    c.annotations.push(Annotation::new(
        ShapeData::Blur {
            rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 50.0)),
            block_size: 16.0,
        },
        Color32::BLACK,
        1.0,
    ));
    let ae = &build_sidecar(&c).annotations[0];
    let ann = export_to_annotation(ae).expect("roundtrip");
    match ann.shape {
        ShapeData::Blur { block_size, rect } => {
            assert!((block_size - 16.0).abs() < 0.001);
            assert!((rect.width() - 100.0).abs() < 0.001);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn sidecar_roundtrips_pen_points() {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    let pts = vec![
        pos2(0.0, 0.0),
        pos2(10.0, 5.0),
        pos2(20.0, 8.0),
        pos2(30.0, 4.0),
    ];
    c.annotations.push(Annotation::new(
        ShapeData::Pen {
            smoothed_points: pts.clone(),
            raw_points: pts.clone(),
        },
        Color32::RED,
        3.0,
    ));
    let ae = &build_sidecar(&c).annotations[0];
    let ann = export_to_annotation(ae).expect("roundtrip");
    match ann.shape {
        ShapeData::Pen { raw_points, .. } => assert_eq!(raw_points.len(), 4),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn sidecar_roundtrips_star_annotation() {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    c.annotations.push(Annotation::new(
        ShapeData::Star {
            rect: Rect::from_min_size(pos2(120.0, 140.0), vec2(180.0, 160.0)),
        },
        Color32::from_rgb(255, 215, 0),
        4.0,
    ));
    let ae = &build_sidecar(&c).annotations[0];
    assert_eq!(ae.kind, "star");
    let ann = export_to_annotation(ae).expect("roundtrip");
    match ann.shape {
        ShapeData::Star { rect } => {
            assert!((rect.min.x - 120.0).abs() < 0.001);
            assert!((rect.width() - 180.0).abs() < 0.001);
            assert!((rect.height() - 160.0).abs() < 0.001);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn sidecar_roundtrips_image_layer_annotation() {
    let mut c = CanvasState::default();
    c.canvas_size = vec2(1000.0, 1000.0);
    let image = image::RgbaImage::from_pixel(24, 24, image::Rgba([255, 0, 0, 255]));
    c.annotations.push(Annotation::new(
        ShapeData::ImageLayer {
            rect: Rect::from_min_size(pos2(200.0, 220.0), vec2(48.0, 48.0)),
            data: Arc::new(image),
            texture: TextureCell::default(),
        },
        Color32::RED,
        1.0,
    ));
    let ae = &build_sidecar(&c).annotations[0];
    assert_eq!(ae.kind, "image_layer");
    let ann = export_to_annotation(ae).expect("roundtrip");
    match ann.shape {
        ShapeData::ImageLayer { rect, data, .. } => {
            assert!((rect.min.x - 200.0).abs() < 0.001);
            assert_eq!(data.width(), 24);
            assert_eq!(data.height(), 24);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn sidecar_round_trips_human_authored_annotation() {
    use redline::types::ParticipantId;
    let ann = Annotation::new(
        ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(10.0, 10.0),
        )),
        Color32::RED,
        1.0,
    );
    assert_eq!(ann.author, ParticipantId::HUMAN_LOCAL);

    let mut canvas = CanvasState::default();
    canvas.canvas_size = egui::vec2(100.0, 100.0);
    canvas.annotations.push(ann);

    let exported = redline::export::build_sidecar(&canvas);
    let json = serde_json::to_string(&exported).unwrap();
    let parsed: redline::types::SidecarExport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.annotations.len(), 1);
    // Author should be serialized as "human-local" for the human user
    assert_eq!(parsed.annotations[0].author.as_deref(), Some("human-local"));

    // Reverse: deserialize and confirm the round-tripped annotation has HUMAN_LOCAL author
    let imported = redline::export::export_to_annotation(&parsed.annotations[0]).unwrap();
    assert_eq!(imported.author, ParticipantId::HUMAN_LOCAL);
}

#[test]
fn sidecar_loads_old_format_without_author_as_human_local() {
    use redline::types::ParticipantId;
    // Construct an AnnotationExport manually with author=None to simulate
    // an old-format sidecar that pre-dates the author field.
    let old_export = redline::types::AnnotationExport {
        id: "00000000-0000-0000-0000-000000000001".to_string(),
        kind: "rect".to_string(),
        coords: None,
        color: "#ff0000".to_string(),
        label: None,
        tag: None,
        author: None,
        thickness: Some(1.0),
        start: None,
        end: None,
        points: None,
        pos: None,
        rect: Some([0.0, 0.0, 10.0, 10.0]),
        content: None,
        text_size: None,
        block_size: None,
        style: None,
        heads: None,
        number: None,
        radius: None,
        fill: None,
        fill_opacity: None,
        stroke: None,
        tail: None,
        image_png: None,
        transform: None,
    };
    let imported =
        redline::export::export_to_annotation(&old_export).expect("rect export should reconstruct");
    assert_eq!(imported.author, ParticipantId::HUMAN_LOCAL);
}
