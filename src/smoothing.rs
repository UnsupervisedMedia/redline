use egui::Pos2;

/// Catmull-Rom spline through `points`.
///
/// `segments_per_span` controls smoothness (8–16 is typical for drawing).
/// Returns an interpolated point sequence including the final input point.
pub fn catmull_rom(points: &[Pos2], segments_per_span: usize) -> Vec<Pos2> {
    if points.len() < 2 {
        return points.to_vec();
    }

    // Extend with phantom endpoints by reflecting the boundary tangents
    let mut pts: Vec<Pos2> = Vec::with_capacity(points.len() + 2);
    pts.push(points[0] + (points[0] - points[1])); // phantom start
    pts.extend_from_slice(points);
    let n = points.len();
    pts.push(points[n - 1] + (points[n - 1] - points[n - 2])); // phantom end

    let mut result = Vec::new();
    let n_spans = pts.len() - 3;

    for i in 0..n_spans {
        let (p0, p1, p2, p3) = (pts[i], pts[i + 1], pts[i + 2], pts[i + 3]);
        for j in 0..segments_per_span {
            let t = j as f32 / segments_per_span as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let x = 0.5
                * (2.0 * p1.x
                    + (-p0.x + p2.x) * t
                    + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
                    + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3);
            let y = 0.5
                * (2.0 * p1.y
                    + (-p0.y + p2.y) * t
                    + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
                    + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
            result.push(Pos2::new(x, y));
        }
    }

    // Append the exact final input point
    result.push(*points.last().unwrap());
    result
}
