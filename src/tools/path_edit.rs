//! Pure, GUI-free path geometry used by the freehand pen (`tools/pen.rs`) and
//! the Select tool's path-weld feature.
//!
//! Everything here operates in image-pixel space and has no egui-input or
//! rendering dependency, so it can be unit-tested in isolation.

use egui::Pos2;
use kurbo::{BezPath, CurveFitSample, ParamCurveFit, PathEl, Point, Vec2 as KVec2};

use crate::types::{BezierNode, NodeType};

fn to_kpt(p: Pos2) -> Point {
    Point::new(p.x as f64, p.y as f64)
}
fn to_pos(p: Point) -> Pos2 {
    Pos2::new(p.x as f32, p.y as f32)
}

/// Ramer–Douglas–Peucker polyline simplification (px tolerance). Keeps the
/// first and last points. `eps` is the max perpendicular deviation allowed.
fn rdp(pts: &[Pos2], eps: f32) -> Vec<Pos2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    rdp_rec(pts, 0, pts.len() - 1, eps, &mut keep);
    pts.iter()
        .zip(keep.iter())
        .filter_map(|(p, &k)| if k { Some(*p) } else { None })
        .collect()
}

fn rdp_rec(pts: &[Pos2], first: usize, last: usize, eps: f32, keep: &mut [bool]) {
    if last <= first + 1 {
        return;
    }
    let a = pts[first];
    let b = pts[last];
    let ab = b - a;
    let ab_len2 = ab.x * ab.x + ab.y * ab.y;
    let mut max_d = 0.0_f32;
    let mut max_i = first;
    for (i, p) in pts.iter().enumerate().take(last).skip(first + 1) {
        let d = if ab_len2 < f32::EPSILON {
            (*p - a).length()
        } else {
            // perpendicular distance from p to segment a-b
            let ap = *p - a;
            let cross = ap.x * ab.y - ap.y * ab.x;
            cross.abs() / ab_len2.sqrt()
        };
        if d > max_d {
            max_d = d;
            max_i = i;
        }
    }
    if max_d > eps {
        keep[max_i] = true;
        rdp_rec(pts, first, max_i, eps, keep);
        rdp_rec(pts, max_i, last, eps, keep);
    }
}

/// A `ParamCurveFit` source over a polyline, parameterised by normalised
/// cumulative chord length. Standard polyline → fittable-curve shim.
struct PolyFit {
    pts: Vec<Point>,
    /// Cumulative chord length, normalised to 0..=1.
    cum: Vec<f64>,
}

impl PolyFit {
    fn new(pts: Vec<Point>) -> Self {
        let mut cum = Vec::with_capacity(pts.len());
        let mut total = 0.0;
        cum.push(0.0);
        for w in pts.windows(2) {
            total += (w[1] - w[0]).hypot();
            cum.push(total);
        }
        if total > 0.0 {
            for c in cum.iter_mut() {
                *c /= total;
            }
        }
        Self { pts, cum }
    }

    /// Position + tangent at normalised parameter `t` (0..=1).
    fn at(&self, t: f64) -> (Point, KVec2) {
        let t = t.clamp(0.0, 1.0);
        // Locate the span containing t.
        let mut seg = 0;
        while seg + 1 < self.cum.len() && self.cum[seg + 1] < t {
            seg += 1;
        }
        if seg + 1 >= self.pts.len() {
            seg = self.pts.len().saturating_sub(2);
        }
        let t0 = self.cum[seg];
        let t1 = self.cum[seg + 1];
        let local = if (t1 - t0).abs() < 1e-12 {
            0.0
        } else {
            (t - t0) / (t1 - t0)
        };
        let a = self.pts[seg];
        let b = self.pts[seg + 1];
        let p = a.lerp(b, local);
        let tan = b - a;
        let tan = if tan.hypot() < 1e-9 {
            KVec2::new(1.0, 0.0)
        } else {
            tan
        };
        (p, tan)
    }
}

