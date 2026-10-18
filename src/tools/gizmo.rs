//! Pure gizmo handle geometry for the on-canvas transform gizmo.
//! GUI-free and unit-tested — no AppState, no interaction, no drawing.
//!
//! # Layout ordering
//! - `corners`: [TopLeft, TopRight, BottomRight, BottomLeft]  (clockwise from TL)
//! - `edges`:   [Top, Right, Bottom, Left]                    (clockwise from Top)
//! - `rotate`:  above `corners[0]..corners[1]` midpoint (top-center - (0, 24))
//! - `center`:  bounding-box center

use egui::{Pos2, Rect};

/// Which handle the pointer is over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Rotate,
}

/// Screen-space positions of every gizmo handle for a selected bounding box.
#[derive(Clone, Copy, Debug)]
pub struct GizmoLayout {
    /// Corner handles: [TopLeft, TopRight, BottomRight, BottomLeft]
    pub corners: [Pos2; 4],
    /// Edge midpoint handles: [Top, Right, Bottom, Left]
    pub edges: [Pos2; 4],
    /// Rotate handle — placed 24 px above the top-centre of the bounding box.
    pub rotate: Pos2,
    /// Centre / pivot point (interior, no hit-zone).
    pub center: Pos2,
}

/// Compute [`GizmoLayout`] from a screen-space bounding rectangle.
pub fn gizmo_layout(bbox_screen: Rect) -> GizmoLayout {
    let min = bbox_screen.min;
    let max = bbox_screen.max;
    let cx = (min.x + max.x) * 0.5;
    let cy = (min.y + max.y) * 0.5;

    // Corners: clockwise from TL → TopLeft, TopRight, BottomRight, BottomLeft
    let corners = [
        egui::pos2(min.x, min.y), // TopLeft
        egui::pos2(max.x, min.y), // TopRight
        egui::pos2(max.x, max.y), // BottomRight
        egui::pos2(min.x, max.y), // BottomLeft
    ];

    // Edges midpoints: clockwise from Top → Top, Right, Bottom, Left
    let edges = [
        egui::pos2(cx, min.y), // Top
        egui::pos2(max.x, cy), // Right
        egui::pos2(cx, max.y), // Bottom
        egui::pos2(min.x, cy), // Left
    ];

    // Rotate handle is 24 px above the top-centre
    let rotate = egui::pos2(cx, min.y - 24.0);

    let center = egui::pos2(cx, cy);

    GizmoLayout {
        corners,
        edges,
        rotate,
        center,
    }
}

/// Return the [`Handle`] whose point is within `radius` of `p`, or `None` if the
/// pointer is in the interior (or farther than `radius` from every handle).
///
/// Priority: Rotate is checked first, then corners, then edges.
pub fn handle_at(layout: &GizmoLayout, p: Pos2, radius: f32) -> Option<Handle> {
    let near = |q: Pos2| (p - q).length() <= radius;

    if near(layout.rotate) {
        return Some(Handle::Rotate);
    }

    // corners: [TopLeft, TopRight, BottomRight, BottomLeft]
    let corner_handles = [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomRight,
        Handle::BottomLeft,
    ];
    for (pos, handle) in layout.corners.iter().zip(corner_handles.iter()) {
        if near(*pos) {
            return Some(*handle);
        }
    }

    // edges: [Top, Right, Bottom, Left]
    let edge_handles = [Handle::Top, Handle::Right, Handle::Bottom, Handle::Left];
    for (pos, handle) in layout.edges.iter().zip(edge_handles.iter()) {
        if near(*pos) {
            return Some(*handle);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gizmo_handle_hit_zones() {
        let bb = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let g = gizmo_layout(bb);

        // Top-left corner hit
        assert_eq!(
            handle_at(&g, egui::pos2(0.0, 0.0), 6.0),
            Some(Handle::TopLeft)
        );

        // Right edge midpoint hit
        assert_eq!(
            handle_at(&g, egui::pos2(100.0, 50.0), 6.0),
            Some(Handle::Right)
        );

        // Rotate handle hit (exact position)
        assert!(matches!(handle_at(&g, g.rotate, 6.0), Some(Handle::Rotate)));

        // Interior → None
        assert_eq!(handle_at(&g, egui::pos2(50.0, 50.0), 6.0), None);
    }
}
