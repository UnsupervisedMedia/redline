//! Local AI control panel (egui sidebar).

use std::time::{Duration, Instant};

use crate::local_ai::agent::LocalAiConfig;
use crate::local_ai::roles::AgentRole;
use crate::state::AppState;

/// Memory keys for user-edited connection config that takes effect on the next
/// agent (re)start.
const MODEL_KEY: &str = "local_ai_cfg_model";
const URL_KEY: &str = "local_ai_cfg_url";
const ROLE_KEY: &str = "local_ai_cfg_role";

fn cfg_model(ctx: &egui::Context) -> String {
    ctx.memory_mut(|m| {
        m.data
            .get_temp::<String>(egui::Id::new(MODEL_KEY))
            .unwrap_or_else(|| LocalAiConfig::default().model)
    })
}

fn cfg_url(ctx: &egui::Context) -> String {
    ctx.memory_mut(|m| {
        m.data
            .get_temp::<String>(egui::Id::new(URL_KEY))
            .unwrap_or_else(|| LocalAiConfig::default().ollama_url)
    })
}

fn cfg_role(ctx: &egui::Context) -> AgentRole {
    ctx.memory_mut(|m| {
        *m.data
            .get_temp_mut_or(egui::Id::new(ROLE_KEY), AgentRole::default())
    })
}

/// Build a config from the user-edited fields stored in egui memory.
fn build_config(ctx: &egui::Context) -> LocalAiConfig {
    LocalAiConfig {
        model: cfg_model(ctx),
        ollama_url: cfg_url(ctx),
        role: cfg_role(ctx),
    }
}

