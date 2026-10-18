#![allow(dead_code)]

pub mod file_ops;
pub mod layout;
pub mod notes;
pub mod panels;
pub mod persistence;
pub mod render;
pub mod session_ui;
pub mod state;
pub mod thumbnail;
pub mod toolbar;
pub mod tools;
pub mod types;
pub mod undo;

use egui::Context;

use state::CanvasSortState;

/// Top-level entry point called from app.rs when AppMode::CanvasSort is active.
pub fn show_canvas_sort(ctx: &Context, state: &mut CanvasSortState) {
    if state.show_session_list || state.session.is_none() {
        // Session list / create new
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                session_ui::show(ui, state, ctx);
            });
        });
        return;
    }

    // ── Toolbar ────────────────────────────────────────────────────────────
    egui::TopBottomPanel::top("cs_toolbar")
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_gray(38))
                .inner_margin(egui::Margin::symmetric(6.0, 3.0)),
        )
        .show(ctx, |ui| {
            toolbar::show(ui, state, ctx);
        });

    // ── Side panels ────────────────────────────────────────────────────────
    if state.show_history {
        egui::SidePanel::right("cs_history")
            .default_width(220.0)
            .show(ctx, |ui| {
                panels::show_history(ui, state);
            });
    }
    if state.show_search {
        egui::SidePanel::right("cs_search")
            .default_width(220.0)
            .show(ctx, |ui| {
                panels::show_search(ui, state);
            });
    }
    if state.show_stack_panel {
        egui::SidePanel::right("cs_stacks")
            .default_width(200.0)
            .show(ctx, |ui| {
                panels::show_stack_panel(ui, state);
            });
    }

    // ── Canvas ─────────────────────────────────────────────────────────────
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            render::show(ui, state, ctx);
        });

    // ── Floating dialogs ───────────────────────────────────────────────────
    toolbar::show_arrange_dialog(ctx, state);
    show_zone_config(ctx, state);
    notes::show_note_overlays(ctx, state);
}

// Status-LED colors (mirrors the palette used in lama_cleaner::ui).
const LED_GREEN: egui::Color32 = egui::Color32::from_rgb(52, 199, 89);
const LED_RED: egui::Color32 = egui::Color32::from_rgb(255, 69, 58);
const LED_GRAY: egui::Color32 = egui::Color32::from_rgb(120, 120, 128);

/// Render a single-line text field with a right-click clipboard context menu
/// (Cut / Copy / Paste / Select All). Returns the field's response so callers
/// can detect commits (Enter / lost focus) and value changes.
fn text_edit_with_menu(ui: &mut egui::Ui, text: &mut String, width: f32) -> egui::Response {
    let resp = ui.add_sized(
        egui::Vec2::new(width, 20.0),
        egui::TextEdit::singleline(text),
    );
    let id = resp.id;
    resp.context_menu(|ui| {
        if ui.button("✂  Cut").clicked() {
            ui.output_mut(|o| o.copied_text = text.clone());
            text.clear();
            ui.close_menu();
        }
        if ui.button("⧉  Copy").clicked() {
            ui.output_mut(|o| o.copied_text = text.clone());
            ui.close_menu();
        }
        if ui.button("📋  Paste").clicked() {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Ok(t) = cb.get_text() {
                    *text = t.trim().to_owned();
                }
            }
            ui.ctx().memory_mut(|m| m.request_focus(id));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Select all").clicked() {
            if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                let range = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                );
                st.cursor.set_char_range(Some(range));
                st.store(ui.ctx(), id);
            }
            ui.ctx().memory_mut(|m| m.request_focus(id));
            ui.close_menu();
        }
    });
    resp
}

