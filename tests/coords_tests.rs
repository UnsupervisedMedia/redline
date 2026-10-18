use egui::{pos2, vec2, Rect};
use redline::types::NormalizedRect;

#[test]
fn full_canvas_normalizes_to_1000() {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0));
    let canvas = vec2(1920.0, 1080.0);
    let n = NormalizedRect::from_rect(rect, canvas);
    assert_eq!(n.x, 0.0);
    assert_eq!(n.y, 0.0);
    assert_eq!(n.w, 1000.0);
    assert_eq!(n.h, 1000.0);
}

#[test]
fn center_half_canvas_normalizes_correctly() {
    let rect = Rect::from_min_size(pos2(480.0, 270.0), vec2(960.0, 540.0));
    let canvas = vec2(1920.0, 1080.0);
    let n = NormalizedRect::from_rect(rect, canvas);
    assert_eq!(n.x, 250.0);
    assert_eq!(n.y, 250.0);
    assert_eq!(n.w, 500.0);
    assert_eq!(n.h, 500.0);
}
