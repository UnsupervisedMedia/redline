use crate::{
    command::CommandExecutor,
    stamps::{rasterize_stamp, stamp_by_id},
    state::{AppState, SmoothingMode, Tool, ToolState},
    types::{
        star_points, Annotation, ArrowHeads, BoolOp, Command, LineStyle, ParticipantId, ShapeData,
    },
};
use egui::{Color32, Context, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use uuid::Uuid;

pub const HANDLE_SIZE: f32 = 10.0;

/// Returns the id of the currently selected ImageLayer annotation, if any.
pub fn selected_image_id(state: &AppState) -> Option<Uuid> {
    let id = match state.tool_state {
        ToolState::Selected { id, .. } => id,
        ToolState::ResizingImage { id, .. } => id,
        _ => return None,
    };
    let ann = state.canvas.annotations.iter().find(|a| a.id == id)?;
    matches!(ann.shape, ShapeData::ImageLayer { .. }).then_some(id)
}

/// Four corner points of a rect in screen coords, for drawing resize handles.
pub fn image_layer_corners(r: Rect) -> [Pos2; 4] {
    [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ]
}

/// Lazily create a `TextureHandle` for each ImageLayer annotation that doesn't
/// yet have one. Called from `show()` before any drawing happens.
fn upload_image_layer_textures(ctx: &Context, state: &mut AppState) {
    let mut created = false;
    let ids: Vec<uuid::Uuid> = state
        .canvas
        .document
        .leaves_in_paint_order()
        .iter()
        .map(|lv| lv.id)
        .collect();
    for id in ids {
        if let Some(leaf) = state.canvas.document.leaf_mut(id) {
            if let ShapeData::ImageLayer { data, texture, .. } = &mut leaf.shape {
                if texture.0.is_none() {
                    let (w, h) = (data.width() as usize, data.height() as usize);
                    let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], data.as_raw());
                    texture.0 = Some(crate::texture_upload::load_texture_fitted(
                        ctx,
                        format!("img-layer-{id}"),
                        ci,
                        egui::TextureOptions::LINEAR,
                    ));
                    created = true;
                }
            }
        }
    }
    if created {
        state.canvas.sync_annotations();
    }
}

/// Compute the path-op selection gates from the current tool selection:
/// `(closed_paths, open_paths)` — the selected BezierPath leaf ids split by
/// closedness. Shared by the canvas context menu, the Path menu, and the
/// Pathfinder strip so their enablement never drifts. GUI-free.
pub(crate) fn pathfinder_selection(state: &AppState) -> (Vec<Uuid>, Vec<Uuid>) {
    let sel_ids: Vec<Uuid> = match &state.tool_state {
        ToolState::Selected { id, .. } => vec![*id],
        ToolState::MultiSelected { ids, .. } => ids.clone(),
        _ => Vec::new(),
    };
    let closed_paths: Vec<Uuid> = sel_ids
        .iter()
        .copied()
        .filter(|id| {
            matches!(
                state.canvas.document.leaf(*id).map(|l| &l.shape),
                Some(ShapeData::BezierPath { closed: true, .. })
            )
        })
        .collect();
    let open_paths: Vec<Uuid> = sel_ids
        .iter()
        .copied()
        .filter(|id| {
            matches!(
                state.canvas.document.leaf(*id).map(|l| &l.shape),
                Some(ShapeData::BezierPath { closed: false, .. })
            )
        })
        .collect();
    (closed_paths, open_paths)
}

