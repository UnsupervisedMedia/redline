use crate::command::CommandExecutor;
use crate::shortcuts::{default_shortcuts, save_shortcuts, Action};
use crate::stamps::{all_stamps, stamp_categories};
use crate::state::{
    push_recent_color, AdjustPreview, AppState, CanvasBackground, EdgeMethod, ExportFormat,
    SelKind, SnapMode, SvgBackground, SvgPathEntry, TabDrag, Tool, ToolState, VectorizeEmitMode,
    VectorizeMode, COLOR_PRESETS,
};
use crate::theme::Theme;
use crate::types::{
    ArrowHeads, BoolOp, Command, LineStyle, LogicTagKind, NodeProp, ParticipantId, PropChange,
};
use egui::{Align, Color32, Layout, RichText, Stroke, Ui};
use uuid::Uuid;

use crate::canvas::pathfinder_selection;

// ── Shared File menu ──────────────────────────────────────────────────────────

/// The File menu (New / Open / Save / Export / Clear). Factored out so it is
/// identical everywhere: the full Annotate menu bar (`show_menu_bar`) and the
/// compact file-only bar (`show_file_only_bar`) shown by the other file-based
/// tools both render this single definition.
pub fn file_menu(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    ui.menu_button("File", |ui| {
        if ui.button("🆕 New Canvas…").clicked() {
            ui.close_menu();
            open_new_canvas_dialog(state);
        }
        ui.separator();
        let ctx = ui.ctx().clone();
        if ui.button("📂 Open…").clicked() {
            ui.close_menu();
            open_image_via_dialog(state, &ctx);
        }
        ui.separator();
        let has = state.canvas.texture.is_some();
        // Saved/unsaved dot next to Save.
        if has {
            let (dot, tip) = if state.canvas.is_saved() {
                (egui::Color32::from_rgb(120, 200, 120), "All changes saved")
            } else {
                (egui::Color32::from_rgb(220, 80, 80), "Unsaved changes")
            };
            ui.label(egui::RichText::new("●").color(dot))
                .on_hover_text(tip);
        }
        let save_label = if has && !state.canvas.is_saved() {
            "💾 Save*"
        } else {
            "💾 Save"
        };
        if ui
            .add_enabled(has, egui::Button::new(save_label))
            .on_hover_text("Overwrite the opened file (Ctrl+S)")
            .clicked()
        {
            ui.close_menu();
            save_image(state);
        }
        if ui
            .add_enabled(has, egui::Button::new("💾 Save As…"))
            .clicked()
        {
            ui.close_menu();
            save_image_as(state);
        }
        if ui
            .add_enabled(has, egui::Button::new("📋 Copy Image"))
            .on_hover_text("Copy to clipboard")
            .clicked()
        {
            ui.close_menu();
            copy_image_with_flatten(state);
        }
        ui.separator();
        if ui
            .add_enabled(has, egui::Button::new("📤 Export Sidecar"))
            .on_hover_text("Copy image + JSON to clipboard (Ctrl+Shift+C)")
            .clicked()
        {
            ui.close_menu();
            crate::export::export(state);
        }
        if ui.button("📥 Import Sidecar…").clicked() {
            ui.close_menu();
            let mut dialog = rfd::FileDialog::new().add_filter("RedLine sidecar", &["json"]);
            if let Some(d) = state.last_open_dir.as_ref() {
                dialog = dialog.set_directory(d);
            }
            if let Some(path) = dialog.pick_file() {
                crate::state::remember_open_dir(state, &path);
                match crate::export::import_sidecar(&path, &mut state.canvas) {
                    Ok(()) => state.status_msg = Some("Sidecar loaded".into()),
                    Err(e) => state.status_msg = Some(e),
                }
            }
        }
        ui.separator();
        ui.checkbox(&mut state.settings.flatten_export, "Flatten on Save/Copy")
            .on_hover_text("Burn annotations into the saved/copied image");
        ui.menu_button("🖼 Export Format", |ui| {
            for f in [
                ExportFormat::Png,
                ExportFormat::Jpeg,
                ExportFormat::WebP,
                ExportFormat::Bmp,
            ] {
                let sel = state.settings.export_format == f;
                if ui.selectable_label(sel, f.label()).clicked() {
                    state.settings.export_format = f;
                }
            }
            if state.settings.export_format == ExportFormat::Jpeg {
                ui.separator();
                ui.label(
                    RichText::new("JPEG Quality")
                        .color(theme.text_label)
                        .small(),
                );
                ui.add(egui::Slider::new(&mut state.settings.jpeg_quality, 1..=100).text("%"));
            }
        });
        ui.separator();
        if ui
            .add_enabled(has, egui::Button::new("🗑 Clear Canvas"))
            .clicked()
        {
            ui.close_menu();
            clear_canvas(state);
        }
    });
}

/// Open an image/PDF via the native file dialog and load it into the shared
/// canvas (used by the File menu and the Native Inpaint toolbar). Because the
/// canvas is shared across modes, the opened image appears in every tab.
pub fn open_image_via_dialog(state: &mut AppState, ctx: &egui::Context) {
    let mut dialog = rfd::FileDialog::new().add_filter(
        "Image / PDF",
        &["png", "jpg", "jpeg", "webp", "bmp", "gif", "pdf"],
    );
    if let Some(d) = state.last_open_dir.as_ref() {
        dialog = dialog.set_directory(d);
    }
    if let Some(path) = dialog.pick_file() {
        crate::state::remember_open_dir(state, &path);
        crate::loader::load_file(&path, state, ctx);
    }
}

/// A minimal menu bar that exposes only the shared File menu. Used by the
/// non-Annotate file-based tools (Native Inpaint, SVG editor) so they get the
/// same Open/Save/Export operations without the annotation-specific menus.
pub fn show_file_only_bar(ui: &mut Ui, state: &mut AppState) {
    egui::menu::bar(ui, |ui| {
        file_menu(ui, state);
    });
}

// ── Tool-selection bar ────────────────────────────────────────────────────────

/// The persistent tool-selection bar shown above every tool. Two visually
/// separated groups: **Files** (file-based editors that share the open file
/// tabs — RedLine annotate, Native Inpaint, SVG) and **Folders** (self-contained
/// workspaces that manage their own image set — Canvas Sort, Lama-Cleaner).
pub fn show_tool_bar(ui: &mut Ui, state: &mut AppState) {
    use crate::state::AppMode;
    let theme = state.settings.theme();
    ui.horizontal(|ui| {
        let t = theme.clone();
        let mut tool_button = |ui: &mut Ui, mode: AppMode, label: &str, tip: &str| {
            let active = state.app_mode == mode;
            let (fill, fg, stroke) = if active {
                (t.accent_subtle, t.accent, Stroke::new(1.0, t.accent_dim))
            } else {
                (t.surface_bg, t.text_muted, Stroke::new(1.0, t.border_idle))
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(label).size(12.5).color(fg))
                        .fill(fill)
                        .stroke(stroke)
                        .rounding(egui::Rounding::same(7.0))
                        .min_size(egui::Vec2::new(96.0, 24.0)),
                )
                .on_hover_text(tip)
                .clicked()
            {
                state.app_mode = mode;
            }
        };

        let cap_color = theme.text_dim;
        let caption = move |ui: &mut Ui, text: &str| {
            ui.label(RichText::new(text).size(10.0).color(cap_color));
        };

        caption(ui, "Files");
        tool_button(ui, AppMode::Annotate, "RedLine", "Annotation & drawing");
        tool_button(
            ui,
            AppMode::NativeInpaint,
            "Native Inpaint",
            "AI removal — runs locally in Rust, no Python",
        );
        tool_button(
            ui,
            AppMode::SvgEditor,
            "SVG Editor",
            "Vector editing (coming soon)",
        );

        ui.separator();

        caption(ui, "Folders");
        tool_button(
            ui,
            AppMode::CanvasSort,
            "Canvas Sort",
            "Visual image sorting — works on a folder of images",
        );
        tool_button(
            ui,
            AppMode::LamaCleaner,
            "Lama-Cleaner",
            "AI inpainting (Classic / Python webview) — Native Inpaint is the modern replacement",
        );
    });
}

// ── Menu bar (File / Edit / View) ─────────────────────────────────────────────

pub fn show_menu_bar(ui: &mut Ui, state: &mut AppState) {
    egui::menu::bar(ui, |ui| {
        file_menu(ui, state);

        ui.menu_button("Edit", |ui| {
            let can_undo = state
                .canvas
                .undo_stacks
                .get(&ParticipantId::HUMAN_LOCAL)
                .map_or(false, |s| !s.is_empty());
            let can_redo = state
                .canvas
                .redo_stacks
                .get(&ParticipantId::HUMAN_LOCAL)
                .map_or(false, |s| !s.is_empty());
            if ui
                .add_enabled(can_undo, egui::Button::new("↩ Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                ui.close_menu();
                CommandExecutor::undo(&mut state.canvas, ParticipantId::HUMAN_LOCAL);
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("↪ Redo"))
                .on_hover_text("Ctrl+Y")
                .clicked()
            {
                ui.close_menu();
                CommandExecutor::redo(&mut state.canvas, ParticipantId::HUMAN_LOCAL);
            }
            ui.separator();
            ui.checkbox(&mut state.settings.auto_save, "Auto-save Captures");
            // AI / Agents controls are collapsed behind a single sub-menu and stay
            // opt-in: nothing AI-related runs or shows until the user reveals this
            // section and explicitly enables a feature.
            #[cfg(any(feature = "agent-server", feature = "local-ai"))]
            draw_ai_agents_menu(ui, state);
        });

        ui.menu_button("Image", |ui| {
            let has = state.canvas.texture.is_some();
            let has_sel = state.canvas.selection.is_some();
            if ui
                .add_enabled(has, egui::Button::new("↻ Rotate 90° CW"))
                .clicked()
            {
                ui.close_menu();
                crate::image_ops::rotate_90(state, true);
            }
            if ui
                .add_enabled(has, egui::Button::new("↺ Rotate 90° CCW"))
                .clicked()
            {
                ui.close_menu();
                crate::image_ops::rotate_90(state, false);
            }
            ui.separator();
            if ui
                .add_enabled(has, egui::Button::new("🎛 Adjustments…"))
                .on_hover_text("Brightness / contrast / saturation")
                .clicked()
            {
                ui.close_menu();
                open_adjust_dialog(state);
            }
            if ui
                .add_enabled(has, egui::Button::new("Convert to SVG…"))
                .on_hover_text("Convert image to scalable vector (SVG)")
                .clicked()
            {
                ui.close_menu();
                state.canvas.vectorize_panel.open = true;
                state.settings.show_recents = false;
            }
            ui.separator();
            if ui
                .add_enabled(has_sel, egui::Button::new("◳ Crop to Selection"))
                .clicked()
            {
                ui.close_menu();
                crate::image_ops::crop_to_selection(state);
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("⇄ Invert Selection"))
                .clicked()
            {
                ui.close_menu();
                if let Some(sel) = state.canvas.selection.as_mut() {
                    sel.inverted = !sel.inverted;
                }
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("✖ Clear Selection"))
                .clicked()
            {
                ui.close_menu();
                state.canvas.selection = None;
            }
            ui.separator();
            if ui
                .add_enabled(has, egui::Button::new("📐 Resize Canvas…"))
                .clicked()
            {
                ui.close_menu();
                let (w, h) = if let Some(ci) = state.canvas.color_image.as_ref() {
                    (ci.size[0] as u32, ci.size[1] as u32)
                } else {
                    (800, 600)
                };
                state.resize_canvas_dialog = Some(crate::state::CanvasSizeDialog::new(w, h, None));
            }
            ui.separator();
            if ui
                .add_enabled(has_sel, egui::Button::new("✂ Cut Selection"))
                .clicked()
            {
                ui.close_menu();
                if let Some(buf) = crate::image_ops::cut_selection(state) {
                    copy_rgba_to_clipboard(state, buf, "Cut selection to clipboard");
                }
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("⎘ Copy Selection"))
                .clicked()
            {
                ui.close_menu();
                if let Some(buf) = crate::image_ops::copy_selection_pixels(state) {
                    copy_rgba_to_clipboard(state, buf, "Copied selection to clipboard");
                }
            }
        });

        // ── Object + Path menus (path operations) ──────────────────────────
        let sel_ids: Vec<Uuid> = match &state.tool_state {
            ToolState::Selected { id, .. } => vec![*id],
            ToolState::MultiSelected { ids, .. } => ids.clone(),
            _ => Vec::new(),
        };
        let (closed_paths, open_paths) = pathfinder_selection(state);

        ui.menu_button("Object", |ui| {
            let can_group = sel_ids.len() >= 2;
            if ui
                .add_enabled(can_group, egui::Button::new("⧉ Group"))
                .clicked()
            {
                ui.close_menu();
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Group {
                        ids: sel_ids.clone(),
                        name: "Group".into(),
                    },
                );
            }
            let can_ungroup = sel_ids.len() == 1
                && matches!(
                    state.canvas.document.get(sel_ids[0]).map(|n| &n.kind),
                    Some(crate::scene::NodeKind::Group)
                );
            if ui
                .add_enabled(can_ungroup, egui::Button::new("⤢ Ungroup"))
                .clicked()
            {
                ui.close_menu();
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::Ungroup {
                        group_id: sel_ids[0],
                    },
                );
            }
            ui.separator();
            if ui
                .add_enabled(!sel_ids.is_empty(), egui::Button::new("⬆ Bring to Front"))
                .clicked()
            {
                ui.close_menu();
                for id in &sel_ids {
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::BringToFront(*id),
                    );
                }
            }
            if ui
                .add_enabled(!sel_ids.is_empty(), egui::Button::new("⬇ Send to Back"))
                .clicked()
            {
                ui.close_menu();
                for id in &sel_ids {
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::SendToBack(*id),
                    );
                }
            }
            ui.separator();
            if ui
                .add_enabled(!sel_ids.is_empty(), egui::Button::new("↔ Flip Horizontal"))
                .clicked()
            {
                ui.close_menu();
                if let Some(cmd) = crate::tools::select::flip_command(&state.canvas, &sel_ids, true)
                {
                    CommandExecutor::execute(&mut state.canvas, ParticipantId::HUMAN_LOCAL, cmd);
                }
            }
            if ui
                .add_enabled(!sel_ids.is_empty(), egui::Button::new("↕ Flip Vertical"))
                .clicked()
            {
                ui.close_menu();
                if let Some(cmd) =
                    crate::tools::select::flip_command(&state.canvas, &sel_ids, false)
                {
                    CommandExecutor::execute(&mut state.canvas, ParticipantId::HUMAN_LOCAL, cmd);
                }
            }
            ui.separator();
            // Flatten layer = union all closed vector leaves in the active layer.
            if ui.button("▤ Flatten Layer").clicked() {
                ui.close_menu();
                let layer = state.canvas.active_layer;
                CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    Command::FlattenLayer { layer_id: layer },
                );
            }
        });

        ui.menu_button("Path", |ui| {
            let can_bool = closed_paths.len() >= 2;
            for (label, op) in [
                ("Unite", BoolOp::Unite),
                ("Subtract", BoolOp::Subtract),
                ("Intersect", BoolOp::Intersect),
                ("Exclude", BoolOp::Exclude),
            ] {
                if ui.add_enabled(can_bool, egui::Button::new(label)).clicked() {
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
            let can_weld = open_paths.len() == 2;
            if ui
                .add_enabled(can_weld, egui::Button::new("Weld"))
                .clicked()
            {
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

        ui.menu_button("View", |ui| {
            ui.checkbox(&mut state.annotations_visible, "👁 Show Annotations")
                .on_hover_text("Toggle all annotation visibility (Ctrl+H)");
            ui.separator();
            ui.checkbox(&mut state.settings.magnifier, "⊕ Magnifier")
                .on_hover_text("Toggle magnifier lens (M)");
            ui.checkbox(&mut state.settings.show_recents, "☰ Recent Captures");
            ui.checkbox(&mut state.settings.show_layers, "☰ Layers Panel")
                .on_hover_text("Show/hide the annotations layers panel");
            ui.checkbox(&mut state.settings.show_pathfinder, "▦ Pathfinder Strip")
                .on_hover_text("Show a pinned strip of path operations");
            ui.checkbox(&mut state.settings.show_transform, "⊹ Transform Panel")
                .on_hover_text("Show a numeric X/Y/W/H/angle/skew panel for the selection");
            if ui.button("⌨ Keyboard Shortcuts…").clicked() {
                ui.close_menu();
                state.show_shortcuts_window = true;
            }
            ui.separator();
            ui.menu_button("Theme", |ui| {
                for id in crate::theme::ThemeId::all() {
                    let sel = state.settings.theme_id == *id;
                    if ui.selectable_label(sel, id.label()).clicked() {
                        state.settings.theme_id = *id;
                        crate::config::save_settings(&state.settings);
                        ui.close_menu();
                    }
                }
            });
            ui.separator();
            if ui.button("🔍 Zoom In").clicked() {
                ui.close_menu();
                state.view_zoom = (state.view_zoom * 1.25).min(64.0);
            }
            if ui.button("🔎 Zoom Out").clicked() {
                ui.close_menu();
                state.view_zoom = (state.view_zoom / 1.25).max(0.01);
            }
            if ui
                .button("↺ Reset Zoom")
                .on_hover_text("Shortcut: 0")
                .clicked()
            {
                ui.close_menu();
                state.view_zoom = 1.0;
                state.view_pan = egui::Vec2::ZERO;
            }
            if state.canvas.pdf_pages > 1 {
                ui.separator();
                ui.label(format!(
                    "Page {}/{}",
                    state.canvas.pdf_page + 1,
                    state.canvas.pdf_pages
                ));
                if ui.button("◀ Prev").clicked() && state.canvas.pdf_page > 0 {
                    ui.close_menu();
                    state.canvas.pdf_page -= 1;
                }
                if ui.button("▶ Next").clicked()
                    && state.canvas.pdf_page + 1 < state.canvas.pdf_pages
                {
                    ui.close_menu();
                    state.canvas.pdf_page += 1;
                }
            }
        });

        ui.menu_button("Help", |ui| {
            if ui.button("⌨ Keyboard Shortcuts…").clicked() {
                ui.close_menu();
                state.show_shortcuts_window = true;
            }
        });
    });
}

/// Collapsed "AI / Agents" entry point inside the Edit menu.
///
/// Everything AI/agent-related lives behind this single sub-menu so that the
/// toolbar shows only a compact entry until the user opts in. The agent server
/// and local-AI panel both stay off/closed by default; the controls here are the
/// only way to enable them. Once the user opens this section it stays expanded
/// across sessions (persisted via `ai_section_expanded`).
#[cfg(any(feature = "agent-server", feature = "local-ai"))]
fn draw_ai_agents_menu(ui: &mut Ui, state: &mut AppState) {
    ui.separator();
    let resp = ui.menu_button("🤖 AI / Agents", |ui| {
        // Remember that the user has revealed this section.
        if !state.settings.ai_section_expanded {
            state.settings.ai_section_expanded = true;
            crate::config::save_settings(&state.settings);
        }

        #[cfg(feature = "agent-server")]
        {
            let was_on = state.settings.agent_server_enabled;
            ui.checkbox(
                &mut state.settings.agent_server_enabled,
                "Agent server (MCP)",
            )
            .on_hover_text(
                "Allow external AI agents to draw on this canvas via http://127.0.0.1:9275/mcp",
            );
            if state.settings.agent_server_enabled != was_on {
                crate::config::save_settings(&state.settings);
            }
            let status = if state.agent_server.is_some() {
                "● Running on 127.0.0.1:9275"
            } else {
                "○ Stopped"
            };
            ui.label(RichText::new(status).weak().small());
        }

        #[cfg(feature = "local-ai")]
        {
            #[cfg(feature = "agent-server")]
            ui.separator();
            ui.checkbox(&mut state.local_ai_panel_open, "Local AI panel")
                .on_hover_text("Show the right-side control panel for the local AI agent (Ollama)");
        }
    });
    resp.response
        .on_hover_text("AI agents are off by default — enable them here");
}

// ── Action toolbar (capture + zoom + status) ──────────────────────────────────

// ── Shared toolbar widgets ────────────────────────────────────────────────────

/// A hairline group divider with breathing room on both sides. Toolbars read as
/// clusters of related controls rather than one undifferentiated run of buttons.
fn bar_divider(ui: &mut Ui, theme: &Theme) {
    ui.add_space(5.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 22.0), egui::Sense::hover());
    ui.painter().rect_filled(
        egui::Rect::from_center_size(rect.center(), egui::vec2(1.0, 20.0)),
        0.0,
        theme.divider,
    );
    ui.add_space(5.0);
}

/// Small uppercase-ish caption naming a toolbar cluster.
fn bar_caption(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(RichText::new(text).size(10.5).color(theme.text_label));
}

/// Accent-filled call-to-action. Exactly one per bar — the thing the user came
/// here to do (capture).
pub(crate) fn primary_button(ui: &mut Ui, theme: &Theme, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(theme.on_accent).strong())
            .fill(theme.accent)
            .stroke(Stroke::new(1.0, theme.border_active))
            .rounding(egui::Rounding::same(7.0))
            .min_size(egui::vec2(0.0, 27.0)),
    )
}

