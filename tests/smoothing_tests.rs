use egui::pos2;
use redline::smoothing::catmull_rom;

#[test]
fn fewer_than_two_points_returns_input() {
    let pts = vec![pos2(0.0, 0.0)];
    let result = catmull_rom(&pts, 10);
    assert_eq!(result, pts);
}

#[test]
fn two_points_produces_intermediate_samples() {
    let pts = vec![pos2(0.0, 0.0), pos2(10.0, 0.0)];
    let result = catmull_rom(&pts, 5);
    // 5 samples per span + final point = 6 points
    assert_eq!(result.len(), 6);
    // all y values should be ~0 for a horizontal line
    for p in &result {
        assert!((p.y).abs() < 0.001, "y={} should be ~0", p.y);
    }
    // x values should be monotonically increasing
    for w in result.windows(2) {
        assert!(w[1].x >= w[0].x, "x should increase: {:?}", w);
    }
}

#[test]
fn collinear_points_stay_on_line() {
    // Three collinear points — spline should not deviate
    let pts = vec![pos2(0.0, 0.0), pos2(5.0, 5.0), pos2(10.0, 10.0)];
    let result = catmull_rom(&pts, 4);
    for p in &result {
        let deviation = (p.y - p.x).abs();
        assert!(deviation < 0.5, "deviation={} for point {:?}", deviation, p);
    }
}

#[test]
fn last_point_is_preserved() {
    let pts = vec![pos2(0.0, 0.0), pos2(5.0, 3.0), pos2(10.0, 1.0)];
    let result = catmull_rom(&pts, 6);
    let last = result.last().unwrap();
    assert!((last.x - 10.0).abs() < 0.001);
    assert!((last.y - 1.0).abs() < 0.001);
}
