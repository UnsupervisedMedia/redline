use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use super::{
    process,
    state::{InstanceStatus, LamaCleanerState, LamaInstance, SetupStatus},
};

// ── Colour palette ────────────────────────────────────────────────────────────

const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
const RED: Color32 = Color32::from_rgb(255, 69, 58);
const YELLOW: Color32 = Color32::from_rgb(255, 214, 10);
const BLUE: Color32 = Color32::from_rgb(10, 132, 255);
const GRAY: Color32 = Color32::from_rgb(120, 120, 128);

fn status_color(status: &InstanceStatus) -> Color32 {
    match status {
        InstanceStatus::Running => GREEN,
        InstanceStatus::Stopped => GRAY,
        InstanceStatus::Error(_) => RED,
        InstanceStatus::Starting | InstanceStatus::Restarting => YELLOW,
        InstanceStatus::Stopping => YELLOW,
    }
}

enum InstanceAction {
    None,
    OpenEmbedded(usize),
    CloseEmbedded,
}

// ── Top-level entry point ─────────────────────────────────────────────────────

pub fn show(ctx: &egui::Context, state: &mut LamaCleanerState) {
    egui::TopBottomPanel::top("lama_tab_bar")
        .frame(
            egui::Frame::none()
                .fill(Color32::from_gray(28))
                .inner_margin(egui::Margin::symmetric(8.0, 4.0)),
        )
        .show(ctx, |ui| show_tab_bar(ui, state));

    let tab = state
        .active_tab
        .min(state.instances.len().saturating_sub(1));
    state.active_tab = tab;

    let show_embedded = state
        .instances
        .get(tab)
        .map(|i| i.show_embedded)
        .unwrap_or(false);

    if show_embedded {
        let action = show_split_layout(ctx, state);
        apply_action(state, action);
    } else {
        let action = egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_gray(18)))
            .show(ctx, |ui| {
                // Setup banner takes priority over instance UI.
                if !state.setup.is_ready() {
                    show_setup_panel(ui, state, ctx);
                    return InstanceAction::None;
                }
                egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        if state.instances.is_empty() {
                            show_empty(ui, state);
                            InstanceAction::None
                        } else {
                            let tab = state.active_tab;
                            let tx = state.tx.clone();
                            show_instance(ui, &mut state.instances[tab], tx, ctx)
                        }
                    })
                    .inner
            })
            .inner;
        apply_action(state, action);
    }
}

fn apply_action(state: &mut LamaCleanerState, action: InstanceAction) {
    match action {
        InstanceAction::None => {}
        InstanceAction::OpenEmbedded(id) => state.open_webview_for = Some(id),
        InstanceAction::CloseEmbedded => state.close_webview = true,
    }
}

// ── Setup panel ───────────────────────────────────────────────────────────────