/// Outlined companion to `primary_button` — same weight, no accent fill.
pub(crate) fn secondary_button(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).color(theme.text_muted))
            .fill(theme.surface_bg)
            .stroke(Stroke::new(1.0, theme.border_idle))
            .rounding(egui::Rounding::same(7.0))
            .min_size(egui::vec2(0.0, 27.0)),
    )
}

/// Segmented-control style toggle: transparent when off, accent-tinted when on.
fn chip(ui: &mut Ui, theme: &Theme, label: &str, selected: bool) -> egui::Response {
    let (fill, fg, stroke) = if selected {
        (
            theme.accent_subtle,
            theme.accent,
            Stroke::new(1.0, theme.accent_dim),
        )
    } else {
        (
            Color32::TRANSPARENT,
            theme.text_muted,
            Stroke::new(1.0, theme.border_idle),
        )
    };
    ui.add(
        egui::Button::new(RichText::new(label).color(fg).size(12.0))
            .fill(fill)
            .stroke(stroke)
            .rounding(egui::Rounding::same(6.0))
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

/// Human-readable name of the active tool, for the status bar.
pub fn tool_label(tool: &Tool) -> String {
    match tool {
        Tool::Select => "Select".into(),
        Tool::Pen => "Pen".into(),
        Tool::Rect => "Rectangle".into(),
        Tool::Oval => "Oval".into(),
        Tool::Star => "Star".into(),
        Tool::Stamp(id) => crate::stamps::stamp_by_id(id)
            .map(|s| s.label.to_string())
            .unwrap_or_else(|| "Stamp".into()),
        Tool::Arrow => "Arrow".into(),
        Tool::Line => "Line".into(),
        Tool::Text => "Text".into(),
        Tool::Blur => "Blur".into(),
        Tool::Crop => "Crop".into(),
        Tool::Eyedropper => "Eyedropper".into(),
        Tool::Step => "Step marker".into(),
        Tool::FocusRegion => "Focus region".into(),
        Tool::LogicTag(kind) => kind.label().to_string(),
        Tool::Marquee => "Marquee".into(),
        Tool::Fill => "Flood fill".into(),
        Tool::Lasso => "Lasso".into(),
        Tool::Callout => "Callout".into(),
        Tool::Ruler => "Ruler".into(),
    }
}

// ── Row 1: capture + output ───────────────────────────────────────────────────

/// The screenshot bar. Capture is the app's primary job, so it leads: one
/// accent CTA, a full-screen alternate, the timer, then what you do with the
/// result (copy / save).
pub fn show_capture_bar(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    ui.horizontal_wrapped(|ui| {
        if primary_button(ui, &theme, "⛶  Capture Region")
            .on_hover_text("Drag a region of the screen (Ctrl+Shift+S)")
            .clicked()
        {
            trigger_capture_pub(state, SnapMode::Region);
        }
        if secondary_button(ui, &theme, "🖵  Full Screen", true)
            .on_hover_text("Capture the whole screen")
            .clicked()
        {
            trigger_capture_pub(state, SnapMode::Full);
        }

        bar_divider(ui, &theme);

        bar_caption(ui, &theme, "Timer");
        for (n, label) in [(0u32, "off"), (3, "3s"), (5, "5s"), (10, "10s")] {
            let sel = state.settings.capture_delay_s == n;
            if chip(ui, &theme, label, sel)
                .on_hover_text("Delay before the capture fires")
                .clicked()
            {
                state.settings.capture_delay_s = n;
            }
        }
        ui.add(
            egui::DragValue::new(&mut state.settings.capture_delay_s)
                .range(0..=60)
                .suffix("s")
                .speed(1.0),
        )
        .on_hover_text("Custom delay in seconds (0 = capture immediately)");
        bar_caption(ui, &theme, "custom");

        ui.add_space(6.0);
        ui.checkbox(&mut state.settings.auto_copy_on_capture, "Auto-copy")
            .on_hover_text("Copy every finished capture straight to the clipboard");
        ui.checkbox(&mut state.settings.region_selector_inapp, "Precise region")
            .on_hover_text(
                "Frame the region in RedLine's own overlay — live dimensions, pixel \
             magnifier, arrow-key nudging.\nOff: use the desktop's own selector.",
            );
        ui.checkbox(&mut state.settings.hide_redline_on_capture, "Hide RedLine")
            .on_hover_text(
                "Hide the RedLine window before capture.\nDisable to include the app in the shot.",
            );

        bar_divider(ui, &theme);

        let has = state.canvas.texture.is_some();
        if secondary_button(ui, &theme, "Copy", has)
            .on_hover_text("Copy the flattened image to the clipboard (Ctrl+C)")
            .on_disabled_hover_text("Capture or open an image first")
            .clicked()
        {
            copy_image_with_flatten(state);
        }
        if secondary_button(ui, &theme, "Copy & Close", has)
            .on_hover_text(
                "Copy to the clipboard, then close this tab — the throwaway-screenshot flow",
            )
            .on_disabled_hover_text("Capture or open an image first")
            .clicked()
        {
            copy_image_with_flatten(state);
            let copied = state
                .status_msg
                .as_deref()
                .map(|m| !m.to_ascii_lowercase().contains("fail"))
                .unwrap_or(false);
            if copied {
                state.close_tab(state.active_tab_pos);
                state.status_msg = Some("Copied to clipboard — tab closed".into());
            }
        }
        if secondary_button(ui, &theme, "Save", has)
            .on_hover_text("Save over the current file (Ctrl+S)")
            .on_disabled_hover_text("Capture or open an image first")
            .clicked()
        {
            save_image_with_flatten_pub(state);
        }
        if secondary_button(ui, &theme, "Save As…", has)
            .on_hover_text("Save to a new file")
            .on_disabled_hover_text("Capture or open an image first")
            .clicked()
        {
            save_image_as(state);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if state.canvas.texture.is_some() {
                let sz = state.canvas.canvas_size;
                ui.label(
                    RichText::new(format!("{} × {} px", sz.x as i32, sz.y as i32))
                        .small()
                        .color(theme.text_dim),
                );
            }
        });
    });
}

// ── Row 2: view + appearance ──────────────────────────────────────────────────

/// Everything that changes how the canvas *looks* rather than what it contains:
/// the drawing color, overlays, zoom, and the side-panel toggles.
pub fn show_view_bar(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    ui.horizontal_wrapped(|ui| {
        show_color_controls(ui, state);

        bar_divider(ui, &theme);

        let vis = state.annotations_visible;
        if chip(
            ui,
            &theme,
            if vis {
                "👁 Annotations"
            } else {
                "👁 Hidden"
            },
            !vis,
        )
        .on_hover_text("Toggle all annotations (Ctrl+H)")
        .clicked()
        {
            state.annotations_visible = !vis;
            state.status_msg = Some(if !vis {
                "Annotations shown".into()
            } else {
                "Annotations hidden".into()
            });
        }

        if chip(ui, &theme, "⊞ Grid", state.settings.show_grid)
            .on_hover_text("Grid overlay (display only — never exported)")
            .clicked()
        {
            state.settings.show_grid = !state.settings.show_grid;
        }
        if state.settings.show_grid {
            egui::ComboBox::from_id_salt("grid_size")
                .selected_text(format!("{:.0} px", state.settings.grid_size))
                .width(72.0)
                .show_ui(ui, |ui| {
                    for &size in &[10.0_f32, 25.0, 50.0, 100.0, 200.0] {
                        ui.selectable_value(
                            &mut state.settings.grid_size,
                            size,
                            format!("{size:.0} px"),
                        );
                    }
                });
        }
        if chip(ui, &theme, "🔍 Magnifier", state.settings.magnifier)
            .on_hover_text("Pixel magnifier under the cursor")
            .clicked()
        {
            state.settings.magnifier = !state.settings.magnifier;
        }

        bar_divider(ui, &theme);

        bar_caption(ui, &theme, "Zoom");
        ui.add(
            egui::Slider::new(&mut state.view_zoom, 0.01_f32..=64.0)
                .logarithmic(true)
                .show_value(false)
                .clamping(egui::SliderClamping::Always),
        )
        .on_hover_text("Zoom (Ctrl+scroll)");
        {
            let mut pct = (state.view_zoom * 100.0).round() as u32;
            let resp = ui
                .add(
                    egui::DragValue::new(&mut pct)
                        .range(1..=6400)
                        .suffix("%")
                        .speed(1.0),
                )
                .on_hover_text("Zoom % — drag or type");
            if resp.changed() {
                state.view_zoom = (pct as f32 / 100.0).clamp(0.01, 64.0);
            }
        }
        if chip(ui, &theme, "Fit", false)
            .on_hover_text("Reset zoom and pan (0)")
            .clicked()
        {
            state.view_zoom = 1.0;
            state.view_pan = egui::Vec2::ZERO;
        }

        bar_divider(ui, &theme);

        bar_caption(ui, &theme, "Panels");
        if chip(ui, &theme, "Layers", state.settings.show_layers)
            .on_hover_text("Layers panel")
            .clicked()
        {
            state.settings.show_layers = !state.settings.show_layers;
        }
        if chip(ui, &theme, "Transform", state.settings.show_transform)
            .on_hover_text("Numeric transform panel")
            .clicked()
        {
            state.settings.show_transform = !state.settings.show_transform;
        }
        if chip(ui, &theme, "Pathfinder", state.settings.show_pathfinder)
            .on_hover_text("Boolean path operations strip")
            .clicked()
        {
            state.settings.show_pathfinder = !state.settings.show_pathfinder;
        }
        if chip(ui, &theme, "Recents", state.settings.show_recents)
            .on_hover_text("Recent captures panel")
            .clicked()
        {
            state.settings.show_recents = !state.settings.show_recents;
        }

        let has_img = state.canvas.color_image.is_some();
        if chip(ui, &theme, "SVG…", state.canvas.vectorize_panel.open)
            .on_hover_text("Convert the image to vector paths")
            .clicked()
            && has_img
        {
            state.canvas.vectorize_panel.open = !state.canvas.vectorize_panel.open;
        }
    });
}

// ── Toast ─────────────────────────────────────────────────────────────────────

/// Floating status toast, bottom-centre over the canvas. Fades out on its own;
/// click to dismiss immediately. Returns true while one is on screen so the
/// caller keeps the repaint loop alive for the animation.
pub fn show_toast(ctx: &egui::Context, state: &mut AppState) -> bool {
    let alive = crate::state::Toast::sync(&mut state.status_msg, &mut state.toast);
    let Some(toast) = state.toast.clone() else {
        return false;
    };
    let opacity = toast.opacity();
    if opacity <= 0.0 {
        return alive;
    }

    let theme = state.settings.theme();
    let a = |c: Color32, mul: f32| {
        let f = (mul * opacity).clamp(0.0, 1.0);
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (f * 255.0) as u8)
    };
    let accent = if toast.is_error {
        theme.danger
    } else {
        theme.accent
    };

    let mut dismiss = false;
    egui::Area::new(egui::Id::new("redline_toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -54.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(a(theme.surface_bg, 0.97))
                .stroke(Stroke::new(1.0, a(accent, 0.85)))
                .rounding(egui::Rounding::same(9.0))
                .inner_margin(egui::Margin::symmetric(14.0, 9.0))
                .shadow(egui::epaint::Shadow {
                    offset: egui::vec2(0.0, 5.0),
                    blur: 16.0,
                    spread: 0.0,
                    color: Color32::from_black_alpha((90.0 * opacity) as u8),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Severity pip — colour carries the meaning, no glyph needed.
                        let (pip, _) =
                            ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(pip.center(), 3.5, a(accent, 1.0));
                        ui.label(
                            RichText::new(&toast.text)
                                .color(a(theme.text_heading, 1.0))
                                .size(13.0),
                        );
                    });
                });
            if ui.response().clicked() {
                dismiss = true;
            }
        });

    if dismiss {
        state.status_msg = None;
        state.toast = None;
        return false;
    }
    true
}

// ── Status bar ────────────────────────────────────────────────────────────────

/// Bottom strip: what tool is live, what the app just did, and where the view
/// is. Status messages used to hide in the corner of the top bar — here they
/// have room and a dismiss affordance.
pub fn show_status_bar(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    ui.horizontal(|ui| {
        bar_caption(ui, &theme, "Tool");
        ui.label(
            RichText::new(tool_label(&state.current_tool))
                .small()
                .strong()
                .color(theme.accent),
        );
        let sel = match &state.tool_state {
            ToolState::Selected { .. } => Some("1 selected".to_string()),
            ToolState::MultiSelected { ids, .. } => Some(format!("{} selected", ids.len())),
            _ => None,
        };
        if let Some(sel) = sel {
            ui.label(RichText::new("•").small().color(theme.text_dim));
            ui.label(RichText::new(sel).small().color(theme.text_muted));
        }
        let ann_count = state.canvas.annotations.len();
        if ann_count > 0 {
            ui.label(RichText::new("•").small().color(theme.text_dim));
            ui.label(
                RichText::new(format!("{ann_count} annotations"))
                    .small()
                    .color(theme.text_dim),
            );
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(env!("REDLINE_VERSION"))
                    .small()
                    .color(theme.text_dim),
            );
            ui.label(RichText::new("•").small().color(theme.text_dim));
            ui.label(
                RichText::new(format!("{:.0}%", state.view_zoom * 100.0))
                    .small()
                    .color(theme.text_dim),
            );
        });
    });
}

// ── Pathfinder strip — toggleable bottom row of path operations ───────────────

/// A pinned strip of path operations (`View ▸ Pathfinder`). Buttons are greyed
/// when the selection doesn't qualify; all actions route through `CommandExecutor`
/// so they are undoable.
pub fn show_pathfinder_strip(ui: &mut Ui, state: &mut AppState) {
    let (closed_paths, open_paths) = pathfinder_selection(state);
    ui.horizontal(|ui| {
        let can_bool = closed_paths.len() >= 2;
        for (label, op) in [
            ("Unite", BoolOp::Unite),
            ("Subtract", BoolOp::Subtract),
            ("Intersect", BoolOp::Intersect),
            ("Exclude", BoolOp::Exclude),
        ] {
            if ui
                .add_enabled(can_bool, egui::Button::new(label))
                .on_disabled_hover_text("Select 2+ closed paths")
                .clicked()
            {
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
        if ui
            .add_enabled(open_paths.len() == 2, egui::Button::new("Weld"))
            .on_disabled_hover_text("Select exactly 2 open paths")
            .clicked()
        {
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

// ── Left toolbox panel ────────────────────────────────────────────────────────

pub fn show_toolbox(ui: &mut Ui, state: &mut AppState) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        let panel_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(panel_width, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_min_width(panel_width);
                ui.set_max_width(panel_width);
                show_normal_toolbox(ui, state);
            },
        );
    });
}

const ICON_CROP: &[u8] = include_bytes!("../icons/crop.svg");
const ICON_EYEDROP: &[u8] = include_bytes!("../icons/eyedropper.svg");
const ICON_STEP: &[u8] = include_bytes!("../icons/step-list.svg");
const ICON_MARQUEE: &[u8] = include_bytes!("../icons/marquee.svg");
const TOOL_BUTTON_SIDE: f32 = 38.0;
const MAX_TOOL_BUTTON_COLUMNS: usize = 4;
const TOOL_BUTTON_SPACING_X: f32 = 6.0;
const TOOL_BUTTON_SPACING_Y: f32 = 6.0;

enum ToolGlyph<'a> {
    Text(&'a str),
    Svg(&'static str, &'static [u8]),
    /// Painted directly — several of the shapes we want (▭, ▨, ＋, ✕, ✗) have
    /// no glyph in any bundled font and rendered as empty tofu boxes.
    Draw(GlyphKind),
    #[allow(dead_code)]
    Unused(std::marker::PhantomData<&'a ()>),
}

#[derive(Clone, Copy, PartialEq)]
enum GlyphKind {
    Rect,
    Oval,
    Line,
    Plus,
    Cross,
    CrossCircle,
    FloodFill,
    Pencil,
}

/// Paint a vector glyph centred in `rect`. Strokes scale with the button so the
/// icons stay crisp at any toolbox width.
fn draw_glyph(painter: &egui::Painter, rect: egui::Rect, kind: GlyphKind, color: Color32) {
    let r = rect.shrink(rect.width() * 0.22);
    let w = (rect.width() * 0.075).max(1.4);
    let stroke = Stroke::new(w, color);
    match kind {
        GlyphKind::Rect => {
            painter.rect_stroke(r, egui::Rounding::same(1.5), stroke);
        }
        GlyphKind::Oval => {
            painter.circle_stroke(r.center(), r.width() * 0.5, stroke);
        }
        GlyphKind::Line => {
            painter.line_segment([r.left_bottom(), r.right_top()], stroke);
        }
        GlyphKind::Plus => {
            painter.line_segment(
                [
                    egui::pos2(r.left(), r.center().y),
                    egui::pos2(r.right(), r.center().y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(r.center().x, r.top()),
                    egui::pos2(r.center().x, r.bottom()),
                ],
                stroke,
            );
        }
        GlyphKind::Cross => {
            painter.line_segment([r.left_top(), r.right_bottom()], stroke);
            painter.line_segment([r.left_bottom(), r.right_top()], stroke);
        }
        GlyphKind::CrossCircle => {
            painter.circle_stroke(r.center(), r.width() * 0.5, stroke);
            let inner = r.shrink(r.width() * 0.28);
            painter.line_segment([inner.left_top(), inner.right_bottom()], stroke);
            painter.line_segment([inner.left_bottom(), inner.right_top()], stroke);
        }
        GlyphKind::FloodFill => {
            // Outlined square, bottom half flooded.
            painter.rect_stroke(r, egui::Rounding::same(1.5), stroke);
            let fill = egui::Rect::from_min_max(egui::pos2(r.left(), r.center().y), r.max);
            painter.rect_filled(fill.shrink(w * 0.5), egui::Rounding::same(1.0), color);
        }
        GlyphKind::Pencil => {
            // Shaft plus a nib triangle at the lower-left.
            painter.line_segment(
                [
                    r.left_bottom() + egui::vec2(r.width() * 0.28, -r.height() * 0.28),
                    r.right_top(),
                ],
                stroke,
            );
            let tip = r.left_bottom();
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    tip + egui::vec2(r.width() * 0.30, 0.0),
                    tip + egui::vec2(0.0, -r.height() * 0.30),
                ],
                color,
                Stroke::NONE,
            ));
        }
    }
}

struct ToolButtonSpec<'a> {
    tool: Tool,
    glyph: ToolGlyph<'a>,
    tip: &'a str,
}

fn tool_button_spacing() -> egui::Vec2 {
    egui::vec2(TOOL_BUTTON_SPACING_X, TOOL_BUTTON_SPACING_Y)
}

fn tool_button_columns(ui: &Ui) -> usize {
    let avail = ui.available_width().max(TOOL_BUTTON_SIDE);
    let step = TOOL_BUTTON_SIDE + TOOL_BUTTON_SPACING_X;
    (((avail + TOOL_BUTTON_SPACING_X) / step).floor() as usize).clamp(1, MAX_TOOL_BUTTON_COLUMNS)
}