pub fn show_local_ai_panel(ctx: &egui::Context, state: &mut AppState) {
    let mut open = state.local_ai_panel_open;
    egui::SidePanel::right("local_ai_panel")
        .resizable(true)
        .default_width(280.0)
        .show_animated(ctx, open, |ui| {
            ui.heading("Local AI");

            // ── On/off toggle ────────────────────────────────────────────
            let is_on = state.local_ai.is_some();
            let mut want_on = is_on;
            ui.horizontal(|ui| {
                ui.label("Status:");
                let label = if want_on { "On" } else { "Off" };
                ui.checkbox(&mut want_on, label);
            });

            // Apply toggle change.
            if want_on != is_on {
                if want_on {
                    let tx = state.agent_server.as_ref().map(|s| s.agent_op_tx.clone());
                    let bcast = state.canvas.event_broadcast.clone();
                    let rt = state.agent_server.as_ref().map(|s| s.handle.clone());
                    let default_color = state.settings.local_ai_color;
                    match (tx, bcast, rt) {
                        (Some(agent_op_tx), Some(broadcast), Some(runtime)) => {
                            let cfg = build_config(ui.ctx());
                            let event_rx = broadcast.subscribe();
                            let pid = crate::types::ParticipantId::new();
                            let room_id =
                                crate::agent::handlers::room_id_for_tab_index(state.active_tab_pos);
                            // Keep the live role caches in sync with the role the
                            // agent is actually starting with.
                            let start_role = cfg.role;
                            ui.ctx().memory_mut(|m| {
                                m.data
                                    .insert_temp(egui::Id::new("local_ai_role_cache"), start_role);
                                m.data.insert_temp(
                                    egui::Id::new("local_ai_role_applied"),
                                    start_role,
                                );
                            });
                            state.local_ai = Some(crate::local_ai::agent::spawn(
                                cfg,
                                agent_op_tx,
                                event_rx,
                                pid,
                                room_id,
                                runtime,
                                default_color,
                            ));
                        }
                        _ => {
                            state.status_msg = Some(
                                "Local AI needs the agent server. Enable it in Settings first."
                                    .into(),
                            );
                        }
                    }
                } else if let Some(h) = state.local_ai.take() {
                    h.stop();
                }
            }

            // ── Connection config (model + endpoint + start role) ──────────
            // Editable any time; changes take effect on the next start/restart.
            {
                let mut model = cfg_model(ui.ctx());
                let mut url = cfg_url(ui.ctx());
                let mut start_role = cfg_role(ui.ctx());

                ui.horizontal(|ui| {
                    ui.label("Model:");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut model)
                                .desired_width(f32::INFINITY)
                                .hint_text("qwen3-vl:latest"),
                        )
                        .changed()
                    {
                        ui.ctx().memory_mut(|m| {
                            m.data.insert_temp(egui::Id::new(MODEL_KEY), model.clone())
                        });
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Ollama:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut url)
                            .desired_width(f32::INFINITY)
                            .hint_text("http://localhost:11434"),
                    );
                    if resp.changed() {
                        ui.ctx().memory_mut(|m| {
                            m.data.insert_temp(egui::Id::new(URL_KEY), url.clone())
                        });
                    }
                });
                // Validate the URL inline so the user sees a problem before start.
                if let Err(e) = crate::local_ai::ollama::validate_url(&url) {
                    ui.label(
                        egui::RichText::new(e)
                            .small()
                            .color(egui::Color32::from_rgb(220, 120, 0)),
                    );
                }

                egui::ComboBox::from_label("Start role")
                    .selected_text(start_role.label())
                    .show_ui(ui, |ui| {
                        for r in [
                            AgentRole::Observer,
                            AgentRole::CollaboratorLight,
                            AgentRole::CollaboratorHeavy,
                            AgentRole::Curator,
                            AgentRole::GamePlayer,
                        ] {
                            ui.selectable_value(&mut start_role, r, r.label());
                        }
                    });
                ui.ctx()
                    .memory_mut(|m| m.data.insert_temp(egui::Id::new(ROLE_KEY), start_role));

                if state.local_ai.is_some() {
                    ui.label(
                        egui::RichText::new("Model/endpoint/role apply on restart.")
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                    if ui.button("Restart agent").clicked() {
                        if let Some(h) = state.local_ai.take() {
                            h.stop();
                        }
                        let tx = state.agent_server.as_ref().map(|s| s.agent_op_tx.clone());
                        let bcast = state.canvas.event_broadcast.clone();
                        let rt = state.agent_server.as_ref().map(|s| s.handle.clone());
                        let default_color = state.settings.local_ai_color;
                        if let (Some(agent_op_tx), Some(broadcast), Some(runtime)) = (tx, bcast, rt)
                        {
                            let cfg = build_config(ui.ctx());
                            let restart_role = cfg.role;
                            let event_rx = broadcast.subscribe();
                            let pid = crate::types::ParticipantId::new();
                            let room_id =
                                crate::agent::handlers::room_id_for_tab_index(state.active_tab_pos);
                            ui.ctx().memory_mut(|m| {
                                m.data.insert_temp(
                                    egui::Id::new("local_ai_role_cache"),
                                    restart_role,
                                );
                                m.data.insert_temp(
                                    egui::Id::new("local_ai_role_applied"),
                                    restart_role,
                                );
                            });
                            state.local_ai = Some(crate::local_ai::agent::spawn(
                                cfg,
                                agent_op_tx,
                                event_rx,
                                pid,
                                room_id,
                                runtime,
                                default_color,
                            ));
                        }
                    }
                }
            }

            if let Some(h) = state.local_ai.as_ref() {
                // ── Mode selector ───────────────────────────────────────
                let mut role_now: AgentRole = ui.ctx().memory_mut(|m| {
                    *m.data
                        .get_temp_mut_or(egui::Id::new("local_ai_role_cache"), AgentRole::default())
                });
                egui::ComboBox::from_label("Mode")
                    .selected_text(role_now.mode_label())
                    .show_ui(ui, |ui| {
                        for r in [
                            AgentRole::Observer,
                            AgentRole::CollaboratorLight,
                            AgentRole::CollaboratorHeavy,
                            AgentRole::Curator,
                            AgentRole::GamePlayer,
                        ] {
                            ui.selectable_value(&mut role_now, r, r.mode_label());
                        }
                    });
                ui.label(
                    egui::RichText::new(role_now.mode_description())
                        .small()
                        .color(egui::Color32::GRAY),
                );
                ui.ctx().memory_mut(|m| {
                    m.data
                        .insert_temp(egui::Id::new("local_ai_role_cache"), role_now)
                });

                // Apply role change to the agent when the dropdown value differs
                // from the last applied role.
                let last_applied: AgentRole = ui.ctx().memory_mut(|m| {
                    *m.data.get_temp_mut_or(
                        egui::Id::new("local_ai_role_applied"),
                        AgentRole::default(),
                    )
                });
                if role_now != last_applied {
                    let role_arc = h.role.clone();
                    let budget_arc = h.budget.clone();
                    let role_to_apply = role_now;
                    h.runtime.spawn(async move {
                        *role_arc.write().await = role_to_apply;
                        let mut b = budget_arc.lock().await;
                        *b = crate::local_ai::budget::BudgetPool::new(role_to_apply.budget());
                    });
                    ui.ctx().memory_mut(|m| {
                        m.data
                            .insert_temp(egui::Id::new("local_ai_role_applied"), role_now)
                    });
                }

                // ── AI color picker ─────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.label("Color:");
                    let mut c = state.settings.local_ai_color;
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        state.settings.local_ai_color = c;
                        let color_arc = h.color.clone();
                        let new_color = c;
                        h.runtime.spawn(async move {
                            *color_arc.write().await = new_color;
                        });
                    }
                    ui.label(
                        egui::RichText::new("(used when the model doesn't pick one)")
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                });

                // ── Budget readout (live snapshot via try_lock) ─────────
                let (act, ask, bm) = match h.budget.try_lock() {
                    Ok(b) => b.snapshot(),
                    Err(_) => (0, 0, 0),
                };
                ui.horizontal(|ui| {
                    ui.label(format!("Act: {act}"));
                    ui.separator();
                    ui.label(format!("Ask: {ask}"));
                    ui.separator();
                    ui.label(format!("Bookmark: {bm}"));
                });

                // ── Mute ─────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    if ui.button("Mute 5min").clicked() {
                        let mute = h.mute_until.clone();
                        h.runtime.spawn(async move {
                            *mute.write().await = Some(Instant::now() + Duration::from_secs(300));
                        });
                    }
                    if ui.button("Unmute").clicked() {
                        let mute = h.mute_until.clone();
                        h.runtime.spawn(async move {
                            *mute.write().await = None;
                        });
                    }
                });

                // ── Thinking (live read of agent scratchpad) ────────────
                ui.separator();
                let sp_now: String = h
                    .scratchpad
                    .try_read()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Thinking:");
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(sp_now.clone());
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut sp_now.as_str())
                        .desired_width(f32::INFINITY)
                        .desired_rows(4)
                        .interactive(true),
                );

                // ── Agent log ───────────────────────────────────────────
                ui.separator();
                let log_text: String = h
                    .log
                    .try_read()
                    .map(|l| l.iter().cloned().collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Log:");
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(log_text.clone());
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("local_ai_log_scroll")
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut log_text.as_str())
                                .desired_width(f32::INFINITY)
                                .desired_rows(10)
                                .interactive(true)
                                .font(egui::TextStyle::Monospace),
                        );
                    });

                // ── Chat input ─────────────────────────────────────────
                ui.separator();
                ui.label("Talk to the agent:");
                let mut input: String = ui.ctx().memory_mut(|m| {
                    m.data
                        .get_temp::<String>(egui::Id::new("local_ai_chat_input"))
                        .unwrap_or_default()
                });
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut input)
                        .desired_width(f32::INFINITY)
                        .hint_text("e.g. \"explain what you see\""),
                );
                let send_clicked = ui.button("Send").clicked();
                let enter_pressed =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (send_clicked || enter_pressed) && !input.trim().is_empty() {
                    let _ = h.user_input_tx.send(input.trim().to_string());
                    input.clear();
                }
                ui.ctx().memory_mut(|m| {
                    m.data
                        .insert_temp(egui::Id::new("local_ai_chat_input"), input)
                });

                // Keep the UI repainting so the log/scratchpad refresh
                // while the agent is working in the background.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }

            // Toggle visibility.
            ui.separator();
            if ui.button("Hide panel").clicked() {
                open = false;
            }
        });
    state.local_ai_panel_open = open;
}
