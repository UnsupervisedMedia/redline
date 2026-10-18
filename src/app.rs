use crate::{
    command::CommandExecutor,
    shortcuts::{capture_from_input, save_shortcuts, Action},
    state::{AppState, Tool, ToolState},
    types::{Annotation, Command, ParticipantId, ShapeData},
};
use eframe::egui;
use std::sync::Arc;

/// Push an RgbaImage to the system clipboard.
fn copy_rgba_via_clipboard(state: &mut AppState, img: image::RgbaImage, success_msg: &str) {
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

/// Drop a clipboard image onto the current canvas as a movable/resizable
/// overlay. Centers the image, scales it down if it would overflow the canvas.
fn paste_as_image_layer(state: &mut AppState, buf: image::RgbaImage) {
    let (iw, ih) = (buf.width() as f32, buf.height() as f32);
    let cs = state.canvas.canvas_size;
    if cs.x <= 0.0 || cs.y <= 0.0 || iw <= 0.0 || ih <= 0.0 {
        state.status_msg = Some("Cannot paste: canvas not ready".into());
        return;
    }
    // Fit to half the canvas (whichever dimension binds). Keeps small images 1:1.
    let max_w = cs.x * 0.5;
    let max_h = cs.y * 0.5;
    let scale = (max_w / iw).min(max_h / ih).min(1.0);
    let (w, h) = (iw * scale, ih * scale);
    let top_left = egui::Pos2::new((cs.x - w) * 0.5, (cs.y - h) * 0.5);
    let rect = egui::Rect::from_min_size(top_left, egui::Vec2::new(w, h));

    let shape = ShapeData::ImageLayer {
        rect,
        data: Arc::new(buf),
        texture: crate::types::TextureCell::default(),
    };
    let ann = Annotation::new(shape, egui::Color32::WHITE, 1.0);
    let id = ann.id;
    CommandExecutor::execute(
        &mut state.canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(ann),
    );
    state.current_tool = Tool::Select;
    state.tool_state = ToolState::Selected {
        id,
        drag_offset: None,
    };
    state.status_msg = Some("Pasted image as overlay — drag corners to resize".into());
}

/// Pull RGBA pixels out of a portal capture, whichever form it arrived in.
/// How long to wait for the desktop portal before declaring the capture dead.
/// Generous: on a multi-monitor desktop the portal writes (and we decode) a
/// 60-megapixel PNG, and GNOME's clipboard fallback alone polls for 4 s.
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Longer leash for the permission-priming grab: it sits behind a desktop
/// dialog the user has to read and click.
const PERMISSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn capture_to_rgba(capture: &crate::portal_capture::Capture) -> Option<image::RgbaImage> {
    match capture {
        crate::portal_capture::Capture::File(path) => match image::open(path) {
            Ok(img) => Some(img.to_rgba8()),
            Err(e) => {
                eprintln!("region: reading {} failed: {e}", path.display());
                None
            }
        },
        crate::portal_capture::Capture::Image {
            width,
            height,
            rgba,
        } => image::RgbaImage::from_raw(*width, *height, rgba.clone()),
    }
}

pub struct RedLineApp {
    pub state: AppState,
    /// Fonts are installed once on the first frame — `set_fonts` rebuilds the
    /// whole atlas, so it must never run per-frame.
    fonts_installed: bool,
}

impl Default for RedLineApp {
    fn default() -> Self {
        Self {
            state: AppState::default(),
            fonts_installed: false,
        }
    }
}

impl RedLineApp {
    pub fn with_settings(settings: crate::state::Settings) -> Self {
        let mut app = Self::default();
        app.state.settings = settings;
        app
    }
}

impl RedLineApp {
    /// Render the RedLine annotate workspace body (everything below the shared
    /// chrome): adjust/resize dialogs, action + quick-tool bars, the toolbox and
    /// right-side panels, the central canvas, and the floating windows.
    fn show_annotate_body(&mut self, ctx: &egui::Context) {
        let theme = self.state.settings.theme();

        crate::toolbar::show_adjust_window(ctx, &mut self.state);
        crate::toolbar::show_resize_canvas_dialog(ctx, &mut self.state);

        egui::TopBottomPanel::top("capture_bar")
            .frame(
                egui::Frame::none()
                    .fill(theme.toolbar_bg)
                    .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                    .stroke(egui::Stroke::new(1.0, theme.divider)),
            )
            .show(ctx, |ui| {
                crate::toolbar::show_capture_bar(ui, &mut self.state);
            });

        egui::TopBottomPanel::top("view_bar")
            .frame(
                egui::Frame::none()
                    .fill(theme.panel_bg)
                    .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                    .stroke(egui::Stroke::new(1.0, theme.divider)),
            )
            .show(ctx, |ui| {
                crate::toolbar::show_view_bar(ui, &mut self.state);
            });

        egui::SidePanel::left("toolbox")
            .resizable(true)
            .min_width(120.0)
            .default_width(196.0)
            .max_width(360.0)
            .show(ctx, |ui| {
                crate::toolbar::show_toolbox(ui, &mut self.state);
            });

        if self.state.settings.show_layers {
            egui::SidePanel::right("layers_panel")
                .min_width(140.0)
                .default_width(160.0)
                .show(ctx, |ui| {
                    crate::toolbar::show_layers_panel(ui, &mut self.state);
                });
        }

        if self.state.settings.show_transform {
            egui::SidePanel::right("transform_panel")
                .min_width(180.0)
                .default_width(220.0)
                .show(ctx, |ui| {
                    crate::toolbar::show_transform_panel(ui, &mut self.state);
                });
        }

        if self.state.settings.show_recents {
            egui::SidePanel::right("recents")
                .min_width(180.0)
                .show(ctx, |ui| {
                    crate::toolbar::show_recents_panel(ui, &mut self.state, ctx);
                });
        }

        crate::toolbar::poll_vectorize_result(&mut self.state, ctx);
        crate::toolbar::maybe_dispatch_vectorize_job(&mut self.state, ctx);

        if self.state.canvas.vectorize_panel.open {
            egui::SidePanel::right("vectorize_panel")
                .default_width(220.0)
                .min_width(180.0)
                .show(ctx, |ui| {
                    crate::toolbar::show_vectorize_panel(ui, &mut self.state);
                });
        }

        if self.state.settings.show_pathfinder {
            egui::TopBottomPanel::bottom("pathfinder_strip").show(ctx, |ui| {
                crate::toolbar::show_pathfinder_strip(ui, &mut self.state);
            });
        }

        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::none()
                    .fill(theme.toolbar_bg)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                    .stroke(egui::Stroke::new(1.0, theme.divider)),
            )
            .show(ctx, |ui| {
                crate::toolbar::show_status_bar(ui, &mut self.state);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme.canvas_void))
            .show(ctx, |ui| {
                crate::canvas::show(ui, &mut self.state);

                if let Some((deadline, _)) = self.state.capture_scheduled {
                    let now = std::time::Instant::now();
                    if now < deadline {
                        let remaining = (deadline - now).as_secs_f32();
                        let total = self.state.settings.capture_delay_s;
                        crate::toolbar::draw_countdown_overlay(
                            ui,
                            remaining,
                            total.max(1),
                            theme.accent,
                        );
                    }
                }
            });

        crate::toolbar::show_shortcuts_window(ctx, &mut self.state);

        #[cfg(feature = "local-ai")]
        crate::local_ai::ui::show_local_ai_panel(ctx, &mut self.state);
    }
}