fn tool_button_row_width(columns: usize) -> f32 {
    columns as f32 * TOOL_BUTTON_SIDE + (columns.saturating_sub(1)) as f32 * TOOL_BUTTON_SPACING_X
}

fn tool_button_frame(ui: &mut Ui, active: bool, theme: &Theme) -> (egui::Rect, egui::Response) {
    let size = egui::vec2(TOOL_BUTTON_SIDE, TOOL_BUTTON_SIDE);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    let fill = if active {
        theme.tool_active_fill
    } else if hovered {
        theme.tool_hover_fill
    } else {
        theme.surface_bg
    };
    let stroke = if active {
        Stroke::new(1.5, theme.border_active)
    } else if hovered {
        Stroke::new(1.0, theme.accent_dim)
    } else {
        Stroke::new(1.0, theme.border_idle)
    };
    ui.painter().rect_filled(rect, 7.0, fill);
    ui.painter().rect_stroke(rect, 7.0, stroke);
    (rect, resp)
}

fn toolbox_group(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    default_open: bool,
    add_contents: impl FnOnce(&mut Ui),
) {
    egui::CollapsingHeader::new(label)
        .default_open(default_open)
        .show(ui, |ui| add_contents(ui));
    ui.add_space(4.0);
}

fn tool_grid(ui: &mut Ui, columns: usize, add_contents: impl FnOnce(&mut Ui)) {
    let row_w = tool_button_row_width(columns).min(ui.available_width());
    ui.allocate_ui_with_layout(egui::vec2(row_w, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing = tool_button_spacing();
        ui.set_min_width(row_w);
        ui.set_max_width(row_w);
        add_contents(ui);
    });
}

fn render_tool_button(ui: &mut Ui, state: &mut AppState, theme: &Theme, spec: &ToolButtonSpec<'_>) {
    let active = state.current_tool == spec.tool;
    let clicked = match spec.glyph {
        ToolGlyph::Text(icon) => text_tool_btn(ui, active, theme, icon, spec.tip).clicked(),
        ToolGlyph::Svg(uri, bytes) => {
            svg_tool_btn(ui, active, theme, uri, bytes, spec.tip).clicked()
        }
        ToolGlyph::Draw(kind) => draw_tool_btn(ui, active, theme, kind, spec.tip).clicked(),
        ToolGlyph::Unused(_) => false,
    };
    if clicked {
        if active && spec.tool != Tool::Select {
            state.set_current_tool(Tool::Select);
        } else {
            state.set_current_tool(spec.tool.clone());
        }
    }
}

fn render_tool_grid(
    ui: &mut Ui,
    state: &mut AppState,
    theme: &Theme,
    buttons: &[ToolButtonSpec<'_>],
) {
    let columns = tool_button_columns(ui).min(buttons.len().max(1));
    tool_grid(ui, columns, |ui| {
        for row in buttons.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = tool_button_spacing();
                for spec in row {
                    render_tool_button(ui, state, theme, spec);
                }
            });
        }
    });
}

fn show_tool_category(
    ui: &mut Ui,
    state: &mut AppState,
    theme: &Theme,
    title: &str,
    default_open: bool,
    buttons: &[ToolButtonSpec<'_>],
) {
    toolbox_group(
        ui,
        RichText::new(title).strong().color(theme.text_heading),
        default_open,
        |ui| render_tool_grid(ui, state, theme, buttons),
    );
}

/// Icon tint for a tool button — the active fill is a solid accent, so the
/// glyph has to flip to the on-accent color to stay legible.
fn tool_icon_color(theme: &Theme, active: bool) -> Color32 {
    if active {
        theme.tool_icon_active
    } else {
        theme.tool_icon
    }
}

fn draw_tool_btn(
    ui: &mut Ui,
    active: bool,
    theme: &Theme,
    kind: GlyphKind,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = tool_button_frame(ui, active, theme);
    draw_glyph(ui.painter(), rect, kind, tool_icon_color(theme, active));
    resp.on_hover_text(tip)
}

fn text_tool_btn(
    ui: &mut Ui,
    active: bool,
    theme: &Theme,
    icon: &str,
    tip: &str,
) -> egui::Response {
    let (rect, resp) = tool_button_frame(ui, active, theme);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(19.0),
        tool_icon_color(theme, active),
    );
    resp.on_hover_text(tip)
}

fn svg_tool_btn(
    ui: &mut Ui,
    active: bool,
    theme: &Theme,
    static_uri: &'static str,
    bytes: &'static [u8],
    tip: &str,
) -> egui::Response {
    let uri = static_uri.to_string();
    let (rect, resp) = tool_button_frame(ui, active, theme);
    let icon_rect = rect.shrink(8.0);
    let bytes = crate::stamps::white_svg(static_uri, bytes);
    ui.put(
        icon_rect,
        egui::Image::from_bytes(uri, bytes)
            .tint(tool_icon_color(theme, active))
            .fit_to_exact_size(icon_rect.size()),
    );
    resp.on_hover_text(tip)
}

fn side_panel_heading(ui: &mut Ui, label: &str, theme: &Theme) {
    ui.label(
        RichText::new(label)
            .heading()
            .strong()
            .color(theme.text_heading),
    );
}

fn show_normal_toolbox(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();

    // ── Selected annotation controls ──────────────────────────────────────────
    if let ToolState::Selected { id, .. } = state.tool_state {
        let ann_kind = state
            .canvas
            .annotations
            .iter()
            .find(|a| a.id == id)
            .map(|a| annotation_kind_label(&a.shape));
        toolbox_group(
            ui,
            RichText::new("Selected").strong().color(theme.text_heading),
            true,
            |ui| {
                if let Some(kind) = ann_kind {
                    ui.label(RichText::new(kind).color(theme.text_muted).small());
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("⬆ Front")
                        .on_hover_text("Bring to front")
                        .clicked()
                    {
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::BringToFront(id),
                        );
                    }
                    if ui.button("⬇ Back").on_hover_text("Send to back").clicked() {
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::SendToBack(id),
                        );
                    }
                });
                if ui
                    .button("⧉ Duplicate")
                    .on_hover_text("Duplicate annotation (Ctrl+D)")
                    .clicked()
                {
                    duplicate_selected(state, id);
                }
                if let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == id) {
                    let is_locked = ann.locked;
                    let lock_label = if is_locked {
                        "🔓 Unlock"
                    } else {
                        "🔒 Lock"
                    };
                    if ui
                        .button(lock_label)
                        .on_hover_text(if is_locked {
                            "Unlock this annotation (allow move/delete)"
                        } else {
                            "Lock this annotation (prevent move/delete)"
                        })
                        .clicked()
                    {
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::SetProp {
                                id,
                                prop: PropChange::Locked(!is_locked),
                            },
                        );
                    }
                }
                let current_label = state
                    .canvas
                    .annotations
                    .iter()
                    .find(|a| a.id == id)
                    .and_then(|a| a.label.clone())
                    .unwrap_or_default();
                let mut note = current_label;
                ui.label(RichText::new("Note:").small().color(theme.text_label));
                let resp = ui.text_edit_singleline(&mut note);
                if resp.changed() {
                    let new_label = if note.is_empty() { None } else { Some(note) };
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::SetProp {
                            id,
                            prop: PropChange::Label(new_label),
                        },
                    );
                }
                // Font size control for Text / Question annotations.
                let text_size = state
                    .canvas
                    .annotations
                    .iter()
                    .find(|a| a.id == id)
                    .and_then(|a| match &a.shape {
                        crate::types::ShapeData::Text { size, .. }
                        | crate::types::ShapeData::Question { size, .. } => Some(*size),
                        _ => None,
                    });
                if let Some(mut sz) = text_size {
                    ui.label(RichText::new("Size:").small().color(theme.text_label));
                    if ui
                        .add(
                            egui::Slider::new(&mut sz, 8.0..=120.0)
                                .suffix("pt")
                                .clamping(egui::SliderClamping::Always),
                        )
                        .changed()
                    {
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::SetProp {
                                id,
                                prop: PropChange::TextSize(sz),
                            },
                        );
                    }
                }

                let is_step = state.canvas.annotations.iter().any(|a| {
                    a.id == id && matches!(a.shape, crate::types::ShapeData::StepMarker { .. })
                });
                if is_step {
                    let step_count = state
                        .canvas
                        .annotations
                        .iter()
                        .filter(|a| matches!(a.shape, crate::types::ShapeData::StepMarker { .. }))
                        .count();
                    ui.label(
                        RichText::new(format!("{step_count} step markers"))
                            .small()
                            .color(theme.text_label),
                    );
                    if ui.button("⟳ Renumber Steps").clicked() {
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::RenumberSteps,
                        );
                    }
                }
            },
        );
    }

    // ── Multi-selection controls ──────────────────────────────────────────────
    if let ToolState::MultiSelected { ref ids, .. } = state.tool_state {
        let count = ids.len();
        let ids_clone: Vec<uuid::Uuid> = ids.clone();
        toolbox_group(
            ui,
            RichText::new("Multi-selected")
                .strong()
                .color(theme.text_heading),
            true,
            |ui| {
                ui.label(
                    RichText::new(format!("{count} annotations selected"))
                        .color(theme.text_muted)
                        .small(),
                );

                let mut col = state.settings.stroke_color;
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut col,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    state.settings.stroke_color = col;
                    let cmds: Vec<Command> = ids_clone
                        .iter()
                        .map(|&id| Command::SetProp {
                            id,
                            prop: PropChange::Color(col),
                        })
                        .collect();
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::Batch(cmds),
                    );
                }

                let mut thickness = state.settings.stroke_width;
                if ui
                    .add(
                        egui::Slider::new(&mut thickness, 1.0..=100.0)
                            .text("px")
                            .clamping(egui::SliderClamping::Always),
                    )
                    .changed()
                {
                    state.settings.stroke_width = thickness;
                    let cmds: Vec<Command> = ids_clone
                        .iter()
                        .map(|&id| Command::SetProp {
                            id,
                            prop: PropChange::Thickness(thickness),
                        })
                        .collect();
                    CommandExecutor::execute(
                        &mut state.canvas,
                        ParticipantId::HUMAN_LOCAL,
                        Command::Batch(cmds),
                    );
                }

                // ── Alignment ──────────────────────────────────────────────
                ui.separator();
                ui.label(RichText::new("Align:").small().color(theme.text_label));
                ui.horizontal_wrapped(|ui| {
                    let rects: Vec<(uuid::Uuid, egui::Rect)> = ids_clone
                        .iter()
                        .filter_map(|&aid| {
                            state
                                .canvas
                                .annotations
                                .iter()
                                .find(|a| a.id == aid)
                                .map(|a| (aid, a.shape.bounding_rect()))
                        })
                        .collect();

                    let align_btn = |ui: &mut egui::Ui, label: &str, tip: &str| {
                        ui.add(
                            egui::Button::new(RichText::new(label).size(11.0))
                                .min_size(egui::vec2(28.0, 22.0)),
                        )
                        .on_hover_text(tip)
                        .clicked()
                    };

                    if align_btn(ui, "⬅", "Align left edges") {
                        if let Some(target) = rects.iter().map(|(_, r)| r.min.x).reduce(f32::min) {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(target - r.min.x, 0.0),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                    if align_btn(ui, "↔", "Align centers horizontally") {
                        if let Some(target) = rects
                            .iter()
                            .map(|(_, r)| r.center().x)
                            .reduce(|a, b| a + b)
                            .map(|s| s / rects.len() as f32)
                        {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(target - r.center().x, 0.0),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                    if align_btn(ui, "➡", "Align right edges") {
                        if let Some(target) = rects.iter().map(|(_, r)| r.max.x).reduce(f32::max) {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(target - r.max.x, 0.0),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                    if align_btn(ui, "⬆", "Align top edges") {
                        if let Some(target) = rects.iter().map(|(_, r)| r.min.y).reduce(f32::min) {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(0.0, target - r.min.y),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                    if align_btn(ui, "↕", "Align centers vertically") {
                        if let Some(target) = rects
                            .iter()
                            .map(|(_, r)| r.center().y)
                            .reduce(|a, b| a + b)
                            .map(|s| s / rects.len() as f32)
                        {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(0.0, target - r.center().y),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                    if align_btn(ui, "⬇", "Align bottom edges") {
                        if let Some(target) = rects.iter().map(|(_, r)| r.max.y).reduce(f32::max) {
                            let cmds: Vec<Command> = rects
                                .iter()
                                .map(|&(aid, r)| Command::Move {
                                    id: aid,
                                    delta: egui::vec2(0.0, target - r.max.y),
                                })
                                .collect();
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::Batch(cmds),
                            );
                        }
                    }
                });

                if ui.button("🗑 Delete All").clicked() {
                    let cmds: Vec<Command> =
                        ids_clone.iter().map(|&id| Command::Delete(id)).collect();
                    if !cmds.is_empty() {
                        state.tool_state = ToolState::Idle;
                        CommandExecutor::execute(
                            &mut state.canvas,
                            ParticipantId::HUMAN_LOCAL,
                            Command::Batch(cmds),
                        );
                    }
                }
            },
        );
    }

    let selection_buttons = [
        ToolButtonSpec {
            tool: Tool::Select,
            glyph: ToolGlyph::Text("↖"),
            tip: "Select (V)",
        },
        ToolButtonSpec {
            tool: Tool::Marquee,
            glyph: ToolGlyph::Svg("bytes://marquee.svg", ICON_MARQUEE),
            tip: "Marquee selection (M)",
        },
        ToolButtonSpec {
            tool: Tool::Lasso,
            glyph: ToolGlyph::Text("🔄"),
            tip: "Lasso selection — draw a freehand boundary to select annotations inside",
        },
    ];
    show_tool_category(ui, state, &theme, "Selection", true, &selection_buttons);

    let draw_buttons = [
        ToolButtonSpec {
            tool: Tool::Pen,
            glyph: ToolGlyph::Text("✏"),
            tip: "Pen (P)",
        },
        ToolButtonSpec {
            tool: Tool::Text,
            glyph: ToolGlyph::Text("T"),
            tip: "Text (T)",
        },
        ToolButtonSpec {
            tool: Tool::Rect,
            glyph: ToolGlyph::Draw(GlyphKind::Rect),
            tip: "Rectangle (R)",
        },
        ToolButtonSpec {
            tool: Tool::Oval,
            glyph: ToolGlyph::Draw(GlyphKind::Oval),
            tip: "Oval (O)",
        },
        ToolButtonSpec {
            tool: Tool::Star,
            glyph: ToolGlyph::Text("★"),
            tip: "Star (S)",
        },
        ToolButtonSpec {
            tool: Tool::Arrow,
            glyph: ToolGlyph::Text("▶"),
            tip: "Arrow (A)",
        },
        ToolButtonSpec {
            tool: Tool::Line,
            glyph: ToolGlyph::Draw(GlyphKind::Line),
            tip: "Line (L)",
        },
        ToolButtonSpec {
            tool: Tool::Callout,
            glyph: ToolGlyph::Text("💬"),
            tip: "Callout",
        },
        ToolButtonSpec {
            tool: Tool::Ruler,
            glyph: ToolGlyph::Text("📏"),
            tip: "Ruler",
        },
        ToolButtonSpec {
            tool: Tool::Blur,
            glyph: ToolGlyph::Text("▓"),
            tip: "Blur (B) — pixelate a region",
        },
    ];
    show_tool_category(ui, state, &theme, "Markup Tools", true, &draw_buttons);

    let markup_buttons = [
        ToolButtonSpec {
            tool: Tool::FocusRegion,
            glyph: ToolGlyph::Text("⛶"),
            tip: "Focus Region — mark a region of interest",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::MoveHere),
            glyph: ToolGlyph::Text("↔"),
            tip: "Move Here",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::AddHere),
            glyph: ToolGlyph::Draw(GlyphKind::Plus),
            tip: "Add Here",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::DeleteThis),
            glyph: ToolGlyph::Draw(GlyphKind::Cross),
            tip: "Delete This",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::FixText),
            glyph: ToolGlyph::Draw(GlyphKind::Pencil),
            tip: "Fix Text",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::Wrong),
            glyph: ToolGlyph::Draw(GlyphKind::CrossCircle),
            tip: "Wrong",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::Important),
            glyph: ToolGlyph::Text("❗"),
            tip: "Important",
        },
        ToolButtonSpec {
            tool: Tool::LogicTag(LogicTagKind::Question),
            glyph: ToolGlyph::Text("?"),
            tip: "Question",
        },
    ];
    show_tool_category(ui, state, &theme, "Logic Tags", true, &markup_buttons);

    let image_buttons = [
        ToolButtonSpec {
            tool: Tool::Fill,
            glyph: ToolGlyph::Draw(GlyphKind::FloodFill),
            tip: "Flood Fill",
        },
        ToolButtonSpec {
            tool: Tool::Crop,
            glyph: ToolGlyph::Svg("bytes://crop.svg", ICON_CROP),
            tip: "Crop (K)",
        },
        ToolButtonSpec {
            tool: Tool::Eyedropper,
            glyph: ToolGlyph::Svg("bytes://eyedrop.svg", ICON_EYEDROP),
            tip: "Eyedropper (I)",
        },
        ToolButtonSpec {
            tool: Tool::Step,
            glyph: ToolGlyph::Svg("bytes://step.svg", ICON_STEP),
            tip: "Step (N)",
        },
    ];
    show_tool_category(ui, state, &theme, "Image", true, &image_buttons);
    show_stamp_drawers(ui, state, &theme);

    // Stroke width + tool-specific options
    egui::CollapsingHeader::new(
        RichText::new("Stroke & Options")
            .strong()
            .color(theme.text_heading),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add(
            egui::Slider::new(&mut state.settings.stroke_width, 1.0..=100.0)
                .text("px")
                .clamping(egui::SliderClamping::Always),
        );

        if state.current_tool == Tool::Text {
            ui.add(
                egui::Slider::new(&mut state.settings.text_size, 10.0..=72.0)
                    .text("pt")
                    .clamping(egui::SliderClamping::Always),
            );
            ui.checkbox(&mut state.settings.text_bg, "Text BG")
                .on_hover_text("Draw a semi-transparent dark background behind text annotations");
        }
        if state.current_tool == Tool::Blur {
            ui.add(
                egui::Slider::new(&mut state.settings.blur_block, 4.0..=40.0)
                    .text("block")
                    .clamping(egui::SliderClamping::Always),
            );
        }
        if state.current_tool == Tool::Fill {
            ui.add(
                egui::Slider::new(&mut state.settings.fill_tolerance, 0u8..=255u8)
                    .text("tolerance")
                    .clamping(egui::SliderClamping::Always),
            );
        }
        if state.current_tool == Tool::Step {
            ui.add(
                egui::Slider::new(&mut state.settings.step_radius, 10.0..=48.0)
                    .text("size")
                    .clamping(egui::SliderClamping::Always),
            );
            let next = crate::tools::step::next_step_number(&state.canvas.annotations);
            let step_count = state
                .canvas
                .annotations
                .iter()
                .filter(|a| matches!(a.shape, crate::types::ShapeData::StepMarker { .. }))
                .count();
            ui.label(
                RichText::new(format!("next: {next} ({step_count} total)"))
                    .small()
                    .color(theme.text_label),
            );
            if step_count > 0 && ui.button("⟳ Renumber Steps").clicked() {
                crate::command::CommandExecutor::execute(
                    &mut state.canvas,
                    ParticipantId::HUMAN_LOCAL,
                    crate::types::Command::RenumberSteps,
                );
            }
        }

        if state.current_tool == Tool::Rect {
            ui.add(
                egui::Slider::new(&mut state.settings.corner_radius, 0.0..=50.0)
                    .text("radius")
                    .clamping(egui::SliderClamping::Always),
            );
        }

        if matches!(state.current_tool, Tool::Rect | Tool::Oval) {
            ui.separator();
            ui.label(RichText::new("Fill").small().color(theme.text_label));
            ui.checkbox(&mut state.settings.shape_fill_enabled, "Filled");
            if state.settings.shape_fill_enabled {
                let mut fc = state.settings.shape_fill_color;
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut fc,
                    egui::color_picker::Alpha::OnlyBlend,
                )
                .changed()
                {
                    state.settings.shape_fill_color = fc;
                }
                let mut falpha = state.settings.shape_fill_color.a();
                let alpha_resp = ui
                    .add(
                        egui::Slider::new(&mut falpha, 0u8..=255u8)
                            .show_value(false)
                            .clamping(egui::SliderClamping::Always)
                            .suffix(" opacity"),
                    )
                    .on_hover_text("Fill opacity");
                if alpha_resp.changed() {
                    let c = state.settings.shape_fill_color;
                    state.settings.shape_fill_color =
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), falpha);
                }
            }

            ui.separator();
            ui.checkbox(&mut state.settings.shape_stroke_enabled, "Stroke");

            ui.separator();
            ui.label(
                RichText::new("Quick presets")
                    .small()
                    .color(theme.text_label),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Plain").clicked() {
                    state.settings.shape_fill_enabled = false;
                    state.settings.shape_stroke_enabled = true;
                    if state.current_tool == Tool::Rect {
                        state.settings.corner_radius = 0.0;
                    }
                }
                if state.current_tool == Tool::Rect && ui.button("Highlight").clicked() {
                    state.settings.shape_fill_enabled = true;
                    state.settings.shape_stroke_enabled = false;
                    state.settings.corner_radius = 0.0;
                    let c = state.settings.shape_fill_color;
                    state.settings.shape_fill_color =
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 120);
                }
                if state.current_tool == Tool::Rect && ui.button("Round").clicked() {
                    state.settings.shape_fill_enabled = true;
                    state.settings.shape_stroke_enabled = true;
                    state.settings.corner_radius = 12.0;
                    let c = state.settings.shape_fill_color;
                    state.settings.shape_fill_color =
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 20);
                }
                if ui.button("Redact").clicked() {
                    state.settings.shape_fill_enabled = true;
                    state.settings.shape_stroke_enabled = false;
                    if state.current_tool == Tool::Rect {
                        state.settings.corner_radius = 0.0;
                    }
                    let c = state.settings.shape_fill_color;
                    state.settings.shape_fill_color =
                        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 255);
                }
            });
        }

        if matches!(state.current_tool, Tool::Stamp(_)) {
            ui.label(
                RichText::new("Stamp tint follows the current color.")
                    .small()
                    .color(theme.text_label),
            );
        }

        if matches!(
            state.current_tool,
            Tool::Line | Tool::Arrow | Tool::Rect | Tool::Oval
        ) {
            ui.label(RichText::new("Style").small().color(theme.text_label));
            let solid = state.settings.line_style == LineStyle::Solid;
            let dashed = state.settings.line_style == LineStyle::Dashed;
            let dotted = state.settings.line_style == LineStyle::Dotted;
            if ui.selectable_label(solid, "━ solid").clicked() {
                state.settings.line_style = LineStyle::Solid;
            }
            if ui.selectable_label(dashed, "╌ dashed").clicked() {
                state.settings.line_style = LineStyle::Dashed;
            }
            if ui.selectable_label(dotted, "· · · dotted").clicked() {
                state.settings.line_style = LineStyle::Dotted;
            }
        }
        if state.current_tool == Tool::Arrow {
            let single = state.settings.arrow_heads == ArrowHeads::Single;
            let double_h = state.settings.arrow_heads == ArrowHeads::Double;
            if ui
                .selectable_label(single, "→ single")
                .on_hover_text("Single arrow head")
                .clicked()
            {
                state.settings.arrow_heads = ArrowHeads::Single;
            }
            if ui
                .selectable_label(double_h, "↔ double")
                .on_hover_text("Double-ended arrow")
                .clicked()
            {
                state.settings.arrow_heads = ArrowHeads::Double;
            }
        }

        if state.current_tool == Tool::Marquee {
            ui.label(RichText::new("Shape").small().color(theme.text_label));
            let rect = state.settings.marquee_shape == SelKind::Rect;
            let ell = state.settings.marquee_shape == SelKind::Ellipse;
            if ui.selectable_label(rect, "▭ rect").clicked() {
                state.settings.marquee_shape = SelKind::Rect;
            }
            if ui.selectable_label(ell, "○ ellipse").clicked() {
                state.settings.marquee_shape = SelKind::Ellipse;
            }
            ui.separator();
            let has_sel = state.canvas.selection.is_some();
            if ui
                .add_enabled(has_sel, egui::Button::new("Invert"))
                .clicked()
            {
                if let Some(sel) = state.canvas.selection.as_mut() {
                    sel.inverted = !sel.inverted;
                }
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("Clear"))
                .clicked()
            {
                state.canvas.selection = None;
            }
        }

        ui.add_space(4.0);
        ui.separator();
        if ui
            .small_button("↺ Reset defaults")
            .on_hover_text(
                "Reset all settings to factory defaults.\nKeyboard shortcuts are preserved.",
            )
            .clicked()
        {
            // Preserve shortcuts — they live in a separate file and are
            // not part of the settings that get reset.
            let saved_shortcuts = state.settings.shortcuts.clone();
            state.settings = crate::state::Settings::default();
            state.settings.shortcuts = saved_shortcuts;
            crate::config::save_settings(&state.settings);
        }
    });
}