pub fn show(ui: &mut Ui, state: &mut AppState) {
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
    let canvas_rect = response.rect;

    // Lazily upload textures for ImageLayer annotations (paste-in overlays).
    upload_image_layer_textures(ui.ctx(), state);

    if state.canvas.texture.is_none() {
        draw_empty_backdrop(&painter, canvas_rect, state);
    }

    // Compute base image rect (fit-to-window at zoom=1). Annotations live in
    // this coordinate space. Stash it before applying zoom/pan so tools and
    // flatten can reference it.
    let base_rect = state.canvas.texture.as_ref().map(|_| {
        let img_size = egui::vec2(state.canvas.canvas_size.x, state.canvas.canvas_size.y);
        fit_rect(img_size, canvas_rect)
    });
    state.last_img_draw_rect = base_rect;

    // Handle zoom (Ctrl+wheel) and pan (middle-drag). '0' resets.
    handle_view_input(ui.ctx(), &response, state, canvas_rect);

    // Crosshair cursor when any drawing tool is active over the canvas.
    if !matches!(state.current_tool, Tool::Select) && response.hover_pos().is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    // view_rect = base_rect transformed by zoom/pan. This is where the texture
    // actually renders.
    let view_rect = state.view_rect();

    if let (Some(texture), Some(rect)) = (&state.canvas.texture, view_rect) {
        // Artboard treatment: a soft drop shadow and hairline edge so the image
        // reads as a document sitting on the workspace, not a flat blit.
        for (i, alpha) in [(6.0_f32, 26u8), (12.0, 16), (20.0, 9)] {
            painter.rect_filled(
                rect.expand(i).translate(egui::vec2(0.0, i * 0.35)),
                egui::Rounding::same(3.0 + i * 0.2),
                Color32::from_black_alpha(alpha),
            );
        }
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            rect,
            egui::Rounding::ZERO,
            Stroke::new(1.0, Color32::from_white_alpha(28)),
        );
    }

    // Grid overlay — display only, never exported.
    if state.settings.show_grid {
        if let Some(rect) = view_rect {
            draw_grid(&painter, rect, state.settings.grid_size * state.view_zoom);
        }
    }

    // Committed annotations. Project each from base→view before drawing.
    let project = make_projector(state);
    let text_bg = state.settings.text_bg;
    let accent = state.settings.theme().accent;
    // Global visibility toggle (Ctrl+H) — skip all annotations when hidden.
    if state.annotations_visible {
        // Iterate the scene tree so layer/group visibility + opacity apply.
        // Group transforms are identity in Phase 2 (world is ignored here; will
        // be applied when group transforms ship in a later phase).
        let leaves = state.canvas.document.leaves_in_paint_order();
        for lv in &leaves {
            if !lv.effective_visible {
                continue;
            }
            let Some(ann) = state.canvas.document.leaf_to_annotation(lv.id) else {
                continue;
            };
            let ann = &ann;
            let layer_opacity = lv.effective_opacity;
            let effective_locked = lv.effective_locked;
            // Identity-parity design: if the leaf's world transform is identity,
            // use the EXACT existing project_annotation + draw_annotation_full
            // path (byte-identical output). Only non-identity transforms go
            // through render_xform::draw_transformed.
            if lv.world == kurbo::Affine::IDENTITY {
                let projected = project_annotation(ann, &project, state.view_zoom);
                draw_annotation_full(
                    &painter,
                    &projected,
                    state.canvas.color_image.as_ref(),
                    view_rect,
                    state.canvas.canvas_size,
                    text_bg,
                    accent,
                    layer_opacity,
                );
                // Padlock icon in the top-left corner of locked annotations.
                if ann.locked || effective_locked {
                    let br = projected.shape.bounding_rect();
                    let screen_rect = egui::Rect::from_min_max(project(br.min), project(br.max));
                    let icon_pos = screen_rect.left_top() + egui::vec2(2.0, 2.0);
                    painter.text(
                        icon_pos,
                        egui::Align2::LEFT_TOP,
                        "🔒",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
                    );
                }
                // Dashed yellow outline for provisional (pending-expiry) annotations.
                if matches!(projected.provisional_until, Some(d) if d > std::time::SystemTime::now())
                {
                    let br = projected.shape.bounding_rect();
                    let screen_rect = egui::Rect::from_min_max(project(br.min), project(br.max));
                    let expanded = screen_rect.expand(4.0);
                    let dash_len = 6.0_f32;
                    let gap_len = 4.0_f32;
                    let stroke = egui::Stroke::new(
                        1.5,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 0, 180),
                    );
                    for &(a, b) in &[
                        (expanded.left_top(), expanded.right_top()),
                        (expanded.right_top(), expanded.right_bottom()),
                        (expanded.right_bottom(), expanded.left_bottom()),
                        (expanded.left_bottom(), expanded.left_top()),
                    ] {
                        let total = a.distance(b);
                        if total < 0.5 {
                            continue;
                        }
                        let dir = (b - a) / total;
                        let mut t = 0.0_f32;
                        while t < total {
                            let seg_end = (t + dash_len).min(total);
                            painter.line_segment([a + dir * t, a + dir * seg_end], stroke);
                            t += dash_len + gap_len;
                        }
                    }
                }
            } else {
                // Non-identity world transform: route through render_xform.
                crate::render_xform::draw_transformed(
                    &painter,
                    ann,
                    lv.world,
                    &project,
                    state.view_zoom,
                    ann.color,
                    layer_opacity,
                    accent,
                );
                // Padlock icon for transformed leaves (axis-aligned AABB approx).
                if ann.locked || effective_locked {
                    let br = ann.shape.bounding_rect();
                    let screen_rect = egui::Rect::from_min_max(project(br.min), project(br.max));
                    let icon_pos = screen_rect.left_top() + egui::vec2(2.0, 2.0);
                    painter.text(
                        icon_pos,
                        egui::Align2::LEFT_TOP,
                        "🔒",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
                    );
                }
            }
        }
    }

    draw_tool_preview(ui.ctx(), &painter, state, &project);

    if matches!(state.current_tool, Tool::Select) {
        if let Some(hover_pos) = response.hover_pos() {
            let hover_base = state.unproject(hover_pos);
            let hovered_id =
                crate::tools::select::hittable_leaf_at(&state.canvas.document, hover_base);
            if let Some(ann) =
                hovered_id.and_then(|id| state.canvas.annotations.iter().find(|a| a.id == id))
            {
                let r = ann.shape.bounding_rect().expand(3.0);
                let projected_r = Rect::from_min_max(project(r.min), project(r.max));
                painter.rect_stroke(
                    projected_r,
                    3.0,
                    Stroke::new(1.5, Color32::from_white_alpha(120)),
                );
                // Show annotation note as tooltip if present.
                if let Some(note) = &ann.label {
                    if !note.is_empty() {
                        egui::show_tooltip_text(
                            ui.ctx(),
                            egui::LayerId::new(
                                egui::Order::Tooltip,
                                egui::Id::new("ann_note_layer"),
                            ),
                            egui::Id::new("ann_note").with(ann.id),
                            note,
                        );
                    }
                }
            }
        }
    }

    // Resize handles on the selected image layer.
    if matches!(state.current_tool, Tool::Select) {
        if let Some(id) = selected_image_id(state) {
            if let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == id) {
                if let ShapeData::ImageLayer { rect, .. } = &ann.shape {
                    let screen_rect = Rect::from_min_max(project(rect.min), project(rect.max));
                    let handle_color = state.settings.theme().accent;
                    painter.rect_stroke(screen_rect, 0.0, Stroke::new(1.5, handle_color));
                    for corner in image_layer_corners(screen_rect) {
                        painter.rect_filled(
                            Rect::from_center_size(corner, Vec2::splat(HANDLE_SIZE)),
                            2.0,
                            handle_color,
                        );
                    }
                }
            }
        }
    }

    // Multi-selection: draw highlight border around each selected annotation.
    if matches!(state.current_tool, Tool::Select) {
        if let ToolState::MultiSelected {
            ref ids,
            band_start,
            band_current,
            ..
        } = state.tool_state
        {
            let accent = state.settings.theme().accent_secondary;
            for &sel_id in ids {
                if let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == sel_id) {
                    let r = ann.shape.bounding_rect().expand(3.0);
                    let sr = Rect::from_min_max(project(r.min), project(r.max));
                    painter.rect_stroke(sr, 3.0, Stroke::new(1.5, accent));
                }
            }
            // Rubber-band rectangle.
            if let (Some(bs), Some(bc)) = (band_start, band_current) {
                let band_img_rect = Rect::from_two_pos(bs, bc);
                let sr = Rect::from_min_max(project(band_img_rect.min), project(band_img_rect.max));
                painter.rect_stroke(sr, 0.0, Stroke::new(1.0, Color32::WHITE));
                painter.rect_filled(sr, 0.0, Color32::from_white_alpha(15));
            }
        }
    }

    // Transform gizmo: 8 scale handles + rotate handle + pivot marker around the
    // current selection (any selection in Select mode, except a single ImageLayer
    // which keeps its dedicated resize handles). During a scale drag the gizmo is
    // drawn at the live-previewed bbox.
    if matches!(state.current_tool, Tool::Select) {
        let uniform = ui.input(|i| i.modifiers.shift);
        if let Some(layout) = crate::tools::select::gizmo_overlay(state, uniform) {
            let accent = state.settings.theme().accent;
            // Bounding outline through the four corners.
            let outline = [
                layout.corners[0],
                layout.corners[1],
                layout.corners[2],
                layout.corners[3],
                layout.corners[0],
            ];
            painter.add(egui::Shape::line(
                outline.to_vec(),
                Stroke::new(1.5, accent),
            ));
            // Square scale handles (corners + edge midpoints).
            for p in layout.corners.iter().chain(layout.edges.iter()) {
                painter.rect_filled(
                    Rect::from_center_size(*p, Vec2::splat(HANDLE_SIZE)),
                    2.0,
                    accent,
                );
            }
            // Stem + circular rotate handle (Task 12 wires the interaction).
            let top_center = egui::pos2(
                (layout.corners[0].x + layout.corners[1].x) * 0.5,
                (layout.corners[0].y + layout.corners[1].y) * 0.5,
            );
            painter.line_segment([top_center, layout.rotate], Stroke::new(1.5, accent));
            painter.circle_filled(layout.rotate, HANDLE_SIZE * 0.5, accent);
            // Pivot marker (selection centre).
            let half = HANDLE_SIZE * 0.4;
            painter.line_segment(
                [
                    layout.center - egui::vec2(half, 0.0),
                    layout.center + egui::vec2(half, 0.0),
                ],
                Stroke::new(1.5, accent),
            );
            painter.line_segment(
                [
                    layout.center - egui::vec2(0.0, half),
                    layout.center + egui::vec2(0.0, half),
                ],
                Stroke::new(1.5, accent),
            );
        }
    }

    // Marquee selection overlay (shown whenever a selection exists).
    if let Some(sel) = state.canvas.selection {
        draw_marquee(&painter, &sel, &project, ui.input(|i| i.time));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(60));
    }

    // Magnifier lens (display-only; independent of tool).
    if state.settings.magnifier {
        if let (Some(cursor), Some(texture), Some(rect)) =
            (response.hover_pos(), &state.canvas.texture, view_rect)
        {
            if rect.contains(cursor) {
                draw_magnifier(
                    &painter,
                    texture,
                    rect,
                    cursor,
                    state.settings.theme().accent,
                );
            }
        }
    }

    // Tool input + command application
    let cmd = crate::tools::handle_input(&response, ui.ctx(), state, canvas_rect);
    if let Some(cmd) = cmd {
        CommandExecutor::execute(&mut state.canvas, ParticipantId::HUMAN_LOCAL, cmd);
    }

    // Right-click context menu for path operations (secondary-click only, so it
    // never fights the primary-click tool handling above). Buttons appear only
    // when the selection qualifies; all actions are undoable.
    {
        let (closed_paths, open_paths) = pathfinder_selection(state);
        if closed_paths.len() >= 2 || open_paths.len() == 2 {
            response.context_menu(|ui| {
                if closed_paths.len() >= 2 {
                    for (label, op) in [
                        ("Unite", BoolOp::Unite),
                        ("Subtract", BoolOp::Subtract),
                        ("Intersect", BoolOp::Intersect),
                        ("Exclude", BoolOp::Exclude),
                    ] {
                        if ui.button(label).clicked() {
                            ui.close_menu();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::BooleanOp {
                                    op,
                                    ids: closed_paths.clone(),
                                },
                            );
                        }
                    }
                    ui.separator();
                }
                if open_paths.len() == 2 && ui.button("Weld").clicked() {
                    ui.close_menu();
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::WeldPaths {
                            a: open_paths[0],
                            b: open_paths[1],
                        },
                    );
                }
            });
        }
    }

    // Delete key for selected annotation(s)
    if ui
        .ctx()
        .input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        match &state.tool_state {
            ToolState::Selected { id, .. } => {
                let id = *id;
                // Check if the annotation being deleted is a StepMarker.
                let was_step = state
                    .canvas
                    .annotations
                    .iter()
                    .any(|a| a.id == id && matches!(a.shape, ShapeData::StepMarker { .. }));
                state.tool_state = ToolState::Idle;
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Delete(id),
                );
                // Auto-renumber if a StepMarker was deleted and others remain.
                if was_step
                    && state
                        .canvas
                        .annotations
                        .iter()
                        .any(|a| matches!(a.shape, ShapeData::StepMarker { .. }))
                {
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::RenumberSteps,
                    );
                }
            }
            ToolState::MultiSelected { ids, .. } => {
                let ids: Vec<_> = ids.clone();
                // Check if any of the deleted annotations were StepMarkers.
                let any_step =
                    ids.iter().any(|&del_id| {
                        state.canvas.annotations.iter().any(|a| {
                            a.id == del_id && matches!(a.shape, ShapeData::StepMarker { .. })
                        })
                    });
                state.tool_state = ToolState::Idle;
                let cmds: Vec<Command> = ids.iter().map(|&id| Command::Delete(id)).collect();
                if !cmds.is_empty() {
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::Batch(cmds),
                    );
                }
                // Auto-renumber if any StepMarkers were deleted and others remain.
                if any_step
                    && state
                        .canvas
                        .annotations
                        .iter()
                        .any(|a| matches!(a.shape, ShapeData::StepMarker { .. }))
                {
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::RenumberSteps,
                    );
                }
            }
            _ => {}
        }
    }

    // Arrow key nudging for selected annotation(s). Shift = 10px, plain = 1px.
    {
        let (left, right, up, down, shift) = ui.ctx().input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.modifiers.shift,
            )
        });
        let step = if shift { 10.0_f32 } else { 1.0_f32 };
        let delta = egui::Vec2::new(
            if left {
                -step
            } else if right {
                step
            } else {
                0.0
            },
            if up {
                -step
            } else if down {
                step
            } else {
                0.0
            },
        );
        if delta != egui::Vec2::ZERO {
            let nudge_ids: Vec<uuid::Uuid> = match &state.tool_state {
                ToolState::Selected { id, .. } => vec![*id],
                ToolState::MultiSelected { ids, .. } => ids.clone(),
                _ => vec![],
            };
            if nudge_ids.len() == 1 {
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Move {
                        id: nudge_ids[0],
                        delta,
                    },
                );
            } else if nudge_ids.len() > 1 {
                let cmds: Vec<Command> = nudge_ids
                    .iter()
                    .map(|&id| Command::Move { id, delta })
                    .collect();
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Batch(cmds),
                );
            }
        }
    }

    // In-progress text editor overlay (F1).
    show_text_editor(ui.ctx(), state);

    // Pending-crop confirm buttons (F4).
    if let Some(img_rect) = base_rect {
        show_crop_confirm(ui.ctx(), state, img_rect);
    }

    // Nothing loaded: offer the ways to get pixels in. Drawn last so its
    // buttons sit above the canvas' own click-and-drag surface.
    if state.canvas.texture.is_none() {
        draw_empty_state(ui, canvas_rect, state);
    }
}

