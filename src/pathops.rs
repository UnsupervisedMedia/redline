//! GUI-free boolean path operations over `BezierPath` node lists, backed by
//! `flo_curves` 0.3. All functions take/return `(Vec<BezierNode>, bool /*closed*/)`
//! and operate purely on geometry (no scene-graph / egui dependency), so they are
//! unit-testable in isolation.
//!
//! Booleans operate on CLOSED regions only; open inputs are filtered out by
//! `to_flo` (treated as a no-op rather than coerced shut), so a caller that mixes
//! open and closed paths only ever booleans the closed ones. A 0-contour result
//! (e.g. the intersection of disjoint shapes) returns an empty `Vec`, which the
//! command layer treats as a no-op.

use egui::{Pos2, Vec2};
use flo_curves::bezier::path::{path_add, path_intersect, path_sub, BezierPath, SimpleBezierPath};
use flo_curves::geo::Coord2;
use flo_curves::geo::Coordinate2D;

use crate::types::{BezierNode, NodeType};

/// flo_curves boolean accuracy in image-pixel units. Small relative to typical
/// shape sizes; tune if very large/small canvases show artifacts.
const BOOL_ACCURACY: f64 = 0.01;

fn c2(p: Pos2) -> Coord2 {
    Coord2(p.x as f64, p.y as f64)
}
fn to_pos(c: &Coord2) -> Pos2 {
    Pos2::new(c.x() as f32, c.y() as f32)
}

/// Convert a CLOSED node list to a flo_curves `SimpleBezierPath`.
/// (Booleans require closed regions; callers must pass `closed == true`.)
pub fn nodes_to_simple_path(nodes: &[BezierNode]) -> SimpleBezierPath {
    if nodes.is_empty() {
        return (Coord2(0.0, 0.0), vec![]);
    }
    let n = nodes.len();
    let start = c2(nodes[0].pos);
    let mut segs = Vec::with_capacity(n);
    for i in 0..n {
        let a = &nodes[i];
        let b = &nodes[(i + 1) % n];
        let cp1 = c2(a.pos + a.out_handle.unwrap_or(Vec2::ZERO));
        let cp2 = c2(b.pos + b.in_handle.unwrap_or(Vec2::ZERO));
        let end = c2(b.pos);
        segs.push((cp1, cp2, end));
    }
    (start, segs)
}

/// Convert a flo_curves `SimpleBezierPath` (a single closed contour ending back at
/// its start point) into a closed node list.
pub fn simple_path_to_nodes(path: &SimpleBezierPath) -> (Vec<BezierNode>, bool) {
    let start = to_pos(&path.start_point());
    let segs: Vec<(Coord2, Coord2, Coord2)> = path.points().collect();
    let mut nodes: Vec<BezierNode> = Vec::with_capacity(segs.len());
    nodes.push(BezierNode {
        pos: start,
        in_handle: None,
        out_handle: None,
        node_type: NodeType::Smooth,
    });
    for (i, (cp1, cp2, end)) in segs.iter().enumerate() {
        let cur = nodes.last().unwrap().pos;
        nodes.last_mut().unwrap().out_handle = Some(to_pos(cp1) - cur);
        let end_pos = to_pos(end);
        let is_last = i + 1 == segs.len();
        if is_last && (end_pos - start).length() < 1e-3 {
            // Closing segment returns to start: fold its in_handle onto node 0.
            nodes[0].in_handle = Some(to_pos(cp2) - start);
        } else {
            nodes.push(BezierNode {
                pos: end_pos,
                in_handle: Some(to_pos(cp2) - end_pos),
                out_handle: None,
                node_type: NodeType::Smooth,
            });
        }
    }
    (nodes, true)
}

fn to_flo(inputs: &[(Vec<BezierNode>, bool)]) -> Vec<SimpleBezierPath> {
    inputs
        .iter()
        .filter(|(nodes, closed)| *closed && nodes.len() >= 2)
        .map(|(nodes, _)| nodes_to_simple_path(nodes))
        .collect()
}