fn show_stamp_drawers(ui: &mut Ui, state: &mut AppState, theme: &Theme) {
    for category in stamp_categories() {
        let stamp_buttons: Vec<_> = all_stamps()
            .iter()
            .filter(|s| s.category == category)
            .map(|stamp| ToolButtonSpec {
                tool: Tool::Stamp(stamp.id),
                glyph: ToolGlyph::Svg(stamp.uri, stamp.svg_bytes),
                tip: stamp.label,
            })
            .collect();
        show_tool_category(ui, state, theme, category, false, &stamp_buttons);
    }
}

fn show_color_controls(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    bar_caption(ui, &theme, "Color");

    let prev = state.settings.stroke_color;
    let mut col = state.settings.stroke_color;
    if egui::color_picker::color_edit_button_srgba(
        ui,
        &mut col,
        egui::color_picker::Alpha::OnlyBlend,
    )
    .changed()
    {
        state.settings.stroke_color = col;
    }
    if state.settings.stroke_color != prev {
        push_recent_color(
            &mut state.settings.recent_colors,
            state.settings.stroke_color,
        );
    }

    let mut alpha = state.settings.stroke_color.a();
    let alpha_resp = ui
        .add(
            egui::Slider::new(&mut alpha, 0u8..=255u8)
                .show_value(false)
                .clamping(egui::SliderClamping::Always)
                .text("alpha"),
        )
        .on_hover_text("Stroke opacity");
    if alpha_resp.changed() {
        let c = state.settings.stroke_color;
        state.settings.stroke_color = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);
    }

    for preset in COLOR_PRESETS {
        let sel = {
            let c = state.settings.stroke_color;
            c.r() == preset.r() && c.g() == preset.g() && c.b() == preset.b()
        };
        if color_swatch(ui, preset, sel).clicked() {
            let current_alpha = state.settings.stroke_color.a();
            state.settings.stroke_color =
                Color32::from_rgba_unmultiplied(preset.r(), preset.g(), preset.b(), current_alpha);
            push_recent_color(
                &mut state.settings.recent_colors,
                state.settings.stroke_color,
            );
        }
    }

    let recent = state.settings.recent_colors.first().copied();
    if let Some(recent) = recent {
        bar_caption(ui, &theme, "recent");
        if color_swatch(ui, recent, state.settings.stroke_color == recent).clicked() {
            state.settings.stroke_color = recent;
        }
    }
}

// ── Recents side panel ────────────────────────────────────────────────────────

pub fn show_recents_panel(ui: &mut Ui, state: &mut AppState, ctx: &egui::Context) {
    let theme = state.settings.theme();
    side_panel_heading(ui, "Recent captures", &theme);
    ui.separator();

    egui::CollapsingHeader::new(
        RichText::new("After capture")
            .strong()
            .color(theme.text_heading),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.checkbox(
            &mut state.settings.auto_copy_on_capture,
            "Copy to clipboard",
        )
        .on_hover_text("Every finished capture lands on the clipboard, ready to paste");
        ui.checkbox(&mut state.settings.auto_save, "Save to disk");
        if state.settings.auto_save {
            ui.add_space(2.0);
            ui.label(RichText::new("Folder").small().color(theme.text_label));
            ui.add(
                egui::TextEdit::singleline(&mut state.settings.auto_save_dir_template)
                    .desired_width(f32::INFINITY)
                    .hint_text(crate::recents::DEFAULT_DIR_TEMPLATE),
            )
            .on_hover_text("strftime tokens allowed: %Y %m %d %H %M %S. `~` is your home.");
            ui.label(RichText::new("File name").small().color(theme.text_label));
            ui.add(
                egui::TextEdit::singleline(&mut state.settings.auto_save_name_template)
                    .desired_width(f32::INFINITY)
                    .hint_text(crate::recents::DEFAULT_NAME_TEMPLATE),
            )
            .on_hover_text("Extension is added automatically; duplicates get -1, -2, …");
            let preview = crate::recents::preview_auto_save_path(state);
            ui.label(
                RichText::new(preview.display().to_string())
                    .small()
                    .color(theme.text_dim),
            )
            .on_hover_text(preview.display().to_string());
            if ui
                .small_button("Reset to defaults")
                .on_hover_text("Restore the default folder and file-name templates")
                .clicked()
            {
                state.settings.auto_save_dir_template =
                    crate::recents::DEFAULT_DIR_TEMPLATE.to_string();
                state.settings.auto_save_name_template =
                    crate::recents::DEFAULT_NAME_TEMPLATE.to_string();
            }
        }
    });
    ui.separator();

    if state.recent_captures.is_empty() {
        ui.label(
            RichText::new("No recent captures yet")
                .italics()
                .color(theme.text_label),
        );
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        let entries = state.recent_captures.clone();
        for e in entries.iter() {
            let fname = e
                .image_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("(unnamed)");
            let label = format!("{fname}  ({}×{})", e.width, e.height);
            if ui.button(label).clicked() {
                crate::loader::load_file(&e.image_path, state, ctx);
            }
            ui.small(e.captured_at.split('T').next().unwrap_or(""));
            ui.separator();
        }
        if ui.button("✖ Clear recents").clicked() {
            state.recent_captures.clear();
            crate::state::save_recent_captures(&state.recent_captures);
        }
    });
}

// ── New Canvas / Resize Canvas dialogs ───────────────────────────────────────

fn clipboard_image_dims() -> (u32, u32, Option<(u32, u32)>) {
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.get_image() {
            Ok(img) => {
                let w = img.width as u32;
                let h = img.height as u32;
                (w, h, Some((w, h)))
            }
            Err(_) => (800, 600, None),
        },
        Err(_) => (800, 600, None),
    }
}

struct CanvasPreset {
    name: &'static str,
    w: u32,
    h: u32,
}

struct PresetCategory {
    name: &'static str,
    presets: &'static [CanvasPreset],
}

macro_rules! preset {
    ($n:expr, $w:expr, $h:expr) => {
        CanvasPreset {
            name: $n,
            w: $w,
            h: $h,
        }
    };
}

/// Curated, categorized canvas presets. Dimensions are in pixels; print sizes
/// are computed at 300 DPI.
const CANVAS_CATEGORIES: &[PresetCategory] = &[
    PresetCategory {
        name: "Standard",
        presets: &[
            preset!("Square S", 512, 512),
            preset!("Square M", 1024, 1024),
            preset!("Square L", 2048, 2048),
            preset!("Landscape", 1280, 800),
            preset!("Portrait", 800, 1280),
            preset!("Widescreen", 1600, 900),
        ],
    },
    PresetCategory {
        name: "Screen",
        presets: &[
            preset!("720p", 1280, 720),
            preset!("1080p", 1920, 1080),
            preset!("1440p", 2560, 1440),
            preset!("4K UHD", 3840, 2160),
            preset!("Laptop", 1366, 768),
            preset!("5K iMac", 5120, 2880),
        ],
    },
    PresetCategory {
        name: "Social",
        presets: &[
            preset!("IG Post", 1080, 1080),
            preset!("IG Story", 1080, 1920),
            preset!("IG Portrait", 1080, 1350),
            preset!("FB Cover", 1640, 624),
            preset!("X Post", 1600, 900),
            preset!("YT Thumb", 1280, 720),
            preset!("LinkedIn", 1584, 396),
            preset!("Pinterest", 1000, 1500),
        ],
    },
    PresetCategory {
        name: "Print",
        presets: &[
            preset!("A5", 1748, 2480),
            preset!("A4", 2480, 3508),
            preset!("A3", 3508, 4961),
            preset!("Letter", 2550, 3300),
            preset!("Legal", 2550, 4200),
            preset!("Tabloid", 3300, 5100),
            preset!("Card", 1050, 600),
            preset!("Poster", 5400, 7200),
        ],
    },
    PresetCategory {
        name: "Photo",
        presets: &[
            preset!("4 × 6", 1800, 1200),
            preset!("5 × 7", 2100, 1500),
            preset!("8 × 10", 3000, 2400),
            preset!("Square", 2048, 2048),
            preset!("16 : 9", 1920, 1080),
            preset!("3 : 2", 1800, 1200),
        ],
    },
    PresetCategory {
        name: "Mobile",
        presets: &[
            preset!("iPhone", 1170, 2532),
            preset!("iPad", 1640, 2360),
            preset!("Android", 1080, 1920),
            preset!("Watch", 396, 484),
            preset!("Surface", 1500, 1000),
        ],
    },
];

pub fn open_new_canvas_dialog(state: &mut AppState) {
    let (width, height, hint) = clipboard_image_dims();
    state.new_canvas_dialog = Some(crate::state::CanvasSizeDialog::new(width, height, hint));
}

pub fn show_new_canvas_dialog(ctx: &egui::Context, state: &mut AppState) {
    if state.new_canvas_dialog.is_none() {
        return;
    }
    let theme = state.settings.theme();

    let mut create = false;
    let mut cancel = false;

    // Center on first open, but leave the window free to be dragged anywhere
    // (including partway off the app edges). Using `.anchor()` here would pin
    // the window in place and prevent dragging entirely, so we use a centered
    // default position + pivot and disable the screen-edge constraint instead.
    let screen_center = ctx.screen_rect().center();
    egui::Window::new(RichText::new("New Canvas").strong())
        .collapsible(false)
        .resizable(false)
        .constrain(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(screen_center)
        .default_width(560.0)
        .show(ctx, |ui| {
            let dlg = state.new_canvas_dialog.as_mut().unwrap();
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.add_space(2.0);

            // ── Category tabs ──────────────────────────────────────────────
            ui.horizontal_wrapped(|ui| {
                for (i, cat) in CANVAS_CATEGORIES.iter().enumerate() {
                    if category_tab(ui, &theme, cat.name, dlg.category == i).clicked() {
                        dlg.category = i;
                    }
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(10.0);

            if let Some((cw, ch)) = dlg.clipboard_hint {
                ui.label(
                    RichText::new(format!("📋 Clipboard image detected — {cw} × {ch}"))
                        .color(theme.text_ok)
                        .small(),
                );
                ui.add_space(8.0);
            }

            // ── Body: preset grid (left) + details panel (right) ───────────
            // The vertical separator below expands to fill all *available*
            // height; in an auto-sizing window that's the whole screen, which
            // would stretch the dialog top-to-bottom and push the title bar /
            // Create button off-screen. Allocate a fixed-height region so the
            // separator (and the row) can never grow past the content.
            let body_size = egui::vec2(544.0, 320.0);
            ui.allocate_ui_with_layout(body_size, egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(348.0);
                        ui.label(
                            RichText::new(CANVAS_CATEGORIES[dlg.category].name.to_uppercase())
                                .small()
                                .strong()
                                .color(theme.text_label),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(296.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for p in CANVAS_CATEGORIES[dlg.category].presets {
                                        let sel = dlg.width == p.w && dlg.height == p.h;
                                        if preset_card(ui, &theme, p.name, p.w, p.h, sel).clicked()
                                        {
                                            dlg.width = p.w;
                                            dlg.height = p.h;
                                        }
                                    }
                                    if let Some((cw, ch)) = dlg.clipboard_hint {
                                        let sel = dlg.width == cw && dlg.height == ch;
                                        if preset_card(ui, &theme, "Clipboard", cw, ch, sel)
                                            .clicked()
                                        {
                                            dlg.width = cw;
                                            dlg.height = ch;
                                        }
                                    }
                                });
                            });
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // ── Details panel ──────────────────────────────────────────
                    ui.vertical(|ui| {
                        ui.set_width(170.0);
                        ui.label(
                            RichText::new("PRESET DETAILS")
                                .small()
                                .strong()
                                .color(theme.text_label),
                        );
                        ui.add_space(6.0);

                        canvas_preview(ui, &theme, dlg);
                        ui.add_space(10.0);

                        egui::Grid::new("new_canvas_grid")
                            .num_columns(2)
                            .spacing([10.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Width").color(theme.text_muted));
                                ui.add(
                                    egui::DragValue::new(&mut dlg.width)
                                        .range(1..=16384)
                                        .suffix(" px")
                                        .speed(2.0),
                                );
                                ui.end_row();
                                ui.label(RichText::new("Height").color(theme.text_muted));
                                ui.add(
                                    egui::DragValue::new(&mut dlg.height)
                                        .range(1..=16384)
                                        .suffix(" px")
                                        .speed(2.0),
                                );
                                ui.end_row();
                            });

                        ui.add_space(10.0);
                        ui.label(RichText::new("Orientation").color(theme.text_muted));
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let landscape = dlg.width >= dlg.height;
                            if ui.selectable_label(landscape, "  ▭  Landscape  ").clicked()
                                && !landscape
                            {
                                std::mem::swap(&mut dlg.width, &mut dlg.height);
                            }
                            if ui.selectable_label(!landscape, "  ▯  Portrait  ").clicked()
                                && landscape
                            {
                                std::mem::swap(&mut dlg.width, &mut dlg.height);
                            }
                        });

                        ui.add_space(10.0);
                        ui.label(RichText::new("Background").color(theme.text_muted));
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            if bg_swatch(
                                ui,
                                &theme,
                                Some(Color32::WHITE),
                                dlg.background == CanvasBackground::White,
                            )
                            .on_hover_text("White")
                            .clicked()
                            {
                                dlg.background = CanvasBackground::White;
                            }
                            if bg_swatch(
                                ui,
                                &theme,
                                None,
                                dlg.background == CanvasBackground::Transparent,
                            )
                            .on_hover_text("Transparent")
                            .clicked()
                            {
                                dlg.background = CanvasBackground::Transparent;
                            }
                            if bg_swatch(
                                ui,
                                &theme,
                                Some(Color32::BLACK),
                                dlg.background == CanvasBackground::Black,
                            )
                            .on_hover_text("Black")
                            .clicked()
                            {
                                dlg.background = CanvasBackground::Black;
                            }
                            let mut col = dlg.custom_color;
                            let resp = ui
                                .color_edit_button_srgb(&mut col)
                                .on_hover_text("Custom color");
                            if resp.changed() || resp.clicked() {
                                dlg.custom_color = col;
                                dlg.background = CanvasBackground::Custom;
                            }
                        });
                    });
                });
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Footer ─────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} × {} px", dlg.width, dlg.height))
                        .color(theme.text_muted),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let create_btn = egui::Button::new(
                        RichText::new("Create").color(theme.active_text).strong(),
                    )
                    .fill(theme.accent)
                    .min_size(egui::vec2(96.0, 28.0));
                    if ui.add(create_btn).clicked() {
                        create = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(80.0, 28.0)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        });

    if create {
        if let Some(dlg) = state.new_canvas_dialog.take() {
            crate::image_ops::new_blank_canvas_with_fill(
                dlg.width,
                dlg.height,
                dlg.fill_rgba(),
                state,
                ctx,
            );
        }
    } else if cancel {
        state.new_canvas_dialog = None;
    }
}