fn show_setup_panel(ui: &mut egui::Ui, state: &mut LamaCleanerState, ctx: &egui::Context) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(560.0);

        match &state.setup.clone() {
            SetupStatus::Unknown | SetupStatus::Checking => {
                ui.spinner();
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Checking for lama-cleaner…")
                        .size(14.0)
                        .color(GRAY),
                );
            }

            SetupStatus::NeedsInstall => {
                ui.label(
                    RichText::new("lama-cleaner is not installed")
                        .size(18.0)
                        .color(Color32::WHITE),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Redline will create a self-contained Python environment and install\n\
                         lama-cleaner into it. No other software is required.",
                    )
                    .size(13.0)
                    .color(GRAY),
                );
                ui.add_space(16.0);
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("⬇  Install lama-cleaner")
                                .size(14.0)
                                .color(Color32::WHITE),
                        )
                        .min_size(Vec2::new(220.0, 40.0))
                        .fill(BLUE),
                    )
                    .clicked()
                {
                    state.setup_log.clear();
                    process::install(state.tx.clone());
                }
            }

            SetupStatus::Installing => {
                ui.label(
                    RichText::new("Installing lama-cleaner…")
                        .size(16.0)
                        .color(Color32::WHITE),
                );
                ui.add_space(6.0);
                ui.spinner();
                ui.add_space(12.0);

                // Scrollable install log
                egui::ScrollArea::vertical()
                    .id_salt("lama_install_log")
                    .max_height(260.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        egui::Frame::none()
                            .fill(Color32::from_gray(12))
                            .inner_margin(egui::Margin::same(8.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                ui.set_min_width(520.0);
                                for line in &state.setup_log {
                                    ui.label(
                                        RichText::new(line)
                                            .size(11.0)
                                            .monospace()
                                            .color(Color32::from_gray(210)),
                                    );
                                }
                            });
                    });

                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }

            SetupStatus::Error(e) => {
                let err = e.clone();
                ui.label(RichText::new("Installation failed").size(16.0).color(RED));
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(180, 30, 30, 40))
                    .inner_margin(egui::Margin::same(8.0))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(&err).size(12.0).color(RED).monospace());
                    });
                ui.add_space(12.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new("↺  Retry").size(13.0))
                            .min_size(Vec2::new(120.0, 32.0)),
                    )
                    .clicked()
                {
                    state.setup_log.clear();
                    process::install(state.tx.clone());
                }
            }

            SetupStatus::Ready => {} // handled by caller
        }
    });
}

// ── Split layout: sidebar + webview area ─────────────────────────────────────

fn show_split_layout(ctx: &egui::Context, state: &mut LamaCleanerState) -> InstanceAction {
    let mut action = InstanceAction::None;

    egui::SidePanel::left("lama_sidebar")
        .resizable(false)
        .exact_width(290.0)
        .frame(
            egui::Frame::none()
                .fill(Color32::from_gray(22))
                .inner_margin(egui::Margin::same(12.0)),
        )
        .show(ctx, |ui| {
            let tab = state.active_tab;
            let tx = state.tx.clone();
            egui::ScrollArea::vertical().show(ui, |ui| {
                action = show_instance_sidebar(ui, &mut state.instances[tab], tx, ctx);
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::from_gray(8)))
        .show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            state.webview_target_rect = Some(rect);
            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(8));
        });

    action
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

fn show_tab_bar(ui: &mut egui::Ui, state: &mut LamaCleanerState) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Lama-Cleaner")
                .size(13.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.separator();

        let n = state.instances.len();
        for i in 0..n {
            let active = state.active_tab == i;
            let running = state.instances[i].status.is_running();
            let dot = if running { "● " } else { "○ " };
            let label = state.instances[i].label.clone();

            let btn = ui.add(
                egui::Button::new(RichText::new(format!("{}{}", dot, label)).size(12.0).color(
                    if active {
                        Color32::WHITE
                    } else {
                        Color32::from_white_alpha(160)
                    },
                ))
                .selected(active)
                .min_size(Vec2::new(100.0, 22.0))
                .stroke(if active {
                    Stroke::new(1.0, BLUE)
                } else {
                    Stroke::NONE
                }),
            );
            if btn.clicked() {
                state.active_tab = i;
            }
        }

        let ready = state.setup.is_ready();
        if ui
            .add_enabled(
                ready,
                egui::Button::new(RichText::new("+  New Instance").size(12.0))
                    .min_size(Vec2::new(110.0, 22.0)),
            )
            .on_hover_text(if ready {
                "Add another instance"
            } else {
                "Install lama-cleaner first"
            })
            .clicked()
        {
            add_instance(state);
        }

        // Setup status chip on the right
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| match &state.setup {
                SetupStatus::Ready => {
                    ui.label(
                        RichText::new("● lama-cleaner ready")
                            .size(11.0)
                            .color(GREEN),
                    );
                }
                SetupStatus::Installing => {
                    ui.spinner();
                    ui.label(RichText::new("Installing…").size(11.0).color(YELLOW));
                }
                SetupStatus::Checking => {
                    ui.spinner();
                }
                SetupStatus::NeedsInstall => {
                    ui.label(RichText::new("⚠ Not installed").size(11.0).color(YELLOW));
                }
                SetupStatus::Error(_) => {
                    ui.label(RichText::new("✗ Install failed").size(11.0).color(RED));
                }
                SetupStatus::Unknown => {}
            },
        );
    });
}

