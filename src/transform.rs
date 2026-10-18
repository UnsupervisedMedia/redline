//! Pure affine-transform math for the vector editor. GUI-free and unit-tested.
use egui::Pos2;
use kurbo::{Affine, Point, Vec2};

/// Convert an egui `Pos2` to a kurbo `Point`.
pub fn to_kpoint(p: Pos2) -> Point {
    Point::new(p.x as f64, p.y as f64)
}
/// Convert a kurbo `Point` back to an egui `Pos2`.
pub fn to_pos2(p: Point) -> Pos2 {
    egui::pos2(p.x as f32, p.y as f32)
}
/// Apply an affine transform to an egui `Pos2` and return the resulting `Pos2`.
pub fn apply(a: Affine, p: Pos2) -> Pos2 {
    to_pos2(a * to_kpoint(p))
}

fn about(c: Pos2, m: Affine) -> Affine {
    let v = Vec2::new(c.x as f64, c.y as f64);
    Affine::translate(v) * m * Affine::translate(-v)
}

/// Return an affine that rotates by `theta` radians (CCW in math coords) about pivot `c`.
pub fn rotate_about(c: Pos2, theta: f64) -> Affine {
    about(c, Affine::rotate(theta))
}
/// Return an affine that scales by `(sx, sy)` about pivot `c`.
pub fn scale_about(c: Pos2, sx: f64, sy: f64) -> Affine {
    about(c, Affine::scale_non_uniform(sx, sy))
}
/// Return an affine that applies skew coefficients `(kx, ky)` in radians about pivot `c`.
pub fn skew_about(c: Pos2, kx: f64, ky: f64) -> Affine {
    about(c, Affine::skew(kx, ky))
}
/// Return an affine that flips horizontally (across vertical axis) or vertically about pivot `c`.
pub fn flip_about(c: Pos2, horizontal: bool) -> Affine {
    if horizontal {
        scale_about(c, -1.0, 1.0)
    } else {
        scale_about(c, 1.0, -1.0)
    }
}

/// New local transform so that the node's new world = `delta_world * (parent_world * local)`.
pub fn apply_world_delta(parent_world: Affine, local: Affine, delta_world: Affine) -> Affine {
    parent_world.inverse() * delta_world * parent_world * local
}

/// Flat representation of an affine transform split into translation, scale, rotation, and skew.
/// `rotation` and `skew` are in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decomposed {
    pub tx: f64,
    pub ty: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
    pub skew: f64, // radians
}

/// Decompose an `Affine` into `Decomposed` (translation, scale, rotation in radians, skew in radians).
pub fn decompose(a: Affine) -> Decomposed {
    let [m0, m1, m2, m3, e, f] = a.as_coeffs(); // [a,b,c,d,e,f]
    let scale_x = (m0 * m0 + m1 * m1).sqrt();
    let rotation = m1.atan2(m0);
    let sx_safe = if scale_x.abs() < 1e-12 {
        1e-12
    } else {
        scale_x
    };
    let shear = (m0 * m2 + m1 * m3) / sx_safe; // = sy·tan(skew)
    let scale_y = (m0 * m3 - m1 * m2) / sx_safe; // signed det / sx
    let sy_safe = if scale_y.abs() < 1e-12 {
        1e-12
    } else {
        scale_y
    };
    let skew = (shear / sy_safe).atan();
    Decomposed {
        tx: e,
        ty: f,
        scale_x,
        scale_y,
        rotation,
        skew,
    }
}

/// Recompose a `Decomposed` back into a kurbo `Affine`.
pub fn recompose(d: Decomposed) -> Affine {
    let (cos, sin) = (d.rotation.cos(), d.rotation.sin());
    let t = d.skew.tan();
    let a = cos * d.scale_x;
    let b = sin * d.scale_x;
    let c = cos * (d.scale_y * t) - sin * d.scale_y;
    let dd = sin * (d.scale_y * t) + cos * d.scale_y;
    Affine::new([a, b, c, dd, d.tx, d.ty])
}

/// Return the four corners of `rect` transformed by `world` (TL, TR, BR, BL order).
pub fn transformed_corners(rect: egui::Rect, world: Affine) -> [egui::Pos2; 4] {
    [
        apply(world, rect.left_top()),
        apply(world, rect.right_top()),
        apply(world, rect.right_bottom()),
        apply(world, rect.left_bottom()),
    ]
}