/// A flat, pill-style category tab for the New Canvas dialog.
fn category_tab(ui: &mut Ui, theme: &Theme, label: &str, selected: bool) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).color(if selected {
        theme.active_text
    } else {
        theme.text_muted
    }))
    .fill(if selected {
        theme.accent
    } else {
        Color32::TRANSPARENT
    })
    .stroke(Stroke::NONE);
    ui.add(btn)
}

/// A clean preset card: aspect-ratio thumbnail above name + dimensions.
fn preset_card(
    ui: &mut Ui,
    theme: &Theme,
    name: &str,
    w: u32,
    h: u32,
    selected: bool,
) -> egui::Response {
    let size = egui::vec2(106.0, 78.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    let rounding = egui::Rounding::same(6.0);

    let bg = if selected {
        theme.accent_subtle
    } else if resp.hovered() {
        theme.hover_bg
    } else {
        theme.surface_bg
    };
    painter.rect_filled(rect, rounding, bg);
    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected {
                theme.accent
            } else {
                theme.border_idle
            },
        ),
    );

    // Aspect-ratio thumbnail centered in the upper region.
    let (max_w, max_h) = (48.0, 30.0);
    let ar = w as f32 / h as f32;
    let (tw, th) = if ar >= max_w / max_h {
        (max_w, max_w / ar)
    } else {
        (max_h * ar, max_h)
    };
    let thumb = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 23.0),
        egui::vec2(tw.max(4.0), th.max(4.0)),
    );
    painter.rect_filled(thumb, egui::Rounding::same(2.0), theme.panel_bg);
    painter.rect_stroke(
        thumb,
        egui::Rounding::same(2.0),
        Stroke::new(
            1.0,
            if selected {
                theme.accent
            } else {
                theme.text_dim
            },
        ),
    );

    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 22.0),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(13.0),
        if selected {
            theme.text_heading
        } else {
            theme.text_muted
        },
    );
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 9.0),
        egui::Align2::CENTER_CENTER,
        format!("{w} × {h}"),
        egui::FontId::proportional(10.0),
        theme.text_label,
    );

    resp.on_hover_text(format!("{name} — {w} × {h} px"))
}

/// Live preview of the canvas aspect ratio and chosen background fill.
fn canvas_preview(ui: &mut Ui, theme: &Theme, dlg: &crate::state::CanvasSizeDialog) {
    let size = egui::vec2(ui.available_width(), 96.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(4.0), theme.tab_canvas_bg);

    let avail = rect.shrink(12.0);
    let ar = dlg.width as f32 / dlg.height as f32;
    let (cw, ch) = if ar >= avail.width() / avail.height() {
        (avail.width(), avail.width() / ar)
    } else {
        (avail.height() * ar, avail.height())
    };
    let crect = egui::Rect::from_center_size(rect.center(), egui::vec2(cw.max(4.0), ch.max(4.0)));
    let round = egui::Rounding::same(2.0);
    match dlg.background {
        CanvasBackground::Transparent => draw_checkerboard(&painter, crect),
        CanvasBackground::White => {
            painter.rect_filled(crect, round, Color32::WHITE);
        }
        CanvasBackground::Black => {
            painter.rect_filled(crect, round, Color32::BLACK);
        }
        CanvasBackground::Custom => {
            painter.rect_filled(
                crect,
                round,
                Color32::from_rgb(
                    dlg.custom_color[0],
                    dlg.custom_color[1],
                    dlg.custom_color[2],
                ),
            );
        }
    }
    painter.rect_stroke(crect, round, Stroke::new(1.0, theme.border_idle));
}

/// A 24×24 background-fill swatch. `None` fill renders a transparency checker.
fn bg_swatch(ui: &mut Ui, theme: &Theme, fill: Option<Color32>, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let painter = ui.painter();
    let inner = rect.shrink(2.0);
    let round = egui::Rounding::same(3.0);
    match fill {
        Some(c) => {
            painter.rect_filled(inner, round, c);
        }
        None => draw_checkerboard(&painter, inner),
    }
    painter.rect_stroke(
        rect,
        egui::Rounding::same(4.0),
        Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected {
                theme.accent
            } else {
                theme.border_idle
            },
        ),
    );
    resp
}

/// Paint a small light/grey checkerboard to represent transparency.
fn draw_checkerboard(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, egui::Rounding::same(2.0), Color32::from_gray(205));
    let cells = 4;
    let s = rect.width() / cells as f32;
    let sy = rect.height() / cells as f32;
    for row in 0..cells {
        for col in 0..cells {
            if (row + col) % 2 == 0 {
                let min = rect.min + egui::vec2(col as f32 * s, row as f32 * sy);
                let cell = egui::Rect::from_min_size(min, egui::vec2(s, sy));
                painter.rect_filled(cell, egui::Rounding::ZERO, Color32::from_gray(150));
            }
        }
    }
}

pub fn show_resize_canvas_dialog(ctx: &egui::Context, state: &mut AppState) {
    if state.resize_canvas_dialog.is_none() {
        return;
    }
    let theme = state.settings.theme();

    let current_size = state
        .canvas
        .color_image
        .as_ref()
        .map(|ci| (ci.size[0] as u32, ci.size[1] as u32));

    let mut apply = false;
    let mut cancel = false;

    egui::Window::new("📐 Resize Canvas")
        .collapsible(false)
        .resizable(false)
        .default_width(280.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let dlg = state.resize_canvas_dialog.as_mut().unwrap();

            if let Some((cw, ch)) = current_size {
                ui.label(
                    RichText::new(format!("Current size: {cw}×{ch}"))
                        .color(theme.text_label)
                        .small(),
                );
                ui.add_space(4.0);
            }
            ui.label(
                RichText::new("Shrinking crops; growing pads with white.")
                    .color(theme.text_label)
                    .small(),
            );
            ui.add_space(4.0);

            egui::Grid::new("resize_canvas_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Width");
                    ui.add(
                        egui::DragValue::new(&mut dlg.width)
                            .range(1..=16384)
                            .suffix(" px")
                            .speed(1.0),
                    );
                    ui.end_row();
                    ui.label("Height");
                    ui.add(
                        egui::DragValue::new(&mut dlg.height)
                            .range(1..=16384)
                            .suffix(" px")
                            .speed(1.0),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

    if apply {
        if let Some(dlg) = state.resize_canvas_dialog.take() {
            crate::image_ops::resize_canvas(dlg.width, dlg.height, state);
        }
    } else if cancel {
        state.resize_canvas_dialog = None;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn color_swatch(ui: &mut Ui, color: Color32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect.shrink(2.0), 3.0, color);
    if selected {
        painter.rect_stroke(rect, 3.0, Stroke::new(2.0, Color32::WHITE));
    } else {
        painter.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_gray(80)));
    }
    resp
}

fn clear_canvas(state: &mut AppState) {
    state.canvas.texture = None;
    state.canvas.color_image = None;
    state.canvas.canvas_size = egui::Vec2::ZERO;
    state.canvas.document = crate::scene::Document::from_flat(Vec::new());
    state.canvas.sync_annotations();
    state.canvas.clear_all_undo_redo();
    state.tool_state = crate::state::ToolState::Idle;
    state.canvas.selection = None;
    state.canvas.adjust_preview = None;
    state.canvas.marquee_state = crate::state::MarqueeState::Idle;
    state.view_zoom = 1.0;
    state.view_pan = egui::Vec2::ZERO;
    state.last_img_draw_rect = None;
    state.canvas.pdf_page = 0;
    state.canvas.pdf_pages = 0;
}

fn build_output_image(state: &AppState) -> Option<image::RgbaImage> {
    if state.settings.flatten_export {
        if let Some(screen) = state.last_img_draw_rect {
            if let Some(img) = crate::flatten::flatten(
                &state.canvas.annotations,
                state.canvas.color_image.as_ref(),
                state.canvas.canvas_size,
                screen,
            ) {
                return Some(img);
            }
        }
    }
    let ci = state.canvas.color_image.as_ref()?;
    let (w, h) = (ci.size[0] as u32, ci.size[1] as u32);
    let raw: Vec<u8> = ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    image::RgbaImage::from_raw(w, h, raw)
}

/// Image extensions we can safely overwrite in place on "Save".
fn is_writable_raster(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "bmp")
    )
}

/// Pick the export format from a path's extension, falling back to `fallback`.
fn format_for_path(path: &std::path::Path, fallback: ExportFormat) -> ExportFormat {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => ExportFormat::Png,
        Some("jpg") | Some("jpeg") => ExportFormat::Jpeg,
        Some("webp") => ExportFormat::WebP,
        Some("bmp") => ExportFormat::Bmp,
        _ => fallback,
    }
}

/// Encode the flattened output to `path`, then adopt it as the canvas source
/// and mark the canvas saved. Returns true on success.
fn write_output_to_path(state: &mut AppState, path: &std::path::Path) -> bool {
    let Some(img) = build_output_image(state) else {
        state.status_msg = Some("No image to save".into());
        return false;
    };
    let fmt = format_for_path(path, state.settings.export_format);
    let quality = state.settings.jpeg_quality;
    if let Err(e) = encode_image(&img, path, fmt, quality) {
        state.status_msg = Some(format!("Save failed: {e}"));
        false
    } else {
        state.canvas.source_path = Some(path.to_path_buf());
        state.canvas.mark_saved();
        let suffix = if fmt == ExportFormat::Jpeg {
            format!(" (JPEG q{quality})")
        } else {
            String::new()
        };
        state.status_msg = Some(format!("Saved {}{suffix}", path.display()));
        true
    }
}

/// Public "Save": overwrite the opened file directly when the canvas has a
/// writable raster source path; otherwise fall back to the Save As dialog.
pub fn save_image_with_flatten_pub(state: &mut AppState) {
    save_image(state);
}

pub fn save_image(state: &mut AppState) {
    if let Some(p) = state.canvas.source_path.clone() {
        if is_writable_raster(&p) {
            write_output_to_path(state, &p);
            return;
        }
    }
    save_image_as(state);
}

/// "Save As": always prompt for a path/format, then save there.
pub fn save_image_as(state: &mut AppState) {
    if build_output_image(state).is_none() {
        state.status_msg = Some("No image to save".into());
        return;
    }
    let fmt = state.settings.export_format;
    let ext = fmt.extension();

    // Default the dialog to the opened file (so one click overwrites it),
    // else to a fresh "redline.<ext>".
    let default_name = state
        .canvas
        .source_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("redline.{ext}"));
    let mut dialog = rfd::FileDialog::new().set_file_name(&default_name);
    let start_dir = state
        .canvas
        .source_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .or_else(|| state.last_save_dir.clone());
    if let Some(d) = start_dir {
        dialog = dialog.set_directory(d);
    }
    let all = [
        (ExportFormat::Png, "PNG", &["png"][..]),
        (ExportFormat::Jpeg, "JPEG", &["jpg", "jpeg"][..]),
        (ExportFormat::WebP, "WebP", &["webp"][..]),
        (ExportFormat::Bmp, "BMP", &["bmp"][..]),
    ];
    if let Some(current) = all.iter().find(|(f, _, _)| *f == fmt) {
        dialog = dialog.add_filter(current.1, current.2);
    }
    for (f, name, exts) in all.iter() {
        if *f != fmt {
            dialog = dialog.add_filter(*name, exts);
        }
    }
    let Some(mut path) = dialog.save_file() else {
        return;
    };
    // If the user typed no recognized extension, force the selected format's.
    if format_for_path(&path, fmt) == fmt && !is_writable_raster(&path) {
        path.set_extension(ext);
    }
    crate::state::remember_save_dir(state, &path);
    write_output_to_path(state, &path);
}

fn encode_image(
    img: &image::RgbaImage,
    path: &std::path::Path,
    fmt: ExportFormat,
    quality: u8,
) -> Result<(), String> {
    use image::ImageEncoder;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let (w, h) = (img.width(), img.height());
    match fmt {
        ExportFormat::Png => image::codecs::png::PngEncoder::new(&mut writer)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string()),
        ExportFormat::Jpeg => {
            // JPEG has no alpha — drop to RGB first.
            let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, quality.clamp(1, 100))
                .write_image(rgb.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())
        }
        ExportFormat::WebP => image::codecs::webp::WebPEncoder::new_lossless(&mut writer)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string()),
        ExportFormat::Bmp => image::codecs::bmp::BmpEncoder::new(&mut writer)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string()),
    }
}

pub fn copy_image_with_flatten_pub(state: &mut AppState) {
    copy_image_with_flatten(state);
}

/// Push an arbitrary RgbaImage to the system clipboard and set `state.status_msg`.
fn copy_rgba_to_clipboard(state: &mut AppState, img: image::RgbaImage, success_msg: &str) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let bytes = img.into_raw();
    match arboard::Clipboard::new() {
        Ok(mut cb) => {
            let data = arboard::ImageData {
                width: w,
                height: h,
                bytes: bytes.into(),
            };
            match cb.set_image(data) {
                Ok(()) => state.status_msg = Some(success_msg.into()),
                Err(e) => state.status_msg = Some(format!("Copy failed: {e}")),
            }
        }
        Err(e) => state.status_msg = Some(format!("Clipboard unavailable: {e}")),
    }
}

/// Open the brightness/contrast/saturation dialog. Snapshots current pixels so
/// Cancel fully reverts.
pub fn open_adjust_dialog(state: &mut AppState) {
    let Some(ci) = state.canvas.color_image.as_ref() else {
        state.status_msg = Some("No image loaded".into());
        return;
    };
    state.canvas.adjust_preview = Some(AdjustPreview {
        brightness: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        original_pixels: ci.pixels.clone(),
        original_size: ci.size,
        selection: state.canvas.selection,
    });
}

/// Modal-ish adjustments window. Called every frame from app.rs; no-op unless
/// `state.canvas.adjust_preview` is Some.
pub fn show_adjust_window(ctx: &egui::Context, state: &mut AppState) {
    if state.canvas.adjust_preview.is_none() {
        return;
    }
    let theme = state.settings.theme();

    let mut apply = false;
    let mut cancel = false;
    let mut changed = false;
    let has_selection = state
        .canvas
        .adjust_preview
        .as_ref()
        .map(|a| a.selection.is_some())
        .unwrap_or(false);

    egui::Window::new("🎛 Adjustments")
        .collapsible(false)
        .resizable(false)
        .default_width(320.0)
        .anchor(egui::Align2::RIGHT_TOP, [-20.0, 60.0])
        .show(ctx, |ui| {
            let ap = state.canvas.adjust_preview.as_mut().unwrap();
            ui.label(
                RichText::new(if has_selection {
                    "Applying to selection"
                } else {
                    "Applying to whole image"
                })
                .color(theme.text_label)
                .small(),
            );
            ui.separator();
            ui.label("Brightness");
            changed |= ui
                .add(egui::Slider::new(&mut ap.brightness, -1.0..=1.0).fixed_decimals(2))
                .changed();
            ui.label("Contrast");
            changed |= ui
                .add(egui::Slider::new(&mut ap.contrast, -1.0..=1.0).fixed_decimals(2))
                .changed();
            ui.label("Saturation");
            changed |= ui
                .add(egui::Slider::new(&mut ap.saturation, -1.0..=1.0).fixed_decimals(2))
                .changed();
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    ap.brightness = 0.0;
                    ap.contrast = 0.0;
                    ap.saturation = 0.0;
                    changed = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Apply").clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        });

    if changed {
        crate::image_ops::rerun_adjust_preview(state);
    }
    if apply {
        crate::image_ops::commit_adjust(state);
    } else if cancel {
        crate::image_ops::cancel_adjust(state);
    }
}

fn copy_image_with_flatten(state: &mut AppState) {
    let Some(img) = build_output_image(state) else {
        state.status_msg = Some("No image to copy".into());
        return;
    };
    let (w, h) = (img.width() as usize, img.height() as usize);
    let bytes = img.into_raw();
    match arboard::Clipboard::new() {
        Ok(mut cb) => {
            let data = arboard::ImageData {
                width: w,
                height: h,
                bytes: bytes.into(),
            };
            match cb.set_image(data) {
                Ok(()) => state.status_msg = Some("Copied image to clipboard".into()),
                Err(e) => state.status_msg = Some(format!("Copy failed: {e}")),
            }
        }
        Err(e) => state.status_msg = Some(format!("Clipboard unavailable: {e}")),
    }
}

// ── Countdown overlay ─────────────────────────────────────────────────────────

/// Draw a full-canvas countdown overlay when a timed capture is pending.
/// `remaining` is seconds until capture fires; `total` is the configured delay.
pub fn draw_countdown_overlay(ui: &mut Ui, remaining: f32, total: u32, accent: Color32) {
    let rect = ui.ctx().screen_rect();
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("redline-countdown"),
    ));

    // Dim veil
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));

    let center = rect.center();
    let radius = 72.0_f32;

    // Arc background circle
    painter.circle_filled(center, radius, egui::Color32::from_black_alpha(180));
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(3.0, egui::Color32::from_gray(60)),
    );

    // Progress arc (white, sweeps CW from top)
    let fraction = (remaining / total as f32).clamp(0.0, 1.0);
    let tau = std::f32::consts::TAU;
    let start_angle = -std::f32::consts::FRAC_PI_2; // 12-o'clock
    let sweep = fraction * tau;
    let steps = 64;
    let r = radius - 4.0;
    for i in 0..steps {
        let t0 = start_angle + (i as f32 / steps as f32) * sweep;
        let t1 = start_angle + ((i + 1) as f32 / steps as f32) * sweep;
        let p0 = center + egui::vec2(t0.cos(), t0.sin()) * r;
        let p1 = center + egui::vec2(t1.cos(), t1.sin()) * r;
        painter.line_segment([p0, p1], egui::Stroke::new(5.0, accent));
    }

    // Countdown number
    let secs_left = remaining.ceil() as u32;
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        secs_left.to_string(),
        egui::FontId::proportional(52.0),
        egui::Color32::WHITE,
    );

    // "Esc to cancel" hint
    painter.text(
        center + egui::vec2(0.0, radius + 18.0),
        egui::Align2::CENTER_CENTER,
        "Esc to cancel",
        egui::FontId::proportional(13.0),
        egui::Color32::from_gray(180),
    );
}

// ── Capture helpers (called from app.rs) ──────────────────────────────────────

pub fn trigger_capture_pub(state: &mut AppState, mode: SnapMode) {
    crate::portal_capture::capture_log(|| format!("trigger {mode:?}"));
    if state.pending_capture.is_some()
        || state.capture_scheduled.is_some()
        || state.capture_hide_pending.is_some()
        || state.region_select.is_some()
    {
        // Silently ignoring the click is what made a wedged capture look like
        // a dead button — say so instead.
        crate::portal_capture::capture_log(|| {
            "trigger ignored: a capture is already running".into()
        });
        state.status_msg = Some("A capture is already running — Esc cancels it".into());
        return;
    }
    let delay = state.settings.capture_delay_s;
    if delay == 0 {
        begin_hide_for_capture(state, mode);
    } else {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(delay as u64);
        state.capture_scheduled = Some((deadline, mode));
        state.status_msg = Some(format!("Capturing in {delay}s…"));
    }
}

pub fn begin_hide_for_capture(state: &mut AppState, mode: SnapMode) {
    crate::portal_capture::capture_log(|| {
        format!(
            "begin hide (hide_redline={})",
            state.settings.hide_redline_on_capture
        )
    });
    if !state.settings.hide_redline_on_capture {
        fire_capture(state, mode);
        return;
    }
    state.window_hidden_for_capture = true;
    let fire_after = std::time::Instant::now() + std::time::Duration::from_millis(300);
    state.capture_hide_pending = Some((fire_after, mode));
}

pub fn fire_capture(state: &mut AppState, mode: SnapMode) {
    // In-app region selection grabs the whole screen non-interactively and
    // frames it in our own overlay; the portal's selector is the fallback.
    let use_overlay = matches!(mode, SnapMode::Region) && state.settings.region_selector_inapp;
    state.region_pending = use_overlay;
    let interactive = matches!(mode, SnapMode::Region) && !use_overlay;
    state.pending_capture = Some(crate::portal_capture::start(interactive, use_overlay));
    state.pending_capture_since = Some(std::time::Instant::now());
    state.capture_attempt = Some((mode, interactive));
    state.capture_priming = false;
    state.status_msg = if use_overlay {
        Some("Grabbing the screen…".into())
    } else {
        None
    };
}

