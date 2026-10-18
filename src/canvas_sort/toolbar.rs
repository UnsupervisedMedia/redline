use egui::{Color32, RichText, Ui, Vec2};
use uuid::Uuid;

use super::state::CanvasSortState;
use super::types::{CanvasTool, SortMode};

pub fn show(ui: &mut Ui, state: &mut CanvasSortState, ctx: &egui::Context) {
    ui.horizontal(|ui| {
        // ── Tool selector ──────────────────────────────────────────────
        ui.label("Tool:");
        let tools = [
            (CanvasTool::Select, "⬚", "Select (V)"),
            (CanvasTool::DrawZone, "⬜", "Draw Zone (B)"),
            (CanvasTool::Annotation, "📝", "Sticky Note (N)"),
            (CanvasTool::StickyCollect, "+", "Sticky Collect"),
        ];
        for (tool, icon, tip) in tools {
            let active = state.tool == tool;
            if ui
                .add(
                    egui::Button::new(RichText::new(icon).size(16.0))
                        .selected(active)
                        .min_size(Vec2::new(28.0, 24.0)),
                )
                .on_hover_text(tip)
                .clicked()
            {
                state.tool = tool;
                state.tool_state = super::state::ToolInteraction::Idle;
            }
        }

        ui.separator();

        // ── Zoom controls ──────────────────────────────────────────────
        if ui.button("−").on_hover_text("Zoom out (step)").clicked() {
            let center = ui.clip_rect().center();
            state.zoom_out_at(center);
        }

        // Logarithmic slider: map [ZOOM_MIN, ZOOM_MAX] → [0, 1] via log
        let log_min = super::state::ZOOM_MIN.ln();
        let log_max = super::state::ZOOM_MAX.ln();
        let mut log_zoom = state.zoom.ln();
        let slider = egui::Slider::new(&mut log_zoom, log_min..=log_max)
            .show_value(false)
            .clamping(egui::SliderClamping::Always);
        let resp_slider = ui.add_sized(egui::Vec2::new(110.0, 16.0), slider);
        if resp_slider.changed() {
            let new_zoom = log_zoom
                .exp()
                .clamp(super::state::ZOOM_MIN, super::state::ZOOM_MAX);
            let center = ui.clip_rect().center();
            state.zoom_at(new_zoom, center);
        }

        if ui.button("+").on_hover_text("Zoom in (step)").clicked() {
            let center = ui.clip_rect().center();
            state.zoom_in_at(center);
        }

        // Editable percentage label — click to type an exact value
        let mut pct = format!("{:.0}", state.zoom * 100.0);
        let pct_resp = ui.add_sized(
            egui::Vec2::new(42.0, 16.0),
            egui::TextEdit::singleline(&mut pct).horizontal_align(egui::Align::Center),
        );
        if pct_resp.lost_focus() {
            if let Ok(v) = pct.trim_end_matches('%').trim().parse::<f32>() {
                let new_zoom = (v / 100.0).clamp(super::state::ZOOM_MIN, super::state::ZOOM_MAX);
                let center = ui.clip_rect().center();
                state.zoom_at(new_zoom, center);
            }
        }
        ui.label("%");

        if ui.button("F").on_hover_text("Fit all (F)").clicked() {
            state.zoom_to_fit();
        }
        if ui.button("C").on_hover_text("Center (C)").clicked() {
            state.center_view();
        }

        ui.separator();

        // ── Sort menu ──────────────────────────────────────────────────
        let mut do_sort: Option<(SortMode, usize, f32)> = None;
        ui.menu_button("Sort ▾", |ui| {
            for mode in [
                SortMode::NameAsc,
                SortMode::NameDesc,
                SortMode::DateNewest,
                SortMode::DateOldest,
                SortMode::SizeLargest,
                SortMode::SizeSmallest,
                SortMode::TypeAsc,
                SortMode::Random,
            ] {
                if ui.button(mode.label()).clicked() {
                    do_sort = Some((mode, 8, 20.0));
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui.button("Arrange in Grid…").clicked() {
                state.arrange_dialog = Some(super::state::ArrangeDialog::default());
                ui.close_menu();
            }
        });

        if let Some((mode, cols, spacing)) = do_sort {
            apply_sort(state, mode, cols, spacing);
        }

        // ── Align menu ─────────────────────────────────────────────────
        ui.menu_button("Align ▾", |ui| {
            let ids: Vec<Uuid> = state.selected_images.iter().copied().collect();
            if ids.len() >= 2 {
                if ui.button("Align Left").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::align_left(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                if ui.button("Align Right").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::align_right(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                if ui.button("Align Top").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::align_top(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                if ui.button("Align Bottom").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::align_bottom(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Distribute H").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::distribute_h(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                if ui.button("Distribute V").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        super::layout::distribute_v(s, &ids)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quick Grid").clicked() {
                    let moves = if let Some(s) = &mut state.session {
                        let origin = s
                            .image_by_id(ids[0])
                            .map(|i| egui::Vec2::new(i.pos[0], i.pos[1]))
                            .unwrap_or_default();
                        super::layout::smart_grid(s, &ids, 20.0, origin)
                    } else {
                        vec![]
                    };
                    if !moves.is_empty() {
                        state.execute(super::undo::CanvasCommand::SortImages { moves });
                    }
                    ui.close_menu();
                }
            } else {
                ui.label(RichText::new("Select 2+ images").weak());
            }
        });

        ui.separator();

        // ── Undo / Redo ────────────────────────────────────────────────
        if ui
            .add_enabled(!state.undo_stack.is_empty(), egui::Button::new("⟲"))
            .on_hover_text("Undo (Ctrl+Z)")
            .clicked()
        {
            state.undo();
        }
        if ui
            .add_enabled(!state.redo_stack.is_empty(), egui::Button::new("⟳"))
            .on_hover_text("Redo (Ctrl+Y)")
            .clicked()
        {
            state.redo();
        }

        ui.separator();

        // ── Panel toggles ──────────────────────────────────────────────
        if ui
            .selectable_label(state.show_minimap, "M")
            .on_hover_text("Minimap (M)")
            .clicked()
        {
            state.show_minimap = !state.show_minimap;
        }
        if ui
            .selectable_label(state.show_search, "🔍")
            .on_hover_text("Search (Ctrl+F)")
            .clicked()
        {
            state.show_search = !state.show_search;
        }
        if ui
            .selectable_label(state.show_history, "H")
            .on_hover_text("History (H)")
            .clicked()
        {
            state.show_history = !state.show_history;
        }
        if ui
            .selectable_label(state.show_stack_panel, "⊞")
            .on_hover_text("Stacks")
            .clicked()
        {
            state.show_stack_panel = !state.show_stack_panel;
        }

        ui.separator();

        // ── Entropy toggle ─────────────────────────────────────────────
        let entropy_label = if state.entropy_enabled {
            "🌀 ON"
        } else {
            "🌀"
        };
        if ui
            .selectable_label(state.entropy_enabled, entropy_label)
            .on_hover_text("Entropy mode: periodically scatter images")
            .clicked()
        {
            state.entropy_enabled = !state.entropy_enabled;
        }
        if state.entropy_enabled {
            ui.menu_button("⚙", |ui| {
                ui.label("Frequency (min):");
                ui.add(
                    egui::Slider::new(&mut state.entropy_frequency_mins, 1.0..=120.0).step_by(1.0),
                );
                ui.label("Intensity (%):");
                ui.add(
                    egui::Slider::new(&mut state.entropy_intensity_pct, 1.0..=100.0).step_by(1.0),
                );
            });
        }

        ui.separator();

        // ── Virtualization toggle ──────────────────────────────────────
        if ui
            .selectable_label(state.virtualization, "⚡ Virt")
            .on_hover_text("Viewport virtualization")
            .clicked()
        {
            state.virtualization = !state.virtualization;
        }

        ui.separator();

        // ── Load folder ────────────────────────────────────────────────
        if ui.button("📂 Add Folder").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                load_folder(state, dir, ctx);
            }
        }

        // ── Session list ───────────────────────────────────────────────
        if ui.button("🗂 Sessions").clicked() {
            state.show_session_list = true;
        }

        // ── Image count ────────────────────────────────────────────────
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(s) = &state.session {
                let total = s.images.len();
                ui.label(format!("{total} images"));
                if state.should_virtualize() {
                    ui.label(RichText::new("VIRT").color(Color32::from_rgb(0x44, 0xCC, 0x44)));
                }
            }
            if let Some(msg) = &state.status {
                ui.label(RichText::new(msg.as_str()).weak());
            }
        });
    });
}

// ── Arrange dialog ────────────────────────────────────────────────────────────

pub fn show_arrange_dialog(ctx: &egui::Context, state: &mut CanvasSortState) {
    let Some(dialog) = state.arrange_dialog.clone() else {
        return;
    };

    let mut do_arrange = false;
    let mut do_close = false;
    let mut d = dialog.clone();

    egui::Window::new("Arrange in Grid")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.fixed_columns, true, "Fixed columns");
                ui.radio_value(&mut d.fixed_columns, false, "Fixed rows");
            });
            let lbl = if d.fixed_columns { "Columns:" } else { "Rows:" };
            ui.add(egui::Slider::new(&mut d.count, 1..=50).text(lbl));
            ui.add(egui::Slider::new(&mut d.spacing, 0.0..=200.0).text("Spacing"));
            ui.checkbox(&mut d.selected_only, "Selected images only");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Arrange").clicked() {
                    do_arrange = true;
                }
                if ui.button("Cancel").clicked() {
                    do_close = true;
                }
            });
        });

    if do_arrange {
        let ids = if d.selected_only {
            state.selected_images.iter().copied().collect::<Vec<_>>()
        } else {
            state
                .session
                .as_ref()
                .map(|s| s.loose_image_ids())
                .unwrap_or_default()
        };
        let origin = state
            .session
            .as_ref()
            .and_then(|s| s.image_by_id(*ids.first().unwrap_or(&uuid::Uuid::nil())))
            .map(|i| egui::Vec2::new(i.pos[0], i.pos[1]))
            .unwrap_or_default();
        let moves = if let Some(s) = &mut state.session {
            super::layout::arrange_in_grid(s, &ids, d.fixed_columns, d.count, d.spacing, origin)
        } else {
            vec![]
        };
        if !moves.is_empty() {
            state.execute(super::undo::CanvasCommand::SortImages { moves });
        }
        do_close = true;
    }

    if do_close {
        state.arrange_dialog = None;
    } else {
        state.arrange_dialog = Some(d);
    }
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn apply_sort(state: &mut CanvasSortState, mode: SortMode, cols: usize, spacing: f32) {
    let origin = state
        .session
        .as_ref()
        .and_then(|s| s.images_bounds())
        .map(|b| egui::Vec2::new(b.min.x, b.min.y))
        .unwrap_or(egui::Vec2::new(100.0, 100.0));
    let moves = if let Some(s) = &mut state.session {
        super::layout::sort_images(s, mode, cols, spacing, origin)
    } else {
        return;
    };
    if !moves.is_empty() {
        state.execute(super::undo::CanvasCommand::SortImages { moves });
    }
}