fn from_flo(paths: Vec<SimpleBezierPath>) -> Vec<(Vec<BezierNode>, bool)> {
    paths
        .iter()
        .map(simple_path_to_nodes)
        .filter(|(nodes, _)| nodes.len() >= 3)
        .collect()
}

/// Union of all (closed) inputs. ≥0 contours out.
pub fn unite(inputs: &[(Vec<BezierNode>, bool)]) -> Vec<(Vec<BezierNode>, bool)> {
    let flo = to_flo(inputs);
    let mut it = flo.into_iter();
    let Some(first) = it.next() else {
        return Vec::new();
    };
    let mut acc: Vec<SimpleBezierPath> = vec![first];
    for p in it {
        acc = path_add::<_, _, SimpleBezierPath>(&acc, &vec![p], BOOL_ACCURACY);
    }
    from_flo(acc)
}

/// `inputs[0]` minus every later input (paint order = operand order).
pub fn subtract(inputs: &[(Vec<BezierNode>, bool)]) -> Vec<(Vec<BezierNode>, bool)> {
    let flo = to_flo(inputs);
    let mut it = flo.into_iter();
    let Some(first) = it.next() else {
        return Vec::new();
    };
    let mut acc: Vec<SimpleBezierPath> = vec![first];
    for p in it {
        acc = path_sub::<_, _, SimpleBezierPath>(&acc, &vec![p], BOOL_ACCURACY);
    }
    from_flo(acc)
}

/// Intersection of all inputs.
pub fn intersect(inputs: &[(Vec<BezierNode>, bool)]) -> Vec<(Vec<BezierNode>, bool)> {
    let flo = to_flo(inputs);
    let mut it = flo.into_iter();
    let Some(first) = it.next() else {
        return Vec::new();
    };
    let mut acc: Vec<SimpleBezierPath> = vec![first];
    for p in it {
        acc = path_intersect::<_, _, SimpleBezierPath>(&acc, &vec![p], BOOL_ACCURACY);
    }
    from_flo(acc)
}