/// Compute the axis-aligned bounding box of a slice of world-space points.
pub fn world_aabb(corners: &[egui::Pos2]) -> egui::Rect {
    let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
    let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in corners {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    egui::Rect::from_min_max(min, max)
}

/// Compute the world-space AABB enclosing all items' transformed corners; returns `None` if empty.
pub fn selection_world_bbox(items: &[(egui::Rect, Affine)]) -> Option<egui::Rect> {
    if items.is_empty() {
        return None;
    }
    let mut all = Vec::with_capacity(items.len() * 4);
    for (r, w) in items {
        all.extend_from_slice(&transformed_corners(*r, *w));
    }
    Some(world_aabb(&all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn approx(a: egui::Pos2, b: egui::Pos2) {
        assert!(
            (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn rotate_about_pivot_keeps_pivot_fixed() {
        let c = pos2(10.0, 10.0);
        let d = rotate_about(c, std::f64::consts::FRAC_PI_2);
        approx(apply(d, c), c); // pivot fixed
        approx(apply(d, pos2(20.0, 10.0)), pos2(10.0, 20.0)); // 90° CCW->CW per screen-y-down
    }

    #[test]
    fn scale_about_pivot_scales_from_pivot() {
        let c = pos2(0.0, 0.0);
        let d = scale_about(c, 2.0, 3.0);
        approx(apply(d, pos2(5.0, 5.0)), pos2(10.0, 15.0));
    }

    #[test]
    fn flip_h_mirrors_across_pivot_x() {
        let c = pos2(10.0, 0.0);
        let d = flip_about(c, true);
        approx(apply(d, pos2(12.0, 5.0)), pos2(8.0, 5.0));
    }

    #[test]
    fn world_delta_composes_into_local() {
        use kurbo::Affine;
        let parent = Affine::translate((100.0, 0.0)); // group offset
        let local = Affine::scale(2.0);
        let delta = rotate_about(egui::pos2(0.0, 0.0), 0.3);
        let new_local = apply_world_delta(parent, local, delta);
        // new world must equal delta * old world
        let old_world = parent * local;
        let want = delta * old_world;
        let got = parent * new_local;
        let (wa, ga) = (want.as_coeffs(), got.as_coeffs());
        for i in 0..6 {
            assert!((wa[i] - ga[i]).abs() < 1e-9, "coeff {i}");
        }
    }

    #[test]
    fn world_delta_identity_parent_is_just_delta_times_local() {
        use kurbo::Affine;
        let delta = scale_about(egui::pos2(5.0, 5.0), 2.0, 2.0);
        let local = Affine::translate((3.0, 4.0));
        let got = apply_world_delta(Affine::IDENTITY, local, delta);
        let want = delta * local;
        let (wa, ga) = (want.as_coeffs(), got.as_coeffs());
        for i in 0..6 {
            assert!((wa[i] - ga[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn decompose_recompose_roundtrips() {
        use kurbo::Affine;
        let cases = [
            Affine::IDENTITY,
            Affine::translate((12.0, -7.0)),
            Affine::rotate(0.7),
            Affine::scale_non_uniform(2.0, 0.5),
            Affine::translate((5.0, 5.0))
                * Affine::rotate(0.4)
                * Affine::scale_non_uniform(1.5, 2.0),
        ];
        for a in cases {
            let r = recompose(decompose(a));
            let (x, y) = (a.as_coeffs(), r.as_coeffs());
            for i in 0..6 {
                assert!((x[i] - y[i]).abs() < 1e-6, "coeff {i}: {x:?} vs {y:?}");
            }
        }
    }

    #[test]
    fn decompose_reads_rotation_and_scale() {
        use kurbo::Affine;
        let d = decompose(Affine::rotate(0.5) * Affine::scale_non_uniform(2.0, 3.0));
        assert!((d.rotation - 0.5).abs() < 1e-6);
        assert!((d.scale_x - 2.0).abs() < 1e-6);
        assert!((d.scale_y - 3.0).abs() < 1e-6);
        assert!(d.skew.abs() < 1e-6);
    }

    #[test]
    fn transformed_corners_of_rotated_rect() {
        let r = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0));
        let corners = transformed_corners(
            r,
            rotate_about(egui::pos2(5.0, 5.0), std::f64::consts::FRAC_PI_2),
        );
        let bb = world_aabb(&corners);
        // 90° rotation about center maps the same 10×10 box onto itself
        assert!((bb.min.x - 0.0).abs() < 1e-3 && (bb.max.x - 10.0).abs() < 1e-3);
        assert!((bb.min.y - 0.0).abs() < 1e-3 && (bb.max.y - 10.0).abs() < 1e-3);
    }

    #[test]
    fn selection_bbox_unions_items() {
        use kurbo::Affine;
        let a = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0));
        let b = egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(30.0, 30.0));
        let bb = selection_world_bbox(&[(a, Affine::IDENTITY), (b, Affine::IDENTITY)]).unwrap();
        assert_eq!(bb.min, egui::pos2(0.0, 0.0));
        assert_eq!(bb.max, egui::pos2(30.0, 30.0));
    }
}
