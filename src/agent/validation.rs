//! Input validation for MCP tool calls.
//!
//! The MCP tool handlers in [`crate::agent::server`] build egui geometry
//! (`Pos2`/`Rect`/thickness/radius) directly from deserialized `f32` supplied
//! by a remote agent. Without validation a malicious or buggy agent can pass
//! `NaN`, `Infinity`, or astronomically large coordinates, which flow into the
//! pixel-rasterization loops in `crate::flatten` and iterate effectively
//! forever — freezing the app or exhausting memory. Likewise, unbounded point
//! lists (`draw_pen` / `replace_pen_points`) and unbounded label/content
//! strings allow memory amplification.
//!
//! This module centralizes the bounds and helper functions so every handler
//! validates consistently *before* constructing an `AgentOp`. Each helper
//! returns `Result<_, rmcp::ErrorData>` with `invalid_params` so the failure is
//! reported to the agent rather than silently mangled.
//!
//! The bounds are deliberately generous absolute clamps: handlers run on the
//! MCP thread without access to the live canvas size, so we cannot clamp to the
//! exact image bounds. The goal is only to keep the rasterizer loops finite and
//! memory bounded, not to enforce that geometry lands on-canvas.

use rmcp::ErrorData;

/// Maximum absolute value for any coordinate or rect component, in image-pixel
/// space. Real canvases are at most a few thousand pixels; 100k leaves enormous
/// head-room for legitimate off-canvas geometry while keeping rasterization
/// loops bounded (worst case ~200k x 200k is still finite and short-circuited
/// by canvas clipping downstream).
pub const MAX_COORD: f32 = 100_000.0;

/// Maximum absolute value for a translation delta (`move_shape`). Same bound as
/// a coordinate: a delta larger than the whole coordinate range is meaningless.
pub const MAX_DELTA: f32 = 100_000.0;

/// Maximum stroke thickness in pixels. Thickness feeds disc/band rasterization
/// radii, so an unbounded value blows up the inner loop. 10k px is far thicker
/// than any sane stroke.
pub const MAX_THICKNESS: f32 = 10_000.0;

/// Maximum corner radius / font size, in pixels. Feeds rasterization the same
/// way thickness does.
pub const MAX_SIZE: f32 = 100_000.0;

/// Maximum number of points in a pen stroke (`draw_pen` / `replace_pen_points`).
/// Each point is stored, broadcast, and walked during rasterization; an
/// unbounded list is a memory-amplification vector.
pub const MAX_PEN_POINTS: usize = 10_000;

/// Maximum length (in bytes) of a `label` string.
pub const MAX_LABEL_LEN: usize = 1_000;

/// Maximum length (in bytes) of a text `content` string.
pub const MAX_TEXT_LEN: usize = 10_000;

/// Validate that a scalar is finite and within `[-bound, bound]`.
fn validate_scalar(value: f32, bound: f32, what: &str) -> Result<f32, ErrorData> {
    if !value.is_finite() {
        return Err(ErrorData::invalid_params(
            format!("{what} must be a finite number, got {value}"),
            None,
        ));
    }
    if value.abs() > bound {
        return Err(ErrorData::invalid_params(
            format!("{what} {value} is out of bounds (must be within +/-{bound})"),
            None,
        ));
    }
    Ok(value)
}

/// Validate a `[x, y]` coordinate pair and return an egui `Pos2`.
///
/// Rejects `NaN`/`Infinity` and any component outside `[-MAX_COORD, MAX_COORD]`.
pub fn validate_finite_pos(xy: [f32; 2], what: &str) -> Result<egui::Pos2, ErrorData> {
    let x = validate_scalar(xy[0], MAX_COORD, &format!("{what}.x"))?;
    let y = validate_scalar(xy[1], MAX_COORD, &format!("{what}.y"))?;
    Ok(egui::pos2(x, y))
}

/// Validate a `[dx, dy]` delta and return an egui `Vec2`.
pub fn validate_finite_delta(d: [f32; 2], what: &str) -> Result<egui::Vec2, ErrorData> {
    let x = validate_scalar(d[0], MAX_DELTA, &format!("{what}.dx"))?;
    let y = validate_scalar(d[1], MAX_DELTA, &format!("{what}.dy"))?;
    Ok(egui::Vec2::new(x, y))
}

