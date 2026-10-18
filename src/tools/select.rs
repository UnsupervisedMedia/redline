use crate::{
    state::{AppState, ToolState},
    tools::gizmo::{gizmo_layout, handle_at, Handle},
    types::{Command, NodeProp, ParticipantId, ShapeData},
};
use egui::{Context, Pos2, Rect, Response, Vec2};
use uuid::Uuid;

/// Screen-pixel radius within which a click on a corner counts as grabbing the handle.
const HANDLE_HIT_RADIUS: f32 = 10.0;

/// Minimum hit-test radius (image-pixel space) for thin shapes like Line and Arrow.
const LINE_HIT_RADIUS: f32 = 6.0;

/// Screen-pixel radius for grabbing a transform-gizmo handle.
const GIZMO_HIT_RADIUS: f32 = 10.0;

/// Image-space point of a gizmo `handle` on `bbox`. For the non-positional
/// `Rotate` handle (unused for scaling) this returns the top-centre, matching
/// the gizmo layout convention.
pub fn handle_point(handle: Handle, bbox: Rect) -> Pos2 {
    let min = bbox.min;
    let max = bbox.max;
    let cx = (min.x + max.x) * 0.5;
    let cy = (min.y + max.y) * 0.5;
    match handle {
        Handle::TopLeft => egui::pos2(min.x, min.y),
        Handle::Top => egui::pos2(cx, min.y),
        Handle::TopRight => egui::pos2(max.x, min.y),
        Handle::Right => egui::pos2(max.x, cy),
        Handle::BottomRight => egui::pos2(max.x, max.y),
        Handle::Bottom => egui::pos2(cx, max.y),
        Handle::BottomLeft => egui::pos2(min.x, max.y),
        Handle::Left => egui::pos2(min.x, cy),
        Handle::Rotate => egui::pos2(cx, min.y),
    }
}

/// The handle diagonally/axis-opposite to `handle`, used as the default scale
/// pivot (anchored point) when Alt is not held.
pub fn opposite_handle(handle: Handle) -> Handle {
    match handle {
        Handle::TopLeft => Handle::BottomRight,
        Handle::Top => Handle::Bottom,
        Handle::TopRight => Handle::BottomLeft,
        Handle::Right => Handle::Left,
        Handle::BottomRight => Handle::TopLeft,
        Handle::Bottom => Handle::Top,
        Handle::BottomLeft => Handle::TopRight,
        Handle::Left => Handle::Right,
        Handle::Rotate => Handle::Rotate,
    }
}

/// True for the four edge-midpoint handles, which constrain scaling to a single
/// axis (the off-axis factor is locked to 1.0).
fn is_edge_handle(handle: Handle) -> bool {
    matches!(
        handle,
        Handle::Top | Handle::Bottom | Handle::Left | Handle::Right
    )
}

/// Compute the rotation delta `D` for dragging from `start_cursor` to `cursor`,
/// both orbiting `pivot`. All inputs are in IMAGE space.
///
/// The angle is `atan2(cursor - pivot) - atan2(start_cursor - pivot)`.
/// If `snap_15` is true the angle is rounded to the nearest 15°.
/// Returns `transform::rotate_about(pivot, angle)`.
pub fn rotate_delta_for_drag(
    pivot: egui::Pos2,
    start_cursor: egui::Pos2,
    cursor: egui::Pos2,
    snap_15: bool,
) -> kurbo::Affine {
    let v0 = start_cursor - pivot;
    let v1 = cursor - pivot;
    let angle_start = (v0.y as f64).atan2(v0.x as f64);
    let angle_end = (v1.y as f64).atan2(v1.x as f64);
    let mut angle = angle_end - angle_start;
    if snap_15 {
        let step = 15f64.to_radians();
        angle = (angle / step).round() * step;
    }
    crate::transform::rotate_about(pivot, angle)
}

/// Compute the skew delta `D` for dragging an edge handle.
///
/// Convention:
/// - **Horizontal edge handles** (`Top`, `Bottom`): dragging horizontally produces
///   an x-skew (`kx = cursor_x_offset / bbox_height`). The vertical extent of
///   the bbox is the normalising denominator so the skew feels proportional.
/// - **Vertical edge handles** (`Left`, `Right`): dragging vertically produces a
///   y-skew (`ky = cursor_y_offset / bbox_width`). The horizontal extent is the
///   normalising denominator.
///
/// In both cases the result is `transform::skew_about(pivot, kx, ky)`. All inputs
/// are in IMAGE space.
pub fn skew_delta_for_drag(
    handle: Handle,
    pivot: egui::Pos2,
    start_cursor: egui::Pos2,
    cursor: egui::Pos2,
    start_bbox: egui::Rect,
) -> kurbo::Affine {
    let dx = (cursor.x - start_cursor.x) as f64;
    let dy = (cursor.y - start_cursor.y) as f64;
    let w = start_bbox.width() as f64;
    let h = start_bbox.height() as f64;

    let (kx, ky) = match handle {
        // Top / Bottom edge: horizontal drag → x-skew, normalised by height.
        Handle::Top | Handle::Bottom => {
            let denom = if h.abs() < 1e-6 { 1.0 } else { h };
            (dx / denom, 0.0)
        }
        // Left / Right edge: vertical drag → y-skew, normalised by width.
        Handle::Left | Handle::Right => {
            let denom = if w.abs() < 1e-6 { 1.0 } else { w };
            (0.0, dy / denom)
        }
        // Corner / rotate handles are not used for skew — return identity.
        _ => (0.0, 0.0),
    };

    crate::transform::skew_about(pivot, kx, ky)
}