fn add_instance(state: &mut LamaCleanerState) {
    let id = state.next_id;
    state.next_id += 1;
    let port = process::find_free_port().unwrap_or(9200 + id as u16);
    state.instances.push(LamaInstance::new(id, "", port));
    state.active_tab = state.instances.len() - 1;
}

// ── Empty-state panel ─────────────────────────────────────────────────────────

fn show_empty(ui: &mut egui::Ui, state: &mut LamaCleanerState) {
    ui.add_space(80.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("No Lama-Cleaner instances yet")
                .size(18.0)
                .color(Color32::from_white_alpha(180)),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new("Click \"+ New Instance\" to start one")
                .size(13.0)
                .color(GRAY),
        );
        ui.add_space(20.0);
        if ui
            .add(
                egui::Button::new(
                    RichText::new("+ New Instance")
                        .size(13.0)
                        .color(Color32::WHITE),
                )
                .min_size(Vec2::new(160.0, 36.0))
                .fill(BLUE),
            )
            .clicked()
        {
            add_instance(state);
        }

        ui.add_space(40.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "Lama-Cleaner removes unwanted objects from images using AI inpainting.\n\
                 Each instance runs as a local Python process — no Docker required.\n\
                 Run multiple instances to edit different folders simultaneously.",
            )
            .size(12.0)
            .color(GRAY),
        );
    });
}

// ── Per-instance panel (full layout) ─────────────────────────────────────────

fn show_instance(
    ui: &mut egui::Ui,
    inst: &mut LamaInstance,
    tx: std::sync::mpsc::Sender<super::state::LamaMsg>,
    ctx: &egui::Context,
) -> InstanceAction {
    let running = inst.status.is_running();
    let busy = inst.status.is_busy();
    let mut action = InstanceAction::None;

    ui.add_space(16.0);

    // Label / rename
    ui.horizontal(|ui| {
        if inst.editing_label {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut inst.label_edit)
                    .desired_width(220.0)
                    .font(egui::TextStyle::Heading),
            );
            if resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                inst.label = inst.label_edit.trim().to_owned();
                inst.editing_label = false;
                if inst.label.is_empty() {
                    inst.label = format!("Instance {}", inst.id + 1);
                    inst.label_edit = inst.label.clone();
                }
            }
            resp.request_focus();
        } else {
            ui.heading(RichText::new(&inst.label).size(18.0).color(Color32::WHITE));
            if ui.small_button("✏").on_hover_text("Rename").clicked() {
                inst.label_edit = inst.label.clone();
                inst.editing_label = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            status_badge(ui, &inst.status);
        });
    });

    ui.add_space(12.0);
    show_folder_row(ui, inst, running, busy, ctx);
    ui.add_space(12.0);

    if running {
        if let Some(a) = show_url_section(ui, inst, ctx) {
            action = a;
        }
    }

    show_controls(ui, inst, tx.clone(), running, busy);
    show_meta_row(ui, inst);
    show_error(ui, inst);
    show_logs_panel(ui, inst, tx, ctx);
    ui.add_space(20.0);
    show_howto(ui, running);

    action
}

// ── Sidebar panel (compact, used in split layout) ────────────────────────────