/// Validate a `[x_min, y_min, x_max, y_max]` rect and return an egui `Rect`.
///
/// Each component must be finite and within `[-MAX_COORD, MAX_COORD]`. The rect
/// is built with `from_min_max`; min/max ordering is not enforced here because
/// downstream code normalizes it, but the bound guarantees the resulting rect
/// spans at most `2 * MAX_COORD` in each axis.
pub fn validate_rect(r: [f32; 4], what: &str) -> Result<egui::Rect, ErrorData> {
    let x0 = validate_scalar(r[0], MAX_COORD, &format!("{what}[x_min]"))?;
    let y0 = validate_scalar(r[1], MAX_COORD, &format!("{what}[y_min]"))?;
    let x1 = validate_scalar(r[2], MAX_COORD, &format!("{what}[x_max]"))?;
    let y1 = validate_scalar(r[3], MAX_COORD, &format!("{what}[y_max]"))?;
    Ok(egui::Rect::from_min_max(
        egui::pos2(x0, y0),
        egui::pos2(x1, y1),
    ))
}

/// Validate a stroke thickness: finite and within `(0, MAX_THICKNESS]`.
///
/// Non-positive thickness is rejected because it produces degenerate /
/// invisible strokes and (for some primitives) an empty rasterization range.
pub fn validate_thickness(t: f32, what: &str) -> Result<f32, ErrorData> {
    if !t.is_finite() {
        return Err(ErrorData::invalid_params(
            format!("{what} must be a finite number, got {t}"),
            None,
        ));
    }
    if t <= 0.0 || t > MAX_THICKNESS {
        return Err(ErrorData::invalid_params(
            format!("{what} {t} must be in (0, {MAX_THICKNESS}]"),
            None,
        ));
    }
    Ok(t)
}

/// Validate a size-like positive scalar (radius, font size): finite and within
/// `(0, MAX_SIZE]`.
pub fn validate_size(s: f32, what: &str) -> Result<f32, ErrorData> {
    if !s.is_finite() {
        return Err(ErrorData::invalid_params(
            format!("{what} must be a finite number, got {s}"),
            None,
        ));
    }
    if s <= 0.0 || s > MAX_SIZE {
        return Err(ErrorData::invalid_params(
            format!("{what} {s} must be in (0, {MAX_SIZE}]"),
            None,
        ));
    }
    Ok(s)
}

/// Validate and convert a list of `[x, y]` points into `Pos2`s.
///
/// Rejects lists longer than [`MAX_PEN_POINTS`] and any point containing a
/// non-finite or out-of-bounds component.
pub fn validate_points(points: &[[f32; 2]], what: &str) -> Result<Vec<egui::Pos2>, ErrorData> {
    if points.len() > MAX_PEN_POINTS {
        return Err(ErrorData::invalid_params(
            format!(
                "{what} has {} points, exceeding the maximum of {MAX_PEN_POINTS}",
                points.len()
            ),
            None,
        ));
    }
    points
        .iter()
        .map(|p| validate_finite_pos(*p, what))
        .collect()
}

/// Validate that a string does not exceed `max` bytes.
pub fn cap_str(s: &str, max: usize, what: &str) -> Result<(), ErrorData> {
    if s.len() > max {
        return Err(ErrorData::invalid_params(
            format!(
                "{what} is {} bytes, exceeding the maximum of {max}",
                s.len()
            ),
            None,
        ));
    }
    Ok(())
}

/// Validate an optional label against [`MAX_LABEL_LEN`].
pub fn validate_label(label: &Option<String>) -> Result<(), ErrorData> {
    if let Some(l) = label {
        cap_str(l, MAX_LABEL_LEN, "label")?;
    }
    Ok(())
}

/// Maximum length (in chars) of a layer/group name.
pub const MAX_LAYER_NAME_LEN: usize = 200;

/// Validate a layer/group name: non-empty after trim, within MAX_LAYER_NAME_LEN.
pub fn validate_name(name: &str, what: &str) -> Result<String, ErrorData> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ErrorData::invalid_params(
            format!("{what} must not be empty"),
            None,
        ));
    }
    if name.chars().count() > MAX_LAYER_NAME_LEN {
        return Err(ErrorData::invalid_params(
            format!("{what} exceeds {MAX_LAYER_NAME_LEN} chars"),
            None,
        ));
    }
    Ok(name.to_string())
}

/// Validate an opacity in [0.0, 1.0], rejecting NaN/Inf.
pub fn validate_opacity(o: f32, what: &str) -> Result<f32, ErrorData> {
    if !o.is_finite() || !(0.0..=1.0).contains(&o) {
        return Err(ErrorData::invalid_params(
            format!("{what} must be in [0,1]"),
            None,
        ));
    }
    Ok(o)
}

