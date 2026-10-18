use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use eframe::egui;

use crate::{
    command::CommandExecutor,
    state::AppState,
    types::{Annotation, Command, ParticipantId, ShapeData, TextureCell},
};

use super::{
    history::InpaintAction,
    job,
    mask::{MaskBuffer, MaskPatch},
    models,
    state::MaskTool,
};

/// Convert an `image::RgbaImage` into an egui `ColorImage`; exact for the opaque
/// images the inpaint engine emits.
fn rgba_to_color_image(img: &image::RgbaImage) -> egui::ColorImage {
    let (w, h) = img.dimensions();
    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw())
}

/// Re-derive the working composite from the active canvas: base image +
/// annotations/shapes + prior inpaint result layers, flattened at image-pixel
/// resolution. Bumps `flattened_rev` so the base-texture cache refreshes.
fn reflatten(state: &mut AppState) {
    let canvas = &state.canvas;
    let size = canvas
        .color_image
        .as_ref()
        .map(|ci| egui::vec2(ci.size[0] as f32, ci.size[1] as f32))
        .unwrap_or(canvas.canvas_size);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    state.native_inpaint.flattened = crate::flatten::flatten(
        &canvas.annotations,
        canvas.color_image.as_ref(),
        canvas.canvas_size,
        rect,
    );
    state.native_inpaint.flattened_rev = state.native_inpaint.flattened_rev.wrapping_add(1);
}

/// Build a translucent-red overlay `ColorImage` from the mask: set pixels →
/// semi-transparent red, unset → fully transparent.
fn mask_overlay_color_image(mask: &image::GrayImage) -> egui::ColorImage {
    let (w, h) = mask.dimensions();
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for px in mask.pixels() {
        if px.0[0] > 0 {
            pixels.push(egui::Color32::from_rgba_unmultiplied(255, 32, 32, 128));
        } else {
            pixels.push(egui::Color32::TRANSPARENT);
        }
    }
    egui::ColorImage {
        size: [w as usize, h as usize],
        pixels,
    }
}

/// Commit the inpaint result as a non-destructive, undoable ImageLayer
/// annotation covering the full image rect in base (image-pixel) coords.
fn commit_result_layer(state: &mut AppState, img: image::RgbaImage, rect: egui::Rect) {
    let shape = ShapeData::ImageLayer {
        rect,
        data: Arc::new(img),
        texture: TextureCell::default(),
    };
    let ann = Annotation::new(shape, egui::Color32::WHITE, 1.0);
    CommandExecutor::execute(
        &mut state.canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(ann),
    );
    state.status_msg = Some("Inpaint result committed as a layer (undoable)".into());
    state.native_inpaint.last_status = Some(
        "Result committed as a layer (undoable). Mask cleared — ready for the next run.".into(),
    );
}

