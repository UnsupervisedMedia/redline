use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Stroke, Vec2};
use std::path::PathBuf;
use uuid::Uuid;

use super::state::{CanvasSortState, ToolInteraction};
use super::types::CanvasTool;

/// Entry point called from `show_canvas_sort` when a session is active.
pub fn show(ui: &mut egui::Ui, state: &mut CanvasSortState, ctx: &egui::Context) {
    state.ensure_thumb_worker();
    state.drain_thumbnails(ctx);
    state.maybe_auto_save();
    state.maybe_fire_entropy();

    let available = ui.available_rect_before_wrap();
    state.panel_rect = available;

    let (resp, painter) = ui.allocate_painter(available.size(), Sense::click_and_drag());

    // ── Background ─────────────────────────────────────────────────────────
    painter.rect_filled(available, 0.0, Color32::from_gray(28));
    draw_grid_background(&painter, state, available);

    // ── Content ────────────────────────────────────────────────────────────
    if state.session.is_some() {
        draw_zones(state, &painter);
        draw_images_and_request_thumbs(state, &painter, ctx);
        draw_stacks(state, &painter);
        draw_sticky_notes(state, &painter);
        draw_selection_highlights(state, &painter);
    }

    // ── Tool overlay ───────────────────────────────────────────────────────
    draw_tool_overlay(state, &painter);

    // ── Input handling ─────────────────────────────────────────────────────
    super::tools::handle_input(state, &resp, ctx);

    // ── Right-click context menu ───────────────────────────────────────────
    // Capture the click position the moment the secondary button fires, before
    // egui moves on to menu rendering frames where pointer_pos may be gone.
    if resp.secondary_clicked() {
        state.right_click_canvas_pos = resp.interact_pointer_pos().map(|p| state.to_canvas(p));
    }
    show_context_menu(state, &resp, ctx);

    // ── Minimap overlay ────────────────────────────────────────────────────
    if state.show_minimap && state.session.is_some() {
        super::panels::draw_minimap(&painter, state);
    }

    // ── Loading progress overlay ───────────────────────────────────────────
    if state.loading_folder {
        let center = available.center();
        let box_rect = Rect::from_center_size(center, Vec2::new(220.0, 60.0));
        painter.rect_filled(box_rect, 8.0, Color32::from_black_alpha(200));
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            "Scanning folder…",
            FontId::proportional(16.0),
            Color32::WHITE,
        );
    }
}

// ── Grid background ───────────────────────────────────────────────────────────

fn draw_grid_background(painter: &egui::Painter, state: &CanvasSortState, panel: Rect) {
    // Draw a subtle dot-grid based on current zoom
    let grid_spacing_canvas = 200.0f32;
    let grid_spacing_screen = grid_spacing_canvas * state.zoom;

    // Only draw grid if dots would be reasonably spaced
    if grid_spacing_screen < 8.0 {
        return;
    }

    let vis = state.visible_canvas_rect();
    let start_x = (vis.min.x / grid_spacing_canvas).floor() * grid_spacing_canvas;
    let start_y = (vis.min.y / grid_spacing_canvas).floor() * grid_spacing_canvas;

    let dot_r = (grid_spacing_screen * 0.04).clamp(0.5, 2.5);
    let dot_color = Color32::from_white_alpha(20);

    let mut x = start_x;
    while x < vis.max.x + grid_spacing_canvas {
        let mut y = start_y;
        while y < vis.max.y + grid_spacing_canvas {
            let screen = state.to_screen(Pos2::new(x, y));
            if panel.contains(screen) {
                painter.circle_filled(screen, dot_r, dot_color);
            }
            y += grid_spacing_canvas;
        }
        x += grid_spacing_canvas;
    }
}

// ── Action zones ──────────────────────────────────────────────────────────────