fn show_instance_sidebar(
    ui: &mut egui::Ui,
    inst: &mut LamaInstance,
    tx: std::sync::mpsc::Sender<super::state::LamaMsg>,
    ctx: &egui::Context,
) -> InstanceAction {
    let running = inst.status.is_running();
    let busy = inst.status.is_busy();
    let mut action = InstanceAction::None;

    ui.horizontal(|ui| {
        ui.heading(RichText::new(&inst.label).size(15.0).color(Color32::WHITE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            status_badge(ui, &inst.status);
        });
    });

    if ui
        .add(
            egui::Button::new(RichText::new("✕ Close Embedded View").size(12.0))
                .min_size(Vec2::new(200.0, 26.0))
                .fill(Color32::from_rgb(60, 60, 60)),
        )
        .on_hover_text("Return to normal layout")
        .clicked()
    {
        action = InstanceAction::CloseEmbedded;
    }

    ui.add_space(8.0);

    if running {
        let url = inst.url();
        section_heading(ui, "URL");
        ui.horizontal(|ui| {
            ui.label(RichText::new(&url).size(11.0).color(BLUE).monospace());
            if ui.small_button("⧉").on_hover_text("Copy URL").clicked() {
                ctx.copy_text(url.clone());
            }
        });
        if ui
            .add(
                egui::Button::new(RichText::new("🌐 Open in New Window").size(11.0))
                    .min_size(Vec2::new(200.0, 24.0)),
            )
            .on_hover_text("Open Lama-Cleaner in a separate browser window")
            .clicked()
        {
            process::open_in_app_window(&url);
        }
        ui.add_space(8.0);
    }

    show_folder_row(ui, inst, running, busy, ctx);
    ui.add_space(8.0);
    show_controls(ui, inst, tx.clone(), running, busy);
    show_meta_row(ui, inst);
    show_error(ui, inst);
    show_logs_panel(ui, inst, tx, ctx);

    action
}

// ── URL section ───────────────────────────────────────────────────────────────

fn show_url_section(
    ui: &mut egui::Ui,
    inst: &LamaInstance,
    ctx: &egui::Context,
) -> Option<InstanceAction> {
    section_heading(ui, "Server URL");
    let url = inst.url();
    ui.horizontal(|ui| {
        ui.label(RichText::new(&url).size(13.0).color(BLUE).monospace());
        if ui.small_button("⧉").on_hover_text("Copy URL").clicked() {
            ctx.copy_text(url.clone());
        }
    });
    ui.add_space(8.0);

    let mut result = None;
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new("⬛ View in App")
                        .size(12.0)
                        .color(Color32::WHITE),
                )
                .min_size(Vec2::new(130.0, 30.0))
                .fill(BLUE),
            )
            .on_hover_text("Embed Lama-Cleaner inside this window")
            .clicked()
        {
            result = Some(InstanceAction::OpenEmbedded(inst.id));
        }
        if ui
            .add(
                egui::Button::new(RichText::new("🌐 New Window").size(12.0))
                    .min_size(Vec2::new(120.0, 30.0)),
            )
            .on_hover_text("Open in an external browser window (no address bar)")
            .clicked()
        {
            process::open_in_app_window(&url);
        }
        if ui
            .add(
                egui::Button::new(RichText::new("↗ Browser").size(12.0))
                    .min_size(Vec2::new(90.0, 30.0)),
            )
            .on_hover_text("Open in default system browser")
            .clicked()
        {
            process::open_in_browser(&url);
        }
    });

    ui.add_space(12.0);
    result
}

// ── Folder row ────────────────────────────────────────────────────────────────

