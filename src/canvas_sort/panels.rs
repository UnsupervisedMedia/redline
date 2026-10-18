use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};
use uuid::Uuid;

use super::state::CanvasSortState;

// ── Minimap ───────────────────────────────────────────────────────────────────

pub fn draw_minimap(painter: &egui::Painter, state: &CanvasSortState) {
    let s = match &state.session {
        Some(s) => s,
        None => return,
    };
    let panel = state.panel_rect;
    if !panel.is_finite() || panel.area() <= 0.0 {
        return;
    }

    let mm_size = Vec2::new(180.0, 140.0);
    let mm_rect = Rect::from_min_size(
        Pos2::new(
            panel.max.x - mm_size.x - 12.0,
            panel.max.y - mm_size.y - 12.0,
        ),
        mm_size,
    );

    // Background
    painter.rect_filled(mm_rect, 4.0, Color32::from_black_alpha(160));
    painter.rect_stroke(
        mm_rect,
        4.0,
        Stroke::new(1.0, Color32::from_white_alpha(80)),
    );

    // Canvas bounds → minimap transform
    let cs = s.canvas_size_vec();
    let scale_x = mm_size.x / cs.x;
    let scale_y = mm_size.y / cs.y;
    let scale = scale_x.min(scale_y);
    let offset_x = mm_rect.min.x + (mm_size.x - cs.x * scale) * 0.5;
    let offset_y = mm_rect.min.y + (mm_size.y - cs.y * scale) * 0.5;

    let mm_pos = |canvas: Pos2| -> Pos2 {
        Pos2::new(offset_x + canvas.x * scale, offset_y + canvas.y * scale)
    };

    // Draw images as tiny dots
    for img in &s.images {
        let r = Rect::from_min_size(
            mm_pos(img.canvas_pos()),
            Vec2::new(
                (img.size[0] * scale).max(1.5),
                (img.size[1] * scale).max(1.5),
            ),
        );
        painter.rect_filled(r, 0.0, Color32::from_rgb(0x88, 0x88, 0xCC));
    }

    // Draw viewport rectangle
    let vis = state.visible_canvas_rect();
    let vr = Rect::from_min_max(mm_pos(vis.min), mm_pos(vis.max));
    painter.rect_stroke(
        vr,
        1.0,
        Stroke::new(1.5, Color32::from_rgb(0xFF, 0xFF, 0x44)),
    );
}

// ── Search panel ──────────────────────────────────────────────────────────────

pub fn show_search(ui: &mut Ui, state: &mut CanvasSortState) {
    ui.heading("Search");
    let changed = ui.text_edit_singleline(&mut state.search_query).changed();
    if changed || !state.search_query.is_empty() {
        let query = state.search_query.to_lowercase();
        ui.separator();
        if let Some(s) = &state.session {
            let matches: Vec<_> = s
                .images
                .iter()
                .filter(|img| {
                    img.path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.to_lowercase().contains(&query))
                        .unwrap_or(false)
                })
                .collect();
            let count = matches.len();
            ui.label(format!("{count} result(s)"));
            ui.separator();
            let mut pan_to: Option<Pos2> = None;
            for img in matches.iter().take(50) {
                let name = img.path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                if ui.selectable_label(false, name).clicked() {
                    pan_to = Some(img.center());
                }
            }
            if let Some(center) = pan_to {
                // Pan so center is at panel center
                let panel = state.panel_rect;
                state.pan = center.to_vec2() - (panel.center() - panel.min) / state.zoom;
            }
        }
    }
}

// ── History panel ─────────────────────────────────────────────────────────────

pub fn show_history(ui: &mut Ui, state: &mut CanvasSortState) {
    ui.heading("History");
    ui.horizontal(|ui| {
        if ui.button("⟲ Undo").clicked() {
            state.undo();
        }
        if ui.button("⟳ Redo").clicked() {
            state.redo();
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, entry) in state.history_log.iter().enumerate().rev() {
            ui.label(format!("{}: {}", i + 1, entry.label));
        }
        if state.history_log.is_empty() {
            ui.label(egui::RichText::new("No history").weak());
        }
    });
}

// ── Stack management panel ────────────────────────────────────────────────────

pub fn show_stack_panel(ui: &mut Ui, state: &mut CanvasSortState) {
    ui.heading("Stacks");
    ui.separator();

    let stacks: Vec<(Uuid, String, usize)> = {
        let Some(s) = state.session.as_ref() else {
            return;
        };
        s.stacks
            .iter()
            .map(|st| (st.id, st.label.clone(), st.image_ids.len()))
            .collect()
    };

    let mut to_delete: Option<Uuid> = None;
    let mut to_scatter: Option<Uuid> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (id, label, count) in &stacks {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let is_selected = state.selected_stacks.contains(id);
                    let lbl = if label.is_empty() {
                        format!("Stack ({count} images)")
                    } else {
                        format!("{label} ({count})")
                    };
                    if ui.selectable_label(is_selected, lbl).clicked() {
                        if is_selected {
                            state.selected_stacks.remove(id);
                        } else {
                            state.selected_stacks.insert(*id);
                        }
                    }
                    if ui.small_button("↗").on_hover_text("Scatter").clicked() {
                        to_scatter = Some(*id);
                    }
                    if ui.small_button("🗑").on_hover_text("Delete").clicked() {
                        to_delete = Some(*id);
                    }
                });
            });
        }
    });

    if let Some(id) = to_delete {
        if let Some(stack) = state
            .session
            .as_ref()
            .and_then(|s| s.stack_by_id(id))
            .cloned()
        {
            state.execute(super::undo::CanvasCommand::DeleteStack { stack });
        }
    }
}

// ── Zone action picker ────────────────────────────────────────────────────────

pub fn show_zone_picker(ui: &mut Ui, state: &mut CanvasSortState) {
    ui.heading("Zone Action");
    ui.separator();

    let actions = super::types::ZoneAction::all_templates();
    let _current_icon = state
        .zone_picker
        .as_ref()
        .map(|p| p.pending.icon())
        .unwrap_or("?");

    for action in &actions {
        let selected = state
            .zone_picker
            .as_ref()
            .map(|p| p.pending.icon() == action.icon())
            .unwrap_or(false);
        if ui
            .selectable_label(
                selected,
                format!("{} {}", action.icon(), action.default_label()),
            )
            .clicked()
        {
            state.zone_picker = Some(super::state::ZonePicker::new(action.clone()));
        }
    }

    // Show configuration for the selected action
    if let Some(picker) = &mut state.zone_picker {
        ui.separator();
        match &picker.pending {
            super::types::ZoneAction::MoveToFolder { .. }
            | super::types::ZoneAction::CopyToFolder { .. } => {
                ui.label("Target folder:");
                ui.text_edit_singleline(&mut picker.target_path_edit);
            }
            super::types::ZoneAction::AddTags { .. } => {
                ui.label("Tags (comma-separated):");
                ui.text_edit_singleline(&mut picker.tags_edit);
            }
            super::types::ZoneAction::Resize { .. } => {
                ui.horizontal(|ui| {
                    ui.label("W:");
                    ui.text_edit_singleline(&mut picker.resize_w);
                    ui.label("H:");
                    ui.text_edit_singleline(&mut picker.resize_h);
                });
            }
            _ => {}
        }
    }
}