/// Compute the world-space scale delta `D` for dragging `handle` from its start
/// position on `start_bbox` to `cursor_img`, scaling about `pivot`. All inputs
/// are in IMAGE space.
///
/// `sx = (cursor - pivot).x / (handle_start - pivot).x`, `sy` likewise. Edge
/// handles lock the off-axis factor to 1.0. `uniform` forces `sx == sy` to the
/// larger magnitude (preserving each factor's sign). Divide-by-zero on a
/// degenerate bbox extent is guarded by falling back to a factor of 1.0.
pub fn scale_delta_for_drag(
    handle: Handle,
    start_bbox: Rect,
    pivot: Pos2,
    cursor_img: Pos2,
    uniform: bool,
) -> kurbo::Affine {
    let start = handle_point(handle, start_bbox);
    let denom_x = (start.x - pivot.x) as f64;
    let denom_y = (start.y - pivot.y) as f64;

    let raw_sx = if denom_x.abs() < 1e-6 {
        1.0
    } else {
        (cursor_img.x - pivot.x) as f64 / denom_x
    };
    let raw_sy = if denom_y.abs() < 1e-6 {
        1.0
    } else {
        (cursor_img.y - pivot.y) as f64 / denom_y
    };

    let (mut sx, mut sy) = if is_edge_handle(handle) {
        match handle {
            Handle::Left | Handle::Right => (raw_sx, 1.0),
            _ => (1.0, raw_sy), // Top | Bottom
        }
    } else {
        (raw_sx, raw_sy)
    };

    if uniform {
        // Force a single magnitude (the larger), preserving per-axis sign so a
        // flipped drag still flips. Axes locked to exactly 1.0 (edge handles)
        // are left alone so a uniform edge drag stays single-axis.
        let mag = sx.abs().max(sy.abs());
        if !is_edge_handle(handle) {
            sx = mag * sx.signum();
            sy = mag * sy.signum();
        }
    }

    crate::transform::scale_about(pivot, sx, sy)
}

/// Unified delta dispatcher: selects `rotate_delta_for_drag`, `skew_delta_for_drag`,
/// or `scale_delta_for_drag` based on `handle` and modifier flags.
///
/// - `Handle::Rotate` → rotate (Shift snaps to 15°)
/// - Edge handle + `ctrl_held` → skew
/// - Otherwise → scale (`uniform` = Shift for corners)
fn transform_delta_for_drag(
    handle: Handle,
    start_bbox: egui::Rect,
    pivot: egui::Pos2,
    start_cursor: egui::Pos2,
    cursor: egui::Pos2,
    uniform: bool,
    ctrl_held: bool,
) -> kurbo::Affine {
    if handle == Handle::Rotate {
        rotate_delta_for_drag(pivot, start_cursor, cursor, uniform)
    } else if ctrl_held && is_edge_handle(handle) {
        skew_delta_for_drag(handle, pivot, start_cursor, cursor, start_bbox)
    } else {
        scale_delta_for_drag(handle, start_bbox, pivot, cursor, uniform)
    }
}

/// Squared distance from point `p` to line segment `(a, b)`, in image-pixel space.
fn point_to_segment_sq(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq < 1e-6 {
        return (p - a).length_sq();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    (p - closest).length_sq()
}

/// Returns true if `p` is within `radius` image-pixels of segment `(a, b)`.
fn hits_segment(p: Pos2, a: Pos2, b: Pos2, radius: f32) -> bool {
    point_to_segment_sq(p, a, b) <= radius * radius
}

/// True if `img_pos` hits `shape` precisely enough to count as a click.
/// Rect-like shapes use bounding-rect containment (with 4px padding).
/// Line/Arrow/Pen/BezierPath use segment proximity so overlapping bboxes
/// don't cause the latest-drawn shape to always win.
fn shape_hit(shape: &ShapeData, img_pos: Pos2, thickness: f32) -> bool {
    let radius = (thickness * 0.5 + LINE_HIT_RADIUS).max(LINE_HIT_RADIUS);
    match shape {
        ShapeData::Line { start, end, .. } | ShapeData::Arrow { start, end, .. } => {
            hits_segment(img_pos, *start, *end, radius)
        }
        ShapeData::Ruler { start, end } => hits_segment(img_pos, *start, *end, radius),
        ShapeData::Pen {
            smoothed_points,
            raw_points,
            ..
        } => {
            let pts = if smoothed_points.is_empty() {
                raw_points
            } else {
                smoothed_points
            };
            pts.windows(2)
                .any(|w| hits_segment(img_pos, w[0], w[1], radius))
        }
        _ => shape.bounding_rect().expand(4.0).contains(img_pos),
    }
}

/// If `click_screen` is near an ImageLayer corner for any annotation, return
/// (annotation id, anchor corner in image-pixel space, starting rect).
fn hit_image_handle(state: &AppState, click_screen: Pos2) -> Option<(uuid::Uuid, Pos2, Rect)> {
    let hittable = hittable_ids(&state.canvas.document);
    for ann in state.canvas.annotations.iter().rev() {
        if ann.locked || !hittable.contains(&ann.id) {
            continue;
        }
        let ShapeData::ImageLayer { rect, .. } = &ann.shape else {
            continue;
        };
        let corners_img = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ];
        let opposites_img = [
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
            rect.right_top(),
        ];
        for (c_img, anchor) in corners_img.iter().zip(opposites_img.iter()) {
            let c_screen = state.project(*c_img);
            if (c_screen - click_screen).length() <= HANDLE_HIT_RADIUS {
                return Some((ann.id, *anchor, *rect));
            }
        }
    }
    None
}

/// Ids of leaves that can receive a hit at `img_pos`, top-most first: a leaf is
/// hittable only if it is effectively visible AND not effectively locked.
pub(crate) fn hittable_leaf_at(
    doc: &crate::scene::Document,
    img_pos: egui::Pos2,
) -> Option<uuid::Uuid> {
    let leaves = doc.leaves_in_paint_order();
    leaves.iter().rev().find_map(|lv| {
        if !lv.effective_visible || lv.effective_locked {
            return None;
        }
        let local = if lv.world == kurbo::Affine::IDENTITY {
            img_pos
        } else {
            crate::transform::apply(lv.world.inverse(), img_pos)
        };
        if shape_hit(&lv.leaf.shape, local, lv.leaf.thickness) {
            Some(lv.id)
        } else {
            None
        }
    })
}

/// The set of leaf ids that are effectively visible AND not effectively locked,
/// used to gate the mirror-driven hit/hover loops by layer/group flags.
pub(crate) fn hittable_ids(doc: &crate::scene::Document) -> std::collections::HashSet<uuid::Uuid> {
    doc.leaves_in_paint_order()
        .iter()
        .filter(|lv| lv.effective_visible && !lv.effective_locked)
        .map(|lv| lv.id)
        .collect()
}

/// Find the topmost annotation under `img_pos` (image-pixel space).
fn hit_annotation(state: &AppState, img_pos: Pos2) -> Option<Uuid> {
    hittable_leaf_at(&state.canvas.document, img_pos)
}