fn draw_zones(state: &CanvasSortState, painter: &egui::Painter) {
    let s = match &state.session {
        Some(s) => s,
        None => return,
    };
    let vis = state.visible_canvas_rect();

    for zone in &s.zones {
        let canvas_rect = zone.canvas_rect();
        if !canvas_rect.intersects(vis) {
            continue;
        }
        let screen_rect = state.rect_to_screen(canvas_rect);
        let c: Color32 = zone.color.into();
        let _alpha = (zone.opacity * 255.0) as u8;

        // Fill
        painter.rect_filled(screen_rect, 4.0, c.gamma_multiply(zone.opacity));
        // Border
        let is_selected = state.selected_zones.contains(&zone.id);
        let border_color = if is_selected {
            Color32::WHITE
        } else {
            c.linear_multiply(1.5)
        };
        let border_w = if is_selected { 2.0 } else { 1.5 };
        painter.rect_stroke(screen_rect, 4.0, Stroke::new(border_w, border_color));

        // Label
        let icon = zone.action.icon();
        let label = format!("{icon} {}", zone.label);
        let label_pos = screen_rect.min + Vec2::new(6.0, 4.0);
        let font_size = (14.0 * state.zoom.sqrt()).clamp(10.0, 22.0);
        painter.text(
            label_pos,
            egui::Align2::LEFT_TOP,
            &label,
            FontId::proportional(font_size),
            Color32::WHITE,
        );

        // Image count badge
        if !zone.contained_image_ids.is_empty() {
            let count = zone.contained_image_ids.len();
            let badge_pos = screen_rect.min + Vec2::new(6.0, screen_rect.height() - 20.0);
            painter.text(
                badge_pos,
                egui::Align2::LEFT_TOP,
                format!("{count}"),
                FontId::proportional(12.0),
                Color32::from_white_alpha(180),
            );
        }

        // Resize handles (when selected)
        if is_selected {
            draw_zone_handles(painter, state, canvas_rect);
        }
    }
}

fn draw_zone_handles(painter: &egui::Painter, state: &CanvasSortState, canvas_rect: Rect) {
    // Constant on-screen size — handles drawn in screen space so they stay a
    // consistent, grabbable size at any zoom level.
    let hs = super::tools::ZONE_HANDLE_DRAW_PX;

    let handles = super::tools::zone_handles_pub(canvas_rect);
    for h in &handles {
        let screen_h = state.to_screen(*h);
        let hr = Rect::from_center_size(screen_h, Vec2::splat(hs));
        painter.rect_filled(hr, 2.0, Color32::WHITE);
        painter.rect_stroke(hr, 2.0, Stroke::new(1.0, Color32::from_gray(80)));
    }
}

// ── Images ────────────────────────────────────────────────────────────────────