fn show_folder_row(
    ui: &mut egui::Ui,
    inst: &mut LamaInstance,
    running: bool,
    busy: bool,
    ctx: &egui::Context,
) {
    section_heading(ui, "Image Folder");

    ui.horizontal(|ui| {
        let resp = ui.add_enabled(
            !running && !busy,
            egui::TextEdit::singleline(&mut inst.folder_edit)
                .hint_text("/path/to/images")
                .desired_width(ui.available_width() - 94.0),
        );

        if resp.changed() {
            let trimmed = inst.folder_edit.trim().to_owned();
            inst.folder_path = trimmed;
        }

        resp.context_menu(|ui| {
            if ui
                .add_enabled(!running && !busy, egui::Button::new("📋  Paste"))
                .clicked()
            {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        let text = text.trim().to_owned();
                        inst.folder_edit = text.clone();
                        inst.folder_path = text;
                    }
                }
                ui.close_menu();
            }
            if ui.button("⧉  Copy path").clicked() {
                ctx.copy_text(inst.folder_edit.clone());
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(!running && !busy, egui::Button::new("✕  Clear"))
                .clicked()
            {
                inst.folder_edit.clear();
                inst.folder_path.clear();
                ui.close_menu();
            }
        });

        if ui
            .add_enabled(!running && !busy, egui::Button::new("Browse…"))
            .clicked()
        {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                let path = dir.display().to_string();
                inst.folder_edit = path.clone();
                inst.folder_path = path;
            }
        }
    });

    if !inst.folder_path.is_empty() {
        let exists = std::path::Path::new(&inst.folder_path).is_dir();
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            if exists {
                ui.label(RichText::new("✓ folder found").size(11.0).color(GREEN));
            } else {
                ui.label(RichText::new("✗ folder not found").size(11.0).color(RED));
            }
        });
    }
}

// ── Controls row ──────────────────────────────────────────────────────────────

fn show_controls(
    ui: &mut egui::Ui,
    inst: &mut LamaInstance,
    tx: std::sync::mpsc::Sender<super::state::LamaMsg>,
    running: bool,
    busy: bool,
) {
    section_heading(ui, "Controls");
    ui.horizontal(|ui| {
        let can_start = !running && !busy && !inst.folder_path.is_empty();
        if ui
            .add_enabled(
                can_start,
                egui::Button::new(RichText::new("▶ Start").size(12.0))
                    .min_size(Vec2::new(75.0, 30.0))
                    .fill(if can_start {
                        Color32::from_rgb(36, 138, 61)
                    } else {
                        Color32::from_gray(60)
                    }),
            )
            .clicked()
        {
            inst.folder_path = inst.folder_edit.trim().to_owned();
            inst.status = InstanceStatus::Starting;
            inst.log_lines.clear();
            process::start(inst.id, inst.folder_path.clone(), inst.port, tx.clone());
        }

        if ui
            .add_enabled(
                running || inst.status == InstanceStatus::Starting,
                egui::Button::new(RichText::new("■ Stop").size(12.0))
                    .min_size(Vec2::new(75.0, 30.0))
                    .fill(Color32::from_rgb(140, 30, 30)),
            )
            .clicked()
        {
            inst.status = InstanceStatus::Stopping;
            process::stop(
                inst.id,
                inst.child_pid.unwrap_or(0),
                inst.folder_path.clone(),
                tx.clone(),
            );
        }

        if ui
            .add_enabled(
                running,
                egui::Button::new(RichText::new("↺ Restart").size(12.0))
                    .min_size(Vec2::new(80.0, 30.0)),
            )
            .clicked()
        {
            inst.status = InstanceStatus::Restarting;
            process::restart(
                inst.id,
                inst.child_pid.unwrap_or(0),
                inst.folder_path.clone(),
                inst.port,
                tx.clone(),
            );
        }

        let log_label = if inst.show_logs {
            "▼ Logs"
        } else {
            "▶ Logs"
        };
        if ui
            .add(
                egui::Button::new(RichText::new(log_label).size(12.0))
                    .min_size(Vec2::new(70.0, 30.0)),
            )
            .clicked()
        {
            inst.show_logs = !inst.show_logs;
        }
    });
}