impl ParamCurveFit for PolyFit {
    fn sample_pt_tangent(&self, t: f64, _sign: f64) -> CurveFitSample {
        let (p, tangent) = self.at(t);
        CurveFitSample { p, tangent }
    }
    fn sample_pt_deriv(&self, t: f64) -> (Point, KVec2) {
        self.at(t)
    }
    fn break_cusp(&self, _range: std::ops::Range<f64>) -> Option<f64> {
        None
    }
}

/// Convert a fitted `BezPath` (MoveTo + CurveTo*) into `BezierNode`s with
/// in/out handles. Each `CurveTo(c1, c2, p)` sets the previous node's
/// `out_handle = c1 - prev.pos` and the new node's `in_handle = c2 - p`.
fn bezpath_to_nodes(bez: &BezPath) -> Vec<BezierNode> {
    let mut nodes: Vec<BezierNode> = Vec::new();
    for el in bez.elements() {
        match el {
            PathEl::MoveTo(p) => {
                nodes.push(BezierNode {
                    pos: to_pos(*p),
                    in_handle: None,
                    out_handle: None,
                    node_type: NodeType::Smooth,
                });
            }
            PathEl::LineTo(p) => {
                nodes.push(BezierNode {
                    pos: to_pos(*p),
                    in_handle: None,
                    out_handle: None,
                    node_type: NodeType::Corner,
                });
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(prev) = nodes.last_mut() {
                    prev.out_handle = Some(to_pos(*c1) - prev.pos);
                }
                let pos = to_pos(*p);
                nodes.push(BezierNode {
                    pos,
                    in_handle: Some(to_pos(*c2) - pos),
                    out_handle: None,
                    node_type: NodeType::Smooth,
                });
            }
            PathEl::QuadTo(c, p) => {
                // Elevate the quadratic to a cubic for our node model.
                if let Some(prev) = nodes.last() {
                    let p0 = to_kpt(prev.pos);
                    let cc = *c;
                    let pe = *p;
                    let c1 = p0 + (cc - p0) * (2.0 / 3.0);
                    let c2 = pe + (cc - pe) * (2.0 / 3.0);
                    if let Some(prev) = nodes.last_mut() {
                        prev.out_handle = Some(to_pos(c1) - prev.pos);
                    }
                    let pos = to_pos(pe);
                    nodes.push(BezierNode {
                        pos,
                        in_handle: Some(to_pos(c2) - pos),
                        out_handle: None,
                        node_type: NodeType::Smooth,
                    });
                }
            }
            PathEl::ClosePath => {}
        }
    }
    nodes
}

/// Simplify a raw point stream and fit a tidy cubic `BezierPath` to it.
/// Returns `(nodes, closed)`. `tol` is the fit tolerance in px (higher → fewer
/// nodes). Degenerate (<2 points) input is returned as corner nodes.
pub fn points_to_bezier(points: &[Pos2], tol: f32) -> (Vec<BezierNode>, bool) {
    if points.len() < 2 {
        return (
            points
                .iter()
                .map(|p| BezierNode {
                    pos: *p,
                    in_handle: None,
                    out_handle: None,
                    node_type: NodeType::Corner,
                })
                .collect(),
            false,
        );
    }

    let closed = points.len() > 3
        && points.first().unwrap().distance(*points.last().unwrap()) < tol.max(1.0) * 1.5;

    let simplified = rdp(points, (tol * 0.5).max(0.5));
    if simplified.len() < 2 {
        // Degenerate after simplify — fall back to a corner polyline.
        return (corner_polyline(points), closed);
    }

    let fit = PolyFit::new(simplified.iter().map(|p| to_kpt(*p)).collect());
    let bez = kurbo::fit_to_bezpath(&fit, (tol as f64).max(0.5));
    let nodes = bezpath_to_nodes(&bez);
    if nodes.len() < 2 {
        return (corner_polyline(points), closed);
    }
    (nodes, closed)
}

fn corner_polyline(points: &[Pos2]) -> Vec<BezierNode> {
    points
        .iter()
        .map(|p| BezierNode {
            pos: *p,
            in_handle: None,
            out_handle: None,
            node_type: NodeType::Corner,
        })
        .collect()
}