fn load_folder(state: &mut CanvasSortState, dir: std::path::PathBuf, _ctx: &egui::Context) {
    state.status = Some(format!("Scanning {}…", dir.display()));
    state.loading_folder = true;

    // Scan on a background thread, send results via channel
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let dir_clone = dir.clone();
    std::thread::spawn(move || {
        let pairs = super::file_ops::scan_folder(&dir_clone);
        let _ = tx.send(pairs);
    });

    // For simplicity, block briefly (real implementation polls via state)
    // TODO: in production, poll the receiver each frame instead of blocking
    if let Ok(pairs) = rx.recv_timeout(std::time::Duration::from_secs(30)) {
        state.loading_folder = false;
        if let Some(s) = &mut state.session {
            let _thumb_size = s.settings.effective_thumb_size();
            let start_x = 100.0f32;
            let start_y = 100.0f32;
            let cols = 8usize;
            let spacing = 220.0f32;
            let existing_hashes: std::collections::HashSet<[u8; 32]> =
                s.images.iter().map(|i| i.file_hash).collect();
            let mut new_images = Vec::new();
            for (path, hash) in pairs {
                if existing_hashes.contains(&hash) {
                    continue;
                }
                let idx = new_images.len();
                let mut img = super::types::SortImage::new(path.clone(), hash);
                // Read image dimensions from the file header (fast, header-only).
                // This sets the correct aspect-ratio placeholder before the thumbnail loads.
                if let Ok((w, h)) = image::image_dimensions(&path) {
                    let max_dim = 200.0_f32;
                    img.size = if w >= h {
                        [max_dim, max_dim * h as f32 / w as f32]
                    } else {
                        [max_dim * w as f32 / h as f32, max_dim]
                    };
                }
                let col = idx % cols;
                let row = idx / cols;
                img.pos = [
                    start_x + col as f32 * spacing,
                    start_y + row as f32 * spacing,
                ];
                img.z_index = s.next_z_index() + idx as i32;
                new_images.push(img);
            }
            if !s.folder_paths.contains(&dir) {
                s.folder_paths.push(dir);
            }
            let to_add: Vec<_> = new_images.clone();
            s.images.extend(new_images);
            state.dirty = true;
            state.status = Some(format!("Loaded {} images", to_add.len()));
        }
    } else {
        state.loading_folder = false;
        state.status = Some("Folder scan timed out".into());
    }
}
