pub mod process;
pub mod state;
pub mod ui;

pub use state::LamaCleanerState;

/// Hide or show the embedded webview without destroying it.
/// Called every frame from app.rs so the webview disappears on other tabs.
#[cfg(feature = "lama-webview")]
pub fn sync_webview_visibility(state: &mut LamaCleanerState, visible: bool) {
    if let Some(wv) = &state.webview {
        let _ = wv.view.set_visible(visible);
    }
}

/// Called each frame when `AppMode::LamaCleaner` is active.
pub fn show(ctx: &eframe::egui::Context, frame: &eframe::Frame, state: &mut LamaCleanerState) {
    state.drain_msgs();

    // Kick off setup check the first time we're shown.
    if state.setup == state::SetupStatus::Unknown {
        process::check_setup(state.tx.clone());
    }

    // Periodic status poll (every 3 s, non-blocking).
    if state.last_poll.elapsed() > std::time::Duration::from_secs(3) {
        state.last_poll = std::time::Instant::now();
        let busy_ids: Vec<usize> = state
            .instances
            .iter()
            .filter(|i| i.status.is_busy())
            .map(|i| i.id)
            .collect();

        for inst in &state.instances {
            if !busy_ids.contains(&inst.id) {
                process::poll_status(inst.id, inst.child_pid, state.tx.clone());
            }
        }
    }

    // Pump GTK event loop so the embedded WebKit view stays responsive.
    #[cfg(feature = "lama-webview")]
    {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }

    ui::show(ctx, state);

    // ── Embedded webview lifecycle ────────────────────────────────────────────
    #[cfg(feature = "lama-webview")]
    {
        let active_inst_running = state
            .instances
            .get(state.active_tab)
            .map(|i| i.status.is_running())
            .unwrap_or(false);

        if state.close_webview || !active_inst_running {
            if state.close_webview || state.webview.is_some() {
                for inst in &mut state.instances {
                    inst.show_embedded = false;
                }
                state.webview = None;
            }
            state.close_webview = false;
        }

        if let Some(inst_id) = state.open_webview_for.take() {
            let url_opt = state
                .instances
                .iter()
                .find(|i| i.id == inst_id)
                .map(|i| i.url());

            if let Some(url) = url_opt {
                state.webview = None;
                for inst in &mut state.instances {
                    inst.show_embedded = false;
                }

                let rect = state.webview_target_rect.unwrap_or_else(|| {
                    egui::Rect::from_min_size(egui::pos2(280.0, 60.0), egui::vec2(960.0, 720.0))
                });
                let ppp = ctx.pixels_per_point();
                let bounds = wry::Rect {
                    position: wry::dpi::PhysicalPosition::new(
                        (rect.min.x * ppp) as i32,
                        (rect.min.y * ppp) as i32,
                    )
                    .into(),
                    size: wry::dpi::PhysicalSize::new(
                        ((rect.width() * ppp) as u32).max(1),
                        ((rect.height() * ppp) as u32).max(1),
                    )
                    .into(),
                };

                match wry::WebViewBuilder::new()
                    .with_url(&url)
                    .with_bounds(bounds)
                    .with_clipboard(true)
                    .build_as_child(frame)
                {
                    Ok(view) => {
                        state.webview = Some(state::EmbeddedWebView {
                            view,
                            instance_id: inst_id,
                            last_rect: Some(rect),
                        });
                        if let Some(inst) = state.instances.iter_mut().find(|i| i.id == inst_id) {
                            inst.show_embedded = true;
                        }
                    }
                    Err(e) => {
                        eprintln!("lama-webview: could not create child webview: {e}");
                        process::open_in_app_window(&url);
                    }
                }
            }
        }

        // Resize webview to match the allocated panel rect when it changes.
        if let (Some(wv), Some(new_rect)) = (&mut state.webview, state.webview_target_rect) {
            let changed = wv.last_rect.map_or(true, |r| r != new_rect);
            if changed {
                wv.last_rect = Some(new_rect);
                let ppp = ctx.pixels_per_point();
                let _ = wv.view.set_bounds(wry::Rect {
                    position: wry::dpi::PhysicalPosition::new(
                        (new_rect.min.x * ppp) as i32,
                        (new_rect.min.y * ppp) as i32,
                    )
                    .into(),
                    size: wry::dpi::PhysicalSize::new(
                        ((new_rect.width() * ppp) as u32).max(1),
                        ((new_rect.height() * ppp) as u32).max(1),
                    )
                    .into(),
                });
            }
        }
    }

    if state.instances.iter().any(|i| i.status.is_busy()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    // Repaint continuously while installing or while the webview is live.
    if matches!(
        state.setup,
        state::SetupStatus::Installing | state::SetupStatus::Checking
    ) {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    #[cfg(feature = "lama-webview")]
    if state.webview.is_some() {
        ctx.request_repaint();
    }
}