fn draw_images_and_request_thumbs(
    state: &mut CanvasSortState,
    painter: &egui::Painter,
    _ctx: &egui::Context,
) {
    let vis = state.visible_canvas_rect();
    let virt = state.should_virtualize();
    let vis_buffered = if virt {
        vis.expand(vis.size().max_elem() * (super::state::VIRT_BUFFER - 1.0) * 0.5)
    } else {
        Rect::EVERYTHING
    };

    // Collect what we need to avoid borrow conflicts
    let images: Vec<(Uuid, Rect, [f32; 2], bool)> = state
        .session
        .as_ref()
        .map(|s| {
            let mut imgs: Vec<_> = s
                .images
                .iter()
                .filter(|i| i.stack_id.is_none())
                .map(|i| (i.id, i.rect(), i.pos, i.is_locked))
                .collect();
            // Sort by z_index ascending (low drawn first / under top)
            let z_map: std::collections::HashMap<Uuid, i32> =
                s.images.iter().map(|i| (i.id, i.z_index)).collect();
            imgs.sort_by_key(|(id, _, _, _)| z_map.get(id).copied().unwrap_or(0));
            imgs
        })
        .unwrap_or_default();

    let thumb_size = state
        .session
        .as_ref()
        .map(|s| s.settings.effective_thumb_size())
        .unwrap_or(200);

    let mut thumb_requests: Vec<(Uuid, PathBuf, u32)> = Vec::new();

    for (id, canvas_rect, _pos, is_locked) in &images {
        if !canvas_rect.intersects(vis_buffered) {
            continue;
        }
        let screen_rect = state.rect_to_screen(*canvas_rect);
        if screen_rect.width() < 1.0 || screen_rect.height() < 1.0 {
            continue;
        }

        if let Some(tex) = state.textures.get(id) {
            // Fit texture within screen_rect preserving its intrinsic aspect ratio.
            let tex_size = tex.size_vec2();
            let scale = (screen_rect.width() / tex_size.x).min(screen_rect.height() / tex_size.y);
            let draw_rect = Rect::from_min_size(screen_rect.min, tex_size * scale);
            painter.image(
                tex.id(),
                draw_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else if state.thumb_failed.contains(id) {
            // Failed to decode — show error placeholder
            painter.rect_filled(screen_rect, 2.0, Color32::from_rgb(80, 30, 30));
            if screen_rect.width() > 24.0 {
                painter.text(
                    screen_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "⚠",
                    FontId::proportional((screen_rect.width() * 0.25).clamp(10.0, 28.0)),
                    Color32::from_rgb(255, 100, 100),
                );
            }
        } else {
            // Placeholder — loading
            painter.rect_filled(screen_rect, 2.0, Color32::from_gray(50));
            // Request thumbnail
            if !state.thumb_pending.contains(id) {
                if let Some(path) = state
                    .session
                    .as_ref()
                    .and_then(|s| s.image_by_id(*id))
                    .map(|i| i.path.clone())
                {
                    thumb_requests.push((*id, path, thumb_size));
                }
            }
            // Show filename in placeholder if large enough
            if screen_rect.width() > 40.0 {
                let name = state
                    .session
                    .as_ref()
                    .and_then(|s| s.image_by_id(*id))
                    .and_then(|i| i.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let font_size = (screen_rect.width() * 0.08).clamp(8.0, 12.0);
                painter.text(
                    screen_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    truncate(&name, 18),
                    FontId::proportional(font_size),
                    Color32::from_white_alpha(120),
                );
            }
        }

        // Lock indicator
        if *is_locked {
            let lock_pos = screen_rect.min + Vec2::new(4.0, 4.0);
            painter.text(
                lock_pos,
                egui::Align2::LEFT_TOP,
                "🔒",
                FontId::proportional(10.0),
                Color32::WHITE,
            );
        }
    }

    // Apply thumbnail requests
    for (id, path, size) in thumb_requests {
        state.request_thumbnail(id, &path, size);
    }
}

// ── Stacks ────────────────────────────────────────────────────────────────────

fn draw_stacks(state: &CanvasSortState, painter: &egui::Painter) {
    let s = match &state.session {
        Some(s) => s,
        None => return,
    };

    for stack in &s.stacks {
        let screen_center = state.to_screen(stack.canvas_pos());
        let is_selected = state.selected_stacks.contains(&stack.id);
        let c: Color32 = stack.color.into();

        if stack.is_expanded {
            // Grid layout: images side-by-side in rows of 4, anchored at stack center.
            let cols = 4.min(stack.image_ids.len()).max(1);
            let cell = 130.0 * state.zoom; // screen px per cell
            let gap = 4.0 * state.zoom;
            let step = cell + gap;
            let grid_w = cols as f32 * step - gap;
            let origin = screen_center - Vec2::new(grid_w * 0.5, cell * 0.5);

            for (i, img_id) in stack.image_ids.iter().enumerate() {
                let col = (i % cols) as f32;
                let row = (i / cols) as f32;
                let cell_min = origin + Vec2::new(col * step, row * step);
                let cell_rect = Rect::from_min_size(cell_min, Vec2::splat(cell));
                if let Some(tex) = state.textures.get(img_id) {
                    let tex_size = tex.size_vec2();
                    let scale = (cell / tex_size.x).min(cell / tex_size.y);
                    let draw_rect = Rect::from_min_size(cell_min, tex_size * scale);
                    painter.image(
                        tex.id(),
                        draw_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    painter.rect_filled(cell_rect, 2.0, Color32::from_gray(50));
                }
                let border_c = if is_selected {
                    Color32::from_white_alpha(200)
                } else {
                    c.linear_multiply(0.5)
                };
                painter.rect_stroke(cell_rect, 2.0, Stroke::new(1.0, border_c));
            }
        } else {
            // Fanned pile: draw up to 4 cards with offset so depth is visible
            let card_size = 140.0 * state.zoom;
            let fan_count = stack.image_ids.len().min(4);
            let card_shift = 8.0 * state.zoom; // px offset per layer
            for i in 0..fan_count {
                let layer = (fan_count - 1 - i) as f32; // 0 = front/top
                let offset = Vec2::new(layer * card_shift, layer * card_shift);
                let r = Rect::from_min_size(
                    screen_center - Vec2::splat(card_size * 0.5) + offset,
                    Vec2::splat(card_size),
                );
                let img_id = stack.image_ids.get(i).copied();
                if let Some(id) = img_id {
                    if let Some(tex) = state.textures.get(&id) {
                        painter.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        painter.rect_filled(r, 2.0, Color32::from_gray(60));
                    }
                }
                let border_c = if layer == 0.0 && is_selected {
                    Color32::WHITE
                } else {
                    c.linear_multiply(0.6)
                };
                painter.rect_stroke(r, 2.0, Stroke::new(1.5, border_c));

                // Count badge on the front card
                if layer == 0.0 {
                    let badge = format!("×{}", stack.image_ids.len());
                    let badge_font =
                        FontId::proportional((12.0 * state.zoom.sqrt()).clamp(9.0, 18.0));
                    let text_pos = r.max - Vec2::new(4.0, 4.0);
                    // Shadow
                    painter.text(
                        text_pos + Vec2::splat(1.0),
                        egui::Align2::RIGHT_BOTTOM,
                        &badge,
                        badge_font.clone(),
                        Color32::BLACK,
                    );
                    painter.text(
                        text_pos,
                        egui::Align2::RIGHT_BOTTOM,
                        &badge,
                        badge_font,
                        Color32::WHITE,
                    );
                }
            }

            // Label below
            let label_text = if stack.label.is_empty() {
                format!("{} images", stack.image_ids.len())
            } else {
                format!("{} ({})", stack.label, stack.image_ids.len())
            };
            let font_size = (11.0 * state.zoom.sqrt()).clamp(9.0, 14.0);
            let label_y =
                screen_center.y + card_size * 0.5 + (fan_count as f32 - 1.0) * card_shift + 4.0;
            painter.text(
                Pos2::new(screen_center.x, label_y),
                egui::Align2::CENTER_TOP,
                &label_text,
                FontId::proportional(font_size),
                Color32::from_white_alpha(200),
            );

            // "double-click to expand" hint when selected
            if is_selected {
                let hint_y = label_y + font_size + 2.0;
                painter.text(
                    Pos2::new(screen_center.x, hint_y),
                    egui::Align2::CENTER_TOP,
                    "dbl-click to expand",
                    FontId::proportional((9.0 * state.zoom.sqrt()).clamp(7.0, 11.0)),
                    Color32::from_white_alpha(100),
                );
            }
        }
    }
}

// ── Sticky notes ──────────────────────────────────────────────────────────────

fn draw_sticky_notes(state: &CanvasSortState, painter: &egui::Painter) {
    for note in &state.sticky_notes {
        let screen_rect = state.rect_to_screen(note.rect);
        if screen_rect.width() < 4.0 {
            continue;
        }
        painter.rect_filled(screen_rect, 4.0, note.color);
        painter.rect_stroke(
            screen_rect,
            4.0,
            Stroke::new(1.0, note.color.linear_multiply(0.7)),
        );
        let font_size = (note.font_size * state.zoom).clamp(6.0, 32.0);
        let text_rect = screen_rect.shrink(6.0);
        // Simple text drawing (multi-line not supported in painter.text directly)
        if super::notes::should_draw_note_text(note) {
            painter.text(
                text_rect.min,
                egui::Align2::LEFT_TOP,
                &note.text,
                FontId::proportional(font_size),
                Color32::from_black_alpha(200),
            );
        }
    }
}

// ── Selection highlights ──────────────────────────────────────────────────────

fn draw_selection_highlights(state: &CanvasSortState, painter: &egui::Painter) {
    let s = match &state.session {
        Some(s) => s,
        None => return,
    };

    let sel_color = Color32::from_rgb(0x44, 0x88, 0xFF);
    let sel_stroke = Stroke::new(2.0, sel_color);

    for id in &state.selected_images {
        if let Some(img) = s.image_by_id(*id) {
            let sr = state.rect_to_screen(img.rect());
            painter.rect_stroke(sr, 1.0, sel_stroke);
            // Corner handles
            let hsize = 5.0f32;
            for corner in [sr.min, sr.right_top(), sr.left_bottom(), sr.max] {
                painter.rect_filled(
                    Rect::from_center_size(corner, Vec2::splat(hsize)),
                    0.0,
                    sel_color,
                );
            }
        }
    }

    for id in &state.selected_stacks {
        if let Some(st) = s.stack_by_id(*id) {
            let sr = state.rect_to_screen(st.rect());
            painter.rect_stroke(
                sr,
                1.0,
                Stroke::new(2.0, Color32::from_rgb(0xFF, 0xCC, 0x44)),
            );
        }
    }

    if let Some(id) = state.selected_note {
        if let Some(note) = state.sticky_notes.iter().find(|n| n.id == id) {
            let sr = state.rect_to_screen(note.rect);
            painter.rect_stroke(sr, 4.0, sel_stroke);
            draw_zone_handles(painter, state, note.rect);
        }
    }
}

// ── Tool overlay ──────────────────────────────────────────────────────────────

fn draw_tool_overlay(state: &CanvasSortState, painter: &egui::Painter) {
    match &state.tool_state {
        ToolInteraction::Marquee { start, current } => {
            let r = Rect::from_two_pos(*start, *current);
            let sr = state.rect_to_screen(r);
            painter.rect_filled(
                sr,
                0.0,
                Color32::from_rgba_unmultiplied(0x44, 0x88, 0xFF, 30),
            );
            painter.rect_stroke(
                sr,
                0.0,
                Stroke::new(1.5, Color32::from_rgb(0x44, 0x88, 0xFF)),
            );
        }
        ToolInteraction::Drawing {
            start_canvas,
            current_canvas,
        } => {
            let r = Rect::from_two_pos(*start_canvas, *current_canvas);
            let sr = state.rect_to_screen(r);
            let (fill, stroke_c) = match state.tool {
                CanvasTool::DrawZone => (
                    Color32::from_rgba_unmultiplied(0x22, 0x88, 0x44, 40),
                    Color32::from_rgb(0x22, 0xFF, 0x88),
                ),
                CanvasTool::Annotation => (
                    Color32::from_rgba_unmultiplied(0xFF, 0xEE, 0x88, 80),
                    Color32::from_rgb(0xCC, 0xAA, 0x00),
                ),
                _ => (Color32::TRANSPARENT, Color32::WHITE),
            };
            painter.rect_filled(sr, 2.0, fill);
            painter.rect_stroke(sr, 2.0, Stroke::new(1.5, stroke_c));
        }
        ToolInteraction::DraggingImages { .. } => {
            // Images move live — nothing extra to draw
        }
        _ => {}
    }

    // Sticky-collect indicator
    if state.tool == CanvasTool::StickyCollect && !state.collecting.is_empty() {
        let panel = state.panel_rect;
        painter.text(
            Pos2::new(panel.center().x, panel.min.y + 30.0),
            egui::Align2::CENTER_TOP,
            format!(
                "Collecting {} images — click empty canvas to place as stack",
                state.collecting.len()
            ),
            FontId::proportional(13.0),
            Color32::from_rgb(0xFF, 0xEE, 0x44),
        );
    }
}

// ── Context menu ──────────────────────────────────────────────────────────────

fn show_context_menu(state: &mut CanvasSortState, resp: &Response, _ctx: &egui::Context) {
    resp.context_menu(|ui| {
        let any_selected = !state.selected_images.is_empty();
        // Use the position captured on secondary_clicked — reliable across menu frames.
        let rcp = state.right_click_canvas_pos;
        let hit_img = rcp.and_then(|cp| super::tools::hit_image(state, cp));
        let hit_zone = rcp.and_then(|cp| super::tools::hit_zone_body(state, cp));

        // ── Image-specific actions ─────────────────────────────────────────
        if let Some(id) = hit_img {
            let path_str = state
                .session
                .as_ref()
                .and_then(|s| s.image_by_id(id))
                .map(|i| i.path.to_string_lossy().into_owned());
            if let Some(path) = path_str {
                if ui.button("Copy Path").clicked() {
                    ui.output_mut(|o| o.copied_text = path);
                    ui.close_menu();
                }
                if ui.button("Open in File Manager").clicked() {
                    // Best-effort reveal
                    let parent = state
                        .session
                        .as_ref()
                        .and_then(|s| s.image_by_id(id))
                        .and_then(|i| i.path.parent().map(|p| p.to_owned()));
                    if let Some(dir) = parent {
                        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
                    }
                    ui.close_menu();
                }
            }
            ui.separator();
        }

        // ── Zone-specific actions ──────────────────────────────────────────
        if let Some(zone_id) = hit_zone {
            if ui.button("Configure Zone…").clicked() {
                state.zone_edit_id = Some(zone_id);
                ui.close_menu();
            }
            if ui.button("Delete Zone").clicked() {
                if let Some(zone) = state
                    .session
                    .as_ref()
                    .and_then(|s| s.zone_by_id(zone_id))
                    .cloned()
                {
                    state.execute(super::undo::CanvasCommand::DeleteZone { zone });
                }
                ui.close_menu();
            }
            ui.separator();
        }

        // ── Canvas actions ─────────────────────────────────────────────────
        if ui.button("Select All").clicked() {
            state.select_all_images();
            ui.close_menu();
        }
        if ui
            .add_enabled(any_selected, egui::Button::new("Deselect All"))
            .clicked()
        {
            state.clear_selection();
            ui.close_menu();
        }
        ui.separator();
        if ui
            .add_enabled(any_selected, egui::Button::new("Remove from Canvas"))
            .clicked()
        {
            state.delete_selected();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Zoom to Fit (F)").clicked() {
            state.zoom_to_fit();
            ui.close_menu();
        }
        if ui.button("Center View (C)").clicked() {
            state.center_view();
            ui.close_menu();
        }
        if ui.button("Reset Zoom").clicked() {
            state.reset_zoom();
            ui.close_menu();
        }
    });
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_multibyte_does_not_panic_and_limits_chars() {
        // 8 multi-byte chars; truncating to 4 must not split a byte sequence.
        let s = "café_ünïç";
        let out = truncate(s, 4);
        assert_eq!(out.chars().count(), 4);
        assert_eq!(out, "café");
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hi", 18), "hi");
    }
}