pub fn show(ctx: &egui::Context, state: &mut AppState) {
    // Kind of the currently-selected model (drives generative controls / dispatch).
    let selected_is_generative = {
        let manifest = models::Manifest::load_embedded();
        manifest
            .get(&state.native_inpaint.model_id)
            .map(|m| m.kind == models::ModelKind::Generative)
            .unwrap_or(false)
    };

    // Undo/redo intent, collected from buttons + keyboard and applied once below.
    let mut do_undo = false;
    let mut do_redo = false;

    // ── Top panel: tools & status ─────────────────────────────────────────
    egui::TopBottomPanel::top("native_inpaint_top").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Native Inpaint");
            ui.separator();
            if ui
                .button("📂 Open")
                .on_hover_text("Open an image (also opens it in the RedLine editor)")
                .clicked()
            {
                crate::toolbar::open_image_via_dialog(state, ctx);
            }
            ui.separator();
            // Model picker (manifest-driven).
            let manifest = models::Manifest::load_embedded();
            let cur = state.native_inpaint.model_id.clone();
            egui::ComboBox::from_id_salt("native_inpaint_model")
                .selected_text(cur.clone())
                .show_ui(ui, |ui| {
                    for m in &manifest.models {
                        if m.kind == models::ModelKind::Generative
                            && !cfg!(feature = "native-generative")
                        {
                            continue;
                        }
                        if ui
                            .selectable_label(state.native_inpaint.model_id == m.id, &m.id)
                            .clicked()
                            && state.native_inpaint.model_id != m.id
                        {
                            state.native_inpaint.model_id = m.id.clone();
                            // Force a reload on next removal; the old device badge
                            // no longer reflects a loaded model.
                            state.native_inpaint.backend = None;
                            state.native_inpaint.backend_model_id = None;
                            state.native_inpaint.device = None;
                        }
                    }
                });
            // Persistent compute-device badge.
            if let Some(dev) = &state.native_inpaint.device {
                let color = if dev.starts_with("GPU") {
                    egui::Color32::from_rgb(120, 200, 120)
                } else {
                    egui::Color32::from_rgb(200, 180, 80)
                };
                ui.label(egui::RichText::new(format!("● {dev}")).color(color))
                    .on_hover_text("Compute device used by the inpaint engine");
            }
            // ── Load / Unload the model (free or pre-warm GPU VRAM) ────────
            ui.separator();
            let busy = state.native_inpaint.running;
            if state.native_inpaint.backend.is_some() {
                if ui
                    .add_enabled(!busy, egui::Button::new("⏏ Unload model"))
                    .on_hover_text(
                        "Free the model from GPU memory (VRAM). The app stays open; \
                         it reloads automatically on the next run, or via Load model.",
                    )
                    .clicked()
                {
                    // Dropping the cached Arc frees the model + CUDA context (no
                    // in-flight job holds a clone while idle).
                    let ni = &mut state.native_inpaint;
                    ni.backend = None;
                    ni.backend_model_id = None;
                    ni.device = None;
                    ni.last_error = None;
                    ni.last_status = Some("Model unloaded — GPU memory freed.".into());
                }
            } else if ui
                .add_enabled(!busy, egui::Button::new("⬇ Load model"))
                .on_hover_text(
                    "Load the selected model onto the GPU now (otherwise it loads \
                     automatically the first time you run).",
                )
                .clicked()
            {
                let ni = &mut state.native_inpaint;
                ni.cancel = Arc::new(AtomicBool::new(false));
                ni.cancel_requested = false;
                ni.running = true;
                ni.last_error = None;
                ni.last_status = None;
                ni.stage = Some("Loading model…".into());
                job::spawn_load(
                    ni.model_id.clone(),
                    selected_is_generative,
                    ni.gen_scheduler.clone(),
                    ni.cancel.clone(),
                    ni.tx.clone(),
                );
            }
        });

        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.native_inpaint.tool, MaskTool::Brush, "🖌 Brush");
            ui.selectable_value(&mut state.native_inpaint.tool, MaskTool::Wand, "🪄 Wand");
            ui.separator();
            match state.native_inpaint.tool {
                MaskTool::Brush => {
                    ui.label("Brush");
                    ui.add(
                        egui::Slider::new(&mut state.native_inpaint.brush_radius, 2.0..=200.0)
                            .text("radius"),
                    );
                    ui.separator();
                    ui.selectable_value(&mut state.native_inpaint.paint_mode, true, "Paint");
                    ui.selectable_value(&mut state.native_inpaint.paint_mode, false, "Erase");
                }
                MaskTool::Wand => {
                    ui.label("Tolerance");
                    ui.add(egui::Slider::new(
                        &mut state.native_inpaint.wand_tolerance,
                        0..=128,
                    ));
                    ui.separator();
                    ui.selectable_value(&mut state.native_inpaint.paint_mode, true, "Add");
                    ui.selectable_value(&mut state.native_inpaint.paint_mode, false, "Subtract");
                }
            }
            ui.separator();
            if ui.button("Clear mask").clicked() {
                let before = state
                    .native_inpaint
                    .mask
                    .as_ref()
                    .map(|m| m.image().clone());
                if let Some(mask) = state.native_inpaint.mask.as_mut() {
                    mask.clear();
                }
                let patch = match (before, state.native_inpaint.mask.as_ref()) {
                    (Some(b), Some(m)) => MaskPatch::between(&b, m.image()),
                    _ => None,
                };
                if let Some(patch) = patch {
                    state
                        .native_inpaint
                        .history
                        .push(InpaintAction::MaskEdit(patch));
                }
            }
            ui.separator();
            let can_undo = state.native_inpaint.history.can_undo();
            let can_redo = state.native_inpaint.history.can_redo();
            if ui
                .add_enabled(can_undo, egui::Button::new("↩ Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                do_undo = true;
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("↪ Redo"))
                .on_hover_text("Ctrl+Shift+Z / Ctrl+Y")
                .clicked()
            {
                do_redo = true;
            }
            if ui
                .button("Reset view")
                .on_hover_text("Fit to window (shortcut: 0)")
                .clicked()
            {
                state.native_inpaint.zoom = 1.0;
                state.native_inpaint.pan = egui::Vec2::ZERO;
            }
            ui.separator();
            ui.checkbox(&mut state.native_inpaint.auto_apply, "Auto-apply")
                .on_hover_text("Run automatically when you finish a brush stroke");
        });

        // ── Generative controls (prompt / params) ─────────────────────────
        if selected_is_generative {
            let ni = &mut state.native_inpaint;
            ui.horizontal(|ui| {
                ui.label("Prompt");
                ui.add(
                    egui::TextEdit::singleline(&mut ni.gen_prompt)
                        .hint_text("what to paint into the masked region")
                        .desired_width(360.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Negative");
                ui.add(
                    egui::TextEdit::singleline(&mut ni.gen_negative)
                        .hint_text("what to avoid")
                        .desired_width(360.0),
                );
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut ni.gen_steps, 1..=80).text("steps"));
                ui.add(egui::Slider::new(&mut ni.gen_strength, 0.0..=1.0).text("strength"));
                ui.add(egui::Slider::new(&mut ni.gen_guidance, 0.0..=20.0).text("cfg"));
            });
            ui.horizontal(|ui| {
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut ni.gen_seed));
                if ui.button("🎲").on_hover_text("Randomize seed").clicked() {
                    let s = &mut ni.gen_seed;
                    *s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                }
                ui.separator();
                ui.label("Scheduler");
                let schedulers = ni.gen_schedulers.clone();
                let cur_sched = ni.gen_scheduler.clone();
                egui::ComboBox::from_id_salt("native_inpaint_scheduler")
                    .selected_text(cur_sched)
                    .show_ui(ui, |ui| {
                        for s in &schedulers {
                            ui.selectable_value(&mut ni.gen_scheduler, s.clone(), s);
                        }
                    });
            });
        }

        // Progress / status row.
        if let Some(p) = state.native_inpaint.progress {
            ui.add(egui::ProgressBar::new(p).text("Downloading model…"));
        } else if state.native_inpaint.running {
            let stage = state
                .native_inpaint
                .stage
                .clone()
                .unwrap_or_else(|| "Working…".to_string());
            if let Some(f) = state.native_inpaint.infer_progress {
                ui.add(egui::ProgressBar::new(f).text(stage));
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(stage);
                });
            }
        }
        if let Some(err) = &state.native_inpaint.last_error {
            ui.colored_label(egui::Color32::RED, err);
        }
        if let Some(status) = &state.native_inpaint.last_status {
            ui.colored_label(egui::Color32::from_rgb(120, 200, 120), status);
        }
    });

    // No image loaded → hint and bail.
    if state.canvas.color_image.is_none() {
        let result_waiting = state.native_inpaint.result.is_some();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.label(
                    "Open an image in RedLine first, then return to this tab to paint a mask.",
                );
                ui.add_space(6.0);
                ui.weak("Paint over what you want to remove (or use the Wand), then click Remove.");
                if result_waiting {
                    ui.add_space(8.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 180, 80),
                        "A removal result is waiting — re-open an image to place it.",
                    );
                }
            });
        });
        return;
    }

    // ── Live sync: bind to the stable canvas_uid; reset on document change,
    // re-flatten on content change. Source the working image from the flattened
    // composite (base + annotations + prior inpaint layers). ──────────────
    let cur_uid = state.canvas.canvas_uid;
    let cur_rev = state.canvas.current_rev();
    match super::state::decide_sync(
        state.native_inpaint.synced_uid,
        state.native_inpaint.synced_rev,
        cur_uid,
        cur_rev,
    ) {
        super::state::SyncAction::Reset => {
            state.native_inpaint.reset_for_uid(cur_uid);
            reflatten(state);
            state.native_inpaint.synced_rev = cur_rev;
        }
        super::state::SyncAction::Reflatten => {
            reflatten(state);
            state.native_inpaint.synced_rev = cur_rev;
        }
        super::state::SyncAction::NoChange => {}
    }
    let Some(work) = state.native_inpaint.flattened.clone() else {
        return;
    };
    let (iw, ih) = work.dimensions();

    // Lazily (re)allocate the mask to match the source size.
    let needs_new_mask = match state.native_inpaint.mask.as_ref() {
        None => true,
        Some(m) => m.dimensions() != (iw, ih),
    };
    if needs_new_mask {
        // A size change always routes through reset_for_uid first (flattened /
        // base texture already cleared); here we just (re)allocate the mask.
        state.native_inpaint.mask = Some(MaskBuffer::new(iw, ih));
        state.native_inpaint.mask_tex = None;
    }

    // ── Undo/redo: keyboard (Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y) + button intent.
    // The global handler is suppressed in this mode (app.rs), so the panel owns
    // these. Ignore while a text field (prompt/negative) is focused.
    if ctx.memory(|m| m.focused().is_none()) {
        let (k_undo, k_redo) = ctx.input(|i| {
            let ctrl = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            (ctrl && !shift && z, ctrl && (y || (shift && z)))
        });
        if k_undo {
            do_undo = true;
        }
        if k_redo {
            do_redo = true;
        }
    }
    if do_undo || do_redo {
        let canvas = &mut state.canvas;
        let ni = &mut state.native_inpaint;
        if do_undo {
            ni.history.undo(ni.mask.as_mut(), canvas);
        } else if do_redo {
            ni.history.redo(ni.mask.as_mut(), canvas);
        }
        // Applied-result undo mutated the canvas; re-flatten + texture refresh
        // happen on the next frame via live sync.
        ctx.request_repaint();
    }

    // ── Bug #3: only commit a finished result if the canvas still shows the
    // image it was computed for, and the user didn't cancel. ───────────────
    // (A cancelled result is already dropped in `drain_msgs`; here we only guard
    // that the canvas still shows the image the result was computed for.)
    let pending_result = {
        let ni = &mut state.native_inpaint;
        if ni.result.is_some() {
            if ni.job_uid == Some(cur_uid) {
                ni.take_committed_result()
            } else {
                ni.result = None;
                None
            }
        } else {
            None
        }
    };

    // ── Bug #4: cache the base texture; rebuild only when the composite changes ─
    if state.native_inpaint.base_tex.is_none()
        || state.native_inpaint.base_tex_token != state.native_inpaint.flattened_rev
    {
        let ci = rgba_to_color_image(&work);
        state.native_inpaint.base_tex = Some(crate::texture_upload::load_texture_fitted(
            ctx,
            "native-inpaint-base",
            ci,
            egui::TextureOptions::LINEAR,
        ));
        state.native_inpaint.base_tex_token = state.native_inpaint.flattened_rev;
    }

    // ── Bug #4: cache the mask overlay texture; rebuild only on mask change ─
    if let Some(mask) = state.native_inpaint.mask.as_ref() {
        let v = mask.version();
        if state.native_inpaint.mask_tex.is_none() || state.native_inpaint.mask_tex_version != v {
            let overlay = mask_overlay_color_image(mask.image());
            state.native_inpaint.mask_tex = Some(crate::texture_upload::load_texture_fitted(
                ctx,
                "native-inpaint-mask",
                overlay,
                egui::TextureOptions::NEAREST,
            ));
            state.native_inpaint.mask_tex_version = v;
        }
    }

    let running = state.native_inpaint.running;
    let mut brush_radius = state.native_inpaint.brush_radius;
    let paint_mode = state.native_inpaint.paint_mode;
    let tool = state.native_inpaint.tool;
    let wand_tol = state.native_inpaint.wand_tolerance;
    let auto_apply = state.native_inpaint.auto_apply;
    // Whether the mask currently has any painted pixels (drives the action
    // button, Enter, and auto-apply).
    let has_mask = state
        .native_inpaint
        .mask
        .as_ref()
        .map(|m| m.any_set())
        .unwrap_or(false);

    // Collected side effects (created inside the closure, applied after).
    let mut start_removal = false;
    let mut wand_click: Option<(u32, u32)> = None;
    // Set when a brush stroke ends this frame (for auto-apply).
    let mut stroke_released = false;

    egui::CentralPanel::default().show(ctx, |ui| {
        let panel_rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(panel_rect, egui::Sense::click_and_drag());

        // Fit-to-window base scale (× zoom). Never divide by zero.
        let base_fit = (panel_rect.width() / iw.max(1) as f32)
            .min(panel_rect.height() / ih.max(1) as f32)
            .max(0.001);
        let zoom = state.native_inpaint.zoom;
        let scale = base_fit * zoom;
        let draw = egui::vec2(iw as f32 * scale, ih as f32 * scale);
        let img_min = panel_rect.center() - draw / 2.0 + state.native_inpaint.pan;
        let img_rect = egui::Rect::from_min_size(img_min, draw);

        // Pointer → image pixel coords.
        let to_image = |pos: egui::Pos2| -> (f32, f32) {
            (
                (pos.x - img_rect.min.x) / scale,
                (pos.y - img_rect.min.y) / scale,
            )
        };

        // ── Zoom (mouse wheel, cursor-anchored) & pan (middle-drag) ───────────
        let (scroll_y, hover_pos) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if response.hovered() && scroll_y.abs() > 0.0 {
            if let Some(p) = hover_pos {
                let (ix, iy) = to_image(p);
                let factor = (scroll_y * 0.0015).exp();
                let new_zoom = (zoom * factor).clamp(0.1, 20.0);
                let new_scale = base_fit * new_zoom;
                let new_draw = egui::vec2(iw as f32 * new_scale, ih as f32 * new_scale);
                let new_min = egui::pos2(p.x - ix * new_scale, p.y - iy * new_scale);
                state.native_inpaint.zoom = new_zoom;
                state.native_inpaint.pan = (new_min - panel_rect.center()) + new_draw / 2.0;
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            state.native_inpaint.pan += response.drag_delta();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num0)) {
            state.native_inpaint.zoom = 1.0;
            state.native_inpaint.pan = egui::Vec2::ZERO;
        }

        // Base image.
        if let Some(tex) = &state.native_inpaint.base_tex {
            ui.painter().image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // ── Bug #2 / tool input ───────────────────────────────────────────
        if !running {
            let cmd = ui.input(|i| i.modifiers.command);
            let primary_drag = response.dragged_by(egui::PointerButton::Primary)
                || response.drag_started_by(egui::PointerButton::Primary);
            match tool {
                MaskTool::Brush => {
                    // Snapshot the mask once at stroke start to build a history
                    // patch when the stroke is released (Ctrl-resize is not a stroke).
                    if response.drag_started_by(egui::PointerButton::Primary)
                        && !cmd
                        && state.native_inpaint.stroke_before.is_none()
                    {
                        state.native_inpaint.stroke_before = state
                            .native_inpaint
                            .mask
                            .as_ref()
                            .map(|m| m.image().clone());
                    }
                    if primary_drag && cmd {
                        // Ctrl+drag horizontally → resize the brush (don't paint).
                        let dx = response.drag_delta().x;
                        if dx != 0.0 {
                            brush_radius = (brush_radius + dx * 0.5).clamp(2.0, 200.0);
                            state.native_inpaint.brush_radius = brush_radius;
                        }
                        state.native_inpaint.last_paint = None;
                    } else if primary_drag {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let (ix, iy) = to_image(pos);
                            if ix >= 0.0 && iy >= 0.0 && ix < iw as f32 && iy < ih as f32 {
                                if let Some(mask) = state.native_inpaint.mask.as_mut() {
                                    match state.native_inpaint.last_paint {
                                        Some((lx, ly)) => mask.stamp_line(
                                            lx,
                                            ly,
                                            ix,
                                            iy,
                                            brush_radius,
                                            paint_mode,
                                        ),
                                        None => mask.stamp(ix, iy, brush_radius, paint_mode),
                                    }
                                }
                                state.native_inpaint.last_paint = Some((ix, iy));
                            }
                        }
                    }
                    // Stroke ended this frame → flag for auto-apply (but not when
                    // the gesture was a Ctrl-resize).
                    if response.drag_stopped_by(egui::PointerButton::Primary) {
                        if !cmd {
                            stroke_released = true;
                            let patch = match (
                                state.native_inpaint.stroke_before.take(),
                                state.native_inpaint.mask.as_ref(),
                            ) {
                                (Some(before), Some(mask)) => {
                                    MaskPatch::between(&before, mask.image())
                                }
                                _ => None,
                            };
                            if let Some(patch) = patch {
                                state
                                    .native_inpaint
                                    .history
                                    .push(InpaintAction::MaskEdit(patch));
                            }
                        } else {
                            state.native_inpaint.stroke_before = None;
                        }
                        state.native_inpaint.last_paint = None;
                    }
                    if !response.dragged_by(egui::PointerButton::Primary) {
                        state.native_inpaint.last_paint = None;
                    }
                }
                MaskTool::Wand => {
                    if response.clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let (ix, iy) = to_image(pos);
                            if ix >= 0.0 && iy >= 0.0 && ix < iw as f32 && iy < ih as f32 {
                                wand_click = Some((ix as u32, iy as u32));
                            }
                        }
                    }
                }
            }
        }

        // Mask overlay.
        if let Some(tex) = &state.native_inpaint.mask_tex {
            ui.painter().image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // Brush cursor ring (shows size & position).
        if tool == MaskTool::Brush && !running {
            if let Some(p) = hover_pos {
                if img_rect.contains(p) {
                    ui.painter().circle_stroke(
                        p,
                        brush_radius * scale,
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 80, 80)),
                    );
                }
            }
        }

        // Action row. (`has_mask` is computed once per frame above.)
        egui::Area::new("native_inpaint_actions".into())
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if running {
                            if ui.button("Cancel").clicked() {
                                state.native_inpaint.request_cancel();
                            }
                            ui.spinner();
                            ui.label(
                                state
                                    .native_inpaint
                                    .stage
                                    .clone()
                                    .unwrap_or_else(|| "Working…".into()),
                            );
                        } else {
                            let label = if selected_is_generative {
                                "Generate"
                            } else {
                                "Remove"
                            };
                            let resp = ui.add_enabled(has_mask, egui::Button::new(label));
                            if resp.clicked() {
                                start_removal = true;
                            }
                            if !has_mask {
                                resp.on_hover_text("Paint or wand-select a region first.");
                            }
                        }
                    });
                });
            });
    });

    // ── Feature #3: apply a wand click (flood-fill into the mask) ──────────
    if let Some(seed) = wand_click {
        // Use the flattened working composite as the colour source.
        let before = state
            .native_inpaint
            .mask
            .as_ref()
            .map(|m| m.image().clone());
        if let Some(mask) = state.native_inpaint.mask.as_mut() {
            mask.flood_fill(&work, seed, wand_tol, paint_mode);
        }
        let patch = match (before, state.native_inpaint.mask.as_ref()) {
            (Some(b), Some(m)) => MaskPatch::between(&b, m.image()),
            _ => None,
        };
        if let Some(patch) = patch {
            state
                .native_inpaint
                .history
                .push(InpaintAction::MaskEdit(patch));
        }
    }

    // ── Enter key + auto-apply trigger the current action ──────────────────
    // Recompute mask state so painting from THIS frame counts.
    let has_mask_now = state
        .native_inpaint
        .mask
        .as_ref()
        .map(|m| m.any_set())
        .unwrap_or(false);
    if !running && has_mask_now {
        // Enter runs the action — unless a text field (prompt/negative) is focused.
        let enter =
            ctx.input(|i| i.key_pressed(egui::Key::Enter)) && ctx.memory(|m| m.focused().is_none());
        if enter || (auto_apply && stroke_released) {
            start_removal = true;
        }
    }

    // ── Kick off a removal ────────────────────────────────────────────────
    if start_removal {
        let work_rgba = work.clone();
        let mask_img = state
            .native_inpaint
            .mask
            .as_ref()
            .map(|m| m.image().clone());
        if let Some(mask_img) = mask_img {
            let ni = &mut state.native_inpaint;
            ni.cancel = Arc::new(AtomicBool::new(false));
            ni.cancel_requested = false;
            ni.running = true;
            ni.job_uid = Some(cur_uid);
            ni.last_error = None;
            ni.last_status = None;
            ni.infer_progress = Some(0.0);
            ni.stage = Some("Starting…".into());
            // Reuse the cached backend only if it matches the selected model.
            let cached = if ni.backend_model_id.as_deref() == Some(ni.model_id.as_str()) {
                ni.backend.clone()
            } else {
                None
            };
            if selected_is_generative {
                let params = crate::native_inpaint::backend::InpaintParams {
                    model_id: ni.model_id.clone(),
                    steps: ni.gen_steps,
                    strength: ni.gen_strength,
                    seed: ni.gen_seed,
                    guidance: ni.gen_guidance,
                    negative_prompt: ni.gen_negative.clone(),
                    scheduler: ni.gen_scheduler.clone(),
                    ..Default::default()
                };
                job::spawn_generate(
                    cached,
                    ni.model_id.clone(),
                    work_rgba,
                    mask_img,
                    ni.gen_prompt.clone(),
                    params,
                    ni.cancel.clone(),
                    ni.tx.clone(),
                );
            } else {
                job::spawn_removal(
                    cached,
                    ni.model_id.clone(),
                    work_rgba,
                    mask_img,
                    ni.cancel.clone(),
                    ni.tx.clone(),
                );
            }
        }
    }

    if state.native_inpaint.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    // ── Commit a finished result as a non-destructive, undoable layer ──────
    if let Some(result) = pending_result {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(iw as f32, ih as f32));
        commit_result_layer(state, result, rect);
        state
            .native_inpaint
            .history
            .push(InpaintAction::AppliedResult);
        if let Some(mask) = state.native_inpaint.mask.as_mut() {
            mask.clear();
        }
    }
}