/// Join two OPEN bezier paths into one open path by connecting their nearest
/// pair of endpoints. Returns `None` if either input is closed or has < 2 nodes.
///
/// Of the four endpoint pairings (a.start|a.end × b.start|b.end) the closest pair
/// is welded. `a` is kept forward; `b` is appended (reversed if needed) so the two
/// chosen endpoints meet. When the two chosen endpoints coincide (within EPS) the
/// duplicate anchor is dropped and the surviving anchor is marked `Corner`.
pub fn weld_paths(
    a: &[BezierNode],
    a_closed: bool,
    b: &[BezierNode],
    b_closed: bool,
) -> Option<Vec<BezierNode>> {
    if a_closed || b_closed || a.len() < 2 || b.len() < 2 {
        return None;
    }
    let a_start = a.first().unwrap().pos;
    let a_end = a.last().unwrap().pos;
    let b_start = b.first().unwrap().pos;
    let b_end = b.last().unwrap().pos;

    let dist2 = |p: Pos2, q: Pos2| (p - q).length_sq();
    // (rev_a, rev_b, tail_of_a_after_rev, head_of_b_after_rev)
    let combos = [
        (false, false, a_end, b_start),
        (false, true, a_end, b_end),
        (true, false, a_start, b_start),
        (true, true, a_start, b_end),
    ];
    let (rev_a, rev_b, _, _) = combos
        .iter()
        .copied()
        .min_by(|x, y| dist2(x.2, x.3).partial_cmp(&dist2(y.2, y.3)).unwrap())
        .unwrap();

    let reversed = |v: &[BezierNode]| -> Vec<BezierNode> {
        v.iter()
            .rev()
            .map(|n| BezierNode {
                pos: n.pos,
                in_handle: n.out_handle, // swap on reverse
                out_handle: n.in_handle,
                node_type: n.node_type,
            })
            .collect()
    };

    let mut left = if rev_a { reversed(a) } else { a.to_vec() };
    let right = if rev_b { reversed(b) } else { b.to_vec() };

    const EPS: f32 = 1e-3;
    if (left.last().unwrap().pos - right.first().unwrap().pos).length() < EPS {
        // Coincident seam: merge the two anchors into one corner node carrying
        // left's in_handle and right's out_handle.
        let seam = BezierNode {
            pos: left.last().unwrap().pos,
            in_handle: left.last().unwrap().in_handle,
            out_handle: right.first().unwrap().out_handle,
            node_type: NodeType::Corner,
        };
        *left.last_mut().unwrap() = seam;
        left.extend_from_slice(&right[1..]);
    } else {
        left.extend_from_slice(&right);
    }
    // Open path: clear dangling cross-handles on the two outer endpoints.
    left.first_mut().unwrap().in_handle = None;
    left.last_mut().unwrap().out_handle = None;
    Some(left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn open(seq: &[(f32, f32)]) -> Vec<BezierNode> {
        seq.iter()
            .map(|&(x, y)| BezierNode {
                pos: pos2(x, y),
                in_handle: None,
                out_handle: None,
                node_type: NodeType::Corner,
            })
            .collect()
    }

    #[test]
    fn weld_joins_at_nearest_endpoints_a_end_to_b_start() {
        let a = open(&[(0.0, 0.0), (10.0, 0.0)]); // ends at (10,0)
        let b = open(&[(11.0, 0.0), (20.0, 0.0)]); // starts near (11,0)
        let merged = weld_paths(&a, false, &b, false).unwrap();
        // a.end (10,0) is nearest to b.start (11,0): straight concat, seam not duplicated.
        assert_eq!(merged.len(), 4);
        assert!((merged[0].pos - pos2(0.0, 0.0)).length() < 1e-3);
        assert!((merged.last().unwrap().pos - pos2(20.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn weld_reverses_b_when_b_end_is_nearest() {
        let a = open(&[(0.0, 0.0), (10.0, 0.0)]); // ends at (10,0)
        let b = open(&[(20.0, 0.0), (11.0, 0.0)]); // b.end (11,0) is nearest
        let merged = weld_paths(&a, false, &b, false).unwrap();
        assert_eq!(merged.len(), 4);
        // After reversing b: …(10,0) -> (11,0) -> (20,0)
        assert!((merged.last().unwrap().pos - pos2(20.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn weld_rejects_closed_inputs() {
        let a = open(&[(0.0, 0.0), (10.0, 0.0)]);
        let b = open(&[(11.0, 0.0), (20.0, 0.0)]);
        assert!(weld_paths(&a, true, &b, false).is_none());
        assert!(weld_paths(&a, false, &b, true).is_none());
    }

    #[test]
    fn weld_merges_coincident_seam() {
        let a = open(&[(0.0, 0.0), (10.0, 0.0)]);
        let b = open(&[(10.0, 0.0), (20.0, 0.0)]); // b.start coincides with a.end
        let merged = weld_paths(&a, false, &b, false).unwrap();
        // Coincident seam dropped: 4 anchors → 3.
        assert_eq!(merged.len(), 3);
        assert!((merged[1].pos - pos2(10.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn points_to_bezier_reduces_node_count() {
        // Dense sampling of a smooth arc → far fewer nodes than samples.
        let pts: Vec<Pos2> = (0..=80)
            .map(|i| {
                let t = i as f32 / 80.0 * std::f32::consts::PI;
                Pos2::new(t.cos() * 50.0, t.sin() * 50.0)
            })
            .collect();
        let (nodes, closed) = points_to_bezier(&pts, 1.0);
        assert!(!closed);
        assert!(nodes.len() >= 2);
        assert!(
            nodes.len() < pts.len() / 4,
            "expected heavy reduction, got {}",
            nodes.len()
        );
    }

    #[test]
    fn points_to_bezier_stays_within_tolerance() {
        let pts: Vec<Pos2> = (0..=60)
            .map(|i| {
                let t = i as f32 / 60.0;
                Pos2::new(t * 100.0, (t * std::f32::consts::TAU).sin() * 20.0)
            })
            .collect();
        let (nodes, _) = points_to_bezier(&pts, 2.0);
        let fit = crate::bezier::tessellate(&nodes, false, 24);
        for p in &pts {
            let d = fit
                .iter()
                .map(|q| q.distance(*p))
                .fold(f32::INFINITY, f32::min);
            assert!(d <= 6.0, "sample {p:?} was {d} from fitted curve");
        }
    }

    #[test]
    fn points_to_bezier_detects_closed_loop() {
        let mut pts: Vec<Pos2> = (0..40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                Pos2::new(a.cos() * 30.0, a.sin() * 30.0)
            })
            .collect();
        pts.push(pts[0]); // return to start
        let (_, closed) = points_to_bezier(&pts, 1.0);
        assert!(closed);
    }

    #[test]
    fn points_to_bezier_degenerate_input() {
        assert_eq!(points_to_bezier(&[], 1.0).0.len(), 0);
        let one = points_to_bezier(&[Pos2::new(1.0, 1.0)], 1.0);
        assert!(one.0.len() <= 1);
    }

    #[test]
    fn bezpath_to_nodes_reconstructs_handles() {
        // A single cubic from (0,0) to (10,0) with control points pulling up.
        let mut bp = BezPath::new();
        bp.move_to(Point::new(0.0, 0.0));
        bp.curve_to(
            Point::new(3.0, 4.0),
            Point::new(7.0, 4.0),
            Point::new(10.0, 0.0),
        );
        let nodes = bezpath_to_nodes(&bp);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].pos, Pos2::new(0.0, 0.0));
        assert_eq!(nodes[1].pos, Pos2::new(10.0, 0.0));
        // out_handle of node 0 = c1 - p0 = (3,4)
        assert_eq!(nodes[0].out_handle, Some(egui::vec2(3.0, 4.0)));
        // in_handle of node 1 = c2 - p1 = (7-10, 4-0) = (-3, 4)
        assert_eq!(nodes[1].in_handle, Some(egui::vec2(-3.0, 4.0)));
    }
}