fn show_meta_row(ui: &mut egui::Ui, inst: &LamaInstance) {
    ui.add_space(6.0);
    let pid_str = inst.child_pid.map_or("—".into(), |p| p.to_string());
    ui.label(
        RichText::new(format!("PID: {}    Port: {}", pid_str, inst.port))
            .size(11.0)
            .color(GRAY),
    );
}

fn show_error(ui: &mut egui::Ui, inst: &LamaInstance) {
    if let InstanceStatus::Error(ref e) = inst.status.clone() {
        ui.add_space(8.0);
        egui::Frame::none()
            .fill(Color32::from_rgba_unmultiplied(180, 30, 30, 40))
            .inner_margin(egui::Margin::same(8.0))
            .rounding(4.0)
            .show(ui, |ui| {
                ui.label(RichText::new(format!("Error: {}", e)).size(12.0).color(RED));
            });
    }
}

fn show_logs_panel(
    ui: &mut egui::Ui,
    inst: &mut LamaInstance,
    _tx: std::sync::mpsc::Sender<super::state::LamaMsg>,
    ctx: &egui::Context,
) {
    if !inst.show_logs {
        return;
    }
    ui.add_space(12.0);
    section_heading(ui, "Process Output");

    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(RichText::new("⧉ Copy All").size(11.0))
                    .min_size(Vec2::new(72.0, 22.0)),
            )
            .on_hover_text("Copy all log lines to clipboard")
            .clicked()
        {
            ctx.copy_text(inst.log_lines.join("\n"));
        }
        if ui
            .add(
                egui::Button::new(RichText::new("✕ Clear").size(11.0))
                    .min_size(Vec2::new(55.0, 22.0)),
            )
            .clicked()
        {
            inst.log_lines.clear();
        }
        ui.label(
            RichText::new(format!("{} lines", inst.log_lines.len()))
                .size(11.0)
                .color(GRAY),
        );
    });

    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt(("lama_logs", inst.id))
        .max_height(220.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            egui::Frame::none()
                .fill(Color32::from_gray(12))
                .inner_margin(egui::Margin::same(6.0))
                .rounding(4.0)
                .show(ui, |ui| {
                    if inst.log_lines.is_empty() {
                        ui.label(RichText::new("No output yet.").size(11.0).color(GRAY));
                    } else {
                        for line in &inst.log_lines {
                            ui.label(
                                RichText::new(line)
                                    .size(11.0)
                                    .color(Color32::from_gray(220))
                                    .monospace(),
                            );
                        }
                    }
                });
        });
}

fn show_howto(ui: &mut egui::Ui, running: bool) {
    if running {
        return;
    }
    ui.separator();
    ui.add_space(8.0);
    egui::Frame::none()
        .fill(Color32::from_gray(28))
        .inner_margin(egui::Margin::same(12.0))
        .rounding(6.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new("How to use")
                    .size(13.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(6.0);
            for (n, step) in [
                ("1", "Select the folder containing images to edit"),
                (
                    "2",
                    "Click Start — the AI model downloads on first run (~1–2 min)",
                ),
                (
                    "3",
                    "Once Running, click \"View in App\" to edit inside this window",
                ),
                (
                    "4",
                    "Paint over objects to remove them; the AI fills in the background",
                ),
                ("5", "Click Stop when done to free resources"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(n).size(12.0).strong().color(BLUE));
                    ui.label(
                        RichText::new(step)
                            .size(12.0)
                            .color(Color32::from_white_alpha(200)),
                    );
                });
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Multiple instances can run simultaneously for different folders.\n\
                     Uses GPU automatically if nvidia-smi is available.",
                )
                .size(11.0)
                .color(GRAY),
            );
        });
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(11.0)
            .strong()
            .color(Color32::from_white_alpha(160)),
    );
    ui.add_space(3.0);
}

fn status_badge(ui: &mut egui::Ui, status: &InstanceStatus) {
    let col = status_color(status);
    let text = status.label();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(90.0, 22.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 4.0, col.linear_multiply(0.25));
    ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, col));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        col,
    );
}