// ── Annotation drawing ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_annotation_full(
    painter: &Painter,
    ann: &Annotation,
    color_image: Option<&egui::ColorImage>,
    img_draw_rect: Option<Rect>,
    canvas_size: Vec2,
    text_bg: bool,
    accent: Color32,
    layer_opacity: f32,
) {
    // Derive effective alpha: provisional annotations are ghosted (50% opacity)
    // while their deadline is still in the future. The layer/group effective
    // opacity modulates this further (1.0 for the single-layer case → identical).
    let provisional_alpha = match ann.provisional_until {
        Some(deadline) if deadline > std::time::SystemTime::now() => 128u8,
        _ => 255u8,
    };
    let alpha_mul = (provisional_alpha as f32 / 255.0) * layer_opacity.clamp(0.0, 1.0);
    let ann_color = {
        let c = ann.color;
        egui::Color32::from_rgba_unmultiplied(
            c.r(),
            c.g(),
            c.b(),
            (c.a() as f32 * alpha_mul).round() as u8,
        )
    };

    match &ann.shape {
        ShapeData::Blur { rect, block_size } => {
            draw_blur(
                painter,
                *rect,
                *block_size,
                color_image,
                img_draw_rect,
                canvas_size,
            );
        }
        ShapeData::Text { pos, content, size } => {
            if !content.is_empty() {
                if text_bg {
                    let bg_rect = Rect::from_min_size(
                        *pos,
                        egui::Vec2::new(
                            content.chars().count() as f32 * size * 0.6 + 8.0,
                            size + 6.0,
                        ),
                    );
                    painter.rect_filled(
                        bg_rect,
                        2.0,
                        Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                    );
                }
                let text_pos = if text_bg {
                    *pos + egui::vec2(3.0, 3.0)
                } else {
                    *pos
                };
                painter.text(
                    text_pos,
                    egui::Align2::LEFT_TOP,
                    content,
                    egui::FontId::proportional(*size),
                    ann_color,
                );
            }
        }
        ShapeData::Question { pos, content, size } => {
            if !content.is_empty() {
                // Use the annotation's own color (set by the agent / settings).
                // Soft outline bounding box at half alpha for the "ghost bubble" feel.
                let est_w = content.chars().count() as f32 * size * 0.6 + 8.0;
                let bb = Rect::from_min_size(*pos, egui::Vec2::new(est_w, size + 6.0));
                let outline = egui::Color32::from_rgba_unmultiplied(
                    ann_color.r(),
                    ann_color.g(),
                    ann_color.b(),
                    ann_color.a() / 2,
                );
                painter.rect_stroke(bb, 3.0, egui::Stroke::new(1.0, outline));
                painter.text(
                    *pos + egui::vec2(3.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    content,
                    egui::FontId::proportional(*size),
                    ann_color,
                );
            }
        }
        ShapeData::StepMarker {
            pos,
            number,
            radius,
        } => {
            draw_step_marker(painter, *pos, *number, *radius, ann.color);
        }
        ShapeData::ImageLayer { rect, texture, .. } => {
            if let Some(tex) = &texture.0 {
                painter.image(
                    tex.id(),
                    *rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                // Texture not uploaded yet — placeholder.
                painter.rect_filled(*rect, 0.0, Color32::from_gray(40));
            }
        }
        _ => draw_annotation(painter, ann, ann_color, accent),
    }
}

/// Build a closure that projects a point from image-pixel space to screen space.
fn make_projector(state: &AppState) -> impl Fn(Pos2) -> Pos2 {
    let base = state.last_img_draw_rect;
    let zoom = state.view_zoom;
    let pan = state.view_pan;
    let cs = state.canvas.canvas_size;
    move |img_px: Pos2| -> Pos2 {
        let Some(base) = base else {
            return img_px;
        };
        if cs.x < f32::EPSILON || cs.y < f32::EPSILON {
            return img_px;
        }
        let frac = Vec2::new(img_px.x / cs.x, img_px.y / cs.y);
        base.min + frac * base.size() * zoom + pan
    }
}

fn project_annotation(
    ann: &Annotation,
    project: &impl Fn(Pos2) -> Pos2,
    view_zoom: f32,
) -> Annotation {
    let mut out = ann.clone();
    let p = project;
    let project_rect = |r: Rect| Rect::from_min_max(p(r.min), p(r.max));
    match &mut out.shape {
        ShapeData::Rect { rect, .. }
        | ShapeData::Oval { rect, .. }
        | ShapeData::Star { rect }
        | ShapeData::FocusRegion { rect }
        | ShapeData::ImageLayer { rect, .. } => {
            *rect = project_rect(*rect);
        }
        ShapeData::Blur { rect, block_size } => {
            *rect = project_rect(*rect);
            *block_size *= view_zoom;
        }
        ShapeData::Arrow { start, end, .. }
        | ShapeData::Line { start, end, .. }
        | ShapeData::Ruler { start, end } => {
            *start = p(*start);
            *end = p(*end);
        }
        ShapeData::Pen {
            raw_points,
            smoothed_points,
        } => {
            for q in raw_points.iter_mut().chain(smoothed_points.iter_mut()) {
                *q = p(*q);
            }
        }
        ShapeData::LogicTag { pos, .. } => {
            *pos = p(*pos);
        }
        ShapeData::Text { pos, size, .. } => {
            *pos = p(*pos);
            *size *= view_zoom;
        }
        ShapeData::Question { pos, size, .. } => {
            *pos = p(*pos);
            *size *= view_zoom;
        }
        ShapeData::StepMarker { pos, radius, .. } => {
            *pos = p(*pos);
            *radius *= view_zoom;
        }
        ShapeData::Callout { rect, tail } => {
            *rect = project_rect(*rect);
            *tail = p(*tail);
        }
        ShapeData::BezierPath { nodes, .. } => {
            for n in nodes.iter_mut() {
                n.pos = p(n.pos);
                if let Some(h) = n.in_handle {
                    n.in_handle = Some(h * view_zoom);
                }
                if let Some(h) = n.out_handle {
                    n.out_handle = Some(h * view_zoom);
                }
            }
        }
    }
    out.thickness *= view_zoom;
    out
}

fn draw_step_marker(painter: &Painter, pos: Pos2, number: u32, radius: f32, color: Color32) {
    painter.circle_filled(pos, radius, color);
    painter.circle_stroke(pos, radius, Stroke::new(1.5, Color32::WHITE));
    let font_size = (radius * 1.1).max(10.0);
    painter.text(
        pos,
        egui::Align2::CENTER_CENTER,
        number.to_string(),
        egui::FontId::proportional(font_size),
        Color32::WHITE,
    );
}

pub fn draw_annotation(painter: &Painter, ann: &Annotation, color: Color32, accent: Color32) {
    let stroke = Stroke::new(ann.thickness, color);
    match &ann.shape {
        ShapeData::Rect {
            rect,
            radius,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            if let Some(fill_color) = fill {
                painter.rect_filled(*rect, *radius, *fill_color);
            }
            if *has_stroke {
                match stroke_style {
                    LineStyle::Solid => {
                        painter.rect_stroke(*rect, *radius, stroke);
                    }
                    LineStyle::Dashed | LineStyle::Dotted => {
                        let corners = [
                            rect.left_top(),
                            rect.right_top(),
                            rect.right_bottom(),
                            rect.left_bottom(),
                        ];
                        for i in 0..4 {
                            draw_line_styled(
                                painter,
                                corners[i],
                                corners[(i + 1) % 4],
                                stroke,
                                *stroke_style,
                            );
                        }
                    }
                }
            }
        }
        ShapeData::Oval {
            rect,
            fill,
            stroke: has_stroke,
            stroke_style,
        } => {
            if let Some(fill_color) = fill {
                painter.add(egui::Shape::ellipse_filled(
                    rect.center(),
                    rect.size() * 0.5,
                    *fill_color,
                ));
            }
            if *has_stroke {
                match stroke_style {
                    LineStyle::Solid => {
                        painter.add(egui::Shape::ellipse_stroke(
                            rect.center(),
                            rect.size() * 0.5,
                            stroke,
                        ));
                    }
                    LineStyle::Dashed | LineStyle::Dotted => {
                        const N: usize = 48;
                        let pts: Vec<Pos2> = (0..=N)
                            .map(|i| {
                                let t = i as f32 / N as f32 * std::f32::consts::TAU;
                                Pos2::new(
                                    rect.center().x + rect.width() * 0.5 * t.cos(),
                                    rect.center().y + rect.height() * 0.5 * t.sin(),
                                )
                            })
                            .collect();
                        for w in pts.windows(2) {
                            draw_line_styled(painter, w[0], w[1], stroke, *stroke_style);
                        }
                    }
                }
            }
        }
        ShapeData::Star { rect } => {
            painter.add(egui::Shape::closed_line(star_points(*rect), stroke));
        }
        ShapeData::Arrow {
            start,
            end,
            style,
            heads,
        } => {
            draw_arrow(painter, *start, *end, stroke, *style, *heads);
        }
        ShapeData::Line { start, end, style } => {
            draw_line_styled(painter, *start, *end, stroke, *style);
        }
        ShapeData::Pen {
            smoothed_points,
            raw_points,
        } => {
            let pts = if !smoothed_points.is_empty() {
                smoothed_points
            } else {
                raw_points
            };
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts.clone(), stroke));
            }
        }
        ShapeData::FocusRegion { rect } => {
            painter.rect_stroke(*rect, 0.0, Stroke::new(2.0, accent));
        }
        ShapeData::LogicTag { pos, tag } => {
            let amber = accent;
            let label = tag.label();
            let galley = painter.layout_no_wrap(
                label.to_string(),
                egui::FontId::proportional(12.0),
                Color32::BLACK,
            );
            let bg = Rect::from_min_size(*pos, galley.size() + egui::vec2(8.0, 4.0));
            painter.rect_filled(bg, 3.0, amber);
            painter.galley(*pos + egui::vec2(4.0, 2.0), galley, Color32::BLACK);
        }
        ShapeData::Callout { rect, tail } => {
            let c = color;
            let thickness = ann.thickness;
            // Fill bubble
            painter.rect_filled(
                *rect,
                8.0,
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 40),
            );
            // Stroke bubble
            painter.rect_stroke(*rect, 8.0, Stroke::new(thickness, c));
            // Tail triangle: base is a segment along the bottom of the rect, apex is tail tip
            let center_x = rect.center().x;
            let base_y = rect.max.y;
            let p1 = Pos2::new(center_x - 15.0, base_y);
            let p2 = Pos2::new(center_x + 15.0, base_y);
            let tip = *tail;
            painter.add(egui::Shape::convex_polygon(
                vec![p1, p2, tip],
                c,
                Stroke::NONE,
            ));
        }
        ShapeData::Ruler { start, end } => {
            let s = *start;
            let e = *end;
            let stroke = Stroke::new(ann.thickness, color);
            painter.line_segment([s, e], stroke);
            // Perpendicular tick marks at endpoints
            let dir = e - s;
            let len = dir.length();
            if len > 0.5 {
                let d = dir / len;
                let perp = Vec2::new(-d.y, d.x) * 5.0;
                painter.line_segment([s - perp, s + perp], stroke);
                painter.line_segment([e - perp, e + perp], stroke);
            }
            // Distance label at midpoint
            let dist = len;
            let mid = Pos2::new((s.x + e.x) * 0.5, (s.y + e.y) * 0.5);
            let perp_offset = if len > 0.5 {
                let d = dir / len;
                Vec2::new(-d.y, d.x) * 12.0
            } else {
                Vec2::new(0.0, -12.0)
            };
            painter.text(
                mid + perp_offset,
                egui::Align2::CENTER_CENTER,
                format!("{:.0}px", dist),
                egui::FontId::proportional(12.0),
                color,
            );
        }
        ShapeData::BezierPath {
            nodes,
            closed,
            fill,
        } => {
            let tess = crate::bezier::tessellate(nodes, *closed, 12);
            if tess.len() < 2 {
                return;
            }
            if let Some(fill_color) = fill {
                if *closed {
                    painter.add(egui::Shape::convex_polygon(
                        tess.clone(),
                        *fill_color,
                        egui::Stroke::NONE,
                    ));
                }
            }
            for w in tess.windows(2) {
                painter.line_segment([w[0], w[1]], egui::Stroke::new(ann.thickness, color));
            }
        }
        // Handled by draw_annotation_full
        ShapeData::Text { .. }
        | ShapeData::Question { .. }
        | ShapeData::Blur { .. }
        | ShapeData::StepMarker { .. }
        | ShapeData::ImageLayer { .. } => {}
    }
}