/// The currently-selected leaf ids in Select mode (single or multi). Empty for
/// any non-selection tool state.
pub(crate) fn selected_ids(state: &AppState) -> Vec<Uuid> {
    match &state.tool_state {
        ToolState::Selected { id, .. } => vec![*id],
        ToolState::MultiSelected { ids, .. } => ids.clone(),
        _ => Vec::new(),
    }
}

/// World transform of `id`'s parent (identity if the parent is root / absent).
pub(crate) fn parent_world(doc: &crate::scene::Document, id: Uuid) -> kurbo::Affine {
    match doc.get(id).and_then(|n| n.parent) {
        Some(p) => doc.world_transform(p),
        None => kurbo::Affine::IDENTITY,
    }
}

/// Image-space AABB enclosing every selected leaf (each leaf's local
/// `bounding_rect()` transformed by its world). `None` if `ids` is empty or no
/// leaf resolves.
pub(crate) fn selection_bbox_img(state: &AppState, ids: &[Uuid]) -> Option<Rect> {
    let doc = &state.canvas.document;
    let items: Vec<(Rect, kurbo::Affine)> = ids
        .iter()
        .filter_map(|id| {
            let leaf = doc.leaf(*id)?;
            Some((leaf.shape.bounding_rect(), doc.world_transform(*id)))
        })
        .collect();
    crate::transform::selection_world_bbox(&items)
}

/// Returns the image-space bbox and screen-space [`GizmoLayout`] for the current
/// resting selection, or `None` when the gizmo should not be active:
///   - no selection,
///   - bbox cannot be resolved, or
///   - exactly one selected `ImageLayer` (keeps its legacy corner-resize handles).
///
/// This is the single source of truth shared by [`gizmo_overlay`] (draw) and
/// [`hit_gizmo_handle`] (hit-test) so neither path can diverge on the exclusion
/// condition.
fn active_gizmo(state: &AppState) -> Option<(Rect, crate::tools::gizmo::GizmoLayout)> {
    let ids = selected_ids(state);
    if ids.is_empty() {
        return None;
    }
    // A single selected ImageLayer keeps its dedicated resize handles — no gizmo.
    if ids.len() == 1 {
        if let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == ids[0]) {
            if matches!(ann.shape, ShapeData::ImageLayer { .. }) {
                return None;
            }
        }
    }
    let bbox_img = selection_bbox_img(state, &ids)?;
    let bbox_screen = Rect::from_min_max(state.project(bbox_img.min), state.project(bbox_img.max));
    Some((bbox_img, gizmo_layout(bbox_screen)))
}

/// Screen-space gizmo layout to draw for the current Select selection, if any.
///
/// Resting selection: delegates to [`active_gizmo`] which enforces the
/// single-ImageLayer exclusion. During a `Transforming` drag: the
/// live-previewed bbox (transformed by the in-progress delta) so the
/// gizmo tracks the cursor without mutating the document.
pub fn gizmo_overlay(state: &AppState, uniform: bool) -> Option<crate::tools::gizmo::GizmoLayout> {
    // During an active transform drag, preview the transformed bbox.
    if let ToolState::Transforming {
        handle,
        pivot,
        start_bbox,
        start_cursor,
        cursor,
        skew,
        ..
    } = &state.tool_state
    {
        // Use the frozen `skew` flag captured at drag-start so the live preview
        // always matches what will be committed on release (even if Ctrl has
        // since been released).
        let d = transform_delta_for_drag(
            *handle,
            *start_bbox,
            *pivot,
            *start_cursor,
            *cursor,
            uniform,
            *skew,
        );
        let p0 = crate::transform::apply(d, start_bbox.min);
        let p1 = crate::transform::apply(d, start_bbox.max);
        let bbox_img = Rect::from_two_pos(p0, p1);
        let bbox_screen =
            Rect::from_min_max(state.project(bbox_img.min), state.project(bbox_img.max));
        return Some(gizmo_layout(bbox_screen));
    }

    active_gizmo(state).map(|(_, layout)| layout)
}

/// If `screen_pos` is over any transform handle of the current selection's gizmo,
/// return the grabbed `Handle` and the image-space `start_bbox`. All nine handles
/// (eight scale/skew handles plus the rotate handle) are hit-testable.
///
/// Uses [`active_gizmo`] for the exclusion check, so a single `ImageLayer`
/// selection produces no hit-testable handles (matching the draw path).
fn hit_gizmo_handle(state: &AppState, screen_pos: Pos2) -> Option<(Handle, Rect)> {
    let (bbox_img, layout) = active_gizmo(state)?;
    match handle_at(&layout, screen_pos, GIZMO_HIT_RADIUS) {
        None => None,
        Some(h) => Some((h, bbox_img)),
    }
}