/// Exclude / symmetric difference: (A ∪ B) − (A ∩ B). For ≥3 inputs, fold pairwise.
pub fn exclude(inputs: &[(Vec<BezierNode>, bool)]) -> Vec<(Vec<BezierNode>, bool)> {
    let flo = to_flo(inputs);
    if flo.len() < 2 {
        return from_flo(flo);
    }
    let mut acc: Vec<SimpleBezierPath> = vec![flo[0].clone()];
    for p in flo.into_iter().skip(1) {
        let other = vec![p];
        let union = path_add::<_, _, SimpleBezierPath>(&acc, &other, BOOL_ACCURACY);
        let inter = path_intersect::<_, _, SimpleBezierPath>(&acc, &other, BOOL_ACCURACY);
        acc = path_sub::<_, _, SimpleBezierPath>(&union, &inter, BOOL_ACCURACY);
    }
    from_flo(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;
    use kurbo::{CubicBez, ParamCurve, Point};

    /// Closed rectangle as 4 corner nodes (no handles → straight edges).
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> (Vec<BezierNode>, bool) {
        let mk = |x, y| BezierNode {
            pos: pos2(x, y),
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        };
        (vec![mk(x0, y0), mk(x1, y0), mk(x1, y1), mk(x0, y1)], true)
    }

    /// Tessellate one closed contour to a polyline and return its absolute shoelace area.
    fn contour_area(nodes: &[BezierNode]) -> f64 {
        let n = nodes.len();
        let mut pts: Vec<Point> = Vec::new();
        for i in 0..n {
            let a = &nodes[i];
            let b = &nodes[(i + 1) % n];
            let c = CubicBez::new(
                Point::new(a.pos.x as f64, a.pos.y as f64),
                Point::new(
                    (a.pos + a.out_handle.unwrap_or_default()).x as f64,
                    (a.pos + a.out_handle.unwrap_or_default()).y as f64,
                ),
                Point::new(
                    (b.pos + b.in_handle.unwrap_or_default()).x as f64,
                    (b.pos + b.in_handle.unwrap_or_default()).y as f64,
                ),
                Point::new(b.pos.x as f64, b.pos.y as f64),
            );
            for k in 0..16 {
                pts.push(c.eval(k as f64 / 16.0));
            }
        }
        let mut a2 = 0.0;
        for i in 0..pts.len() {
            let p = pts[i];
            let q = pts[(i + 1) % pts.len()];
            a2 += p.x * q.y - q.x * p.y;
        }
        (a2 / 2.0).abs()
    }

    fn total_area(result: &[(Vec<BezierNode>, bool)]) -> f64 {
        result.iter().map(|(n, _)| contour_area(n)).sum()
    }

    #[test]
    fn union_of_overlapping_squares_is_one_contour_area_between_max_and_sum() {
        let a = rect(0.0, 0.0, 10.0, 10.0); // area 100
        let b = rect(5.0, 5.0, 15.0, 15.0); // area 100, overlap 25
        let out = unite(&[a.clone(), b.clone()]);
        assert_eq!(out.len(), 1, "overlapping union = single contour");
        let area = total_area(&out);
        assert!(area > 100.0 - 1.0 && area < 200.0 + 1.0, "area={area}");
        assert!(
            (area - 175.0).abs() < 2.0,
            "union area ≈ 100+100-25=175, got {area}"
        );
    }

    #[test]
    fn union_of_disjoint_squares_is_two_contours() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(50.0, 50.0, 60.0, 60.0);
        let out = unite(&[a, b]);
        assert_eq!(out.len(), 2, "disjoint union = two contours");
        assert!((total_area(&out) - 200.0).abs() < 2.0);
    }

    #[test]
    fn intersection_is_at_most_the_smaller_input() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 15.0, 15.0);
        let out = intersect(&[a, b]);
        assert_eq!(out.len(), 1);
        assert!((total_area(&out) - 25.0).abs() < 2.0, "A∩B ≈ 25");
    }

    #[test]
    fn intersection_of_disjoint_is_empty() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(50.0, 50.0, 60.0, 60.0);
        assert!(intersect(&[a, b]).is_empty());
    }

    #[test]
    fn subtract_removes_overlap_from_first_operand() {
        let a = rect(0.0, 0.0, 10.0, 10.0); // base, area 100
        let b = rect(5.0, 5.0, 15.0, 15.0); // overlap 25
        let out = subtract(&[a, b]);
        assert!((total_area(&out) - 75.0).abs() < 2.0, "A−B ≈ 75");
        assert!(total_area(&out) <= 100.0 + 1.0);
    }

    #[test]
    fn exclude_is_union_minus_intersection() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 15.0, 15.0);
        let out = exclude(&[a, b]);
        // (175) − (25) = 150
        assert!(
            (total_area(&out) - 150.0).abs() < 3.0,
            "A⊕B ≈ 150, got {}",
            total_area(&out)
        );
    }

    #[test]
    fn roundtrip_square_preserves_area() {
        let (nodes, _) = rect(0.0, 0.0, 10.0, 10.0);
        let sp = nodes_to_simple_path(&nodes);
        let (back, closed) = simple_path_to_nodes(&sp);
        assert!(closed);
        assert!((contour_area(&back) - 100.0).abs() < 1.0);
    }

    #[test]
    fn nodes_to_simple_path_empty_does_not_panic() {
        let _ = nodes_to_simple_path(&[]);
    }

    #[test]
    fn open_inputs_are_filtered_out() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let open = {
            let mut o = rect(5.0, 5.0, 15.0, 15.0);
            o.1 = false;
            o
        };
        // Only one closed input survives → union of a single shape is itself.
        let out = unite(&[a, open]);
        assert_eq!(out.len(), 1);
        assert!((total_area(&out) - 100.0).abs() < 1.0);
    }
}