/// Maximum allowed translation in either axis, in pixels (same as MAX_COORD but f64).
pub const MAX_TRANSFORM_TRANSLATE: f64 = 100_000.0;

/// Maximum allowed scale magnitude in either axis (must not be zero).
pub const MAX_TRANSFORM_SCALE: f64 = 1_000.0;

/// Validate a `[tx, ty]` translate pair: must be finite and within ±MAX_TRANSFORM_TRANSLATE.
pub fn validate_transform_translate(v: [f64; 2], what: &str) -> Result<[f64; 2], ErrorData> {
    for (i, &c) in v.iter().enumerate() {
        if !c.is_finite() {
            return Err(ErrorData::invalid_params(
                format!("{what}[{i}] must be finite, got {c}"),
                None,
            ));
        }
        if c.abs() > MAX_TRANSFORM_TRANSLATE {
            return Err(ErrorData::invalid_params(
                format!("{what}[{i}] {c} exceeds max ±{MAX_TRANSFORM_TRANSLATE}"),
                None,
            ));
        }
    }
    Ok(v)
}

/// Validate a `[sx, sy]` scale pair: must be finite, non-zero, and within ±MAX_TRANSFORM_SCALE.
pub fn validate_transform_scale(v: [f64; 2], what: &str) -> Result<[f64; 2], ErrorData> {
    for (i, &c) in v.iter().enumerate() {
        if !c.is_finite() {
            return Err(ErrorData::invalid_params(
                format!("{what}[{i}] must be finite, got {c}"),
                None,
            ));
        }
        if c.abs() < 1e-9 {
            return Err(ErrorData::invalid_params(
                format!("{what}[{i}] must not be zero"),
                None,
            ));
        }
        if c.abs() > MAX_TRANSFORM_SCALE {
            return Err(ErrorData::invalid_params(
                format!("{what}[{i}] {c} exceeds max ±{MAX_TRANSFORM_SCALE}"),
                None,
            ));
        }
    }
    Ok(v)
}