pub fn handle(
    response: &Response,
    ctx: &Context,
    state: &mut AppState,
    _canvas_rect: Rect,
) -> Option<Command> {
    let shift_held = ctx.input(|i| i.modifiers.shift);

    // ── Image-layer resize takes priority ─────────────────────────────────────
    if response.drag_started() {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            if let Some((id, anchor, start_rect)) = hit_image_handle(state, screen_pos) {
                state.tool_state = ToolState::ResizingImage {
                    id,
                    anchor,
                    start_rect,
                };
            }
        }
    }

    // ── Transform gizmo (scale / rotate / skew) takes priority over move/marquee ──
    // Grabbing any handle of the current selection's gizmo enters `Transforming`
    // instead of starting a move or rubber-band.
    if response.drag_started() && !matches!(state.tool_state, ToolState::ResizingImage { .. }) {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            if let Some((handle, start_bbox)) = hit_gizmo_handle(state, screen_pos) {
                let alt_held = ctx.input(|i| i.modifiers.alt);
                // Rotate always pivots about bbox center.
                // Scale: pivot is the opposite handle, or center if Alt held.
                let pivot = if handle == Handle::Rotate {
                    start_bbox.center()
                } else if alt_held {
                    start_bbox.center()
                } else {
                    handle_point(opposite_handle(handle), start_bbox)
                };
                let ids = selected_ids(state);
                let start: Vec<(Uuid, kurbo::Affine)> = ids
                    .iter()
                    .filter_map(|id| state.canvas.document.get(*id).map(|n| (*id, n.transform)))
                    .collect();
                let cursor_img = state.unproject(screen_pos);
                // Freeze the skew mode at drag-start so releasing Ctrl before
                // the mouse-up still commits a skew, not a scale.
                let ctrl_at_press = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
                let skew = is_edge_handle(handle) && ctrl_at_press;
                state.tool_state = ToolState::Transforming {
                    handle,
                    pivot,
                    start_bbox,
                    start_cursor: cursor_img,
                    cursor: cursor_img,
                    start,
                    skew,
                };
            }
        }
    }

    // ── Active transform drag (scale / rotate / skew) ────────────────────────
    if let ToolState::Transforming { .. } = state.tool_state {
        if response.dragged() {
            if let Some(screen_pos) = response.interact_pointer_pos() {
                let cursor_img = state.unproject(screen_pos);
                if let ToolState::Transforming { ref mut cursor, .. } = state.tool_state {
                    *cursor = cursor_img;
                }
            }
        }
        if response.drag_stopped() {
            let uniform = shift_held;
            let (handle, pivot, start_bbox, start_cursor, cursor, start, skew) =
                if let ToolState::Transforming {
                    handle,
                    pivot,
                    start_bbox,
                    start_cursor,
                    cursor,
                    ref start,
                    skew,
                } = state.tool_state
                {
                    (
                        handle,
                        pivot,
                        start_bbox,
                        start_cursor,
                        cursor,
                        start.clone(),
                        skew,
                    )
                } else {
                    unreachable!()
                };

            // Use the frozen `skew` flag captured at drag-start — not a freshly-read
            // Ctrl modifier — so releasing Ctrl a frame before mouse-up still
            // correctly commits a skew rather than silently falling back to scale.
            let d = transform_delta_for_drag(
                handle,
                start_bbox,
                pivot,
                start_cursor,
                cursor,
                uniform,
                skew,
            );

            // Only commit if the drag actually changed scale (avoids a no-op
            // undo entry on a click-sized wiggle).
            let coeffs = d.as_coeffs();
            let is_identity = (coeffs[0] - 1.0).abs() < 1e-6
                && coeffs[1].abs() < 1e-6
                && coeffs[2].abs() < 1e-6
                && (coeffs[3] - 1.0).abs() < 1e-6
                && coeffs[4].abs() < 1e-3
                && coeffs[5].abs() < 1e-3;

            if !is_identity && !start.is_empty() {
                let cmds: Vec<Command> = start
                    .iter()
                    .map(|(id, start_local)| {
                        let pw = parent_world(&state.canvas.document, *id);
                        let new_local = crate::transform::apply_world_delta(pw, *start_local, d);
                        Command::SetNodeProp {
                            id: *id,
                            prop: NodeProp::Transform(new_local),
                        }
                    })
                    .collect();
                let cmd = if cmds.len() == 1 {
                    cmds.into_iter().next().unwrap()
                } else {
                    Command::Batch(cmds)
                };
                crate::command::CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    cmd,
                );
            }

            // Return to a resting selection over the same ids.
            let ids: Vec<Uuid> = start.iter().map(|(id, _)| *id).collect();
            state.tool_state = match ids.len() {
                0 => ToolState::Idle,
                1 => ToolState::Selected {
                    id: ids[0],
                    drag_offset: None,
                },
                _ => ToolState::MultiSelected {
                    ids,
                    drag_offset: None,
                    band_start: None,
                    band_current: None,
                },
            };
        }
        return None;
    }

    if let ToolState::ResizingImage {
        id,
        anchor,
        start_rect,
    } = state.tool_state
    {
        if response.dragged() {
            if let Some(screen_pos) = response.interact_pointer_pos() {
                let cur_img = state.unproject(screen_pos);
                let new_rect = Rect::from_two_pos(anchor, cur_img);
                if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                    leaf.shape.set_rect(new_rect);
                }
                state.canvas.sync_annotations();
            }
        }
        if response.drag_stopped() {
            let final_rect = state
                .canvas
                .annotations
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.shape.bounding_rect())
                .unwrap_or(start_rect);
            if (final_rect.min - start_rect.min).length() > 0.5
                || (final_rect.max - start_rect.max).length() > 0.5
            {
                // Revert the visible mutation so execute() reapplies through the proper channel.
                if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                    leaf.shape.set_rect(start_rect);
                }
                state.canvas.sync_annotations();
                let applied = crate::command::CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Resize {
                        id,
                        new_rect: final_rect,
                    },
                );
                if !applied {
                    // Shape became locked between drag-start and drag-end.
                    // Re-apply the visible resize so the user's gesture isn't silently lost.
                    // Note: this mutation is NOT in the undo history since execute() rejected it;
                    // next agent action on the shape will resync state.
                    if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                        leaf.shape.set_rect(final_rect);
                    }
                    state.canvas.sync_annotations();
                }
            }
            state.tool_state = ToolState::Selected {
                id,
                drag_offset: None,
            };
        }
        return None;
    }

    // ── Double-click: re-edit existing Text / Question shapes ────────────────
    if response.double_clicked() {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            let img_pos = state.unproject(screen_pos);
            if let Some(id) = hit_annotation(state, img_pos) {
                if let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == id) {
                    let maybe_edit = match &ann.shape {
                        ShapeData::Text { pos, content, size } => {
                            Some((*pos, content.clone(), ann.color, *size))
                        }
                        ShapeData::Question { pos, content, size } => {
                            Some((*pos, content.clone(), ann.color, *size))
                        }
                        _ => None,
                    };
                    if let Some((pos, content, color, size)) = maybe_edit {
                        state.tool_state = ToolState::EditingText {
                            pos,
                            buffer: content,
                            color,
                            size,
                            editing_id: Some(id),
                        };
                        return None;
                    }
                }
            }
        }
    }

    // ── Click handling (primary button) ──────────────────────────────────────
    if response.clicked() {
        let click_screen = response.interact_pointer_pos()?;
        let click_img = state.unproject(click_screen);
        let hit_id = hit_annotation(state, click_img);

        if shift_held {
            // Shift+click: add/remove from selection.
            match &state.tool_state {
                ToolState::Selected {
                    id: existing_id, ..
                } => {
                    let existing = *existing_id;
                    if let Some(new_id) = hit_id {
                        if new_id == existing {
                            // Toggle off the only selected item.
                            state.tool_state = ToolState::Idle;
                        } else {
                            state.tool_state = ToolState::MultiSelected {
                                ids: vec![existing, new_id],
                                drag_offset: None,
                                band_start: None,
                                band_current: None,
                            };
                        }
                    }
                    // Shift+click on empty: keep current single selection.
                }
                ToolState::MultiSelected { ids, .. } => {
                    let mut ids = ids.clone();
                    if let Some(new_id) = hit_id {
                        if let Some(pos) = ids.iter().position(|&x| x == new_id) {
                            ids.remove(pos);
                        } else {
                            ids.push(new_id);
                        }
                    }
                    state.tool_state = match ids.len() {
                        0 => ToolState::Idle,
                        1 => ToolState::Selected {
                            id: ids[0],
                            drag_offset: None,
                        },
                        _ => ToolState::MultiSelected {
                            ids,
                            drag_offset: None,
                            band_start: None,
                            band_current: None,
                        },
                    };
                }
                _ => {
                    // No current selection — shift+click on annotation selects it.
                    if let Some(new_id) = hit_id {
                        state.tool_state = ToolState::Selected {
                            id: new_id,
                            drag_offset: None,
                        };
                    }
                }
            }
        } else {
            // Normal click: single-select or deselect.
            state.tool_state = match hit_id {
                Some(id) => ToolState::Selected {
                    id,
                    drag_offset: None,
                },
                None => ToolState::Idle,
            };
        }
        return None;
    }

    // ── Drag started ─────────────────────────────────────────────────────────
    if response.drag_started() {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            let img_pos = state.unproject(screen_pos);
            let hit_id = hit_annotation(state, img_pos);

            match &state.tool_state {
                ToolState::Selected { id, .. } => {
                    let existing_id = *id;
                    if hit_id.is_some() {
                        // Start drag on the currently-selected (or any) annotation.
                        if let ToolState::Selected {
                            ref mut drag_offset,
                            ..
                        } = state.tool_state
                        {
                            *drag_offset = Some(Vec2::ZERO);
                        }
                    } else if shift_held {
                        // Shift+drag on empty: start rubber band keeping current selection.
                        state.tool_state = ToolState::MultiSelected {
                            ids: vec![existing_id],
                            drag_offset: None,
                            band_start: Some(img_pos),
                            band_current: Some(img_pos),
                        };
                    } else {
                        // Drag on empty: start rubber band (deselects).
                        state.tool_state = ToolState::MultiSelected {
                            ids: vec![],
                            drag_offset: None,
                            band_start: Some(img_pos),
                            band_current: Some(img_pos),
                        };
                    }
                }
                ToolState::MultiSelected { .. } => {
                    let ids_clone: Vec<Uuid> =
                        if let ToolState::MultiSelected { ref ids, .. } = state.tool_state {
                            ids.clone()
                        } else {
                            vec![]
                        };

                    if let Some(hid) = hit_id {
                        if ids_clone.contains(&hid) || ids_clone.is_empty() {
                            // Drag started on one of the selected annotations → move all.
                            if let ToolState::MultiSelected {
                                ref mut drag_offset,
                                ..
                            } = state.tool_state
                            {
                                *drag_offset = Some(Vec2::ZERO);
                            }
                        } else {
                            // Drag started on unselected annotation → single-select and drag.
                            state.tool_state = ToolState::Selected {
                                id: hid,
                                drag_offset: Some(Vec2::ZERO),
                            };
                        }
                    } else {
                        // Drag on empty: start rubber band.
                        let base_ids = if shift_held { ids_clone } else { vec![] };
                        state.tool_state = ToolState::MultiSelected {
                            ids: base_ids,
                            drag_offset: None,
                            band_start: Some(img_pos),
                            band_current: Some(img_pos),
                        };
                    }
                }
                ToolState::Idle => {
                    if let Some(hid) = hit_id {
                        state.tool_state = ToolState::Selected {
                            id: hid,
                            drag_offset: Some(Vec2::ZERO),
                        };
                    } else {
                        // Start rubber band from idle.
                        state.tool_state = ToolState::MultiSelected {
                            ids: vec![],
                            drag_offset: None,
                            band_start: Some(img_pos),
                            band_current: Some(img_pos),
                        };
                    }
                }
                _ => {}
            }
        }
    }

    // ── Drag held ─────────────────────────────────────────────────────────────
    if response.dragged() {
        let raw_delta = response.drag_delta();
        let img_delta = state.screen_delta_to_img(raw_delta);

        // Single-annotation drag.
        if matches!(
            state.tool_state,
            ToolState::Selected {
                drag_offset: Some(_),
                ..
            }
        ) {
            if img_delta.length_sq() > 0.0 {
                let id = if let ToolState::Selected { id, .. } = state.tool_state {
                    id
                } else {
                    unreachable!()
                };
                if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                    leaf.shape.translate(img_delta);
                }
                state.canvas.sync_annotations();
                if let ToolState::Selected {
                    ref mut drag_offset,
                    ..
                } = state.tool_state
                {
                    if let Some(acc) = drag_offset {
                        *acc += img_delta;
                    }
                }
            }
        }

        // Multi-annotation drag.
        if matches!(
            state.tool_state,
            ToolState::MultiSelected {
                drag_offset: Some(_),
                ..
            }
        ) {
            if img_delta.length_sq() > 0.0 {
                let ids: Vec<Uuid> =
                    if let ToolState::MultiSelected { ref ids, .. } = state.tool_state {
                        ids.clone()
                    } else {
                        vec![]
                    };
                for id in &ids {
                    if let Some(leaf) = state.canvas.document.leaf_mut(*id) {
                        leaf.shape.translate(img_delta);
                    }
                }
                state.canvas.sync_annotations();
                if let ToolState::MultiSelected {
                    ref mut drag_offset,
                    ..
                } = state.tool_state
                {
                    if let Some(acc) = drag_offset {
                        *acc += img_delta;
                    }
                }
            }
        }

        // Rubber-band update.
        if matches!(
            state.tool_state,
            ToolState::MultiSelected {
                band_start: Some(_),
                ..
            }
        ) {
            if let Some(screen_pos) = response.interact_pointer_pos() {
                let img_pos = state.unproject(screen_pos);
                if let ToolState::MultiSelected {
                    ref mut band_current,
                    ..
                } = state.tool_state
                {
                    *band_current = Some(img_pos);
                }
            }
        }
    }

    // ── Drag released ─────────────────────────────────────────────────────────
    if response.drag_stopped() {
        // Single-annotation drag released.
        if matches!(state.tool_state, ToolState::Selected { .. }) {
            let (id, total) = if let ToolState::Selected {
                id,
                ref mut drag_offset,
            } = state.tool_state
            {
                (id, drag_offset.take())
            } else {
                unreachable!()
            };
            if let Some(total_delta) = total {
                if total_delta.length_sq() > 0.01_f32 {
                    // Revert the visible translation so execute() reapplies through the proper channel.
                    if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                        leaf.shape.translate(-total_delta);
                    }
                    state.canvas.sync_annotations();
                    let applied = crate::command::CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::Move {
                            id,
                            delta: total_delta,
                        },
                    );
                    if !applied {
                        // Shape became locked between drag-start and drag-end.
                        // Re-apply the visible translation so the user's gesture isn't silently lost.
                        // Note: this mutation is NOT in the undo history since execute() rejected it;
                        // next agent action on the shape will resync state.
                        if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                            leaf.shape.translate(total_delta);
                        }
                        state.canvas.sync_annotations();
                    }
                }
            }
        }

        // Multi-annotation drag released: emit Batch Move.
        if matches!(
            state.tool_state,
            ToolState::MultiSelected {
                drag_offset: Some(_),
                ..
            }
        ) {
            let (ids, total) = if let ToolState::MultiSelected {
                ref ids,
                ref mut drag_offset,
                ..
            } = state.tool_state
            {
                (ids.clone(), drag_offset.take())
            } else {
                unreachable!()
            };
            if let Some(total_delta) = total {
                if total_delta.length_sq() > 0.01_f32 {
                    // Revert each visible translation so execute() reapplies through the proper channel.
                    for &id in &ids {
                        if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                            leaf.shape.translate(-total_delta);
                        }
                    }
                    state.canvas.sync_annotations();
                    let cmds: Vec<Command> = ids
                        .iter()
                        .map(|&id| Command::Move {
                            id,
                            delta: total_delta,
                        })
                        .collect();
                    let applied = crate::command::CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::Batch(cmds),
                    );
                    if !applied {
                        // One or more shapes became locked between drag-start and drag-end.
                        // Re-apply the visible translations so the user's gesture isn't silently lost.
                        // Note: this mutation is NOT in the undo history since execute() rejected it;
                        // next agent action on the shapes will resync state.
                        for &id in &ids {
                            if let Some(leaf) = state.canvas.document.leaf_mut(id) {
                                leaf.shape.translate(total_delta);
                            }
                        }
                        state.canvas.sync_annotations();
                    }
                }
            }
        }

        // Rubber band released: collect intersecting annotations.
        if matches!(
            state.tool_state,
            ToolState::MultiSelected {
                band_start: Some(_),
                ..
            }
        ) {
            let (band_s, band_c, existing_ids) = if let ToolState::MultiSelected {
                band_start,
                band_current,
                ref ids,
                ..
            } = state.tool_state
            {
                (band_start, band_current, ids.clone())
            } else {
                unreachable!()
            };

            if let (Some(s), Some(c)) = (band_s, band_c) {
                let band_rect = Rect::from_two_pos(s, c);
                let hittable = hittable_ids(&state.canvas.document);
                let mut new_ids = existing_ids;
                for ann in &state.canvas.annotations {
                    if !ann.locked
                        && hittable.contains(&ann.id)
                        && band_rect.intersects(ann.shape.bounding_rect())
                        && !new_ids.contains(&ann.id)
                    {
                        new_ids.push(ann.id);
                    }
                }
                state.tool_state = match new_ids.len() {
                    0 => ToolState::Idle,
                    1 => ToolState::Selected {
                        id: new_ids[0],
                        drag_offset: None,
                    },
                    _ => ToolState::MultiSelected {
                        ids: new_ids,
                        drag_offset: None,
                        band_start: None,
                        band_current: None,
                    },
                };
            } else {
                // Band had no extent — just clear band fields.
                if let ToolState::MultiSelected { ref ids, .. } = state.tool_state {
                    let l = ids.len();
                    let ids_c = ids.clone();
                    state.tool_state = match l {
                        0 => ToolState::Idle,
                        1 => ToolState::Selected {
                            id: ids_c[0],
                            drag_offset: None,
                        },
                        _ => ToolState::MultiSelected {
                            ids: ids_c,
                            drag_offset: None,
                            band_start: None,
                            band_current: None,
                        },
                    };
                }
            }
        }
    }

    None
}