/// Render a pixelated mosaic inside `rect` (screen coords) by sampling the
/// underlying color_image. Falls back to a solid dark fill if no image or if
/// the draw rect is unknown.
fn draw_blur(
    painter: &Painter,
    rect: Rect,
    block_size: f32,
    color_image: Option<&egui::ColorImage>,
    img_draw_rect: Option<Rect>,
    canvas_size: Vec2,
) {
    let block = block_size.max(4.0);
    let (Some(ci), Some(draw_rect)) = (color_image, img_draw_rect) else {
        painter.rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(30, 30, 30, 230));
        return;
    };
    if draw_rect.width() <= 0.0 || draw_rect.height() <= 0.0 || canvas_size.x <= 0.0 {
        painter.rect_filled(rect, 2.0, Color32::from_rgba_unmultiplied(30, 30, 30, 230));
        return;
    }
    let scale = draw_rect.width() / canvas_size.x;
    let (iw, ih) = (ci.size[0] as i32, ci.size[1] as i32);
    let pixels = ci.pixels.as_slice();

    let mut y = rect.min.y;
    while y < rect.max.y {
        let mut x = rect.min.x;
        let cell_h = (y + block).min(rect.max.y) - y;
        while x < rect.max.x {
            let cell_w = (x + block).min(rect.max.x) - x;
            let cx_img = (x + cell_w * 0.5 - draw_rect.min.x) / scale;
            let cy_img = (y + cell_h * 0.5 - draw_rect.min.y) / scale;
            let ix = (cx_img as i32).clamp(0, iw - 1);
            let iy = (cy_img as i32).clamp(0, ih - 1);
            let px = pixels[(iy * iw + ix) as usize];
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_w, cell_h)),
                0.0,
                px,
            );
            x += block;
        }
        y += block;
    }
}

