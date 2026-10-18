use egui::{RichText, Ui};
use uuid::Uuid;

use super::state::CanvasSortState;
use super::types::CanvasSortSession;

pub fn show(ui: &mut Ui, state: &mut CanvasSortState, ctx: &egui::Context) {
    // Reload session list if needed
    if state.session_list.is_empty() {
        state.session_list = super::persistence::load_session_list();
    }

    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.heading(RichText::new("Canvas Sort").size(28.0));
        ui.add_space(4.0);
        ui.label(RichText::new("Sort and arrange images visually").weak());
        ui.add_space(20.0);
    });

    // ── Create new session ─────────────────────────────────────────────────
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading("New Session");
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut state.new_session_name);
        });
        ui.horizontal(|ui| {
            ui.label("Folder:");
            ui.text_edit_singleline(&mut state.new_session_folder);
            if ui.button("Browse…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    state.new_session_folder = dir.to_string_lossy().into_owned();
                }
            }
        });
        ui.add_space(4.0);
        let can_create = !state.new_session_name.trim().is_empty();
        if ui
            .add_enabled(can_create, egui::Button::new("✚ Create"))
            .clicked()
        {
            create_session(state, ctx);
        }
    });

    ui.add_space(16.0);

    // ── Existing sessions ──────────────────────────────────────────────────
    if !state.session_list.is_empty() {
        ui.heading("Recent Sessions");
        ui.separator();

        let mut to_open: Option<Uuid> = None;
        let mut to_delete: Option<Uuid> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for meta in &state.session_list {
                ui.horizontal(|ui| {
                    let date = meta.last_accessed.format("%Y-%m-%d %H:%M").to_string();
                    let label = format!("{} — {} images — {}", meta.name, meta.image_count, date);
                    if ui
                        .button(RichText::new(&meta.name).strong())
                        .on_hover_text(label)
                        .clicked()
                    {
                        to_open = Some(meta.id);
                    }
                    ui.label(format!("{} imgs", meta.image_count));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("🗑")
                            .on_hover_text("Delete session")
                            .clicked()
                        {
                            to_delete = Some(meta.id);
                        }
                    });
                });
                ui.separator();
            }
        });

        if let Some(id) = to_open {
            open_session(state, id);
        }
        if let Some(id) = to_delete {
            let _ = super::persistence::delete_session(id);
            state.session_list.retain(|m| m.id != id);
        }
    } else {
        ui.label(RichText::new("No sessions yet. Create one above.").weak());
    }
}

fn create_session(state: &mut CanvasSortState, ctx: &egui::Context) {
    let name = state.new_session_name.trim().to_string();
    let folder = state.new_session_folder.trim().to_string();
    let session = CanvasSortSession::new(name.clone());

    state.new_session_name.clear();
    state.new_session_folder.clear();
    state.clear_selection();
    state.undo_stack.clear();
    state.redo_stack.clear();
    state.history_log.clear();
    state.textures.clear();
    state.thumb_pending.clear();
    state.sticky_notes.clear();
    state.selected_note = None;
    state.session = Some(session);
    state.show_session_list = false;
    state.dirty = true;

    // Load folder if provided
    if !folder.is_empty() {
        let dir = std::path::PathBuf::from(&folder);
        if dir.is_dir() {
            super::toolbar::show_arrange_dialog(ctx, state); // no-op, just ensures dialog is clear
            load_folder_into_session(state, dir, ctx);
        }
    }

    // Auto-save immediately
    let _ = super::persistence::save_session(state);
    state.dirty = false;
    state.session_list = super::persistence::load_session_list();
    state.zoom_to_fit();
    if state.zoom <= 0.021 {
        state.zoom = 0.05;
    }
}

fn open_session(state: &mut CanvasSortState, id: Uuid) {
    if let Some(session) = super::persistence::load_session(id) {
        state.clear_selection();
        state.undo_stack.clear();
        state.redo_stack.clear();
        state.history_log.clear();
        state.textures.clear();
        state.thumb_pending.clear();
        state.sticky_notes = super::persistence::load_notes(id);
        state.selected_note = None;
        state.session = Some(session);
        state.show_session_list = false;
        state.dirty = false;
        state.zoom_to_fit();
        if state.zoom <= 0.021 {
            state.zoom = 0.05;
        }
    }
}

fn load_folder_into_session(
    state: &mut CanvasSortState,
    dir: std::path::PathBuf,
    _ctx: &egui::Context,
) {
    let pairs = super::file_ops::scan_folder(&dir);
    if let Some(s) = &mut state.session {
        let cols = 8usize;
        let spacing = 220.0f32;
        let start_x = 100.0f32;
        let start_y = 100.0f32;
        let mut idx = s.images.len();
        for (path, hash) in pairs {
            let mut img = super::types::SortImage::new(path, hash);
            let col = idx % cols;
            let row = idx / cols;
            img.pos = [
                start_x + col as f32 * spacing,
                start_y + row as f32 * spacing,
            ];
            img.z_index = idx as i32;
            s.images.push(img);
            idx += 1;
        }
        if !s.folder_paths.contains(&dir) {
            s.folder_paths.push(dir);
        }
    }
    state.dirty = true;
}