/// Build a flip command for `ids` without executing it.
///
/// Computes the shared selection bounding box center in world space, constructs
/// `D = flip_about(center, horizontal)`, then for each id derives the new local
/// transform via `apply_world_delta`. Returns a single `SetNodeProp` for one id,
/// a `Batch` for multiple ids, or `None` if `ids` is empty or the bbox can't be
/// computed.
///
/// Called from `toolbar.rs` (binary crate). The lint suppression avoids a false
/// dead_code warning when compiling the lib target in isolation.
#[allow(dead_code)]
pub fn flip_command(
    canvas: &crate::state::CanvasState,
    ids: &[uuid::Uuid],
    horizontal: bool,
) -> Option<crate::types::Command> {
    if ids.is_empty() {
        return None;
    }
    let doc = &canvas.document;
    let items: Vec<(egui::Rect, kurbo::Affine)> = ids
        .iter()
        .filter_map(|id| {
            let leaf = doc.leaf(*id)?;
            Some((leaf.shape.bounding_rect(), doc.world_transform(*id)))
        })
        .collect();
    let bbox = crate::transform::selection_world_bbox(&items)?;
    let center = bbox.center();
    let d = crate::transform::flip_about(center, horizontal);

    let cmds: Vec<crate::types::Command> = ids
        .iter()
        .filter_map(|id| {
            let node = doc.get(*id)?;
            let pw = parent_world(doc, *id);
            let new_local = crate::transform::apply_world_delta(pw, node.transform, d);
            Some(crate::types::Command::SetNodeProp {
                id: *id,
                prop: crate::types::NodeProp::Transform(new_local),
            })
        })
        .collect();

    if cmds.is_empty() {
        return None;
    }
    if cmds.len() == 1 {
        Some(cmds.into_iter().next().unwrap())
    } else {
        Some(crate::types::Command::Batch(cmds))
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;
    use crate::tools::gizmo::Handle;

    #[test]
    fn dragging_right_edge_scales_x_only() {
        let bb = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let pivot = egui::pos2(0.0, 50.0); // opposite (left) edge anchored
        let d = scale_delta_for_drag(Handle::Right, bb, pivot, egui::pos2(200.0, 50.0), false);
        // x doubled, y unchanged
        let p = crate::transform::apply(d, egui::pos2(100.0, 100.0));
        assert!((p.x - 200.0).abs() < 1e-3 && (p.y - 100.0).abs() < 1e-3);
    }

    #[test]
    fn dragging_corner_uniform_locks_aspect() {
        let bb = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let pivot = egui::pos2(0.0, 0.0); // TopLeft anchored, drag BottomRight
                                          // Non-uniform target: x*2, y*1.5
        let cursor = egui::pos2(200.0, 150.0);
        let d = scale_delta_for_drag(Handle::BottomRight, bb, pivot, cursor, true);
        // uniform → both axes take the larger factor (2.0)
        let p = crate::transform::apply(d, egui::pos2(100.0, 100.0));
        assert!((p.x - 200.0).abs() < 1e-3, "x={}", p.x);
        assert!((p.y - 200.0).abs() < 1e-3, "y={}", p.y);
    }

    #[test]
    fn dragging_corner_nonuniform_scales_both_axes() {
        let bb = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let pivot = egui::pos2(0.0, 0.0);
        let d = scale_delta_for_drag(
            Handle::BottomRight,
            bb,
            pivot,
            egui::pos2(200.0, 150.0),
            false,
        );
        let p = crate::transform::apply(d, egui::pos2(100.0, 100.0));
        assert!((p.x - 200.0).abs() < 1e-3 && (p.y - 150.0).abs() < 1e-3);
    }

    #[test]
    fn degenerate_extent_does_not_panic_and_locks_to_one() {
        // Zero-width bbox: dragging a side that has no extent → factor 1.0.
        let bb = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(0.0, 100.0));
        let pivot = egui::pos2(0.0, 50.0);
        let d = scale_delta_for_drag(Handle::Right, bb, pivot, egui::pos2(50.0, 50.0), false);
        let p = crate::transform::apply(d, egui::pos2(0.0, 100.0));
        // x stays 0 (no extent), y unchanged (edge handle)
        assert!((p.x - 0.0).abs() < 1e-3 && (p.y - 100.0).abs() < 1e-3);
    }
}