/// Display-only magnifier lens: a bordered square rendered at cursor offset,
/// showing a sub-UV of the underlying texture at ~3x.
fn draw_magnifier(
    painter: &Painter,
    texture: &egui::TextureHandle,
    img_rect: Rect,
    cursor: Pos2,
    accent: Color32,
) {
    const LENS_RADIUS: f32 = 90.0;
    const ZOOM: f32 = 3.0;
    const OFFSET: Vec2 = Vec2::new(110.0, -110.0);

    let lens_center = cursor + OFFSET;
    let lens_rect = Rect::from_center_size(lens_center, Vec2::splat(LENS_RADIUS * 2.0));

    // UV normalized over the image (0..1).
    let u = (cursor.x - img_rect.min.x) / img_rect.width();
    let v = (cursor.y - img_rect.min.y) / img_rect.height();
    let uv_half = (LENS_RADIUS / ZOOM) / img_rect.width();
    let uv_half_y = (LENS_RADIUS / ZOOM) / img_rect.height();
    let uv = Rect::from_min_max(
        Pos2::new(
            (u - uv_half).clamp(0.0, 1.0),
            (v - uv_half_y).clamp(0.0, 1.0),
        ),
        Pos2::new(
            (u + uv_half).clamp(0.0, 1.0),
            (v + uv_half_y).clamp(0.0, 1.0),
        ),
    );

    painter.rect_filled(lens_rect.expand(3.0), 6.0, Color32::from_gray(15));
    painter.image(texture.id(), lens_rect, uv, Color32::WHITE);
    painter.rect_stroke(lens_rect, 6.0, Stroke::new(2.0, accent));
    // Crosshair at lens center
    painter.line_segment(
        [
            Pos2::new(lens_center.x - 6.0, lens_center.y),
            Pos2::new(lens_center.x + 6.0, lens_center.y),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(180)),
    );
    painter.line_segment(
        [
            Pos2::new(lens_center.x, lens_center.y - 6.0),
            Pos2::new(lens_center.x, lens_center.y + 6.0),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(180)),
    );
}

// ── Text editor overlay ─────────────────────────────────────────────────────

fn show_text_editor(ctx: &Context, state: &mut AppState) {
    let editing = matches!(state.tool_state, ToolState::EditingText { .. });
    if !editing {
        return;
    }

    // Take ownership so we can mutate and potentially replace.
    let taken = std::mem::replace(&mut state.tool_state, ToolState::Idle);
    let ToolState::EditingText {
        pos,
        mut buffer,
        color,
        size,
        editing_id,
    } = taken
    else {
        state.tool_state = taken;
        return;
    };

    let mut commit = false;
    let mut cancel = false;

    let projected_pos = state.project(pos);
    let projected_size = size * state.view_zoom;

    egui::Area::new(egui::Id::new("redline-text-editor"))
        .fixed_pos(projected_pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.style_mut().visuals.extreme_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 200);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buffer)
                    .font(egui::FontId::proportional(projected_size))
                    .text_color(color)
                    .desired_width(240.0)
                    .hint_text("text…"),
            );
            // Capture lost_focus BEFORE request_focus: in egui 0.29 request_focus
            // immediately sets focused_widget, which would make lost_focus() return
            // false in the same frame that the TextEdit surrenders focus on Enter.
            let lost = resp.lost_focus();
            resp.request_focus();
            if lost && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            // Click outside commits too.
            if lost && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                commit = true;
            }
        });

    if commit && !buffer.trim().is_empty() {
        if let Some(id) = editing_id {
            // Editing an existing annotation — just update its text.
            CommandExecutor::execute(
                &mut state.canvas,
                ParticipantId::HUMAN_LOCAL,
                Command::SetTextContent {
                    id,
                    content: buffer.clone(),
                },
            );
        } else {
            let shape = ShapeData::Text {
                pos,
                content: buffer.clone(),
                size,
            };
            let ann = Annotation::new(shape, color, 1.0);
            CommandExecutor::execute(
                &mut state.canvas,
                ParticipantId::HUMAN_LOCAL,
                Command::Add(ann),
            );
        }
        state.tool_state = ToolState::Idle;
    } else if commit || cancel {
        state.tool_state = ToolState::Idle;
    } else {
        state.tool_state = ToolState::EditingText {
            pos,
            buffer,
            color,
            size,
            editing_id,
        };
    }
}

// ── Crop confirm overlay ────────────────────────────────────────────────────

fn show_crop_confirm(ctx: &Context, state: &mut AppState, img_rect: Rect) {
    let pending_rect = if let ToolState::PendingCrop { rect } = state.tool_state {
        rect
    } else {
        return;
    };

    let mut apply = false;
    let mut cancel = false;

    // Project the base-rect pending rect onto the current view for button
    // placement, so it follows zoom/pan.
    let view_rect = state.view_rect().unwrap_or(img_rect);
    let projected = {
        let p = make_projector(state);
        Rect::from_min_max(p(pending_rect.min), p(pending_rect.max))
    };

    // Draw the crop outline at the projected (visible) rect.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("redline-crop-outline"),
    ));
    painter.rect_stroke(projected, 0.0, Stroke::new(2.0, Color32::WHITE));
    painter.rect_filled(projected, 0.0, Color32::from_white_alpha(20));

    egui::Area::new(egui::Id::new("redline-crop-confirm"))
        .fixed_pos(Pos2::new(
            projected.min.x,
            (projected.max.y + 6.0).min(view_rect.max.y - 40.0),
        ))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("✔ Apply crop").clicked() {
                        apply = true;
                    }
                    if ui.button("✖ Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });

    if cancel {
        state.tool_state = ToolState::Idle;
        return;
    }
    if apply {
        apply_crop(state, pending_rect, img_rect);
        state.tool_state = ToolState::Idle;
    }
}

fn apply_crop(state: &mut AppState, crop_rect_screen: Rect, img_draw_rect: Rect) {
    let Some(ci) = state.canvas.color_image.clone() else {
        return;
    };
    let scale = img_draw_rect.width() / state.canvas.canvas_size.x;
    if scale <= 0.0 {
        return;
    }

    let (iw, ih) = (ci.size[0], ci.size[1]);
    let x0 =
        (((crop_rect_screen.min.x - img_draw_rect.min.x) / scale).floor() as i32).max(0) as usize;
    let y0 =
        (((crop_rect_screen.min.y - img_draw_rect.min.y) / scale).floor() as i32).max(0) as usize;
    let x1 =
        (((crop_rect_screen.max.x - img_draw_rect.min.x) / scale).ceil() as i32).max(0) as usize;
    let y1 =
        (((crop_rect_screen.max.y - img_draw_rect.min.y) / scale).ceil() as i32).max(0) as usize;
    let x0 = x0.min(iw.saturating_sub(1));
    let y0 = y0.min(ih.saturating_sub(1));
    let x1 = x1.min(iw);
    let y1 = y1.min(ih);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return;
    }
    let new_w = x1 - x0;
    let new_h = y1 - y0;

    let mut new_pixels = Vec::with_capacity(new_w * new_h);
    for y in y0..y1 {
        let row_start = y * iw + x0;
        new_pixels.extend_from_slice(&ci.pixels[row_start..row_start + new_w]);
    }
    let new_image = egui::ColorImage {
        size: [new_w, new_h],
        pixels: new_pixels,
    };

    // Convert to DynamicImage and route through loader to rebuild texture.
    let raw: Vec<u8> = new_image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    if let Some(buf) = image::RgbaImage::from_raw(new_w as u32, new_h as u32, raw) {
        // Apply directly: we don't have a Context here, so stash the new image
        // and mark the canvas for refresh next frame via a helper.
        state.canvas.color_image = Some(new_image);
        state.canvas.canvas_size = Vec2::new(new_w as f32, new_h as f32);
        state.canvas.document = crate::scene::Document::from_flat(Vec::new());
        state.canvas.sync_annotations();
        state.canvas.clear_all_undo_redo();
        state.status_msg = Some(format!("Cropped to {new_w}×{new_h} (annotations cleared)"));
        state.pending_texture_refresh = Some(image::DynamicImage::ImageRgba8(buf));
    }
}