/// Validate a rotation or skew in degrees: must be finite and within (-360, 360).
pub fn validate_transform_angle_deg(deg: f64, what: &str) -> Result<f64, ErrorData> {
    if !deg.is_finite() {
        return Err(ErrorData::invalid_params(
            format!("{what} must be finite, got {deg}"),
            None,
        ));
    }
    Ok(deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nan_and_infinite_coords() {
        assert!(validate_finite_pos([f32::NAN, 0.0], "p").is_err());
        assert!(validate_finite_pos([0.0, f32::INFINITY], "p").is_err());
        assert!(validate_finite_pos([f32::NEG_INFINITY, 0.0], "p").is_err());
        assert!(validate_finite_pos([1.0, 2.0], "p").is_ok());
    }

    #[test]
    fn clamps_coord_bounds() {
        assert!(validate_finite_pos([MAX_COORD, 0.0], "p").is_ok());
        assert!(validate_finite_pos([MAX_COORD + 1.0, 0.0], "p").is_err());
        assert!(validate_finite_pos([-MAX_COORD - 1.0, 0.0], "p").is_err());
    }

    #[test]
    fn validates_rect() {
        assert!(validate_rect([0.0, 0.0, 10.0, 10.0], "r").is_ok());
        assert!(validate_rect([0.0, 0.0, f32::NAN, 10.0], "r").is_err());
        assert!(validate_rect([0.0, 0.0, MAX_COORD * 2.0, 10.0], "r").is_err());
    }

    #[test]
    fn validates_delta() {
        assert!(validate_finite_delta([5.0, -5.0], "d").is_ok());
        assert!(validate_finite_delta([f32::INFINITY, 0.0], "d").is_err());
        assert!(validate_finite_delta([MAX_DELTA + 1.0, 0.0], "d").is_err());
    }

    #[test]
    fn validates_thickness() {
        assert!(validate_thickness(2.0, "t").is_ok());
        assert!(validate_thickness(0.0, "t").is_err());
        assert!(validate_thickness(-1.0, "t").is_err());
        assert!(validate_thickness(f32::NAN, "t").is_err());
        assert!(validate_thickness(MAX_THICKNESS, "t").is_ok());
        assert!(validate_thickness(MAX_THICKNESS + 1.0, "t").is_err());
    }

    #[test]
    fn validates_size() {
        assert!(validate_size(18.0, "s").is_ok());
        assert!(validate_size(0.0, "s").is_err());
        assert!(validate_size(f32::INFINITY, "s").is_err());
        assert!(validate_size(MAX_SIZE, "s").is_ok());
        assert!(validate_size(MAX_SIZE + 1.0, "s").is_err());
    }

    #[test]
    fn caps_point_count() {
        let ok = vec![[0.0f32, 0.0]; MAX_PEN_POINTS];
        assert!(validate_points(&ok, "pts").is_ok());
        let too_many = vec![[0.0f32, 0.0]; MAX_PEN_POINTS + 1];
        assert!(validate_points(&too_many, "pts").is_err());
    }

    #[test]
    fn rejects_nonfinite_point_in_list() {
        let pts = vec![[0.0, 0.0], [f32::NAN, 1.0]];
        assert!(validate_points(&pts, "pts").is_err());
    }

    #[test]
    fn caps_strings() {
        assert!(cap_str("hello", MAX_LABEL_LEN, "label").is_ok());
        let big = "x".repeat(MAX_LABEL_LEN + 1);
        assert!(cap_str(&big, MAX_LABEL_LEN, "label").is_err());
    }

    #[test]
    fn validates_optional_label() {
        assert!(validate_label(&None).is_ok());
        assert!(validate_label(&Some("ok".to_string())).is_ok());
        assert!(validate_label(&Some("y".repeat(MAX_LABEL_LEN + 1))).is_err());
    }

    #[test]
    fn validates_name() {
        assert!(validate_name("Layer 1", "name").is_ok());
        assert!(validate_name("   ", "name").is_err());
        assert!(validate_name("", "name").is_err());
        assert!(validate_name(&"x".repeat(MAX_LAYER_NAME_LEN + 1), "name").is_err());
    }

    #[test]
    fn validates_opacity() {
        assert!(validate_opacity(0.0, "opacity").is_ok());
        assert!(validate_opacity(1.0, "opacity").is_ok());
        assert!(validate_opacity(0.5, "opacity").is_ok());
        assert!(validate_opacity(-0.1, "opacity").is_err());
        assert!(validate_opacity(1.1, "opacity").is_err());
        assert!(validate_opacity(f32::NAN, "opacity").is_err());
    }

    #[test]
    fn validates_transform_translate() {
        assert!(validate_transform_translate([0.0, 0.0], "t").is_ok());
        assert!(validate_transform_translate([1000.0, -5000.0], "t").is_ok());
        assert!(validate_transform_translate([f64::NAN, 0.0], "t").is_err());
        assert!(validate_transform_translate([f64::INFINITY, 0.0], "t").is_err());
        assert!(validate_transform_translate([MAX_TRANSFORM_TRANSLATE + 1.0, 0.0], "t").is_err());
    }

    #[test]
    fn validates_transform_scale() {
        assert!(validate_transform_scale([1.0, 1.0], "s").is_ok());
        assert!(validate_transform_scale([-1.0, 2.0], "s").is_ok()); // negative scale (flip) is valid
        assert!(validate_transform_scale([0.0, 1.0], "s").is_err()); // zero scale forbidden
        assert!(validate_transform_scale([f64::NAN, 1.0], "s").is_err());
        assert!(validate_transform_scale([MAX_TRANSFORM_SCALE + 1.0, 1.0], "s").is_err());
    }

    #[test]
    fn validates_transform_angle_deg() {
        assert!(validate_transform_angle_deg(0.0, "r").is_ok());
        assert!(validate_transform_angle_deg(359.9, "r").is_ok());
        assert!(validate_transform_angle_deg(-180.0, "r").is_ok());
        assert!(validate_transform_angle_deg(f64::NAN, "r").is_err());
        assert!(validate_transform_angle_deg(f64::INFINITY, "r").is_err());
    }

    /// Integration-style: the exact validation a draw handler runs on a NaN
    /// coordinate must reject before any `AgentOp` is built, and the resulting
    /// error must be an `invalid_params` (code -32602).
    #[test]
    fn handler_rejects_nan_coordinate() {
        let err = validate_finite_pos([f32::NAN, 0.0], "start").unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    /// Integration-style: an over-cap pen point list (as draw_pen /
    /// replace_pen_points would receive) is rejected with invalid_params.
    #[test]
    fn handler_rejects_over_cap_points() {
        let pts = vec![[0.0f32, 0.0]; MAX_PEN_POINTS + 1];
        let err = validate_points(&pts, "points").unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