#[cfg(test)]
mod hit_tests {
    use super::*;
    use crate::scene::Document;
    use crate::types::{Annotation, ShapeData};
    use egui::{Color32, Pos2, Rect};

    fn doc_with_rect() -> (Document, uuid::Uuid, uuid::Uuid) {
        let mut c = crate::state::CanvasState::default();
        let layer = c.document.default_layer();
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(50.0, 50.0))),
            Color32::RED,
            2.0,
        );
        let id = a.id;
        c.add_annotation(a);
        (c.document, layer, id)
    }

    #[test]
    fn hidden_layer_is_not_hittable() {
        let (mut doc, layer, id) = doc_with_rect();
        assert_eq!(hittable_leaf_at(&doc, Pos2::new(10.0, 10.0)), Some(id));
        doc.get_mut(layer).unwrap().visible = false;
        assert_eq!(hittable_leaf_at(&doc, Pos2::new(10.0, 10.0)), None);
    }

    #[test]
    fn locked_layer_is_not_hittable() {
        let (mut doc, layer, _id) = doc_with_rect();
        doc.get_mut(layer).unwrap().locked = true;
        assert_eq!(hittable_leaf_at(&doc, Pos2::new(10.0, 10.0)), None);
    }

    #[test]
    fn hit_test_respects_node_transform() {
        use kurbo::Affine;
        let mut c = crate::state::CanvasState::default();
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0))),
            Color32::RED,
            2.0,
        );
        let id = a.id;
        c.add_annotation(a);
        // translate the leaf +100px in x via its node transform
        c.document.get_mut(id).unwrap().transform = Affine::translate((100.0, 0.0));
        // original location now empty:
        assert_eq!(hittable_leaf_at(&c.document, egui::pos2(5.0, 5.0)), None);
        // translated location hits:
        assert_eq!(
            hittable_leaf_at(&c.document, egui::pos2(105.0, 5.0)),
            Some(id)
        );
    }
}