impl RedLineApp {
    /// Restore the window after a capture. `focus` is false while the region
    /// overlay is taking over — stealing focus back would pull it under.
    fn restore_main_window(&mut self, ctx: &egui::Context, focus: bool) {
        if self.state.window_hidden_for_capture {
            self.state.window_hidden_for_capture = false;
            crate::portal_capture::capture_log(|| format!("restoring window (focus={focus})"));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            if focus {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
    }

    /// Put freshly captured pixels on a new tab.
    fn adopt_capture(&mut self, buf: image::RgbaImage, ctx: &egui::Context) {
        self.state.screenshot_count += 1;
        let n = self.state.screenshot_count;
        self.state
            .push_current_as_tab_and_reset(format!("Screenshot {n}"));
        crate::loader::apply_image(image::DynamicImage::ImageRgba8(buf), &mut self.state, ctx);
    }

    /// Auto-save and/or auto-copy a finished capture, then report what happened
    /// in one toast. Both halves are opt-in and independent.
    fn run_post_capture_automation(&mut self) {
        let loaded = self.state.canvas.texture.is_some();
        if !loaded {
            return;
        }
        let mut done: Vec<String> = Vec::new();
        if self.state.settings.auto_save {
            if let Some(entry) = crate::recents::auto_save_capture(&self.state) {
                let name = entry
                    .image_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("disk")
                    .to_string();
                crate::state::push_recent_capture(&mut self.state.recent_captures, entry);
                done.push(format!("saved {name}"));
            }
        }
        if self.state.settings.auto_copy_on_capture {
            crate::toolbar::copy_image_with_flatten_pub(&mut self.state);
            match self.state.status_msg.take() {
                // A clipboard failure matters more than the "saved" half.
                Some(msg)
                    if msg.to_ascii_lowercase().contains("fail")
                        || msg.to_ascii_lowercase().contains("unavailable") =>
                {
                    done.push(msg)
                }
                _ => done.push("copied to clipboard".into()),
            }
        }
        if !done.is_empty() {
            let mut summary = done.join(" · ");
            if let Some(first) = summary.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            self.state.status_msg = Some(summary);
        }
    }

    /// Drive the region overlay while it is up, and act on its outcome.
    fn drive_region_overlay(&mut self, ctx: &egui::Context) {
        let Some(rs) = self.state.region_select.as_mut() else {
            return;
        };
        // The parent context drives the overlay's frames; without this the
        // overlay freezes as soon as the main window stops getting events.
        ctx.request_repaint();
        // Cheap Arc clone — kept so the fallback below can still use the grab
        // after the overlay has been dropped.
        let shot = rs.shot.clone();
        match crate::region_select::show(ctx, rs) {
            crate::region_select::Outcome::Pending => {}
            crate::region_select::Outcome::Accepted(img) => {
                let (w, h) = (img.width(), img.height());
                self.state.region_select = None;
                crate::region_select::leave_fullscreen(ctx);
                self.adopt_capture(img, ctx);
                self.run_post_capture_automation();
                if self.state.status_msg.is_none() {
                    self.state.status_msg = Some(format!("Captured {w} × {h}"));
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            crate::region_select::Outcome::Cancelled => {
                self.state.region_select = None;
                crate::region_select::leave_fullscreen(ctx);
                self.state.status_msg = Some("Capture cancelled".into());
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            // The window manager never mapped the overlay fullscreen — most
            // often because the un-minimize after the grab didn't take. Rather
            // than leave the user with no window and no capture, keep the
            // pixels: load the whole grab and let them crop in the editor.
            crate::region_select::Outcome::Unavailable => {
                self.state.region_select = None;
                crate::region_select::leave_fullscreen(ctx);
                self.restore_main_window(ctx, true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                let (w, h) = (shot.width(), shot.height());
                // The overlay is gone, so this is the last reference — take the
                // pixels rather than copying 100+ MB of desktop.
                let img = std::sync::Arc::try_unwrap(shot).unwrap_or_else(|a| (*a).clone());
                self.adopt_capture(img, ctx);
                self.run_post_capture_automation();
                self.state.status_msg = Some(format!(
                    "Region overlay wouldn't open — captured the full screen instead                      ({w} × {h}); crop it here, or turn off “Precise region”"
                ));
            }
        }
    }
}

impl eframe::App for RedLineApp {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        crate::config::save_settings(&self.state.settings);
        // eframe calls `save` periodically and on clean exit — flush the session
        // here so quitting never loses open tabs/annotations.
        crate::session::save_now(&mut self.state);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        egui_extras::install_image_loaders(ctx);
        if !self.fonts_installed {
            self.fonts_installed = true;
            crate::theme::install_fonts(ctx);
        }
        let theme = self.state.settings.theme();
        ctx.set_visuals(theme.build_visuals());
        crate::theme::apply_style(ctx);

        // Pump GTK event loop and request continuous repaints while an embedded
        // webview is live.  This runs even when the user is on another tab so
        // the webview doesn't freeze if they briefly switch away.
        #[cfg(feature = "lama-webview")]
        if self.state.lama_cleaner.webview.is_some() {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            ctx.request_repaint();
        }

        // Reconcile agent server state with the Settings toggle, then drain ops.
        #[cfg(feature = "agent-server")]
        {
            let want_on = self.state.settings.agent_server_enabled;
            let is_on = self.state.agent_server.is_some();
            if want_on && !is_on {
                let (server, rx) =
                    crate::agent::AgentServer::start(crate::agent::server::DEFAULT_PORT);
                self.state.canvas.event_broadcast = Some(server.event_broadcast.clone());
                self.state.agent_server = Some(server);
                self.state.agent_op_rx = Some(rx);
            } else if !want_on && is_on {
                if let Some(mut s) = self.state.agent_server.take() {
                    s.stop();
                }
                self.state.agent_op_rx = None;
                self.state.canvas.event_broadcast = None;
            }
            crate::agent::drain_ops(&mut self.state, 64);
            // Keep the egui loop pumping while the agent server is on so MCP
            // ops drain without depending on user input events.
            if self.state.agent_server.is_some() {
                ctx.request_repaint();
            }
        }

        // Cancel any in-flight capture with Escape and restore the window.
        let capture_in_flight = self.state.pending_capture.is_some()
            || self.state.capture_scheduled.is_some()
            || self.state.capture_hide_pending.is_some();
        if capture_in_flight && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.state.pending_capture = None;
            self.state.pending_capture_since = None;
            self.state.capture_scheduled = None;
            self.state.capture_hide_pending = None;
            self.state.capture_attempt = None;
            self.state.capture_priming = false;
            self.state.capture_prime_pending = None;
            self.state.status_msg = Some("Capture cancelled".into());
            if self.state.window_hidden_for_capture {
                self.state.window_hidden_for_capture = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        // Poll a pending portal screenshot first — this clears
        // `window_hidden_for_capture` when the result arrives so the minimize
        // check below doesn't re-send Minimized(true) in the same frame.
        if let Some(rx) = &self.state.pending_capture {
            match rx.try_recv() {
                Ok(Ok(capture)) => {
                    self.state.pending_capture = None;
                    self.state.pending_capture_since = None;
                    let attempt = self.state.capture_attempt.take();
                    if self.state.capture_priming {
                        // Permission just got granted; this grab has RedLine
                        // in it, so bin it and run the capture for real.
                        self.state.capture_priming = false;
                        crate::portal_capture::capture_log(|| {
                            "priming grab succeeded; re-running the real capture".into()
                        });
                        // The portal wrote this shot to ~/Pictures; it is a
                        // picture of RedLine asking for permission, so don't
                        // leave it lying in the user's screenshot folder.
                        if let crate::portal_capture::Capture::File(ref path) = capture {
                            let _ = std::fs::remove_file(path);
                        }
                        if let Some((mode, _)) = attempt {
                            crate::toolbar::begin_hide_for_capture(&mut self.state, mode);
                        }
                    } else if self.state.region_pending {
                        // Destined for the in-app overlay: freeze the grab and
                        // let the user frame it instead of loading it whole.
                        self.state.region_pending = false;
                        match capture_to_rgba(&capture) {
                            Some(shot) => {
                                // The overlay spans the virtual desktop:
                                // physical grab pixels ÷ this window's scale.
                                let ppp = ctx.pixels_per_point().max(0.1);
                                let size = egui::vec2(
                                    shot.width() as f32 / ppp,
                                    shot.height() as f32 / ppp,
                                );
                                self.state.region_select =
                                    Some(crate::region_select::RegionSelect::new(shot, size));
                                // Un-minimize now: the overlay sits on top, and
                                // a minimized parent stops driving the frame
                                // loop on some window managers.
                                self.restore_main_window(ctx, false);
                            }
                            None => {
                                self.restore_main_window(ctx, true);
                                self.state.status_msg =
                                    Some("Capture failed: could not read the screen grab".into());
                            }
                        }
                    } else {
                        self.restore_main_window(ctx, true);
                        match capture {
                            crate::portal_capture::Capture::File(ref path) => {
                                eprintln!("app: loading file {}", path.display());
                                crate::loader::load_file(path, &mut self.state, ctx);
                            }
                            crate::portal_capture::Capture::Image {
                                width,
                                height,
                                rgba,
                            } => {
                                eprintln!(
                                    "app: loading clipboard {width}x{height} ({} bytes)",
                                    rgba.len()
                                );
                                let expected = width as usize * height as usize * 4;
                                if rgba.len() != expected {
                                    eprintln!(
                                        "app: MISMATCH — expected {expected} bytes, got {}",
                                        rgba.len()
                                    );
                                }
                                match image::RgbaImage::from_raw(width, height, rgba) {
                                    Some(buf) => self.adopt_capture(buf, ctx),
                                    None => {
                                        self.state.status_msg =
                                            Some("Clipboard image had wrong dimensions".into())
                                    }
                                }
                            }
                        }
                        let loaded = self.state.canvas.texture.is_some();
                        let sz = self.state.canvas.canvas_size;
                        eprintln!(
                            "app: texture loaded={loaded}, canvas_size={sz:?}, status={:?}",
                            self.state.status_msg
                        );
                        self.run_post_capture_automation();
                    }
                }
                Ok(Err(e)) => {
                    self.state.pending_capture = None;
                    self.state.pending_capture_since = None;
                    self.state.region_pending = false;
                    let attempt = self.state.capture_attempt.take();
                    let was_priming = std::mem::take(&mut self.state.capture_priming);
                    self.restore_main_window(ctx, true);
                    match attempt {
                        // A non-interactive grab that failed before we ever
                        // asked for permission: RedLine was minimized, so the
                        // desktop had nowhere to show its prompt. Ask once,
                        // visibly, then retry.
                        Some((mode, false)) if !was_priming && !self.state.capture_primed => {
                            self.state.capture_primed = true;
                            self.state.capture_prime_pending = Some((
                                std::time::Instant::now() + std::time::Duration::from_millis(400),
                                mode,
                            ));
                            crate::portal_capture::capture_log(|| {
                                format!(
                                    "capture failed ({e}); priming permission with the window up"
                                )
                            });
                            self.state.status_msg =
                                Some("Asking the desktop for screenshot permission…".into());
                        }
                        _ if was_priming => {
                            self.state.status_msg = Some(format!(
                                "Capture failed: {e} — the desktop denied screenshot permission"
                            ));
                        }
                        _ => {
                            self.state.status_msg = Some(format!("Capture failed: {e}"));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint();
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.state.pending_capture = None;
                    self.state.pending_capture_since = None;
                    self.state.capture_attempt = None;
                    self.state.capture_priming = false;
                    if self.state.window_hidden_for_capture {
                        self.state.window_hidden_for_capture = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    self.state.status_msg =
                        Some("Capture failed: capture thread exited unexpectedly".into());
                }
            }
        }

        // Watchdog: a portal request that never answers used to wedge the whole
        // capture path — `pending_capture` stayed Some, so every later Capture
        // click was silently swallowed by the busy guard and the window could
        // stay hidden with nothing on screen. Give up loudly instead.
        if let Some(since) = self.state.pending_capture_since {
            // A priming grab is blocked on a human reading a dialog, so it gets
            // the longer leash.
            let limit = if self.state.capture_priming {
                PERMISSION_TIMEOUT
            } else {
                CAPTURE_TIMEOUT
            };
            if since.elapsed() > limit {
                crate::portal_capture::capture_log(|| {
                    format!("watchdog: no portal answer in {CAPTURE_TIMEOUT:?}; giving up")
                });
                self.state.pending_capture = None;
                self.state.pending_capture_since = None;
                self.state.region_pending = false;
                self.state.capture_attempt = None;
                self.state.capture_priming = false;
                self.restore_main_window(ctx, true);
                self.state.status_msg =
                    Some("Capture timed out — the desktop screenshot portal never answered".into());
            } else {
                // The result arrives on a worker thread, so keep frames coming.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        // Keep the window minimized while a capture is being set up.
        // This runs after the portal poll so a result arriving in this frame
        // clears the flag before we reach here (avoids a spurious Minimized(true)
        // on the same frame as Minimized(false)).
        if self.state.window_hidden_for_capture {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // Fire the priming grab once the window is back up and focused.
        if let Some((fire_after, mode)) = self.state.capture_prime_pending.clone() {
            if std::time::Instant::now() >= fire_after {
                self.state.capture_prime_pending = None;
                crate::toolbar::fire_permission_priming(&mut self.state, mode);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }

        // Fire the portal after the window-hide settle delay.
        if let Some((fire_after, mode)) = self.state.capture_hide_pending.clone() {
            if std::time::Instant::now() >= fire_after {
                self.state.capture_hide_pending = None;
                crate::toolbar::fire_capture(&mut self.state, mode);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }

        // Open a file named on the command line, once, on the first frame.
        if let Some(path) = self.state.startup_file.take() {
            crate::loader::load_file(&path, &mut self.state, ctx);
        }

        // Fire the --capture CLI request once on the first frame.
        if let Some(mode) = self.state.startup_capture.take() {
            crate::toolbar::trigger_capture_pub(&mut self.state, mode);
        }

        // Scheduled (timed) capture: when the countdown elapses, begin the
        // window-hide sequence. While waiting, surface a live countdown.
        if let Some((deadline, mode)) = self.state.capture_scheduled.clone() {
            let now = std::time::Instant::now();
            if now >= deadline {
                self.state.capture_scheduled = None;
                crate::toolbar::begin_hide_for_capture(&mut self.state, mode);
            } else {
                let remaining = (deadline - now).as_secs_f32();
                self.state.status_msg = Some(format!("Capturing in {:.1}s…", remaining));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        // Apply any pending cropped-image texture refresh before the frame draws.
        if let Some(img) = self.state.pending_texture_refresh.take() {
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            let pixels = rgba.into_raw();
            let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
            self.state.canvas.texture = Some(crate::texture_upload::load_texture_fitted(
                ctx,
                "canvas",
                ci,
                egui::TextureOptions::default(),
            ));
            // Source pixels changed (crop/resize/adjust — `color_image` was
            // already updated by image_ops). Invalidate the session blob cache
            // so the new image is persisted on the next autosave.
            self.state.canvas.mark_source_changed();
        }

        // Shortcut recording: intercept the next keypress for the action being configured.
        if self.state.shortcut_recording.is_some() {
            let result = ctx.input(|i| capture_from_input(i));
            if let Some(outcome) = result {
                let action = self.state.shortcut_recording.take().unwrap();
                if let Ok(combo) = outcome {
                    self.state.settings.shortcuts.retain(|_, v| *v != combo);
                    self.state.settings.shortcuts.insert(action, combo);
                    save_shortcuts(&self.state.settings.shortcuts);
                }
                ctx.request_repaint();
            } else {
                ctx.request_repaint();
            }
        }

        // Normal shortcut dispatch — skip while editing text or recording.
        let editing_text = matches!(
            self.state.tool_state,
            crate::state::ToolState::EditingText { .. }
        );
        let mut paste_clipboard_image = std::mem::take(&mut self.state.request_paste);
        let mut copy_canvas_to_clipboard = false;
        let mut cut_selection = false;
        let mut duplicate_annotation = false;
        let mut select_all = false;
        let mut close_tab = false;
        let mut new_canvas = false;
        let mut save_image = false;
        let mut next_tab = false;
        let mut prev_tab = false;
        if !editing_text && self.state.shortcut_recording.is_none() {
            let sc = self.state.settings.shortcuts.clone();
            let mut trigger_region = false;
            let mut trigger_full = false;
            let mut do_export = false;
            let in_native_inpaint = self.state.app_mode == crate::state::AppMode::NativeInpaint;
            ctx.input(|i| {
                macro_rules! pressed {
                    ($action:expr) => {
                        sc.get(&$action).map(|k| k.matches(i)).unwrap_or(false)
                    };
                }
                if !in_native_inpaint && pressed!(Action::Undo) {
                    CommandExecutor::undo(&mut self.state.canvas, ParticipantId::HUMAN_LOCAL);
                }
                if !in_native_inpaint && pressed!(Action::Redo) {
                    CommandExecutor::redo(&mut self.state.canvas, ParticipantId::HUMAN_LOCAL);
                }
                if pressed!(Action::ToolSelect) {
                    self.state.set_current_tool(Tool::Select);
                }
                if pressed!(Action::ToolPen) {
                    self.state.set_current_tool(Tool::Pen);
                }
                if pressed!(Action::ToolRect) {
                    self.state.set_current_tool(Tool::Rect);
                }
                if pressed!(Action::ToolOval) {
                    self.state.set_current_tool(Tool::Oval);
                }
                if pressed!(Action::ToolStar) {
                    self.state.set_current_tool(Tool::Star);
                }
                if pressed!(Action::ToolArrow) {
                    self.state.set_current_tool(Tool::Arrow);
                }
                if pressed!(Action::ToolLine) {
                    self.state.set_current_tool(Tool::Line);
                }
                if pressed!(Action::ToolText) {
                    self.state.set_current_tool(Tool::Text);
                }
                if pressed!(Action::ToolBlur) {
                    self.state.set_current_tool(Tool::Blur);
                }
                if pressed!(Action::ToolCrop) {
                    self.state.set_current_tool(Tool::Crop);
                }
                if pressed!(Action::ToolEyedropper) {
                    self.state.set_current_tool(Tool::Eyedropper);
                }
                if pressed!(Action::ToolStep) {
                    self.state.set_current_tool(Tool::Step);
                }
                if pressed!(Action::ToggleMagnifier) {
                    self.state.settings.magnifier = !self.state.settings.magnifier;
                }
                if pressed!(Action::ResetZoom) {
                    self.state.view_zoom = 1.0;
                    self.state.view_pan = egui::Vec2::ZERO;
                }
                if pressed!(Action::CaptureRegion) {
                    trigger_region = true;
                }
                if pressed!(Action::CaptureFullScreen) {
                    trigger_full = true;
                }
                if pressed!(Action::ExportSidecar) {
                    do_export = true;
                }
                if pressed!(Action::PasteImage) {
                    paste_clipboard_image = true;
                }
                // Ctrl+C (non-configurable: would conflict with text copy in most contexts)
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::C) {
                    copy_canvas_to_clipboard = true;
                }
                // Ctrl+X (cut) — only meaningful with a selection.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::X) {
                    cut_selection = true;
                }
                // Ctrl+D — duplicate selected annotation.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::D) {
                    duplicate_annotation = true;
                }
                // Ctrl+A — select all annotations.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::A) {
                    select_all = true;
                }
                // Ctrl+W — close current tab.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::W) {
                    close_tab = true;
                }
                // Ctrl+N — new canvas dialog.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::N) {
                    new_canvas = true;
                }
                // Ctrl+S — save image.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::S) {
                    save_image = true;
                }
                // Ctrl+Tab / Ctrl+Shift+Tab — cycle through tabs.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Tab) {
                    next_tab = true;
                }
                if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Tab) {
                    prev_tab = true;
                }
                // Ctrl+H — toggle global annotation visibility.
                if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::H) {
                    self.state.annotations_visible = !self.state.annotations_visible;
                    self.state.status_msg = Some(if self.state.annotations_visible {
                        "Annotations shown".into()
                    } else {
                        "Annotations hidden".into()
                    });
                }
            });
            // Esc closes the New Canvas dialog first (modal-like): consume the
            // key so it doesn't also cancel a tool action or clear the canvas
            // selection in the same frame.
            if self.state.new_canvas_dialog.is_some()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.state.new_canvas_dialog = None;
            }
            // Esc clears the marquee selection (outside input closure to avoid borrow issues).
            // Skip if a capture is in flight (handled above) or the adjust dialog is open.
            let cancelled_tool_action = ctx.input_mut(|i| {
                if !capture_in_flight && self.state.has_active_tool_action() {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                } else {
                    false
                }
            });
            if cancelled_tool_action {
                self.state.cancel_active_tool_action();
            }
            let esc_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if esc_pressed
                && !capture_in_flight
                && self.state.canvas.selection.is_some()
                && self.state.canvas.adjust_preview.is_none()
            {
                self.state.canvas.selection = None;
            }
            if trigger_region {
                crate::toolbar::trigger_capture_pub(
                    &mut self.state,
                    crate::state::SnapMode::Region,
                );
            }
            if trigger_full {
                crate::toolbar::trigger_capture_pub(&mut self.state, crate::state::SnapMode::Full);
            }
            if do_export {
                crate::export::export(&self.state);
            }

            // Ctrl+A: select all annotations (requires Select tool or switches to it).
            if select_all && self.state.current_tool == Tool::Select {
                let all_ids: Vec<uuid::Uuid> = self
                    .state
                    .canvas
                    .annotations
                    .iter()
                    .filter(|a| !a.locked)
                    .map(|a| a.id)
                    .collect();
                self.state.tool_state = match all_ids.len() {
                    0 => crate::state::ToolState::Idle,
                    1 => crate::state::ToolState::Selected {
                        id: all_ids[0],
                        drag_offset: None,
                    },
                    _ => crate::state::ToolState::MultiSelected {
                        ids: all_ids,
                        drag_offset: None,
                        band_start: None,
                        band_current: None,
                    },
                };
            }
        }

        if copy_canvas_to_clipboard {
            // Selection-aware copy: if there's an active marquee, copy just those pixels.
            if self.state.canvas.selection.is_some() {
                if let Some(img) = crate::image_ops::copy_selection_pixels(&self.state) {
                    copy_rgba_via_clipboard(&mut self.state, img, "Copied selection to clipboard");
                } else {
                    self.state.status_msg = Some("Selection empty".into());
                }
            } else {
                crate::toolbar::copy_image_with_flatten_pub(&mut self.state);
            }
        }

        if cut_selection {
            if self.state.canvas.selection.is_some() {
                if let Some(img) = crate::image_ops::cut_selection(&mut self.state) {
                    copy_rgba_via_clipboard(&mut self.state, img, "Cut selection to clipboard");
                }
            }
        }

        if duplicate_annotation {
            if let crate::state::ToolState::Selected { id, .. } = self.state.tool_state {
                crate::toolbar::duplicate_selected(&mut self.state, id);
            }
        }

        // Tab navigation only applies to file-based tools (the folder/standalone
        // tools manage their own image set and have no file tabs).
        let file_based = self.state.app_mode.is_file_based();

        if close_tab && file_based {
            let pos = self.state.active_tab_pos;
            self.state.close_tab(pos);
        }

        if new_canvas && self.state.new_canvas_dialog.is_none() {
            crate::toolbar::open_new_canvas_dialog(&mut self.state);
        }

        if save_image && self.state.canvas.texture.is_some() {
            crate::toolbar::save_image_with_flatten_pub(&mut self.state);
        }

        if next_tab && file_based && self.state.tab_count() > 1 {
            let n = (self.state.active_tab_pos + 1) % self.state.tab_count();
            self.state.switch_to_tab(n);
        }

        if prev_tab && file_based && self.state.tab_count() > 1 {
            let count = self.state.tab_count();
            let n = (self.state.active_tab_pos + count - 1) % count;
            self.state.switch_to_tab(n);
        }

        if paste_clipboard_image {
            match arboard::Clipboard::new().and_then(|mut cb| cb.get_image()) {
                Ok(img) => {
                    let buf = image::RgbaImage::from_raw(
                        img.width as u32,
                        img.height as u32,
                        img.bytes.into_owned(),
                    );
                    if let Some(buf) = buf {
                        if self.state.canvas.texture.is_some() {
                            // A canvas is loaded — drop the pasted image in as a
                            // movable/resizable overlay instead of opening a new tab.
                            paste_as_image_layer(&mut self.state, buf);
                        } else {
                            self.state.push_current_as_tab_and_reset("Clipboard".into());
                            crate::loader::apply_image(
                                image::DynamicImage::ImageRgba8(buf),
                                &mut self.state,
                                ctx,
                            );
                            self.state.status_msg = Some("Pasted image from clipboard".into());
                        }
                    } else {
                        self.state.status_msg = Some("Clipboard image had bad dimensions".into());
                    }
                }
                Err(e) => self.state.status_msg = Some(format!("No image on clipboard: {e}")),
            }
        }

        // Drag-and-drop file loading
        let dropped_path = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped_path {
            crate::loader::load_file(&path, &mut self.state, ctx);
        }

        // ── Region selector takes over the whole window while it is up ──────
        if self.state.region_select.is_some() {
            self.drive_region_overlay(ctx);
            crate::toolbar::show_toast(ctx, &mut self.state);
            return;
        }

        // ── Row 1: Workspace switcher (always visible) ──────────────────────
        egui::TopBottomPanel::top("tool_bar")
            .frame(
                egui::Frame::none()
                    .fill(theme.chrome_bg)
                    .inner_margin(egui::Margin::symmetric(8.0, 2.0)),
            )
            .show(ctx, |ui| {
                crate::toolbar::show_tool_bar(ui, &mut self.state);
            });

        // Keep the embedded lama webview hidden unless its workspace is active.
        #[cfg(feature = "lama-webview")]
        crate::lama_cleaner::sync_webview_visibility(
            &mut self.state.lama_cleaner,
            self.state.app_mode == crate::state::AppMode::LamaCleaner,
        );

        let file_based = self.state.app_mode.is_file_based();

        // ── Row 2: Menu bar (file workspaces only) ──────────────────────────
        if file_based {
            egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
                if self.state.app_mode == crate::state::AppMode::Annotate {
                    crate::toolbar::show_menu_bar(ui, &mut self.state);
                } else {
                    crate::toolbar::show_file_only_bar(ui, &mut self.state);
                }
            });
            // New Canvas dialog is reachable from the shared File menu.
            crate::toolbar::show_new_canvas_dialog(ctx, &mut self.state);
        }

        // ── Row 3: File-tab bar (shown in every mode; disabled in folder modes) ─
        if self.state.should_show_tab_bar() {
            let interactive = self.state.app_mode.file_tabs_interactive();
            egui::TopBottomPanel::top("tab_bar")
                .frame(
                    egui::Frame::none()
                        .fill(theme.tab_canvas_bg)
                        .inner_margin(egui::Margin::symmetric(4.0, 2.0)),
                )
                .show(ctx, |ui| {
                    ui.add_enabled_ui(interactive, |ui| {
                        crate::toolbar::show_tab_bar(ui, &mut self.state);
                    });
                });
        }

        // ── Body: dispatch to the active workspace ──────────────────────────
        match self.state.app_mode {
            crate::state::AppMode::CanvasSort => {
                crate::canvas_sort::show_canvas_sort(ctx, &mut self.state.canvas_sort);
            }
            crate::state::AppMode::LamaCleaner => {
                crate::lama_cleaner::show(ctx, frame, &mut self.state.lama_cleaner);
            }
            crate::state::AppMode::NativeInpaint => {
                crate::native_inpaint::show(ctx, &mut self.state);
                // The Native Inpaint panel can't call the bin-only toolbar module,
                // so it requests a save via a flag we honor here.
                if self.state.native_inpaint.request_save {
                    self.state.native_inpaint.request_save = false;
                    crate::toolbar::save_image(&mut self.state);
                }
            }
            crate::state::AppMode::SvgEditor => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.heading("SVG Editor");
                        ui.label(
                            egui::RichText::new("Coming soon — vector editing will move here.")
                                .weak(),
                        );
                    });
                });
            }
            crate::state::AppMode::Annotate => {
                self.show_annotate_body(ctx);
            }
        }

        // ── Status toast (all modes) ────────────────────────────────────────
        if crate::toolbar::show_toast(ctx, &mut self.state) {
            // Keep frames coming so the fade animates even when idle.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        // ── Session autosave (debounced; runs in every mode) ────────────────
        if self.state.canvas.texture.is_some() || !self.state.canvas.annotations.is_empty() {
            self.state.session.mark_dirty();
        }
        crate::session::maybe_autosave(&mut self.state);
    }
}