/// Fire a throwaway non-interactive grab purely to make the desktop show its
/// "allow screenshots?" prompt.
///
/// GNOME only grants that permission through a dialog it refuses to show
/// unless the requesting app is focused ("Only the focused app is allowed to
/// show a system access dialog"). Every real capture minimizes RedLine first,
/// so the very first non-interactive grab on a fresh machine could never get
/// permission — it just failed with an empty portal error. We run one visible
/// grab, throw the pixels away, and re-run the real capture once the desktop
/// has remembered the answer.
pub fn fire_permission_priming(state: &mut AppState, mode: SnapMode) {
    crate::portal_capture::capture_log(|| format!("priming screenshot permission for {mode:?}"));
    state.region_pending = false;
    state.pending_capture = Some(crate::portal_capture::start(false, false));
    state.pending_capture_since = Some(std::time::Instant::now());
    state.capture_attempt = Some((mode, false));
    state.capture_priming = true;
    state.status_msg =
        Some("Allow RedLine to take screenshots — the capture runs right after".into());
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

pub fn show_tab_bar(ui: &mut Ui, state: &mut AppState) {
    let tab_count = state.tab_count();
    let t = state.settings.theme();
    let active_bg = t.tab_active_bg;
    let inactive_bg = t.tab_inactive_bg;
    let active_fg = t.tab_active_fg;
    let inactive_fg = t.tab_inactive_fg;
    let drag_indicator = t.accent;

    let mut switch_to: Option<usize> = None;
    let mut close_idx: Option<usize> = None;
    let mut reorder: Option<(usize, usize)> = None;
    let mut drag_start: Option<(usize, f32)> = None; // (tab_idx, pointer_x)
    let mut drag_end: Option<(usize, f32)> = None; // (tab_idx, final_x)

    // Collect per-tab rects for drop-target computation.
    let mut tab_rects: Vec<egui::Rect> = Vec::with_capacity(tab_count);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;

        for i in 0..tab_count {
            let is_active = i == state.active_tab_pos;
            let label = state.tab_label_at(i).to_string();

            // Dim the label of the tab being dragged.
            let is_being_dragged = state
                .tab_drag
                .as_ref()
                .map(|d| d.dragging_tab == i)
                .unwrap_or(false);

            let fg = if is_being_dragged {
                t.accent_dim
            } else if is_active {
                active_fg
            } else {
                inactive_fg
            };
            let bg = if is_active { active_bg } else { inactive_bg };

            let tab_frame = egui::Frame::none()
                .fill(bg)
                .rounding(egui::Rounding {
                    nw: 7.0,
                    ne: 7.0,
                    sw: 0.0,
                    se: 0.0,
                })
                .inner_margin(egui::Margin {
                    left: 12.0,
                    right: 6.0,
                    top: 5.0,
                    bottom: 5.0,
                });

            let resp = tab_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Active tab in rename mode: show inline TextEdit.
                    if is_active && state.tab_rename_buf.is_some() {
                        let buf = state.tab_rename_buf.as_mut().unwrap();
                        let edit = ui.add(
                            egui::TextEdit::singleline(buf)
                                .font(egui::FontId::proportional(12.0))
                                .desired_width(120.0),
                        );
                        edit.request_focus();
                        let commit =
                            edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if commit || cancel {
                            if commit {
                                let new_label = state.tab_rename_buf.take().unwrap_or_default();
                                if !new_label.trim().is_empty() {
                                    state.tab_label = new_label;
                                }
                            } else {
                                state.tab_rename_buf = None;
                            }
                        }
                        return;
                    }

                    let text = egui::RichText::new(truncate_label(&label, 24))
                        .color(fg)
                        .size(12.0);
                    // click_and_drag so the label responds to both click and drag.
                    let tab_btn =
                        ui.add(egui::Label::new(text).sense(egui::Sense::click_and_drag()));

                    // Double-click on active tab: enter rename mode.
                    if tab_btn.double_clicked() && is_active {
                        state.tab_rename_buf = Some(label.clone());
                    }

                    // Click (no drag) — switch to this tab.
                    if tab_btn.clicked() && !is_active && state.tab_drag.is_none() {
                        switch_to = Some(i);
                    }

                    // Drag started — record the origin.
                    if tab_btn.drag_started() {
                        if let Some(pos) = ui.ctx().pointer_interact_pos() {
                            drag_start = Some((i, pos.x));
                        }
                    }

                    // Drag in progress — update current x for the drop indicator.
                    if tab_btn.dragged() {
                        if let Some(ref mut drag) = state.tab_drag {
                            if drag.dragging_tab == i {
                                if let Some(pos) = ui.ctx().pointer_interact_pos() {
                                    drag.drag_x = pos.x;
                                }
                                ui.ctx().request_repaint();
                            }
                        }
                    }

                    // Drag released — record for reorder computation.
                    if tab_btn.drag_stopped() {
                        if let Some(ref drag) = state.tab_drag {
                            if drag.dragging_tab == i {
                                drag_end = Some((i, drag.drag_x));
                            }
                        }
                    }

                    if is_active {
                        let x_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("×").color(inactive_fg).size(12.0),
                            )
                            .frame(false)
                            .min_size(egui::vec2(16.0, 16.0)),
                        );
                        if x_btn.clicked() {
                            close_idx = Some(i);
                        }
                    }
                });
            });

            let rect = resp.response.rect;
            tab_rects.push(rect);

            // Bottom border: active tab gets an accent line, others get a dim separator.
            if is_active {
                ui.painter().line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    egui::Stroke::new(2.0, t.accent),
                );
            }
        }

        // Draw a vertical drop-position indicator while a drag is active.
        if let Some(ref drag) = state.tab_drag {
            let target = tab_index_from_x(&tab_rects, drag.drag_x);
            let indicator_x = if target < tab_rects.len() {
                tab_rects[target].left()
            } else {
                tab_rects.last().map(|r| r.right()).unwrap_or(0.0)
            };
            if let (Some(first), Some(last)) = (tab_rects.first(), tab_rects.last()) {
                ui.painter().line_segment(
                    [
                        egui::pos2(indicator_x, first.top()),
                        egui::pos2(indicator_x, last.bottom()),
                    ],
                    egui::Stroke::new(2.0, drag_indicator),
                );
            }
        }
    });

    // Commit drag start (after the UI loop, outside the borrow).
    if let Some((tab_idx, pointer_x)) = drag_start {
        state.tab_drag = Some(TabDrag {
            dragging_tab: tab_idx,
            drag_x: pointer_x,
        });
    }

    // Commit drag end → compute drop target and schedule reorder.
    if let Some((from, drag_x)) = drag_end {
        let to = tab_index_from_x(&tab_rects, drag_x).min(tab_count.saturating_sub(1));
        if to != from {
            reorder = Some((from, to));
        }
        state.tab_drag = None;
    }

    if let Some(i) = switch_to {
        state.switch_to_tab(i);
    }
    if let Some(i) = close_idx {
        state.close_tab(i);
    }
    if let Some((f, t)) = reorder {
        state.reorder_tab(f, t);
    }
}

/// Given a list of tab rects and a pointer x position, return the logical tab
/// index that is the best drop target (the tab whose left half the pointer is over,
/// or `rects.len()` if the pointer is past all tabs).
fn tab_index_from_x(rects: &[egui::Rect], x: f32) -> usize {
    for (i, r) in rects.iter().enumerate() {
        if x < r.center().x {
            return i;
        }
    }
    rects.len()
}

/// Map a pointer Y to a visual slot index among rows whose top edges are `row_tops`
/// (ascending). The slot is the index of the row the pointer is over, clamped to
/// the last row. Mirrors `tab_index_from_x` for vertical lists.
fn index_from_y(row_tops: &[f32], y: f32) -> usize {
    if row_tops.is_empty() {
        return 0;
    }
    let mut slot = 0usize;
    for (i, &top) in row_tops.iter().enumerate() {
        if y >= top {
            slot = i;
        }
    }
    slot
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        chars[..max_chars - 1].iter().collect::<String>() + "…"
    }
}

// ── Keyboard shortcuts window ─────────────────────────────────────────────────

pub fn show_shortcuts_window(ctx: &egui::Context, state: &mut AppState) {
    if !state.show_shortcuts_window {
        return;
    }

    let t = state.settings.theme();
    let amber = t.accent;
    let dim = t.dialog_dim_text;
    let key_bg = t.dialog_key_bg;
    let key_border = t.dialog_key_border;
    let recording_action = state.shortcut_recording;

    let mut open = state.show_shortcuts_window;
    egui::Window::new("⌨ Keyboard Shortcuts")
        .open(&mut open)
        .resizable(true)
        .default_width(440.0)
        .default_height(540.0)
        .show(ctx, |ui| {
            // Status bar at top
            if recording_action.is_some() {
                egui::Frame::none()
                    .fill(t.dialog_recording_bg)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("● Press any key combo now…  Esc to cancel")
                                .color(amber)
                                .strong()
                                .small(),
                        );
                    });
            } else {
                ui.label(
                    RichText::new("Click [Set] on any row to reassign it.")
                        .color(dim)
                        .small(),
                );
            }
            ui.add_space(2.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                let groups: &[(&str, &[Action])] = &[
                    (
                        "Capture",
                        &[Action::CaptureRegion, Action::CaptureFullScreen],
                    ),
                    ("File", &[Action::ExportSidecar, Action::PasteImage]),
                    ("Edit", &[Action::Undo, Action::Redo]),
                    ("View", &[Action::ResetZoom, Action::ToggleMagnifier]),
                    (
                        "Tools",
                        &[
                            Action::ToolSelect,
                            Action::ToolPen,
                            Action::ToolRect,
                            Action::ToolOval,
                            Action::ToolStar,
                            Action::ToolArrow,
                            Action::ToolLine,
                            Action::ToolText,
                            Action::ToolBlur,
                            Action::ToolCrop,
                            Action::ToolEyedropper,
                            Action::ToolStep,
                        ],
                    ),
                ];

                let mut set_recording: Option<Action> = None;
                let mut clear_action: Option<Action> = None;

                for (group_name, actions) in groups {
                    ui.add_space(6.0);
                    // Group header
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(*group_name).color(amber).small().strong());
                        let avail = ui.available_width();
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.min.x + 4.0, rect.center().y),
                                egui::pos2(rect.max.x, rect.center().y),
                            ],
                            egui::Stroke::new(1.0, t.dialog_row_border),
                        );
                    });
                    ui.add_space(2.0);

                    for &action in *actions {
                        let is_recording = recording_action == Some(action);

                        ui.horizontal(|ui| {
                            // Action label — fixed width column
                            ui.add_sized(
                                [190.0, 22.0],
                                egui::Label::new(RichText::new(action.label()).color(
                                    if is_recording {
                                        amber
                                    } else {
                                        Color32::from_gray(210)
                                    },
                                )),
                            );

                            // Key badge / recording indicator
                            if is_recording {
                                egui::Frame::none()
                                    .fill(t.accent_subtle)
                                    .rounding(egui::Rounding::same(4.0))
                                    .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new("● recording…")
                                                .color(amber)
                                                .small()
                                                .monospace(),
                                        );
                                    });
                            } else {
                                let badge_text = match state.settings.shortcuts.get(&action) {
                                    Some(k) => k.display(),
                                    None => "(unbound)".into(),
                                };
                                let is_unbound = state.settings.shortcuts.get(&action).is_none();
                                egui::Frame::none()
                                    .fill(if is_unbound {
                                        Color32::TRANSPARENT
                                    } else {
                                        key_bg
                                    })
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        if is_unbound {
                                            Color32::TRANSPARENT
                                        } else {
                                            key_border
                                        },
                                    ))
                                    .rounding(egui::Rounding::same(4.0))
                                    .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(badge_text)
                                                .color(if is_unbound {
                                                    dim
                                                } else {
                                                    Color32::from_gray(220)
                                                })
                                                .small()
                                                .monospace(),
                                        );
                                    });
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !is_recording {
                                        if ui.small_button(RichText::new("✕").color(dim)).clicked()
                                        {
                                            clear_action = Some(action);
                                        }
                                    }
                                    let set_text = if is_recording {
                                        RichText::new("Cancel").color(amber).small()
                                    } else {
                                        RichText::new("Set").color(dim).small()
                                    };
                                    if ui.small_button(set_text).clicked() {
                                        set_recording = Some(action);
                                    }
                                },
                            );
                        });
                    }
                }

                if let Some(action) = set_recording {
                    if state.shortcut_recording == Some(action) {
                        state.shortcut_recording = None;
                    } else {
                        state.shortcut_recording = Some(action);
                    }
                }
                if let Some(action) = clear_action {
                    state.settings.shortcuts.remove(&action);
                    save_shortcuts(&state.settings.shortcuts);
                }

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Reset All to Defaults").clicked() {
                        state.settings.shortcuts = default_shortcuts();
                        state.shortcut_recording = None;
                        save_shortcuts(&state.settings.shortcuts);
                    }
                    ui.label(
                        RichText::new("Shortcuts are saved automatically.")
                            .color(dim)
                            .small(),
                    );
                });
            });
        });

    state.show_shortcuts_window = open;
    if !open {
        state.shortcut_recording = None;
    }
}

// ── Vectorize panel ───────────────────────────────────────────────────────────

/// Drain any worker result into the per-canvas vectorize panel state.
/// Safe to call every frame — no-op when the panel is closed or no result is ready.
pub fn poll_vectorize_result(state: &mut AppState, ctx: &egui::Context) {
    let panel = &mut state.canvas.vectorize_panel;
    if !panel.open {
        return;
    }
    let Ok(mut slot) = panel.auto_preview.lock() else {
        return;
    };
    let Some(result) = slot.take() else { return };
    drop(slot);
    panel.is_computing = false;
    match result {
        Ok(svg) => {
            let (hdr, raw_paths, ftr) = crate::vectorize::parse_svg_parts(&svg);
            panel.svg_header = hdr;
            panel.svg_footer = ftr;
            panel.svg_paths = raw_paths
                .into_iter()
                .map(|e| SvgPathEntry {
                    element: e,
                    enabled: true,
                })
                .collect();
            let assembled = crate::vectorize::assemble_svg(
                &panel.svg_header,
                &panel.svg_paths,
                &panel.svg_footer,
            );
            let old = format!("bytes://vp_{}.svg", panel.preview_gen);
            ctx.forget_image(&old);
            panel.preview_gen = panel.preview_gen.wrapping_add(1);
            panel.preview_svg = Some(assembled.into_bytes());
            panel.preview_error = None;
        }
        Err(e) => {
            panel.svg_paths = Vec::new();
            panel.preview_svg = None;
            panel.preview_error = Some(format!("Preview failed: {e}"));
        }
    }
}

/// If the panel is dirty and the debounce window has elapsed, send a fresh
/// vectorize job to the worker. Safe no-op when panel is closed or in-flight.
pub fn maybe_dispatch_vectorize_job(state: &mut AppState, ctx: &egui::Context) {
    let panel_ref = &state.canvas.vectorize_panel;
    let ready = panel_ref.open
        && panel_ref.dirty
        && !panel_ref.is_computing
        && state.canvas.color_image.is_some()
        && panel_ref.last_dirty.map_or(true, |t| {
            t.elapsed() > std::time::Duration::from_millis(150)
        });
    if !ready {
        if panel_ref.open && panel_ref.dirty && !panel_ref.is_computing {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        return;
    }
    let Some(ci) = state.canvas.color_image.clone() else {
        return;
    };
    if state.vectorize_worker.is_none() {
        state.vectorize_worker = Some(crate::vectorize_worker::VectorizeWorker::spawn());
    }
    let settings_snapshot = state.settings.vectorize.clone();
    let panel = &mut state.canvas.vectorize_panel;
    panel.dirty = false;
    panel.is_computing = true;
    *panel.auto_preview.lock().unwrap() = None;
    let job = crate::vectorize_worker::VectorizeJob {
        image: ci,
        settings: settings_snapshot,
        response: std::sync::Arc::clone(&panel.auto_preview),
        ctx: ctx.clone(),
    };
    state.vectorize_worker.as_ref().unwrap().submit(job);
}

pub fn show_vectorize_panel(ui: &mut Ui, state: &mut AppState) {
    use std::sync::Arc;
    use std::time::Duration;

    if !state.canvas.vectorize_panel.open {
        return;
    }

    let theme = state.settings.theme();
    let has = state.canvas.color_image.is_some();

    // ── Panel UI ──────────────────────────────────────────────────────────────
    let copy_enabled = {
        let panel = &mut state.canvas.vectorize_panel;
        let settings = &mut state.settings.vectorize;

        ui.add_space(4.0);
        ui.label(RichText::new("Convert to SVG").strong());
        ui.separator();

        // Emit-mode selector (Trace / Annotations / Combined)
        ui.horizontal(|ui| {
            ui.label("Emit:");
            let emit_changed = ui
                .selectable_value(
                    &mut settings.emit_mode,
                    VectorizeEmitMode::TraceImage,
                    "Trace",
                )
                .changed()
                | ui.selectable_value(
                    &mut settings.emit_mode,
                    VectorizeEmitMode::Annotations,
                    "Annotations",
                )
                .changed()
                | ui.selectable_value(
                    &mut settings.emit_mode,
                    VectorizeEmitMode::Combined,
                    "Combined",
                )
                .changed();
            if emit_changed {
                panel.dirty = true;
                panel.last_dirty = Some(std::time::Instant::now());
            }
        });

        // Trace-mode selector and per-mode settings only matter when a trace is involved.
        if settings.emit_mode != VectorizeEmitMode::Annotations {
            // Mode selector
            let mode_changed = ui
                .horizontal(|ui| {
                    ui.selectable_value(&mut settings.mode, VectorizeMode::LineArt, "Edges")
                        .changed()
                        | ui.selectable_value(&mut settings.mode, VectorizeMode::Grayscale, "Gray")
                            .changed()
                        | ui.selectable_value(&mut settings.mode, VectorizeMode::Color, "Color")
                            .changed()
                })
                .inner;
            if mode_changed {
                panel.preview_svg = None;
                panel.preview_error = None;
                panel.dirty = true;
                panel.last_dirty = Some(std::time::Instant::now());
            }
            ui.separator();

            let settings_changed = egui::ScrollArea::vertical()
                .id_salt("vectorize_scroll")
                .max_height(220.0)
                .show(ui, |ui| match settings.mode.clone() {
                    VectorizeMode::LineArt => show_line_art_settings(ui, settings),
                    VectorizeMode::Grayscale => show_shared_fill_settings(ui, settings, false),
                    VectorizeMode::Color => show_shared_fill_settings(ui, settings, true),
                })
                .inner;
            if settings_changed {
                panel.dirty = true;
                panel.last_dirty = Some(std::time::Instant::now());
            }
        }

        ui.separator();

        // Status / preview
        if panel.is_computing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Generating…");
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else if let Some(err) = &panel.preview_error {
            ui.colored_label(Color32::RED, err.clone());
        }
        if let Some(bytes) = &panel.preview_svg {
            let uri = format!("bytes://vp_{}.svg", panel.preview_gen);
            let w = ui.available_width();
            ui.add(
                egui::Image::new(egui::ImageSource::Bytes {
                    uri: std::borrow::Cow::Owned(uri),
                    bytes: egui::load::Bytes::Shared(Arc::from(bytes.as_slice())),
                })
                .max_size(egui::vec2(w, w))
                .maintain_aspect_ratio(true),
            );
        }

        // Path editor: lets the user delete individual path elements
        let path_count = panel.svg_paths.len();
        if path_count > 0 {
            let removed = panel.svg_paths.iter().filter(|p| !p.enabled).count();
            let header_label = if removed > 0 {
                format!("Paths ({path_count}, {removed} removed)")
            } else {
                format!("Paths ({path_count})")
            };
            let mut toggled: Option<usize> = None;
            egui::CollapsingHeader::new(header_label)
                .id_salt("svg_path_list")
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("path_list_scroll")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for (i, path) in panel.svg_paths.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if path.enabled {
                                        ui.label(format!("#{}", i + 1));
                                        if ui
                                            .small_button("✕")
                                            .on_hover_text("Remove this path")
                                            .clicked()
                                        {
                                            toggled = Some(i);
                                        }
                                    } else {
                                        ui.label(
                                            RichText::new(format!("#{}", i + 1))
                                                .strikethrough()
                                                .color(theme.text_label),
                                        );
                                        if ui
                                            .small_button("↩")
                                            .on_hover_text("Restore path")
                                            .clicked()
                                        {
                                            toggled = Some(i);
                                        }
                                    }
                                });
                            }
                        });
                });
            if let Some(i) = toggled {
                panel.svg_paths[i].enabled = !panel.svg_paths[i].enabled;
                let assembled = crate::vectorize::assemble_svg(
                    &panel.svg_header,
                    &panel.svg_paths,
                    &panel.svg_footer,
                );
                let old_uri = format!("bytes://vp_{}.svg", panel.preview_gen);
                ui.ctx().forget_image(&old_uri);
                panel.preview_gen = panel.preview_gen.wrapping_add(1);
                panel.preview_svg = Some(assembled.into_bytes());
            }
        }

        ui.separator();
        // Compute copy_enabled before the mutable borrow of panel ends.
        // In Annotations or Combined mode, copy is always available (no raster needed).
        match settings.emit_mode {
            VectorizeEmitMode::Annotations | VectorizeEmitMode::Combined => true,
            VectorizeEmitMode::TraceImage => {
                has && (panel.preview_svg.is_some() || !panel.svg_header.is_empty())
            }
        }
    };

    // Save / Copy / Close buttons — below the mutable panel borrow so
    // build_final_svg can take &AppState without conflict.
    let emit_mode = state.settings.vectorize.emit_mode;
    let save_enabled = match emit_mode {
        VectorizeEmitMode::Annotations | VectorizeEmitMode::Combined => true,
        VectorizeEmitMode::TraceImage => has,
    };
    let save_svg = ui
        .horizontal(|ui| {
            let save = ui
                .add_enabled(
                    save_enabled,
                    egui::Button::new("💾 Save SVG…").fill(theme.accent_secondary),
                )
                .on_disabled_hover_text("Open an image first")
                .clicked();
            if ui
                .add_enabled(copy_enabled, egui::Button::new("📋 Copy"))
                .on_hover_text("Copy SVG to clipboard")
                .on_disabled_hover_text("Generate a preview first")
                .clicked()
            {
                if let Some(svg) = build_final_svg(state) {
                    ui.ctx().copy_text(svg);
                }
            }
            save
        })
        .inner;
    let close = ui.button("✕ Close").clicked();

    if save_svg {
        do_save_svg(state);
    }
    if close {
        state.canvas.vectorize_panel.open = false;
    }
}