#[cfg(test)]
mod rotate_tests {
    use super::*;

    #[test]
    fn rotate_drag_quarter_turn() {
        let pivot = egui::pos2(0.0, 0.0);
        let d = rotate_delta_for_drag(pivot, egui::pos2(10.0, 0.0), egui::pos2(0.0, 10.0), false);
        let p = crate::transform::apply(d, egui::pos2(10.0, 0.0));
        assert!((p.x - 0.0).abs() < 1e-3 && (p.y - 10.0).abs() < 1e-3);
    }

    #[test]
    fn rotate_snaps_to_15_degrees() {
        let pivot = egui::pos2(0.0, 0.0);
        // a ~17° drag should snap to 15°
        let d = rotate_delta_for_drag(
            pivot,
            egui::pos2(10.0, 0.0),
            crate::transform::apply(
                crate::transform::rotate_about(pivot, 17f64.to_radians()),
                egui::pos2(10.0, 0.0),
            ),
            true,
        );
        let ang = crate::transform::decompose(d).rotation.to_degrees();
        assert!((ang - 15.0).abs() < 0.5, "got {ang}");
    }

    #[test]
    fn rotate_pivot_stays_fixed() {
        // The pivot point must be invariant under the rotation delta.
        let pivot = egui::pos2(50.0, 50.0);
        let d = rotate_delta_for_drag(pivot, egui::pos2(60.0, 50.0), egui::pos2(50.0, 60.0), false);
        let p = crate::transform::apply(d, pivot);
        assert!((p.x - pivot.x).abs() < 1e-3 && (p.y - pivot.y).abs() < 1e-3);
    }