// ── Checkerboard / fit ──────────────────────────────────────────────────────

/// Marching-ants marquee outline + four corner handles. Rect or ellipse.
fn draw_marquee(
    painter: &Painter,
    sel: &crate::state::Selection,
    project: &impl Fn(Pos2) -> Pos2,
    time: f64,
) {
    use crate::state::SelKind;
    let screen = Rect::from_min_max(project(sel.rect.min), project(sel.rect.max));

    // Halo (dark stroke under white so it's visible on any background).
    let halo = Stroke::new(2.5, Color32::from_black_alpha(160));
    let white = Color32::WHITE;
    let dash = Stroke::new(1.5, white);

    // "Marching" = phase-shift the dashes using wall-clock time.
    let phase = ((time % 1.0) as f32) * 8.0;

    match sel.kind {
        SelKind::Rect => {
            painter.rect_stroke(screen, 0.0, halo);
            draw_dashed_rect(painter, screen, dash, 6.0, 4.0, phase);
        }
        SelKind::Ellipse => {
            let c = screen.center();
            let radii = screen.size() * 0.5;
            painter.add(egui::Shape::ellipse_stroke(c, radii, halo));
            draw_dashed_ellipse(painter, c, radii, dash, 6.0, 4.0, phase);
        }
    }

    // If inverted, overlay the rect outline with a dashed frame at the canvas
    // edges — we don't know full canvas size here, so just tag the text.
    if sel.inverted {
        painter.text(
            screen.left_top() + egui::vec2(4.0, 4.0),
            egui::Align2::LEFT_TOP,
            "inverted",
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
    }

    // Corner handles (always on the bounding rect even for ellipse).
    for corner in [
        screen.left_top(),
        screen.right_top(),
        screen.right_bottom(),
        screen.left_bottom(),
    ] {
        painter.rect_filled(
            Rect::from_center_size(corner, Vec2::splat(HANDLE_SIZE)),
            1.0,
            Color32::WHITE,
        );
        painter.rect_stroke(
            Rect::from_center_size(corner, Vec2::splat(HANDLE_SIZE)),
            1.0,
            Stroke::new(1.0, Color32::BLACK),
        );
    }
}

fn draw_dashed_rect(
    painter: &Painter,
    rect: Rect,
    stroke: Stroke,
    dash: f32,
    gap: f32,
    phase: f32,
) {
    let pts = [
        rect.left_top(),
        rect.right_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    let mut p = phase;
    for seg in pts.chunks(2) {
        let a = seg[0];
        let b = seg[1];
        let v = b - a;
        let len = v.length();
        if len < 0.5 {
            continue;
        }
        let dir = v / len;
        let mut t = -p;
        while t < len {
            let start = t.max(0.0);
            let end = (t + dash).min(len);
            if end > start {
                painter.line_segment([a + dir * start, a + dir * end], stroke);
            }
            t += dash + gap;
        }
        p = (p + len).rem_euclid(dash + gap);
    }
}

fn draw_dashed_ellipse(
    painter: &Painter,
    center: Pos2,
    radii: Vec2,
    stroke: Stroke,
    dash: f32,
    gap: f32,
    phase: f32,
) {
    let step_angle = 0.05_f32;
    let mut angle = 0.0_f32;
    let mut dist_along = -phase;
    let mut prev: Option<Pos2> = None;
    while angle < std::f32::consts::TAU + step_angle {
        let p = Pos2::new(
            center.x + radii.x * angle.cos(),
            center.y + radii.y * angle.sin(),
        );
        if let Some(prev) = prev {
            let seg_len = (p - prev).length();
            let next_dist = dist_along + seg_len;
            // Draw only if within a dash window.
            let m = (dash + gap).max(0.01);
            let d0 = dist_along.rem_euclid(m);
            if d0 < dash {
                painter.line_segment([prev, p], stroke);
            }
            dist_along = next_dist;
        }
        prev = Some(p);
        angle += step_angle;
    }
}

fn draw_grid(painter: &Painter, rect: Rect, cell: f32) {
    let cell = cell.max(4.0);
    let stroke = Stroke::new(0.5, Color32::from_rgba_unmultiplied(255, 255, 255, 30));

    // Vertical lines
    let mut x = rect.min.x;
    while x <= rect.max.x + 0.5 {
        painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], stroke);
        x += cell;
    }
    // Horizontal lines
    let mut y = rect.min.y;
    while y <= rect.max.y + 0.5 {
        painter.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], stroke);
        y += cell;
    }
}

// ── Empty state ───────────────────────────────────────────────────────────────

/// Soft vignette drawn behind the empty-state card — a flat void reads as a
/// broken render, a graded one reads as an intentional backdrop.
fn draw_empty_backdrop(painter: &Painter, rect: Rect, state: &AppState) {
    let t = state.settings.theme();
    painter.rect_filled(rect, 0.0, t.canvas_void);
    // A faint dot grid reads as "empty workspace" rather than "failed render",
    // and it fades out toward the middle so the card sits on clean ground.
    let step = 26.0_f32;
    let center = rect.center();
    let falloff = (rect.width().min(rect.height()) * 0.42).max(1.0);
    let mut y = rect.top() + step * 0.5;
    while y < rect.bottom() {
        let mut x = rect.left() + step * 0.5;
        while x < rect.right() {
            let p = egui::pos2(x, y);
            let d = (p - center).length() / falloff;
            let a = (d.min(1.6) / 1.6 * 16.0) as u8;
            if a > 1 {
                painter.circle_filled(p, 1.1, Color32::from_white_alpha(a));
            }
            x += step;
        }
        y += step;
    }
}

const LOGO_SVG: &[u8] = include_bytes!("../assets/redline.svg");

/// The "nothing open yet" card. This is the app's front door: the primary job
/// (grab a screenshot) is one accent-coloured click away, with the other ways
/// to get pixels in — open, paste, blank canvas — right beside it.
fn draw_empty_state(ui: &mut Ui, canvas_rect: Rect, state: &mut AppState) {
    let t = state.settings.theme();
    let card_size = egui::vec2(
        460.0_f32.min(canvas_rect.width() - 24.0),
        330.0_f32.min(canvas_rect.height() - 24.0),
    );
    if card_size.x < 260.0 || card_size.y < 190.0 {
        return; // Too cramped to be useful — leave the backdrop clean.
    }
    let card = Rect::from_center_size(canvas_rect.center(), card_size);

    let painter = ui.painter();
    painter.rect_filled(
        card.translate(egui::vec2(0.0, 8.0)),
        egui::Rounding::same(16.0),
        Color32::from_black_alpha(70),
    );
    painter.rect_filled(card, egui::Rounding::same(16.0), t.panel_bg);
    painter.rect_stroke(
        card,
        egui::Rounding::same(16.0),
        Stroke::new(1.0, t.divider),
    );

    let mut want_region = false;
    let mut want_full = false;
    let mut want_open = false;
    let mut want_paste = false;
    let mut want_new = false;

    ui.allocate_new_ui(
        egui::UiBuilder::new()
            .max_rect(card.shrink(22.0))
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.add_space(2.0);
            ui.add(
                egui::Image::from_bytes("bytes://redline-logo.svg", LOGO_SVG)
                    .fit_to_exact_size(egui::vec2(52.0, 52.0)),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Capture a screenshot")
                    .heading()
                    .strong()
                    .color(t.text_heading),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Grab a region, then annotate it — or bring in an image.")
                    .small()
                    .color(t.text_label),
            );
            ui.add_space(16.0);

            ui.horizontal(|ui| {
                let row_w = 300.0_f32;
                ui.add_space(((ui.available_width() - row_w) * 0.5).max(0.0));
                if crate::toolbar::primary_button(ui, &t, "⛶  Capture Region")
                    .on_hover_text("Drag a region of the screen (Ctrl+Shift+S)")
                    .clicked()
                {
                    want_region = true;
                }
                if crate::toolbar::secondary_button(ui, &t, "🖵  Full Screen", true).clicked() {
                    want_full = true;
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let row_w = 340.0_f32;
                ui.add_space(((ui.available_width() - row_w) * 0.5).max(0.0));
                if crate::toolbar::secondary_button(ui, &t, "Open Image…", true)
                    .on_hover_text("Open a file (Ctrl+O)")
                    .clicked()
                {
                    want_open = true;
                }
                if crate::toolbar::secondary_button(ui, &t, "Paste", true)
                    .on_hover_text("Paste an image from the clipboard (Ctrl+V)")
                    .clicked()
                {
                    want_paste = true;
                }
                if crate::toolbar::secondary_button(ui, &t, "New Canvas", true)
                    .on_hover_text("Start from a blank canvas (Ctrl+N)")
                    .clicked()
                {
                    want_new = true;
                }
            });

            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("or drop an image anywhere in this window")
                    .small()
                    .color(t.text_dim),
            );
        },
    );

    if want_region {
        crate::toolbar::trigger_capture_pub(state, crate::state::SnapMode::Region);
    }
    if want_full {
        crate::toolbar::trigger_capture_pub(state, crate::state::SnapMode::Full);
    }
    if want_open {
        let ctx = ui.ctx().clone();
        crate::toolbar::open_image_via_dialog(state, &ctx);
    }
    if want_paste {
        state.request_paste = true;
    }
    if want_new {
        crate::toolbar::open_new_canvas_dialog(state);
    }
}