/// Last path component (folder/file name) of `p`, if any. Used to derive a
/// project label from a pasted folder path.
fn last_path_segment(p: &std::path::Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn show_zone_config(ctx: &Context, state: &mut state::CanvasSortState) {
    use types::ZoneAction;

    let zone_id = match state.zone_edit_id {
        Some(id) => id,
        None => return,
    };

    // Initialise the persistent draft the first time this zone_id opens.
    // We only re-seed from the session when the draft is for a different zone.
    let needs_init = state
        .zone_edit_draft
        .as_ref()
        .map(|d| d.id != zone_id)
        .unwrap_or(true);
    if needs_init {
        match state.session.as_ref().and_then(|s| s.zone_by_id(zone_id)) {
            Some(z) => {
                state.zone_edit_draft = Some(z.clone());
            }
            None => {
                state.zone_edit_id = None;
                state.zone_edit_draft = None;
                return;
            }
        }
    }

    // Take the draft out temporarily so we can pass both it and state to the UI.
    let mut draft = match state.zone_edit_draft.take() {
        Some(d) => d,
        None => return,
    };

    let mut do_close = false;
    let mut do_apply = false;
    let mut browse_clicked = false;
    let mut enter_submit = false;
    let mut create_dir: Option<std::path::PathBuf> = None;

    egui::Window::new("Zone Properties")
        .collapsible(false)
        .resizable(false)
        .default_width(320.0)
        .show(ctx, |ui| {
            ui.label("Label:");
            let label_resp = text_edit_with_menu(ui, &mut draft.label, ui.available_width());
            if label_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                enter_submit = true;
            }

            ui.separator();
            ui.label("Action:");
            egui::ComboBox::from_id_salt("zone_action_type")
                .selected_text(draft.action.label())
                .show_ui(ui, |ui| {
                    for template in ZoneAction::all_templates() {
                        let label = template.label();
                        if ui
                            .selectable_label(
                                std::mem::discriminant(&draft.action)
                                    == std::mem::discriminant(&template),
                                label,
                            )
                            .clicked()
                        {
                            draft.action = template;
                        }
                    }
                });

            ui.separator();

            match &mut draft.action {
                ZoneAction::MoveToFolder { target } | ZoneAction::CopyToFolder { target } => {
                    ui.label("Target folder:");
                    // Edit the path as a string directly — it persists in draft across frames.
                    let mut path_str = target.to_string_lossy().into_owned();
                    let orig = path_str.clone();
                    let prev_seg = last_path_segment(target);
                    let resp = text_edit_with_menu(ui, &mut path_str, ui.available_width());
                    // Compare by value too: context-menu cut/paste mutate the buffer
                    // without setting egui's `changed()` flag.
                    if resp.changed() || path_str != orig {
                        let new_path = std::path::PathBuf::from(path_str.trim());
                        // Auto-derive the project label from the folder's last segment
                        // when the label is still "auto" (empty, an action default, or
                        // tracking the previous folder name) — don't clobber a custom one.
                        let label_is_auto = draft.label.trim().is_empty()
                            || ZoneAction::all_templates()
                                .iter()
                                .any(|t| t.default_label() == draft.label)
                            || prev_seg.as_deref() == Some(draft.label.as_str());
                        if label_is_auto {
                            if let Some(seg) = last_path_segment(&new_path) {
                                draft.label = seg;
                            }
                        }
                        *target = new_path;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        enter_submit = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Browse…").clicked() {
                            browse_clicked = true;
                        }
                        let p = target.as_path();
                        if p.as_os_str().is_empty() {
                            ui.colored_label(LED_GRAY, "●")
                                .on_hover_text("No folder set");
                        } else if p.is_dir() {
                            ui.colored_label(LED_GREEN, "●")
                                .on_hover_text("Folder exists");
                        } else {
                            ui.colored_label(LED_RED, "●")
                                .on_hover_text("Folder does not exist");
                            if ui.button("Create folder").clicked() {
                                create_dir = Some(p.to_path_buf());
                            }
                        }
                    });
                }
                ZoneAction::AddTags { tags } => {
                    ui.label("Tags (comma-separated):");
                    let mut tags_str = tags.join(", ");
                    let orig = tags_str.clone();
                    let resp = text_edit_with_menu(ui, &mut tags_str, ui.available_width());
                    if resp.changed() || tags_str != orig {
                        *tags = tags_str
                            .split(',')
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .collect();
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        enter_submit = true;
                    }
                }
                ZoneAction::Resize { width, height } => {
                    ui.horizontal(|ui| {
                        ui.label("W:");
                        ui.add(egui::DragValue::new(width).speed(1));
                        ui.label("H:");
                        ui.add(egui::DragValue::new(height).speed(1));
                    });
                }
                _ => {
                    ui.label("No additional configuration needed.");
                }
            }

            ui.separator();
            ui.checkbox(&mut draft.auto_process, "Auto-process when images dropped");
            if draft.auto_process {
                ui.indent("zone_auto_opts", |ui| {
                    ui.checkbox(
                        &mut draft.remove_after_action,
                        "Remove from canvas after action",
                    );
                    if !draft.remove_after_action {
                        ui.weak(
                            "Images stay on canvas — drag to another zone or move back freely.",
                        );
                    }
                });
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    do_apply = true;
                }
                if ui.button("Cancel").clicked() {
                    do_close = true;
                }
            });
        });

    // Handle Browse outside the closure so we can mutate state freely.
    if browse_clicked {
        let mut dialog = rfd::FileDialog::new();
        if let Some(last) = &state.zone_last_folder {
            dialog = dialog.set_directory(last);
        }
        if let Some(dir) = dialog.pick_folder() {
            state.zone_last_folder = Some(dir.clone());
            let prev_seg = if let ZoneAction::MoveToFolder { target }
            | ZoneAction::CopyToFolder { target } = &draft.action
            {
                last_path_segment(target)
            } else {
                None
            };
            let label_is_auto = draft.label.trim().is_empty()
                || ZoneAction::all_templates()
                    .iter()
                    .any(|t| t.default_label() == draft.label)
                || prev_seg.as_deref() == Some(draft.label.as_str());
            if label_is_auto {
                if let Some(seg) = last_path_segment(&dir) {
                    draft.label = seg;
                }
            }
            if let ZoneAction::MoveToFolder { target } | ZoneAction::CopyToFolder { target } =
                &mut draft.action
            {
                *target = dir;
            }
        }
    }

    // Create-folder button: make the directory now so the LED turns green.
    if let Some(dir) = create_dir {
        match std::fs::create_dir_all(&dir) {
            Ok(()) => state.status = Some(format!("Created folder: {}", dir.display())),
            Err(e) => state.status = Some(format!("Failed to create folder: {e}")),
        }
    }

    // Enter in any text field submits the form.
    if enter_submit {
        do_apply = true;
    }

    if do_apply {
        // Auto-create the target folder on apply if it doesn't exist yet.
        if let ZoneAction::MoveToFolder { target } | ZoneAction::CopyToFolder { target } =
            &draft.action
        {
            if !target.as_os_str().is_empty() && !target.is_dir() {
                let _ = std::fs::create_dir_all(target);
            }
        }
        state.execute(undo::CanvasCommand::UpdateZone { zone: draft });
        state.zone_edit_id = None;
        state.zone_edit_draft = None;
    } else if do_close {
        state.zone_edit_id = None;
        state.zone_edit_draft = None;
    } else {
        // Put the draft back for next frame.
        state.zone_edit_draft = Some(draft);
    }
}