    #[test]
    fn rotate_zero_drag_is_identity() {
        // When start_cursor == cursor the angle is 0 → identity.
        let pivot = egui::pos2(0.0, 0.0);
        let d = rotate_delta_for_drag(pivot, egui::pos2(10.0, 5.0), egui::pos2(10.0, 5.0), false);
        let p = crate::transform::apply(d, egui::pos2(7.0, 3.0));
        assert!((p.x - 7.0).abs() < 1e-3 && (p.y - 3.0).abs() < 1e-3);
    }
}

#[cfg(test)]
mod skew_tests {
    use super::*;

    /// A right-edge (Left handle) drag downward by half the bbox width should
    /// produce ky = 0.5, so a point on the right edge of a unit square maps
    /// with y shifted by 0.5 × its x distance from the pivot.
    ///
    /// Convention: Left/Right → y-skew (`ky = dy / bbox_width`).
    /// The skew_about(pivot, 0, ky) shear maps (x, y) → (x, y + ky * x) relative
    /// to pivot. With pivot at (0,0), (1, 0) → (1, ky).
    #[test]
    fn skew_right_edge_maps_corner_correctly() {
        // bbox: (0,0)-(1,1); pivot at bbox center (0.5, 0.5) opposite the Right handle.
        let bbox = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let pivot = egui::pos2(0.0, 0.5); // opposite (Left) side, center-y
                                          // Drag the Right edge handle downward by 0.5 (half the bbox width=1).
        let start = egui::pos2(1.0, 0.5);
        let end = egui::pos2(1.0, 1.0); // dy = 0.5 → ky = 0.5 / 1.0 = 0.5
        let d = skew_delta_for_drag(Handle::Right, pivot, start, end, bbox);
        // Point at the Right-edge midpoint (1, 0.5): relative to pivot (0, 0.5)
        // local coords = (1, 0). Under ky=0.5 skew: → (1, 0 + 0.5*1) = local (1, 0.5).
        // World: (1 + 0, 0.5 + 0.5) = (1, 1).
        let p = crate::transform::apply(d, egui::pos2(1.0, 0.5));
        assert!((p.x - 1.0).abs() < 1e-3, "x={}", p.x);
        assert!((p.y - 1.0).abs() < 1e-3, "y={}", p.y);
    }

    #[test]
    fn skew_top_edge_maps_corner_correctly() {
        // bbox: (0,0)-(100,100); pivot at bbox center-x, Bottom edge.
        let bbox = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let pivot = egui::pos2(50.0, 100.0); // Bottom edge center
                                             // Drag the Top edge handle horizontally by 50px → kx = 50/100 = 0.5
        let start = egui::pos2(50.0, 0.0);
        let end = egui::pos2(100.0, 0.0); // dx = 50 → kx = 50/100 = 0.5
        let d = skew_delta_for_drag(Handle::Top, pivot, start, end, bbox);
        // Point at top-left (0, 0): relative to pivot (50, 100) → (-50, -100).
        // kx=0.5 skew: x' = x + kx*y = -50 + 0.5*(-100) = -100. World: -100+50 = -50.
        let p = crate::transform::apply(d, egui::pos2(0.0, 0.0));
        assert!((p.x - (-50.0)).abs() < 1e-3, "x={}", p.x);
        assert!((p.y - 0.0).abs() < 1e-3, "y={}", p.y);
    }

    #[test]
    fn skew_zero_drag_is_near_identity() {
        let bbox = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let pivot = egui::pos2(0.0, 50.0);
        let cursor = egui::pos2(100.0, 50.0);
        let d = skew_delta_for_drag(Handle::Right, pivot, cursor, cursor, bbox);
        // ky = 0 → identity
        let p = crate::transform::apply(d, egui::pos2(50.0, 25.0));
        assert!((p.x - 50.0).abs() < 1e-3 && (p.y - 25.0).abs() < 1e-3);
    }
}

#[cfg(test)]
mod flip_tests {
    use super::*;
    use crate::command::CommandExecutor;
    use crate::types::{Annotation, ParticipantId, ShapeData};
    use egui::{Color32, Rect};

    fn canvas_with_rect(r: Rect) -> (crate::state::CanvasState, uuid::Uuid) {
        let mut c = crate::state::CanvasState::default();
        let ann = Annotation::new(ShapeData::plain_rect(r), Color32::RED, 2.0);
        let id = ann.id;
        c.add_annotation(ann);
        (c, id)
    }

    #[test]
    fn flip_h_mirrors_selection_about_center() {
        let (mut canvas, id) = canvas_with_rect(Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(10.0, 10.0),
        ));
        let cmd = flip_command(&canvas, &[id], true).unwrap();
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, cmd);
        // A left-edge point (0, 5) should now sit at the right edge (x=10),
        // mirrored about center x=5.
        let w = canvas.document.world_transform(id);
        let p = crate::transform::apply(w, egui::pos2(0.0, 5.0));
        assert!((p.x - 10.0).abs() < 1e-3, "expected x≈10, got {}", p.x);
    }

    #[test]
    fn flip_v_mirrors_selection_about_center() {
        let (mut canvas, id) = canvas_with_rect(Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(10.0, 10.0),
        ));
        let cmd = flip_command(&canvas, &[id], false).unwrap();
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, cmd);
        // A top-edge point (5, 0) should now sit at the bottom edge (y=10),
        // mirrored about center y=5.
        let w = canvas.document.world_transform(id);
        let p = crate::transform::apply(w, egui::pos2(5.0, 0.0));
        assert!((p.y - 10.0).abs() < 1e-3, "expected y≈10, got {}", p.y);
    }

    #[test]
    fn flip_empty_ids_returns_none() {
        let (canvas, _) = canvas_with_rect(Rect::ZERO);
        assert!(flip_command(&canvas, &[], true).is_none());
    }

    #[test]
    fn flip_h_is_undoable() {
        let (mut canvas, id) = canvas_with_rect(Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(10.0, 10.0),
        ));
        let original_transform = canvas.document.get(id).unwrap().transform;
        let cmd = flip_command(&canvas, &[id], true).unwrap();
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, cmd);
        CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
        let restored = canvas.document.get(id).unwrap().transform;
        let (oc, rc) = (original_transform.as_coeffs(), restored.as_coeffs());
        for i in 0..6 {
            assert!(
                (oc[i] - rc[i]).abs() < 1e-6,
                "coeff {i} mismatch after undo"
            );
        }
    }
}