fn show_line_art_settings(ui: &mut Ui, dialog: &mut crate::state::VectorizeSettings) -> bool {
    let mut c = false;

    // Method toggle
    c |= ui
        .horizontal(|ui| {
            ui.selectable_value(&mut dialog.edge_method, EdgeMethod::Threshold, "Threshold")
                .changed()
                | ui.selectable_value(&mut dialog.edge_method, EdgeMethod::Skeleton, "Skeleton")
                    .changed()
                | ui.selectable_value(&mut dialog.edge_method, EdgeMethod::Canny, "Canny")
                    .changed()
        })
        .inner;

    match dialog.edge_method {
        EdgeMethod::Threshold | EdgeMethod::Skeleton => {
            c |= ui
                .add(egui::Slider::new(&mut dialog.threshold, 0u8..=255u8).text("Threshold"))
                .changed();
            c |= ui
                .add(
                    egui::Slider::new(&mut dialog.blur_sigma, 0.0..=5.0)
                        .step_by(0.1)
                        .text("Pre-blur σ"),
                )
                .changed();
        }
        EdgeMethod::Canny => {
            c |= ui
                .add(
                    egui::Slider::new(&mut dialog.blur_sigma, 0.0..=5.0)
                        .step_by(0.1)
                        .text("Pre-blur σ"),
                )
                .changed();
            c |= ui
                .add(egui::Slider::new(&mut dialog.edge_low, 0.0..=255.0).text("Low Threshold"))
                .changed();
            dialog.edge_low = dialog.edge_low.min(dialog.edge_high);
            c |= ui
                .add(egui::Slider::new(&mut dialog.edge_high, 0.0..=255.0).text("High Threshold"))
                .changed();
            dialog.edge_high = dialog.edge_high.max(dialog.edge_low);
        }
    }

    c |= ui
        .horizontal(|ui| {
            ui.label("Stroke Color");
            egui::color_picker::color_edit_button_srgba(
                ui,
                &mut dialog.stroke_color,
                egui::color_picker::Alpha::Opaque,
            )
            .changed()
        })
        .inner;
    c |= ui
        .add(
            egui::Slider::new(&mut dialog.stroke_width, 0.5..=20.0)
                .step_by(0.1)
                .text("Stroke Width"),
        )
        .changed();
    c |= ui
        .horizontal(|ui| {
            let changed = ui.checkbox(&mut dialog.svg_fill, "Fill").changed();
            if dialog.svg_fill {
                changed
                    | egui::color_picker::color_edit_button_srgba(
                        ui,
                        &mut dialog.svg_fill_color,
                        egui::color_picker::Alpha::Opaque,
                    )
                    .changed()
            } else {
                changed
            }
        })
        .inner;
    c |= ui
        .horizontal(|ui| {
            ui.selectable_value(&mut dialog.background, SvgBackground::Transparent, "None")
                .changed()
                | ui.selectable_value(&mut dialog.background, SvgBackground::LightGray, "Gray")
                    .changed()
                | ui.selectable_value(&mut dialog.background, SvgBackground::White, "White")
                    .changed()
        })
        .inner;
    c |= ui
        .add(egui::Slider::new(&mut dialog.filter_speckle, 0..=64).text("Noise Filter (px)"))
        .changed();
    c |= ui
        .add(egui::Slider::new(&mut dialog.post_smooth, 0u8..=5u8).text("Post Smooth"))
        .changed();
    c
}

fn show_shared_fill_settings(
    ui: &mut Ui,
    dialog: &mut crate::state::VectorizeSettings,
    show_splice: bool,
) -> bool {
    let label = if show_splice {
        "Color Count"
    } else {
        "Tone Levels"
    };
    let mut c = false;
    c |= ui
        .add(egui::Slider::new(&mut dialog.color_count, 2..=32).text(label))
        .changed();
    c |= ui
        .add(egui::Slider::new(&mut dialog.path_precision, 1..=10).text("Path Smoothing"))
        .changed();
    c |= ui
        .add(egui::Slider::new(&mut dialog.corner_threshold, 0.0..=180.0).text("Corner Threshold°"))
        .changed();
    c |= ui
        .add(egui::Slider::new(&mut dialog.filter_speckle, 0..=64).text("Noise Filter (px)"))
        .changed();
    c |= ui
        .add(egui::Slider::new(&mut dialog.segment_length, 0.0..=20.0).text("Min Segment (px)"))
        .changed();
    if show_splice {
        c |= ui
            .add(egui::Slider::new(&mut dialog.splice_threshold, 0..=180).text("Splice Threshold°"))
            .changed();
    }
    c
}

/// Returns the SVG to emit, dispatching through the selected emit mode.
fn build_final_svg(state: &AppState) -> Option<String> {
    use crate::state::VectorizeEmitMode;
    let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), state.canvas.canvas_size);
    let panel = &state.canvas.vectorize_panel;
    let cached_trace =
        || {
            if !panel.svg_paths.is_empty() || !panel.svg_header.is_empty() {
                Some(crate::vectorize::assemble_svg(
                    &panel.svg_header,
                    &panel.svg_paths,
                    &panel.svg_footer,
                ))
            } else {
                state.canvas.color_image.as_ref().and_then(|ci| {
                    crate::vectorize::convert_to_svg(ci, &state.settings.vectorize).ok()
                })
            }
        };
    match state.settings.vectorize.emit_mode {
        VectorizeEmitMode::TraceImage => cached_trace(),
        VectorizeEmitMode::Annotations => {
            Some(crate::svg_emit::annotations_to_svg(&state.canvas, viewport))
        }
        VectorizeEmitMode::Combined => {
            let trace = cached_trace()?;
            Some(crate::svg_emit::combine_trace_and_annotations(
                &trace,
                &state.canvas,
                viewport,
            ))
        }
    }
}

fn do_save_svg(state: &mut AppState) {
    let Some(svg) = build_final_svg(state) else {
        state.status_msg = Some("SVG generation failed".into());
        return;
    };
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("SVG", &["svg"])
        .set_file_name("output.svg")
        .save_file()
    {
        match std::fs::write(&path, svg.as_bytes()) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "output.svg".into());
                state.status_msg = Some(format!("SVG saved to {name}"));
            }
            Err(e) => state.status_msg = Some(format!("Failed to save SVG: {e}")),
        }
    }
}

// ── Annotation kind label ─────────────────────────────────────────────────────

pub fn annotation_kind_label(shape: &crate::types::ShapeData) -> &'static str {
    use crate::types::ShapeData;
    match shape {
        ShapeData::Rect { .. } => "Rect",
        ShapeData::Oval { .. } => "Oval",
        ShapeData::Star { .. } => "Star",
        ShapeData::Arrow { .. } => "Arrow",
        ShapeData::Line { .. } => "Line",
        ShapeData::Text { .. } => "Text",
        ShapeData::Question { .. } => "Question",
        ShapeData::Pen { .. } => "Pen",
        ShapeData::Blur { .. } => "Blur",
        ShapeData::FocusRegion { .. } => "FocusRegion",
        ShapeData::LogicTag { .. } => "LogicTag",
        ShapeData::StepMarker { .. } => "Step",
        ShapeData::ImageLayer { .. } => "Image",
        ShapeData::Callout { .. } => "Callout",
        ShapeData::Ruler { .. } => "Ruler",
        ShapeData::BezierPath { .. } => "Bezier",
    }
}

// ── Duplicate annotation ──────────────────────────────────────────────────────

pub fn duplicate_selected(state: &mut AppState, id: uuid::Uuid) {
    let Some(ann) = state.canvas.annotations.iter().find(|a| a.id == id) else {
        return;
    };
    let mut new_ann = ann.clone();
    new_ann.id = uuid::Uuid::new_v4();
    new_ann.shape.translate(egui::Vec2::new(10.0, 10.0));
    let new_id = new_ann.id;
    CommandExecutor::execute(
        &mut state.canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(new_ann),
    );
    state.set_current_tool(Tool::Select);
    state.tool_state = ToolState::Selected {
        id: new_id,
        drag_offset: None,
    };
}

// ── Layers panel ──────────────────────────────────────────────────────────────

/// Deferred side-effects collected during the immutable Layers-panel walk and
/// applied once at the end (so the walk can borrow `state` immutably).
#[derive(Default)]
struct LayerPanelActions {
    cmd: Option<Command>,
    select_ids: Option<Vec<uuid::Uuid>>,
    set_active: Option<uuid::Uuid>,
    /// Begin renaming `id`, seeding the edit buffer with the current name.
    begin_rename: Option<(uuid::Uuid, String)>,
    /// Commit the in-progress rename to this name.
    commit_rename: Option<(uuid::Uuid, String)>,
    /// Cancel the in-progress rename.
    cancel_rename: bool,
    /// (node_id, parent_id, row top-y) for every drawn row this frame, in draw order.
    row_geom: Vec<(uuid::Uuid, uuid::Uuid, f32)>,
    /// A drag started on this node id at this pointer y (frame-local).
    drag_begin: Option<(uuid::Uuid, f32)>,
    /// Latest pointer y while dragging this node id (frame-local).
    drag_active: Option<(uuid::Uuid, f32)>,
    /// The primary pointer button was released somewhere this frame.
    pointer_released: bool,
}

/// Leaf ids implied by the current selection (`Selected` → one; `MultiSelected`
/// → all). `None` when nothing is selected.
fn selected_leaf_ids(state: &AppState) -> Option<Vec<uuid::Uuid>> {
    match &state.tool_state {
        ToolState::Selected { id, .. } => Some(vec![*id]),
        ToolState::MultiSelected { ids, .. } if !ids.is_empty() => Some(ids.clone()),
        _ => None,
    }
}

/// The common Group parent of the selected leaves, if they all share one Group.
fn selected_group_id(state: &AppState) -> Option<uuid::Uuid> {
    let ids = selected_leaf_ids(state)?;
    let doc = &state.canvas.document;
    let mut parent: Option<uuid::Uuid> = None;
    for id in &ids {
        let p = doc.get(*id)?.parent?;
        if !matches!(doc.get(p)?.kind, crate::scene::NodeKind::Group) {
            return None;
        }
        match parent {
            Some(existing) if existing != p => return None,
            _ => parent = Some(p),
        }
    }
    parent
}

/// Record drag begin/active + row geometry for a layers-panel row so the
/// controller can resolve a same-parent reorder after the tree is drawn.
fn track_row_drag(
    ui: &egui::Ui,
    state: &AppState,
    label: &egui::Response,
    node_id: uuid::Uuid,
    act: &mut LayerPanelActions,
) {
    let Some(parent) = state.canvas.document.get(node_id).and_then(|n| n.parent) else {
        return;
    };
    act.row_geom.push((node_id, parent, ui.min_rect().top()));
    let drag = label.interact(egui::Sense::drag());
    if drag.drag_started() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            act.drag_begin = Some((node_id, p.y));
        }
    }
    if drag.dragged() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            act.drag_active = Some((node_id, p.y));
        }
        ui.ctx().request_repaint();
    }
}

pub fn show_layers_panel(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    side_panel_heading(ui, "Layers", &theme);
    ui.separator();

    let selected_id = if let ToolState::Selected { id, .. } = state.tool_state {
        Some(id)
    } else {
        None
    };
    let active_layer = state.canvas.active_layer;
    let mut act = LayerPanelActions {
        pointer_released: ui.input(|i| i.pointer.any_released()),
        ..Default::default()
    };

    // ── Action row ──
    ui.horizontal(|ui| {
        if ui.button("➕").on_hover_text("New layer").clicked() {
            let n = state.canvas.document.layers().len() + 1;
            act.cmd = Some(Command::CreateLayer {
                id: uuid::Uuid::new_v4(),
                name: format!("Layer {n}"),
            });
        }
        if ui
            .button("🗑")
            .on_hover_text("Delete active layer")
            .clicked()
        {
            // Never leave zero layers.
            if state.canvas.document.layers().len() > 1 {
                act.cmd = Some(Command::DeleteLayer { id: active_layer });
            }
        }
        if ui.button("⊟").on_hover_text("Group selection").clicked() {
            if let Some(ids) = selected_leaf_ids(state) {
                if !ids.is_empty() {
                    act.cmd = Some(Command::Group {
                        ids,
                        name: "Group".to_string(),
                    });
                }
            }
        }
        if ui.button("⊞").on_hover_text("Ungroup").clicked() {
            if let Some(gid) = selected_group_id(state) {
                act.cmd = Some(Command::Ungroup { group_id: gid });
            }
        }
        if ui
            .button("⬇")
            .on_hover_text("Flatten all layers into one image")
            .clicked()
        {
            act.cmd = Some(Command::FlattenToImage);
        }
    });

    // ── Move selected → [layer ▾] ──
    if let Some(ids) = selected_leaf_ids(state) {
        if !ids.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Move →").small().color(theme.text_muted));
                egui::ComboBox::from_id_salt("move_to_layer")
                    .selected_text("layer")
                    .show_ui(ui, |ui| {
                        for lid in state.canvas.document.layers() {
                            let name = state
                                .canvas
                                .document
                                .get(lid)
                                .map(|n| n.name.clone())
                                .unwrap_or_default();
                            if ui.selectable_label(false, name).clicked() {
                                let moves: Vec<Command> = ids
                                    .iter()
                                    .map(|id| Command::MoveNodeToLayer {
                                        id: *id,
                                        layer_id: lid,
                                        index: usize::MAX,
                                    })
                                    .collect();
                                act.cmd = Some(Command::Batch(moves));
                            }
                        }
                    });
            });
        }
    }
    ui.separator();

    // ── Tree ──
    egui::ScrollArea::vertical().show(ui, |ui| {
        // Layers paint bottom→top; show top-most first.
        let layers: Vec<uuid::Uuid> = state.canvas.document.layers();
        for layer_id in layers.iter().rev().copied() {
            layer_row(
                ui,
                state,
                &theme,
                layer_id,
                active_layer,
                selected_id,
                &mut act,
            );
        }
    });

    apply_layer_panel_actions(state, act);
}