#[allow(dead_code)]
fn draw_checkerboard(painter: &Painter, rect: Rect) {
    let cell = 16.0_f32;
    let cols = ((rect.width() / cell).ceil() as usize).max(1);
    let rows = ((rect.height() / cell).ceil() as usize).max(1);
    for row in 0..rows {
        for col in 0..cols {
            let dark = (row + col) % 2 == 0;
            let fill = if dark {
                Color32::from_gray(50)
            } else {
                Color32::from_gray(70)
            };
            let r = Rect::from_min_size(
                rect.min + egui::vec2(col as f32 * cell, row as f32 * cell),
                egui::vec2(cell, cell),
            );
            painter.rect_filled(r.intersect(rect), 0.0, fill);
        }
    }
}

fn draw_line_styled(painter: &Painter, start: Pos2, end: Pos2, stroke: Stroke, style: LineStyle) {
    match style {
        LineStyle::Solid => {
            painter.line_segment([start, end], stroke);
        }
        LineStyle::Dashed => {
            let dash_len = (stroke.width * 3.0).max(6.0);
            let gap_len = (stroke.width * 2.5).max(4.0);
            let vec = end - start;
            let total = vec.length();
            if total < 0.5 {
                return;
            }
            let dir = vec / total;
            let mut t = 0.0;
            while t < total {
                let t2 = (t + dash_len).min(total);
                let a = start + dir * t;
                let b = start + dir * t2;
                painter.line_segment([a, b], stroke);
                t = t2 + gap_len;
            }
        }
        LineStyle::Dotted => {
            let vec = end - start;
            let len = vec.length();
            if len < 0.5 {
                return;
            }
            let dir = vec / len;
            let gap = stroke.width * 3.0;
            let mut t = 0.0;
            while t < len {
                let pt = start + dir * t;
                painter.circle_filled(pt, stroke.width * 0.6, stroke.color);
                t += gap;
            }
        }
    }
}

fn draw_arrow(
    painter: &Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    style: LineStyle,
    heads: ArrowHeads,
) {
    draw_line_styled(painter, start, end, stroke, style);
    let head_len = (stroke.width * 4.0).clamp(10.0, 28.0);
    draw_arrowhead(painter, end, end - start, stroke, head_len);
    if heads == ArrowHeads::Double {
        draw_arrowhead(painter, start, start - end, stroke, head_len);
    }
}

fn draw_arrowhead(painter: &Painter, tip: Pos2, dir: Vec2, stroke: Stroke, len: f32) {
    let n = dir.length();
    if n < 0.5 {
        return;
    }
    let d = dir / n;
    let perp = Vec2::new(-d.y, d.x);
    let base = tip - d * len;
    let left = base + perp * (len * 0.5);
    let right = base - perp * (len * 0.5);
    painter.line_segment([tip, left], stroke);
    painter.line_segment([tip, right], stroke);
}

fn fit_rect(img_size: egui::Vec2, container: Rect) -> Rect {
    let scale = (container.width() / img_size.x)
        .min(container.height() / img_size.y)
        .min(1.0);
    let draw_size = img_size * scale;
    let offset = (container.size() - draw_size) * 0.5;
    Rect::from_min_size(container.min + offset, draw_size)
}

fn draw_tool_preview(
    ctx: &Context,
    painter: &Painter,
    state: &mut AppState,
    project: &impl Fn(Pos2) -> Pos2,
) {
    let tool_state = &state.tool_state;
    let settings = &state.settings;
    let current_tool = &state.current_tool;
    let arrow_color = settings.stroke_color;
    let stroke = Stroke::new(settings.stroke_width, settings.stroke_color);
    let amber_stroke = Stroke::new(2.0, state.settings.theme().accent);

    match tool_state {
        ToolState::Drawing { start, current } => {
            let start = project(*start);
            let current = project(*current);
            let rect = Rect::from_two_pos(start, current);
            match current_tool {
                Tool::Rect => {
                    let fill = settings
                        .shape_fill_enabled
                        .then_some(settings.shape_fill_color);
                    let radius = settings.corner_radius;
                    if let Some(fill_color) = fill {
                        painter.rect_filled(rect, radius, fill_color);
                    }
                    if settings.shape_stroke_enabled {
                        match settings.line_style {
                            LineStyle::Solid => {
                                painter.rect_stroke(rect, radius, stroke);
                            }
                            LineStyle::Dashed | LineStyle::Dotted => {
                                let corners = [
                                    rect.left_top(),
                                    rect.right_top(),
                                    rect.right_bottom(),
                                    rect.left_bottom(),
                                ];
                                for i in 0..4 {
                                    draw_line_styled(
                                        painter,
                                        corners[i],
                                        corners[(i + 1) % 4],
                                        stroke,
                                        settings.line_style,
                                    );
                                }
                            }
                        }
                    }
                }
                Tool::Oval => {
                    let fill = settings
                        .shape_fill_enabled
                        .then_some(settings.shape_fill_color);
                    if let Some(fill_color) = fill {
                        painter.add(egui::Shape::ellipse_filled(
                            rect.center(),
                            rect.size() * 0.5,
                            fill_color,
                        ));
                    }
                    if settings.shape_stroke_enabled {
                        match settings.line_style {
                            LineStyle::Solid => {
                                painter.add(egui::Shape::ellipse_stroke(
                                    rect.center(),
                                    rect.size() * 0.5,
                                    stroke,
                                ));
                            }
                            LineStyle::Dashed | LineStyle::Dotted => {
                                const N: usize = 48;
                                let pts: Vec<Pos2> = (0..=N)
                                    .map(|i| {
                                        let t = i as f32 / N as f32 * std::f32::consts::TAU;
                                        Pos2::new(
                                            rect.center().x + rect.width() * 0.5 * t.cos(),
                                            rect.center().y + rect.height() * 0.5 * t.sin(),
                                        )
                                    })
                                    .collect();
                                for w in pts.windows(2) {
                                    draw_line_styled(
                                        painter,
                                        w[0],
                                        w[1],
                                        stroke,
                                        settings.line_style,
                                    );
                                }
                            }
                        }
                    }
                }
                Tool::Star => {
                    painter.add(egui::Shape::closed_line(star_points(rect), stroke));
                }
                Tool::Stamp(id) => {
                    draw_stamp_preview(ctx, painter, state, id, rect);
                }
                Tool::Arrow => {
                    let shift = ctx.input(|i| i.modifiers.shift);
                    let (a, b) = if shift {
                        (current, start)
                    } else {
                        (start, current)
                    };
                    draw_arrow(
                        painter,
                        a,
                        b,
                        Stroke::new(settings.stroke_width, arrow_color),
                        settings.line_style,
                        settings.arrow_heads,
                    );
                }
                Tool::Line => {
                    draw_line_styled(painter, start, current, stroke, settings.line_style);
                }
                Tool::FocusRegion => {
                    painter.rect_stroke(rect, 0.0, amber_stroke);
                }
                Tool::Blur => {
                    painter.rect_filled(
                        rect,
                        0.0,
                        Color32::from_rgba_unmultiplied(80, 80, 80, 140),
                    );
                    painter.rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.0, Color32::from_white_alpha(180)),
                    );
                }
                Tool::Crop => {
                    painter.rect_stroke(rect, 0.0, Stroke::new(2.0, Color32::WHITE));
                }
                Tool::Callout => {
                    let c = settings.stroke_color;
                    painter.rect_filled(
                        rect,
                        8.0,
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 40),
                    );
                    painter.rect_stroke(rect, 8.0, stroke);
                    // Preview tail
                    let center_x = rect.center().x;
                    let base_y = rect.max.y;
                    let p1 = Pos2::new(center_x - 15.0, base_y);
                    let p2 = Pos2::new(center_x + 15.0, base_y);
                    let tip = Pos2::new(rect.min.x + 20.0, rect.max.y + 30.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![p1, p2, tip],
                        c,
                        Stroke::NONE,
                    ));
                }
                Tool::Ruler => {
                    // Draw the ruler line from start to current
                    painter.line_segment([start, current], stroke);
                    let dir = current - start;
                    let len = dir.length();
                    if len > 0.5 {
                        let d = dir / len;
                        let perp = Vec2::new(-d.y, d.x) * 5.0;
                        painter.line_segment([start - perp, start + perp], stroke);
                        painter.line_segment([current - perp, current + perp], stroke);
                    }
                    let mid = Pos2::new((start.x + current.x) * 0.5, (start.y + current.y) * 0.5);
                    let perp_offset = if len > 0.5 {
                        let d = dir / len;
                        Vec2::new(-d.y, d.x) * 12.0
                    } else {
                        Vec2::new(0.0, -12.0)
                    };
                    // Distance is in screen pixels here (preview only); actual stored annotation will use image pixels
                    painter.text(
                        mid + perp_offset,
                        egui::Align2::CENTER_CENTER,
                        format!("{:.0}px", len),
                        egui::FontId::proportional(12.0),
                        settings.stroke_color,
                    );
                }
                _ => {
                    painter.rect_stroke(rect, 0.0, stroke);
                }
            }
        }
        ToolState::DrawingPen { raw_points } => {
            if raw_points.len() >= 2 {
                let pts_base = if settings.pen_smoothing
                    && matches!(settings.smoothing_mode, SmoothingMode::Realtime)
                {
                    crate::smoothing::catmull_rom(raw_points, 8)
                } else {
                    raw_points.clone()
                };
                let projected: Vec<Pos2> = pts_base.into_iter().map(project).collect();
                painter.add(egui::Shape::line(projected, stroke));
            }
        }
        ToolState::PendingCrop { rect } => {
            let pr = Rect::from_min_max(project(rect.min), project(rect.max));
            painter.rect_stroke(pr, 0.0, Stroke::new(2.0, Color32::WHITE));
            painter.rect_filled(pr, 0.0, Color32::from_white_alpha(20));
        }
        ToolState::DrawingLasso { points } => {
            if points.len() >= 2 {
                let lasso_stroke = Stroke::new(1.5, Color32::WHITE);
                for i in 1..points.len() {
                    painter
                        .line_segment([project(points[i - 1]), project(points[i])], lasso_stroke);
                }
                // Closing line back to start (faint, shows where polygon will close)
                if points.len() >= 3 {
                    painter.line_segment(
                        [project(*points.last().unwrap()), project(points[0])],
                        Stroke::new(1.0, Color32::from_white_alpha(120)),
                    );
                }
            }
        }
        _ => {}
    }
}