/// Apply the deferred Layers-panel side-effects (single mutation point).
fn apply_layer_panel_actions(state: &mut AppState, act: LayerPanelActions) {
    if act.cancel_rename {
        state.layer_rename = None;
    }
    if let Some((id, name)) = act.begin_rename {
        state.layer_rename = Some((id, name));
    }
    let mut cmd = act.cmd;
    if let Some((id, name)) = act.commit_rename {
        state.layer_rename = None;
        if !name.trim().is_empty() {
            cmd = Some(Command::SetNodeProp {
                id,
                prop: NodeProp::Name(name),
            });
        }
    }
    if let Some(c) = cmd {
        CommandExecutor::execute(&mut state.canvas, ParticipantId::HUMAN_LOCAL, c);
        // active_layer may now be stale (deleted) — clamp.
        if state
            .canvas
            .document
            .get(state.canvas.active_layer)
            .is_none()
        {
            state.canvas.active_layer = state.canvas.document.default_layer();
        }
    }
    if let Some(id) = act.set_active {
        state.canvas.active_layer = id;
    }
    if let Some(ids) = act.select_ids {
        state.set_current_tool(Tool::Select);
        state.tool_state = match ids.as_slice() {
            [one] => ToolState::Selected {
                id: *one,
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

    // Drag-to-reorder (same parent only).
    if let Some((id, y)) = act.drag_begin {
        state.layer_drag = Some(crate::state::LayerDrag { id, y });
    }
    if let Some((id, y)) = act.drag_active {
        if let Some(d) = state.layer_drag.as_mut() {
            if d.id == id {
                d.y = y;
            }
        }
    }
    if act.pointer_released {
        if let Some(d) = state.layer_drag.take() {
            if let Some(parent) = state.canvas.document.get(d.id).and_then(|n| n.parent) {
                // Same-parent sibling rows in draw order.
                let sib_ids: Vec<uuid::Uuid> = act
                    .row_geom
                    .iter()
                    .filter(|(_, p, _)| *p == parent)
                    .map(|(id, _, _)| *id)
                    .collect();
                let tops: Vec<f32> = act
                    .row_geom
                    .iter()
                    .filter(|(_, p, _)| *p == parent)
                    .map(|(_, _, top)| *top)
                    .collect();
                if !sib_ids.is_empty() {
                    let slot = index_from_y(&tops, d.y).min(sib_ids.len() - 1);
                    let target_id = sib_ids[slot];
                    if target_id != d.id {
                        if let Some(target_index) = state.canvas.document.child_index(target_id) {
                            CommandExecutor::execute(
                                &mut state.canvas,
                                ParticipantId::HUMAN_LOCAL,
                                Command::ReorderNode {
                                    id: d.id,
                                    index: target_index,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
}

/// An eye + lock toggle pair for a node. Emits SetNodeProp on click.
fn visibility_lock_toggles(
    ui: &mut Ui,
    theme: &Theme,
    node_id: uuid::Uuid,
    visible: bool,
    locked: bool,
    act: &mut LayerPanelActions,
) {
    let eye = if visible { "👁" } else { "🙈" };
    let eye_color = if visible {
        theme.text_muted
    } else {
        theme.text_dim
    };
    if ui
        .add(
            egui::Button::new(RichText::new(eye).color(eye_color).small())
                .frame(false)
                .min_size(egui::vec2(20.0, 18.0)),
        )
        .on_hover_text(if visible { "Hide" } else { "Show" })
        .clicked()
    {
        act.cmd = Some(Command::SetNodeProp {
            id: node_id,
            prop: NodeProp::Visible(!visible),
        });
    }
    let lock = if locked { "🔒" } else { "🔓" };
    if ui
        .add(
            egui::Button::new(RichText::new(lock).color(theme.text_muted).small())
                .frame(false)
                .min_size(egui::vec2(20.0, 18.0)),
        )
        .on_hover_text(if locked { "Unlock" } else { "Lock" })
        .clicked()
    {
        act.cmd = Some(Command::SetNodeProp {
            id: node_id,
            prop: NodeProp::Locked(!locked),
        });
    }
}

/// A clickable name label OR, when this node is being renamed, an inline editor.
fn name_label_or_editor(
    ui: &mut Ui,
    state: &AppState,
    color: Color32,
    node_id: uuid::Uuid,
    name: &str,
    strong: bool,
    act: &mut LayerPanelActions,
) -> egui::Response {
    if let Some((rid, buf)) = &state.layer_rename {
        if *rid == node_id {
            let mut text = buf.clone();
            let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(110.0));
            resp.request_focus();
            if resp.changed() {
                // Keep the buffer in sync across frames.
                act.begin_rename = Some((node_id, text.clone()));
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                act.commit_rename = Some((node_id, text));
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                act.cancel_rename = true;
            }
            return resp;
        }
    }
    let rt = if strong {
        RichText::new(name).strong().color(color)
    } else {
        RichText::new(name).small().color(color)
    };
    ui.add(egui::Label::new(rt).sense(egui::Sense::click()))
}

/// Decide which delete command a panel row should emit, given whether the row is
/// a Layer and how many layers exist. Returns None for the last remaining layer.
fn row_delete_command_for(
    id: uuid::Uuid,
    is_layer: bool,
    layer_count: usize,
) -> Option<crate::types::Command> {
    if is_layer {
        if layer_count > 1 {
            Some(crate::types::Command::DeleteLayer { id })
        } else {
            None
        }
    } else {
        Some(crate::types::Command::Delete(id))
    }
}

#[allow(clippy::too_many_arguments)]
fn layer_row(
    ui: &mut Ui,
    state: &AppState,
    theme: &Theme,
    layer_id: uuid::Uuid,
    active_layer: uuid::Uuid,
    selected_id: Option<uuid::Uuid>,
    act: &mut LayerPanelActions,
) {
    let Some(node) = state.canvas.document.get(layer_id) else {
        return;
    };
    let is_active = layer_id == active_layer;
    let name = node.name.clone();
    let visible = node.visible;
    let locked = node.locked;
    let opacity = node.opacity;
    let children: Vec<uuid::Uuid> = node.children.clone();

    let frame = egui::Frame::none()
        .fill(if is_active {
            theme.layer_selected_bg
        } else {
            Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(4.0, 2.0));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            visibility_lock_toggles(ui, theme, layer_id, visible, locked, act);
            let color = if is_active {
                theme.layer_selected_text
            } else {
                theme.text_label
            };
            let label = name_label_or_editor(ui, state, color, layer_id, &name, true, act);
            track_row_drag(ui, state, &label, layer_id, act);
            if label.clicked() {
                act.set_active = Some(layer_id);
            }
            if label.double_clicked() {
                act.begin_rename = Some((layer_id, name.clone()));
            }
            let mut op = opacity;
            if ui
                .add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(false))
                .on_hover_text("Layer opacity")
                .drag_stopped()
            {
                act.cmd = Some(Command::SetNodeProp {
                    id: layer_id,
                    prop: NodeProp::Opacity(op),
                });
            }
            let layer_count = state.canvas.document.layers().len();
            if ui
                .add_enabled(layer_count > 1, egui::Button::new("🗑").frame(false).small())
                .on_hover_text("Delete layer")
                .clicked()
            {
                act.cmd = row_delete_command_for(layer_id, true, layer_count);
            }
        });
    });
    ui.indent(layer_id, |ui| {
        for child in children {
            child_row(ui, state, theme, child, selected_id, act);
        }
    });
}

fn child_row(
    ui: &mut Ui,
    state: &AppState,
    theme: &Theme,
    node_id: uuid::Uuid,
    selected_id: Option<uuid::Uuid>,
    act: &mut LayerPanelActions,
) {
    let Some(node) = state.canvas.document.get(node_id) else {
        return;
    };
    let name = node.name.clone();
    let visible = node.visible;
    let locked = node.locked;
    match &node.kind {
        crate::scene::NodeKind::Group => {
            let children: Vec<uuid::Uuid> = node.children.clone();
            ui.horizontal(|ui| {
                visibility_lock_toggles(ui, theme, node_id, visible, locked, act);
                let label =
                    name_label_or_editor(ui, state, theme.text_muted, node_id, &name, false, act);
                track_row_drag(ui, state, &label, node_id, act);
                if label.clicked() {
                    // Selecting a group selects all its descendant leaves.
                    act.select_ids = Some(state.canvas.document.descendant_leaf_ids(node_id));
                }
                if label.double_clicked() {
                    act.begin_rename = Some((node_id, name.clone()));
                }
                if ui
                    .add(egui::Button::new("🗑").frame(false).small())
                    .on_hover_text("Delete")
                    .clicked()
                {
                    act.cmd = row_delete_command_for(node_id, false, 0);
                }
            });
            ui.indent(node_id, |ui| {
                for c in children {
                    child_row(ui, state, theme, c, selected_id, act);
                }
            });
        }
        crate::scene::NodeKind::Shape(_) => {
            let is_selected = selected_id == Some(node_id);
            let color = if is_selected {
                theme.layer_selected_text
            } else if visible {
                theme.text_muted
            } else {
                theme.text_dim
            };
            ui.horizontal(|ui| {
                visibility_lock_toggles(ui, theme, node_id, visible, locked, act);
                let label = name_label_or_editor(ui, state, color, node_id, &name, false, act);
                track_row_drag(ui, state, &label, node_id, act);
                if label.clicked() {
                    act.select_ids = Some(vec![node_id]);
                }
                if label.double_clicked() {
                    act.begin_rename = Some((node_id, name.clone()));
                }
                if ui
                    .add(egui::Button::new("🗑").frame(false).small())
                    .on_hover_text("Delete")
                    .clicked()
                {
                    act.cmd = row_delete_command_for(node_id, false, 0);
                }
            });
        }
        crate::scene::NodeKind::Layer => {} // layers are never nested under another node
    }
}

// ── Transform panel ───────────────────────────────────────────────────────────

/// Return the pivot point on `bbox` corresponding to the 9-grid index
/// (row*3+col, row/col ∈ {0,1,2}): 0=TL, 4=center, 8=BR.
pub fn ref_point_on_bbox(bbox: egui::Rect, idx: u8) -> egui::Pos2 {
    let row = idx / 3;
    let col = idx % 3;
    let x = match col {
        0 => bbox.min.x,
        1 => bbox.center().x,
        _ => bbox.max.x,
    };
    let y = match row {
        0 => bbox.min.y,
        1 => bbox.center().y,
        _ => bbox.max.y,
    };
    egui::pos2(x, y)
}

/// A pinned panel showing X/Y/W/H/angle/skew for the current selection
/// (`View ▸ Transform`). Edits dispatch `SetNodeProp{Transform}` commands.
pub fn show_transform_panel(ui: &mut Ui, state: &mut AppState) {
    let theme = state.settings.theme();
    side_panel_heading(ui, "Transform", &theme);
    ui.separator();

    // ── Gather selection ─────────────────────────────────────────────────────
    let sel_ids: Vec<Uuid> = match &state.tool_state {
        ToolState::Selected { id, .. } => vec![*id],
        ToolState::MultiSelected { ids, .. } => ids.clone(),
        _ => Vec::new(),
    };

    if sel_ids.is_empty() {
        ui.label(
            RichText::new("No selection")
                .small()
                .color(theme.text_muted),
        );
        return;
    }

    let doc = &state.canvas.document;

    // Build (local_bbox, world) pairs for all selected leaves.
    let items: Vec<(egui::Rect, kurbo::Affine)> = sel_ids
        .iter()
        .filter_map(|id| {
            let leaf = doc.leaf(*id)?;
            Some((leaf.shape.bounding_rect(), doc.world_transform(*id)))
        })
        .collect();

    let bbox = match crate::transform::selection_world_bbox(&items) {
        Some(b) => b,
        None => {
            ui.label(RichText::new("No geometry").small().color(theme.text_muted));
            return;
        }
    };

    let is_single = sel_ids.len() == 1;

    // Decompose the single node's local transform for angle/skew display.
    let decomposed_single: Option<crate::transform::Decomposed> = if is_single {
        let id = sel_ids[0];
        doc.get(id)
            .map(|n| crate::transform::decompose(n.transform))
    } else {
        None
    };

    // ── Reference-point 3×3 picker ───────────────────────────────────────────
    ui.label(
        RichText::new("Reference point")
            .small()
            .color(theme.text_muted),
    );
    egui::Grid::new("transform_ref_grid")
        .num_columns(3)
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for row in 0u8..3 {
                for col in 0u8..3 {
                    let idx = row * 3 + col;
                    let selected = state.settings.transform_ref_point == idx;
                    let symbol = if selected { "●" } else { "○" };
                    if ui
                        .selectable_label(selected, symbol)
                        .on_hover_text(ref_point_label(idx))
                        .clicked()
                    {
                        state.settings.transform_ref_point = idx;
                    }
                }
                ui.end_row();
            }
        });

    ui.separator();

    // Pivot in world space for this frame.
    let ref_idx = state.settings.transform_ref_point;
    let pivot = ref_point_on_bbox(bbox, ref_idx);

    // ── Editable fields ───────────────────────────────────────────────────────
    // We drive `f64` edit buffers from the current bbox/decomposed values and
    // dispatch a command only when the user commits (drag_stopped | lost_focus).

    let mut x_val = bbox.min.x as f64;
    let mut y_val = bbox.min.y as f64;
    let mut w_val = bbox.width() as f64;
    let mut h_val = bbox.height() as f64;

    // Angle/skew: single-selection only; shown in degrees.
    let mut angle_deg: f64 = decomposed_single.map_or(0.0, |d| d.rotation.to_degrees());
    let mut skew_deg: f64 = decomposed_single.map_or(0.0, |d| d.skew.to_degrees());

    // Track whether any field was committed this frame.
    let mut cmd: Option<Command> = None;

    egui::Grid::new("transform_fields")
        .num_columns(2)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            // X
            ui.label(RichText::new("X").color(theme.text_muted));
            let rx = ui.add(egui::DragValue::new(&mut x_val).speed(0.5).suffix(" px"));
            ui.end_row();

            // Y
            ui.label(RichText::new("Y").color(theme.text_muted));
            let ry = ui.add(egui::DragValue::new(&mut y_val).speed(0.5).suffix(" px"));
            ui.end_row();

            // W
            ui.label(RichText::new("W").color(theme.text_muted));
            let rw = ui.add(
                egui::DragValue::new(&mut w_val)
                    .speed(0.5)
                    .suffix(" px")
                    .range(0.1..=f64::MAX),
            );
            ui.end_row();

            // H
            ui.label(RichText::new("H").color(theme.text_muted));
            let rh = ui.add(
                egui::DragValue::new(&mut h_val)
                    .speed(0.5)
                    .suffix(" px")
                    .range(0.1..=f64::MAX),
            );
            ui.end_row();

            // Angle (single only)
            ui.label(RichText::new("∠").color(theme.text_muted));
            let ra = ui.add_enabled(
                is_single,
                egui::DragValue::new(&mut angle_deg).speed(0.1).suffix("°"),
            );
            ui.end_row();

            // Skew (single only)
            ui.label(RichText::new("Skew").color(theme.text_muted));
            let rs = ui.add_enabled(
                is_single,
                egui::DragValue::new(&mut skew_deg).speed(0.1).suffix("°"),
            );
            ui.end_row();

            // ── Commit logic ──────────────────────────────────────────────────
            let commit_x = rx.drag_stopped() || rx.lost_focus();
            let commit_y = ry.drag_stopped() || ry.lost_focus();
            let commit_w = rw.drag_stopped() || rw.lost_focus();
            let commit_h = rh.drag_stopped() || rh.lost_focus();
            let commit_a = is_single && (ra.drag_stopped() || ra.lost_focus());
            let commit_s = is_single && (rs.drag_stopped() || rs.lost_focus());

            if commit_x || commit_y {
                // X/Y: translate so bbox.min moves to (x_val, y_val).
                let dx = x_val - bbox.min.x as f64;
                let dy = y_val - bbox.min.y as f64;
                if dx.abs() > 1e-6 || dy.abs() > 1e-6 {
                    let delta = kurbo::Affine::translate((dx, dy));
                    cmd = Some(build_transform_cmd(&state.canvas, &sel_ids, delta));
                }
            } else if commit_w || commit_h {
                // W/H: scale about the chosen pivot so the bbox reaches the new size.
                let old_w = bbox.width() as f64;
                let old_h = bbox.height() as f64;
                let sx = if old_w > 1e-6 { w_val / old_w } else { 1.0 };
                let sy = if old_h > 1e-6 { h_val / old_h } else { 1.0 };
                if (sx - 1.0).abs() > 1e-6 || (sy - 1.0).abs() > 1e-6 {
                    let delta = crate::transform::scale_about(pivot, sx, sy);
                    cmd = Some(build_transform_cmd(&state.canvas, &sel_ids, delta));
                }
            } else if commit_a {
                if let Some(d) = decomposed_single {
                    let new_rot = angle_deg.to_radians();
                    let drot = new_rot - d.rotation;
                    if drot.abs() > 1e-9 {
                        let delta = crate::transform::rotate_about(pivot, drot);
                        cmd = Some(build_transform_cmd(&state.canvas, &sel_ids, delta));
                    }
                }
            } else if commit_s {
                if let Some(d) = decomposed_single {
                    let new_skew = skew_deg.to_radians();
                    let dskew = new_skew - d.skew;
                    if dskew.abs() > 1e-9 {
                        let delta = crate::transform::skew_about(pivot, dskew, 0.0);
                        cmd = Some(build_transform_cmd(&state.canvas, &sel_ids, delta));
                    }
                }
            }
        });

    if let Some(cmd) = cmd {
        CommandExecutor::execute(&mut state.canvas, ParticipantId::HUMAN_LOCAL, cmd);
    }
}

/// Human-readable label for each of the 9 reference-point grid cells.
fn ref_point_label(idx: u8) -> &'static str {
    match idx {
        0 => "Top-left",
        1 => "Top-center",
        2 => "Top-right",
        3 => "Middle-left",
        4 => "Center",
        5 => "Middle-right",
        6 => "Bottom-left",
        7 => "Bottom-center",
        8 => "Bottom-right",
        _ => "?",
    }
}

/// Build a single `SetNodeProp` (one node) or `Batch` (multiple) applying
/// `delta_world` to every node in `ids` through `apply_world_delta`.
fn build_transform_cmd(
    canvas: &crate::state::CanvasState,
    ids: &[Uuid],
    delta_world: kurbo::Affine,
) -> Command {
    let doc = &canvas.document;
    let cmds: Vec<Command> = ids
        .iter()
        .filter_map(|id| {
            let node = doc.get(*id)?;
            let pw = crate::tools::select::parent_world(doc, *id);
            let new_local = crate::transform::apply_world_delta(pw, node.transform, delta_world);
            Some(Command::SetNodeProp {
                id: *id,
                prop: NodeProp::Transform(new_local),
            })
        })
        .collect();
    if cmds.len() == 1 {
        cmds.into_iter().next().unwrap()
    } else {
        Command::Batch(cmds)
    }
}

#[cfg(test)]
mod transform_panel_tests {
    use super::ref_point_on_bbox;
    use egui::{pos2, Rect};

    fn bbox() -> Rect {
        Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 80.0))
    }

    #[test]
    fn ref_point_corners() {
        let b = bbox();
        // Top-left
        assert_eq!(ref_point_on_bbox(b, 0), pos2(10.0, 20.0));
        // Top-right
        assert_eq!(ref_point_on_bbox(b, 2), pos2(110.0, 20.0));
        // Bottom-left
        assert_eq!(ref_point_on_bbox(b, 6), pos2(10.0, 80.0));
        // Bottom-right
        assert_eq!(ref_point_on_bbox(b, 8), pos2(110.0, 80.0));
    }

    #[test]
    fn ref_point_center() {
        let b = bbox();
        assert_eq!(ref_point_on_bbox(b, 4), b.center());
    }

    #[test]
    fn ref_point_edge_midpoints() {
        let b = bbox();
        // Top-center
        assert_eq!(ref_point_on_bbox(b, 1), pos2(60.0, 20.0));
        // Middle-left
        assert_eq!(ref_point_on_bbox(b, 3), pos2(10.0, 50.0));
        // Middle-right
        assert_eq!(ref_point_on_bbox(b, 5), pos2(110.0, 50.0));
        // Bottom-center
        assert_eq!(ref_point_on_bbox(b, 7), pos2(60.0, 80.0));
    }
}

#[cfg(test)]
mod save_preserve_tests {
    use super::*;
    use crate::types::{Annotation, ShapeData};
    use egui::{ColorImage, Pos2, Rect, Vec2};

    fn undo_len(s: &AppState) -> usize {
        s.canvas
            .undo_stacks
            .get(&ParticipantId::HUMAN_LOCAL)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Characterization test: saving overwrites the source file but must NOT
    /// clear the undo history — so undo/redo keep working after a Save (e.g. in
    /// the Native Inpaint tab). Guards against a regression where Save would
    /// wipe the command stack.
    #[test]
    fn save_preserves_undo_history() {
        let mut state = AppState::default();

        // A tiny 4x4 white base image so the save has something to write.
        state.canvas.color_image = Some(ColorImage {
            size: [4, 4],
            pixels: vec![Color32::WHITE; 16],
        });
        state.canvas.canvas_size = Vec2::new(4.0, 4.0);

        // Commit one annotation through the command system (like an inpaint
        // result): the human undo stack now holds one entry.
        let ann = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(2.0, 2.0))),
            Color32::RED,
            2.0,
        );
        CommandExecutor::execute(
            &mut state.canvas,
            ParticipantId::HUMAN_LOCAL,
            Command::Add(ann),
        );
        assert_eq!(undo_len(&state), 1, "precondition: one undoable edit");

        // Save directly to a temp file (source_path is a writable raster, so
        // save_image takes the overwrite path — no dialog).
        let path = std::env::temp_dir().join("redline_save_preserve_undo.png");
        let _ = std::fs::remove_file(&path);
        state.canvas.source_path = Some(path.clone());

        save_image(&mut state);

        // The file was written...
        assert!(path.exists(), "save wrote the output file");
        // ...and the undo history SURVIVED the save (the whole point).
        assert_eq!(
            undo_len(&state),
            1,
            "save must preserve undo history, not clear it"
        );

        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod layer_drag_tests {
    use super::index_from_y;

    #[test]
    fn index_from_y_maps_pointer_to_slot() {
        // Three rows with tops at 0, 20, 40 (each ~20 tall).
        let tops = [0.0_f32, 20.0, 40.0];
        assert_eq!(index_from_y(&tops, -5.0), 0); // above first → slot 0
        assert_eq!(index_from_y(&tops, 5.0), 0); // within row 0 → slot 0
        assert_eq!(index_from_y(&tops, 25.0), 1); // within row 1 → slot 1
        assert_eq!(index_from_y(&tops, 100.0), 2); // below last → last slot
    }
}

#[cfg(test)]
mod layer_panel_delete_tests {
    use super::row_delete_command_for;

    #[test]
    fn row_delete_command_picks_right_command() {
        use crate::types::Command;
        let id = uuid::Uuid::new_v4();
        // A layer, when more than one layer exists → DeleteLayer.
        match row_delete_command_for(id, true, 2) {
            Some(Command::DeleteLayer { id: i }) if i == id => {}
            other => panic!("expected DeleteLayer, got {other:?}"),
        }
        // The last remaining layer → None (never delete the last layer).
        assert!(row_delete_command_for(id, true, 1).is_none());
        // A non-layer row (leaf/group) → Delete.
        match row_delete_command_for(id, false, 3) {
            Some(Command::Delete(i)) if i == id => {}
            other => panic!("expected Delete, got {other:?}"),
        }
    }
}