fn draw_stamp_preview(
    ctx: &Context,
    painter: &Painter,
    state: &mut AppState,
    stamp_id: &str,
    rect: Rect,
) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }

    let width = rect.width().round().max(1.0) as u32;
    let height = rect.height().round().max(1.0) as u32;
    let c = state.settings.stroke_color;
    let key = format!(
        "{stamp_id}:{width}x{height}:{:02x}{:02x}{:02x}{:02x}",
        c.r(),
        c.g(),
        c.b(),
        c.a(),
    );

    if state.stamp_preview_key.as_deref() != Some(key.as_str()) {
        state.stamp_preview_texture = stamp_by_id(stamp_id)
            .and_then(|stamp| rasterize_stamp(stamp, [width, height], c).ok())
            .map(|image| {
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                );
                crate::texture_upload::load_texture_fitted(
                    ctx,
                    format!("stamp-preview-{key}"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            });
        state.stamp_preview_key = Some(key);
    }

    if let Some(texture) = &state.stamp_preview_texture {
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    let dash_stroke = Stroke::new(1.0, Color32::from_white_alpha(180));
    draw_dashed_rect(painter, rect, dash_stroke, 6.0, 4.0, 0.0);
}

/// Ray-casting point-in-polygon test. Returns true if `p` is inside `pts`.
fn point_in_polygon(p: egui::Pos2, pts: &[egui::Pos2]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let xi = pts[i].x;
        let yi = pts[i].y;
        let xj = pts[j].x;
        let yj = pts[j].y;
        if ((yi > p.y) != (yj > p.y)) && (p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Handle lasso tool input. Returns None (the lasso itself never produces a Command —
/// it just transitions tool_state and switches to Select).
pub fn handle_lasso_input(response: &egui::Response, state: &mut AppState) {
    let pointer = &response.ctx.input(|i| i.pointer.clone());

    // Map the current cursor position to image space.
    let img_pos = if let Some(screen) = pointer.latest_pos() {
        state.unproject(screen)
    } else {
        return;
    };

    if pointer.primary_pressed() {
        // Start a new lasso.
        state.tool_state = ToolState::DrawingLasso {
            points: vec![img_pos],
        };
    } else if pointer.primary_down() {
        // Append points while dragging (only if moved > 3px from last point).
        if let ToolState::DrawingLasso { ref mut points } = state.tool_state {
            let should_add = points
                .last()
                .map(|last| (img_pos - *last).length() > 3.0)
                .unwrap_or(true);
            if should_add {
                points.push(img_pos);
            }
        }
    } else if pointer.primary_released() {
        // Finish: collect matching annotations, transition state.
        let points = if let ToolState::DrawingLasso { ref points } = state.tool_state {
            points.clone()
        } else {
            return;
        };

        if points.len() >= 3 {
            let matched_ids: Vec<uuid::Uuid> = state
                .canvas
                .annotations
                .iter()
                .filter(|ann| {
                    ann.visible
                        && !ann.locked
                        && point_in_polygon(ann.shape.bounding_rect().center(), &points)
                })
                .map(|ann| ann.id)
                .collect();

            state.set_current_tool(Tool::Select);
            state.tool_state = match matched_ids.len() {
                0 => ToolState::Idle,
                1 => ToolState::Selected {
                    id: matched_ids[0],
                    drag_offset: None,
                },
                _ => ToolState::MultiSelected {
                    ids: matched_ids,
                    drag_offset: None,
                    band_start: None,
                    band_current: None,
                },
            };
        } else {
            state.tool_state = ToolState::Idle;
        }
    }
}

/// Mouse-wheel + Ctrl zooms around the cursor. Middle-drag pans. '0' resets.
fn handle_view_input(
    ctx: &Context,
    response: &egui::Response,
    state: &mut AppState,
    _canvas_rect: Rect,
) {
    // Only apply when the pointer is over our canvas.
    let hover = response.hover_pos();
    let Some(base) = state.last_img_draw_rect else {
        return;
    };

    // Zoom (Ctrl+wheel or raw wheel if Ctrl not needed — we require Ctrl to
    // avoid fighting with page scroll).
    if let Some(cursor) = hover {
        ctx.input(|i| {
            let raw = i.raw_scroll_delta.y;
            if i.modifiers.ctrl && raw.abs() > 0.0 {
                let old_zoom = state.view_zoom;
                let factor = (raw * 0.01).exp();
                let new_zoom = (old_zoom * factor).clamp(0.01, 64.0);
                if (new_zoom - old_zoom).abs() > f32::EPSILON {
                    // Keep the cursor anchored: compute base-rect point under
                    // cursor with old transform, then adjust pan so the same
                    // base point lands under the cursor at the new zoom.
                    let d = cursor - base.min - state.view_pan;
                    let base_pt = d / old_zoom;
                    let new_pan = cursor - base.min - base_pt * new_zoom;
                    state.view_zoom = new_zoom;
                    state.view_pan = new_pan;
                }
            }
            if i.key_pressed(egui::Key::Num0) {
                state.view_zoom = 1.0;
                state.view_pan = Vec2::ZERO;
            }
        });
    }

    // Middle-mouse drag to pan. Works anywhere over the canvas.
    let middle_down = ctx.input(|i| i.pointer.middle_down());
    let middle_delta = ctx.input(|i| i.pointer.delta());
    if middle_down && middle_delta != Vec2::ZERO {
        state.view_pan += middle_delta;
    }
}
