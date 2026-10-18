use egui::{Key, Pos2, Rect, Response, Vec2};
use uuid::Uuid;

use super::state::{CanvasSortState, ToolInteraction};
use super::types::{ActionZone, CanvasSortSession, CanvasTool, ImageStack, SortImage, ZoneAction};
use super::undo::CanvasCommand;

/// Main input dispatcher — called each frame from render.rs.
pub fn handle_input(state: &mut CanvasSortState, resp: &Response, ctx: &egui::Context) {
    // ── Scroll wheel zoom ──────────────────────────────────────────────────
    // Use raw scroll delta directly so trackpads feel smooth and mouse wheels
    // are incremental rather than fixed 10% jumps.
    let scroll_delta = ctx.input(|i| i.smooth_scroll_delta.y);
    if resp.hovered() && scroll_delta.abs() > 0.1 {
        if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()) {
            let factor = 1.0 + scroll_delta * 0.004; // ~0.4% per scroll unit
            let new_zoom =
                (state.zoom * factor).clamp(super::state::ZOOM_MIN, super::state::ZOOM_MAX);
            state.zoom_at(new_zoom, cursor);
        }
    }

    // ── Middle-mouse / space drag to pan ──────────────────────────────────
    let space_held = ctx.input(|i| i.key_down(Key::Space));
    let middle_drag = resp.dragged_by(egui::PointerButton::Middle);
    if middle_drag || (space_held && resp.dragged_by(egui::PointerButton::Primary)) {
        state.pan -= resp.drag_delta() / state.zoom;
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }
    if space_held {
        ctx.set_cursor_icon(egui::CursorIcon::Grab);
    }

    // When a text field (session name, search, zone label, sticky note) is
    // focused, suppress canvas shortcuts so typing doesn't mutate the canvas.
    let typing = ctx.wants_keyboard_input();

    // ── Keyboard shortcuts ─────────────────────────────────────────────────
    let (
        do_undo,
        do_redo,
        do_delete,
        do_select_all,
        do_fit,
        do_minimap,
        do_grid,
        do_search,
        do_history,
        do_center,
    ) = ctx.input(|i| {
        (
            i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::Z),
            i.modifiers.ctrl
                && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
            i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
            i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::A),
            i.key_pressed(Key::F),
            i.key_pressed(Key::M),
            i.key_pressed(Key::G),
            i.modifiers.ctrl && i.key_pressed(Key::F),
            i.key_pressed(Key::H),
            i.key_pressed(Key::C),
        )
    });

    if do_undo {
        state.undo();
    }
    if do_redo {
        state.redo();
    }
    if !typing {
        if do_delete {
            // A selected note takes delete priority; otherwise remove images.
            if !state.delete_selected_note() {
                state.delete_selected();
            }
        }
        if do_select_all {
            state.select_all_images();
        }
        if do_fit {
            state.zoom_to_fit();
        }
        if do_minimap {
            state.show_minimap = !state.show_minimap;
        }
        if do_grid { /* grid overlay toggle — handled in render */ }
        if do_search {
            state.show_search = !state.show_search;
        }
        if do_history {
            state.show_history = !state.show_history;
        }
        if do_center {
            state.center_view();
        }
    }

    // Escape
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    if esc {
        if let ToolInteraction::EditingNote { note_id } = state.tool_state {
            // Always exit note editing on Esc, even while the text field has focus,
            // and clear the editing flag so the note's text renders again.
            if let Some(n) = state.sticky_notes.iter_mut().find(|n| n.id == note_id) {
                n.editing = false;
            }
            state.tool_state = ToolInteraction::Idle;
            state.dirty = true;
        } else if !typing {
            state.clear_selection();
            state.tool_state = ToolInteraction::Idle;
            state.collecting.clear();
            state.tool = CanvasTool::Select;
        }
    }

    // V = select tool, B = draw zone, N = annotation
    let (v_key, b_key, n_key) = ctx.input(|i| {
        (
            i.key_pressed(Key::V),
            i.key_pressed(Key::B),
            i.key_pressed(Key::N),
        )
    });
    if !typing {
        if v_key {
            state.tool = CanvasTool::Select;
            state.tool_state = ToolInteraction::Idle;
        }
        if b_key {
            state.tool = CanvasTool::DrawZone;
            state.tool_state = ToolInteraction::Idle;
        }
        if n_key {
            state.tool = CanvasTool::Annotation;
            state.tool_state = ToolInteraction::Idle;
        }
    }

    // Ctrl+S = manual save
    let do_save = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::S));
    if do_save {
        let _ = super::persistence::save_session(state);
        state.dirty = false;
        state.status = Some("Saved".into());
    }

    // ── Tool dispatch ─────────────────────────────────────────────────────
    match state.tool {
        CanvasTool::Select => handle_select(state, resp, ctx),
        CanvasTool::DrawZone => handle_draw_zone(state, resp, ctx),
        CanvasTool::Annotation => handle_annotation(state, resp, ctx),
        CanvasTool::StickyCollect => handle_sticky_collect(state, resp, ctx),
    }
}

// ── Select tool ───────────────────────────────────────────────────────────────

fn handle_select(state: &mut CanvasSortState, resp: &Response, ctx: &egui::Context) {
    let ctrl = ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);

    // Pointer position in canvas space
    let canvas_pos = resp.interact_pointer_pos().map(|p| state.to_canvas(p));

    match &state.tool_state {
        ToolInteraction::Idle => {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(cp) = canvas_pos {
                    let hit_img = hit_image(state, cp);
                    let hit_zone = hit_zone_handle(state, cp);
                    let hit_stack = hit_stack(state, cp);

                    if let Some((note_id, handle)) = super::notes::hit_note_handle(state, cp) {
                        // Resize handle of the selected note takes top priority.
                        let rect = state
                            .sticky_notes
                            .iter()
                            .find(|n| n.id == note_id)
                            .map(|n| n.rect)
                            .unwrap_or(Rect::ZERO);
                        state.tool_state = ToolInteraction::ResizingNote {
                            note_id,
                            handle,
                            drag_start_canvas: cp,
                            original_rect: rect,
                        };
                    } else if let Some(note_id) = super::notes::hit_note(&state.sticky_notes, cp) {
                        // Notes are drawn on top, so they grab clicks on top.
                        if !ctrl {
                            state.clear_selection();
                        }
                        state.selected_note = Some(note_id);
                        let rect = state
                            .sticky_notes
                            .iter()
                            .find(|n| n.id == note_id)
                            .map(|n| n.rect)
                            .unwrap_or(Rect::ZERO);
                        state.tool_state = ToolInteraction::MovingNote {
                            note_id,
                            drag_start_canvas: cp,
                            original_rect: rect,
                        };
                    } else {
                        // Clicking off any note clears note selection.
                        state.selected_note = None;

                        if let Some(zone_id) = hit_zone.0 {
                            // Zone resize handles take highest priority.
                            let rect = state
                                .session
                                .as_ref()
                                .and_then(|s| s.zone_by_id(zone_id))
                                .map(|z| z.canvas_rect())
                                .unwrap_or(Rect::ZERO);
                            state.tool_state = ToolInteraction::ResizingZone {
                                zone_id,
                                handle: hit_zone.1,
                                drag_start_canvas: cp,
                                original_rect: rect,
                            };
                        } else if let Some(img_id) = hit_img {
                            // Images take priority over zone bodies so dragging an image
                            // that sits inside a zone grabs the image, not the zone.
                            if !state.selected_images.contains(&img_id) {
                                if !ctrl {
                                    state.clear_selection();
                                }
                                state.selected_images.insert(img_id);
                            }
                            let originals: Vec<(Uuid, [f32; 2])> = state
                                .selected_images
                                .iter()
                                .filter_map(|id| {
                                    state
                                        .session
                                        .as_ref()?
                                        .image_by_id(*id)
                                        .map(|i| (*id, i.pos))
                                })
                                .collect();
                            state.tool_state = ToolInteraction::DraggingImages {
                                drag_start_canvas: cp,
                                originals,
                            };
                        } else if let Some(stack_id) = hit_stack {
                            let pos = state
                                .session
                                .as_ref()
                                .and_then(|s| s.stack_by_id(stack_id))
                                .map(|st| st.pos)
                                .unwrap_or_default();
                            if !state.selected_stacks.contains(&stack_id) {
                                if !ctrl {
                                    state.selected_stacks.clear();
                                    state.selected_images.clear();
                                }
                                state.selected_stacks.insert(stack_id);
                            }
                            state.tool_state = ToolInteraction::DraggingStack {
                                stack_id,
                                drag_start_canvas: cp,
                                original_pos: pos,
                            };
                        } else if let Some(zone_id) = hit_zone_body(state, cp) {
                            let z = state.session.as_ref().and_then(|s| s.zone_by_id(zone_id));
                            if z.map(|z| !z.is_pinned).unwrap_or(false) {
                                let rect = z.map(|z| z.canvas_rect()).unwrap_or(Rect::ZERO);
                                if !state.selected_zones.contains(&zone_id) {
                                    if !ctrl {
                                        state.selected_zones.clear();
                                    }
                                    state.selected_zones.insert(zone_id);
                                }
                                state.tool_state = ToolInteraction::DraggingZone {
                                    zone_id,
                                    drag_start_canvas: cp,
                                    original_rect: rect,
                                };
                            }
                        } else {
                            // Start marquee
                            if !ctrl {
                                state.clear_selection();
                            }
                            state.tool_state = ToolInteraction::Marquee {
                                start: cp,
                                current: cp,
                            };
                        }
                    }
                }
            } else if resp.double_clicked_by(egui::PointerButton::Primary) {
                if let Some(cp) = canvas_pos {
                    if let Some(note_id) = super::notes::hit_note(&state.sticky_notes, cp) {
                        state.selected_note = Some(note_id);
                        if let Some(n) = state.sticky_notes.iter_mut().find(|n| n.id == note_id) {
                            n.editing = true;
                        }
                        state.tool_state = ToolInteraction::EditingNote { note_id };
                        state.note_editor_focus = true;
                    } else if let Some(stack_id) = hit_stack(state, cp) {
                        if let Some(s) = &mut state.session {
                            if let Some(st) = s.stack_by_id_mut(stack_id) {
                                st.is_expanded = !st.is_expanded;
                            }
                        }
                    }
                }
            } else if resp.clicked_by(egui::PointerButton::Primary) {
                if let Some(cp) = canvas_pos {
                    let hit_img = hit_image(state, cp);
                    if hit_img.is_none()
                        && hit_zone_body(state, cp).is_none()
                        && hit_stack(state, cp).is_none()
                    {
                        if !ctrl {
                            state.clear_selection();
                        }
                    }
                }
            }
        }

        ToolInteraction::Marquee { start, .. } => {
            let start = *start;
            if let Some(cp) = canvas_pos {
                state.tool_state = ToolInteraction::Marquee { start, current: cp };
            }
            if resp.drag_stopped() {
                // Commit marquee selection
                if let ToolInteraction::Marquee { start, current } = state.tool_state {
                    let sel_rect = Rect::from_two_pos(start, current);
                    if let Some(s) = &state.session {
                        let newly: Vec<Uuid> = s
                            .images
                            .iter()
                            .filter(|i| i.stack_id.is_none() && sel_rect.intersects(i.rect()))
                            .map(|i| i.id)
                            .collect();
                        if !ctrl {
                            state.selected_images.clear();
                        }
                        for id in newly {
                            state.selected_images.insert(id);
                        }
                    }
                }
                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::DraggingImages {
            drag_start_canvas,
            originals,
        } => {
            let drag_start = *drag_start_canvas;
            let originals_snap = originals.clone();
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                if let Some(s) = &mut state.session {
                    for (id, orig) in &originals_snap {
                        if let Some(img) = s.image_by_id_mut(*id) {
                            if !img.is_locked {
                                img.pos = [orig[0] + delta.x, orig[1] + delta.y];
                            }
                        }
                    }
                }
            }
            if resp.drag_stopped() {
                // Build and record a MoveImages command (orig → final)
                let moves: Vec<(Uuid, [f32; 2], [f32; 2])> = originals_snap
                    .iter()
                    .filter_map(|(id, orig)| {
                        let new_pos = state.session.as_ref()?.image_by_id(*id)?.pos;
                        Some((*id, *orig, new_pos))
                    })
                    .collect();

                // Determine a deliberate stack target. Prefer the dragged image
                // sitting under the cursor; fall back to any selected image.
                let drop_pos = canvas_pos.unwrap_or(drag_start);
                let moving: Vec<Uuid> = state.selected_images.iter().copied().collect();
                let maybe_target = state
                    .session
                    .as_ref()
                    .and_then(|s| deliberate_drop_target(s, &moving, drop_pos));

                if let Some(target_id) = maybe_target {
                    // Stack the target plus every moving image into a new stack.
                    let center = state
                        .session
                        .as_ref()
                        .and_then(|s| s.image_by_id(target_id))
                        .map(|i| i.center())
                        .unwrap_or(drop_pos);
                    let mut members = vec![target_id];
                    members.extend(moving.iter().copied().filter(|id| *id != target_id));
                    let moved: Vec<(Uuid, Option<Uuid>)> = members
                        .iter()
                        .filter_map(|id| {
                            let old = state.session.as_ref()?.image_by_id(*id)?.stack_id;
                            Some((*id, old))
                        })
                        .collect();
                    let mut stack = ImageStack::new(center);
                    stack.image_ids = members;
                    state.execute(CanvasCommand::CreateStack { stack, moved });
                } else if !moves.is_empty() && moves.iter().any(|(_, f, t)| f != t) {
                    state.execute(CanvasCommand::MoveImages { moves });
                }

                // Check if dropped on a zone
                check_zone_drops(state, &originals_snap, drop_pos);

                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::DraggingZone {
            zone_id,
            drag_start_canvas,
            original_rect,
        } => {
            let zone_id = *zone_id;
            let drag_start = *drag_start_canvas;
            let orig = *original_rect;
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                if let Some(s) = &mut state.session {
                    if let Some(z) = s.zone_by_id_mut(zone_id) {
                        let new_rect = Rect::from_min_max(orig.min + delta, orig.max + delta);
                        z.set_rect(new_rect);
                    }
                }
            }
            if resp.drag_stopped() {
                let new_rect = state
                    .session
                    .as_ref()
                    .and_then(|s| s.zone_by_id(zone_id))
                    .map(|z| z.canvas_rect())
                    .unwrap_or(orig);
                state.execute(CanvasCommand::MoveZone {
                    id: zone_id,
                    from: orig,
                    to: new_rect,
                });
                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::ResizingZone {
            zone_id,
            handle,
            drag_start_canvas,
            original_rect,
        } => {
            let zone_id = *zone_id;
            let handle = *handle;
            let drag_start = *drag_start_canvas;
            let orig = *original_rect;
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                let new_rect = apply_handle_resize(orig, handle, delta);
                if let Some(s) = &mut state.session {
                    if let Some(z) = s.zone_by_id_mut(zone_id) {
                        z.set_rect(new_rect);
                    }
                }
            }
            if resp.drag_stopped() {
                let new_rect = state
                    .session
                    .as_ref()
                    .and_then(|s| s.zone_by_id(zone_id))
                    .map(|z| z.canvas_rect())
                    .unwrap_or(orig);
                state.execute(CanvasCommand::ResizeZone {
                    id: zone_id,
                    from: orig,
                    to: new_rect,
                });
                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::DraggingStack {
            stack_id,
            drag_start_canvas,
            original_pos,
        } => {
            let stack_id = *stack_id;
            let drag_start = *drag_start_canvas;
            let orig = *original_pos;
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                let new_pos = Pos2::new(orig[0] + delta.x, orig[1] + delta.y);
                if let Some(s) = &mut state.session {
                    if let Some(st) = s.stack_by_id_mut(stack_id) {
                        st.set_pos(new_pos);
                    }
                }
            }
            if resp.drag_stopped() {
                // If dropped onto a different stack, merge this stack into it.
                let drop_pos = canvas_pos.unwrap_or_else(|| Pos2::new(orig[0], orig[1]));
                let target = state.session.as_ref().and_then(|s| {
                    s.stacks
                        .iter()
                        .find(|st| st.id != stack_id && st.rect().contains(drop_pos))
                        .map(|st| st.id)
                });
                if let Some(target_id) = target {
                    if let Some(s) = state.session.as_ref() {
                        if let (Some(source), Some(target)) =
                            (s.stack_by_id(stack_id).cloned(), s.stack_by_id(target_id))
                        {
                            let target_image_ids_before = target.image_ids.clone();
                            state.execute(CanvasCommand::MergeStacks {
                                source,
                                target_id,
                                target_image_ids_before,
                            });
                        }
                    }
                }
                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::MovingNote {
            note_id,
            drag_start_canvas,
            original_rect,
        } => {
            let note_id = *note_id;
            let drag_start = *drag_start_canvas;
            let orig = *original_rect;
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                if let Some(n) = state.sticky_notes.iter_mut().find(|n| n.id == note_id) {
                    n.rect = Rect::from_min_max(orig.min + delta, orig.max + delta);
                }
            }
            if resp.drag_stopped() {
                state.dirty = true;
                state.tool_state = ToolInteraction::Idle;
            }
        }

        ToolInteraction::ResizingNote {
            note_id,
            handle,
            drag_start_canvas,
            original_rect,
        } => {
            let note_id = *note_id;
            let handle = *handle;
            let drag_start = *drag_start_canvas;
            let orig = *original_rect;
            if let Some(cp) = canvas_pos {
                let delta = cp - drag_start;
                let new_rect = super::notes::apply_note_resize(orig, handle, delta);
                if let Some(n) = state.sticky_notes.iter_mut().find(|n| n.id == note_id) {
                    n.rect = new_rect;
                }
            }
            if resp.drag_stopped() {
                state.dirty = true;
                state.tool_state = ToolInteraction::Idle;
            }
        }

        _ => {}
    }
}

// ── Draw zone tool ────────────────────────────────────────────────────────────

fn handle_draw_zone(state: &mut CanvasSortState, resp: &Response, _ctx: &egui::Context) {
    let canvas_pos = resp.interact_pointer_pos().map(|p| state.to_canvas(p));

    match &state.tool_state {
        ToolInteraction::Idle => {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(cp) = canvas_pos {
                    state.tool_state = ToolInteraction::Drawing {
                        start_canvas: cp,
                        current_canvas: cp,
                    };
                }
            }
        }
        ToolInteraction::Drawing { start_canvas, .. } => {
            let start = *start_canvas;
            if let Some(cp) = canvas_pos {
                state.tool_state = ToolInteraction::Drawing {
                    start_canvas: start,
                    current_canvas: cp,
                };
            }
            if resp.drag_stopped() {
                if let ToolInteraction::Drawing {
                    start_canvas,
                    current_canvas,
                } = state.tool_state
                {
                    let rect = Rect::from_two_pos(start_canvas, current_canvas);
                    if rect.width() > 20.0 && rect.height() > 20.0 {
                        let action = state
                            .zone_picker
                            .as_ref()
                            .map(|p| p.pending.clone())
                            .unwrap_or(ZoneAction::MoveToFolder {
                                target: Default::default(),
                            });
                        let zone = ActionZone::new(rect, action);
                        let zone_id = zone.id;
                        state.execute(CanvasCommand::CreateZone { zone });
                        // Open config dialog immediately so user can set the target folder.
                        state.zone_edit_id = Some(zone_id);
                    }
                }
                state.tool_state = ToolInteraction::Idle;
            }
        }
        _ => {}
    }
}

// ── Annotation (sticky note) tool ─────────────────────────────────────────────

fn handle_annotation(state: &mut CanvasSortState, resp: &Response, _ctx: &egui::Context) {
    let canvas_pos = resp.interact_pointer_pos().map(|p| state.to_canvas(p));
    match &state.tool_state {
        ToolInteraction::Idle => {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(cp) = canvas_pos {
                    state.tool_state = ToolInteraction::Drawing {
                        start_canvas: cp,
                        current_canvas: cp,
                    };
                }
            }
        }
        ToolInteraction::Drawing { start_canvas, .. } => {
            let start = *start_canvas;
            if let Some(cp) = canvas_pos {
                state.tool_state = ToolInteraction::Drawing {
                    start_canvas: start,
                    current_canvas: cp,
                };
            }
            if resp.drag_stopped() {
                if let ToolInteraction::Drawing {
                    start_canvas,
                    current_canvas,
                } = state.tool_state
                {
                    let rect = Rect::from_two_pos(start_canvas, current_canvas);
                    if rect.width() > 40.0 && rect.height() > 30.0 {
                        let note = super::types::StickyNote::new(rect);
                        let note_id = note.id;
                        state.sticky_notes.push(note);
                        state.tool_state = ToolInteraction::EditingNote { note_id };
                        state.selected_note = Some(note_id);
                        state.note_editor_focus = true;
                        state.dirty = true;
                        state.tool = CanvasTool::Select;
                        return;
                    }
                }
                state.tool_state = ToolInteraction::Idle;
                state.tool = CanvasTool::Select;
            }
        }
        _ => {}
    }
}

// ── Sticky collect tool ───────────────────────────────────────────────────────

fn handle_sticky_collect(state: &mut CanvasSortState, resp: &Response, _ctx: &egui::Context) {
    if resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(cp) = resp.interact_pointer_pos().map(|p| state.to_canvas(p)) {
            let hit = hit_image(state, cp);
            if let Some(id) = hit {
                // Add to collection
                if state.collecting.contains(&id) {
                    state.collecting.retain(|c| *c != id);
                } else {
                    state.collecting.push(id);
                }
            } else if !state.collecting.is_empty() {
                // Place collected images as a new stack
                let mut stack = ImageStack::new(cp);
                stack.image_ids = state.collecting.clone();
                let moved: Vec<(Uuid, Option<Uuid>)> = state
                    .collecting
                    .iter()
                    .filter_map(|id| {
                        let old = state.session.as_ref()?.image_by_id(*id)?.stack_id;
                        Some((*id, old))
                    })
                    .collect();
                state.collecting.clear();
                state.execute(CanvasCommand::CreateStack { stack, moved });
                state.tool = CanvasTool::Select;
            }
        }
    }
}

// ── Hit testing ───────────────────────────────────────────────────────────────

pub fn hit_image(state: &CanvasSortState, cp: Pos2) -> Option<Uuid> {
    let s = state.session.as_ref()?;
    // Walk in reverse z-order (highest on top)
    let mut imgs: Vec<&SortImage> = s.images.iter().filter(|i| i.stack_id.is_none()).collect();
    imgs.sort_by_key(|i| std::cmp::Reverse(i.z_index));
    for img in imgs {
        if img.rect().contains(cp) {
            return Some(img.id);
        }
    }
    None
}

fn hit_stack(state: &CanvasSortState, cp: Pos2) -> Option<Uuid> {
    let s = state.session.as_ref()?;
    s.stacks
        .iter()
        .find(|st| st.rect().contains(cp))
        .map(|st| st.id)
}

pub fn hit_zone_body(state: &CanvasSortState, cp: Pos2) -> Option<Uuid> {
    let s = state.session.as_ref()?;
    s.zones
        .iter()
        .find(|z| z.canvas_rect().contains(cp))
        .map(|z| z.id)
}

/// On-screen edge length (px) of a zone's resize-handle square.
pub const ZONE_HANDLE_DRAW_PX: f32 = 10.0;
/// On-screen grab area (px) for hit-testing a handle — a bit larger than the
/// drawn square so the small handles are easy to grab.
pub const ZONE_HANDLE_GRAB_PX: f32 = 16.0;

/// Returns (zone_id, handle_index) if cursor is on a resize handle.
fn hit_zone_handle(state: &CanvasSortState, cp: Pos2) -> (Option<Uuid>, u8) {
    let s = match state.session.as_ref() {
        Some(s) => s,
        None => return (None, 0),
    };
    // Grab area is a constant on-screen size, converted to canvas units so the
    // handle stays grabbable at any zoom (cp is in canvas space).
    let handle_size = ZONE_HANDLE_GRAB_PX / state.zoom;
    for z in s
        .zones
        .iter()
        .filter(|z| state.selected_zones.contains(&z.id))
    {
        let r = z.canvas_rect();
        let handles = zone_handles(r);
        for (i, h) in handles.iter().enumerate() {
            let hr = Rect::from_center_size(*h, Vec2::splat(handle_size));
            if hr.contains(cp) {
                return (Some(z.id), i as u8);
            }
        }
    }
    (None, 0)
}

/// Publicly accessible for rendering handle positions.
pub fn zone_handles_pub(r: Rect) -> [Pos2; 8] {
    zone_handles(r)
}

/// 8 handle positions: TL, T, TR, R, BR, B, BL, L
fn zone_handles(r: Rect) -> [Pos2; 8] {
    let cx = r.center().x;
    let cy = r.center().y;
    [
        r.min,
        Pos2::new(cx, r.min.y),
        Pos2::new(r.max.x, r.min.y),
        Pos2::new(r.max.x, cy),
        r.max,
        Pos2::new(cx, r.max.y),
        Pos2::new(r.min.x, r.max.y),
        Pos2::new(r.min.x, cy),
    ]
}

fn apply_handle_resize(orig: Rect, handle: u8, delta: Vec2) -> Rect {
    let (mut min, mut max) = (orig.min, orig.max);
    match handle {
        0 => {
            min.x += delta.x;
            min.y += delta.y;
        }
        1 => {
            min.y += delta.y;
        }
        2 => {
            max.x += delta.x;
            min.y += delta.y;
        }
        3 => {
            max.x += delta.x;
        }
        4 => {
            max.x += delta.x;
            max.y += delta.y;
        }
        5 => {
            max.y += delta.y;
        }
        6 => {
            min.x += delta.x;
            max.y += delta.y;
        }
        7 => {
            min.x += delta.x;
        }
        _ => {}
    }
    // Normalize so the rect never inverts when a handle is dragged past the
    // opposite edge — the corner simply flips and the zone keeps following the
    // cursor, as in most editors.
    Rect::from_min_max(
        Pos2::new(min.x.min(max.x), min.y.min(max.y)),
        Pos2::new(min.x.max(max.x), min.y.max(max.y)),
    )
}

/// Given a zone's current contained set and the moved images' final rects,
/// return (images that newly entered the zone, images that left it).
pub fn classify_zone_membership(
    zone_rect: Rect,
    contained: &[Uuid],
    moved: &[(Uuid, Rect)],
) -> (Vec<Uuid>, Vec<Uuid>) {
    let mut entered = Vec::new();
    let mut left = Vec::new();
    for (id, rect) in moved {
        let inside = zone_rect.intersects(*rect);
        let was = contained.contains(id);
        if inside && !was {
            entered.push(*id);
        } else if !inside && was {
            left.push(*id);
        }
    }
    (entered, left)
}

/// Returns the id of an image that `drop_pos` lands *squarely* on (inner ~40%
/// region) and that is not part of the moving set — a deliberate "stack onto"
/// gesture. Mere edge overlap does not count, preventing accidental stacks.
pub fn deliberate_drop_target(
    session: &CanvasSortSession,
    moving: &[Uuid],
    drop_pos: Pos2,
) -> Option<Uuid> {
    for img in &session.images {
        if moving.contains(&img.id) {
            continue;
        }
        if img.stack_id.is_some() {
            continue;
        }
        // Shrink each side by 25% of the dimension → remaining inner area is 50%.
        let inner = img.rect().shrink2(img.canvas_size() * 0.25);
        if inner.contains(drop_pos) {
            return Some(img.id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_sort::types::{CanvasSortSession, SortImage};
    use egui::Pos2;

    fn img_at(x: f32, y: f32) -> SortImage {
        let mut i = SortImage::new(std::path::PathBuf::from("x.png"), [0u8; 32]);
        i.pos = [x, y];
        i.size = [100.0, 100.0]; // rect = (x,y) .. (x+100, y+100); center = (x+50, y+50)
        i
    }

    #[test]
    fn deliberate_drop_target_hits_inner_region() {
        let mut s = CanvasSortSession::new("t");
        let target = img_at(0.0, 0.0);
        let target_id = target.id;
        let mover = img_at(500.0, 500.0);
        let mover_id = mover.id;
        s.images.push(target);
        s.images.push(mover);
        // Drop squarely on the target's center → deliberate stack.
        let hit = deliberate_drop_target(&s, &[mover_id], Pos2::new(50.0, 50.0));
        assert_eq!(hit, Some(target_id));
    }

    #[test]
    fn deliberate_drop_target_ignores_edge_overlap() {
        let mut s = CanvasSortSession::new("t");
        let target = img_at(0.0, 0.0);
        let mover = img_at(500.0, 500.0);
        let mover_id = mover.id;
        s.images.push(target);
        s.images.push(mover);
        // Drop near the corner (within rect but outside inner 40%) → no stack.
        let hit = deliberate_drop_target(&s, &[mover_id], Pos2::new(5.0, 5.0));
        assert_eq!(hit, None);
    }

    #[test]
    fn deliberate_drop_target_excludes_moving_images() {
        let mut s = CanvasSortSession::new("t");
        let a = img_at(0.0, 0.0);
        let a_id = a.id;
        s.images.push(a);
        // The only image under the cursor is itself a mover → no target.
        let hit = deliberate_drop_target(&s, &[a_id], Pos2::new(50.0, 50.0));
        assert_eq!(hit, None);
    }

    #[test]
    fn classify_zone_membership_splits_entered_and_left() {
        use egui::Rect;
        let zone = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let already = Uuid::new_v4(); // was contained, now outside → left
        let fresh = Uuid::new_v4(); // not contained, now inside → entered
        let staying = Uuid::new_v4(); // contained and still inside → neither

        let inside = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(20.0, 20.0));
        let outside = Rect::from_min_max(Pos2::new(500.0, 500.0), Pos2::new(510.0, 510.0));

        let moved = vec![(already, outside), (fresh, inside), (staying, inside)];
        let (entered, left) = classify_zone_membership(zone, &[already, staying], &moved);

        assert_eq!(entered, vec![fresh]);
        assert_eq!(left, vec![already]);
    }
}

fn check_zone_drops(
    state: &mut CanvasSortState,
    moved_images: &[(Uuid, [f32; 2])],
    _drop_pos: Pos2,
) {
    let zone_ids: Vec<Uuid> = state
        .session
        .as_ref()
        .map(|s| s.zones.iter().map(|z| z.id).collect())
        .unwrap_or_default();

    for zone_id in zone_ids {
        let (zone_rect, auto_process, remove_after, action, zone_label) =
            match state.session.as_ref().and_then(|s| s.zone_by_id(zone_id)) {
                Some(z) => (
                    z.canvas_rect(),
                    z.auto_process,
                    z.remove_after_action,
                    z.action.clone(),
                    z.label.clone(),
                ),
                None => continue,
            };

        // Final rects for the moved images (positions already updated by MoveImages/CreateStack).
        let moved_rects: Vec<(Uuid, Rect)> = moved_images
            .iter()
            .filter_map(|(id, _)| {
                state
                    .session
                    .as_ref()
                    .and_then(|s| s.image_by_id(*id))
                    .map(|img| (*id, img.rect()))
            })
            .collect();
        let contained = state
            .session
            .as_ref()
            .and_then(|s| s.zone_by_id(zone_id))
            .map(|z| z.contained_image_ids.clone())
            .unwrap_or_default();

        let (entered, left) = classify_zone_membership(zone_rect, &contained, &moved_rects);

        if !left.is_empty() {
            state.execute(CanvasCommand::LeaveZone {
                image_ids: left,
                zone_id,
            });
        }

        if entered.is_empty() {
            continue;
        }

        let dropped_ids = entered;
        state.execute(CanvasCommand::EnterZone {
            image_ids: dropped_ids.clone(),
            zone_id,
        });

        if !auto_process {
            let n = dropped_ids.len();
            state.status = Some(format!(
                "{n} image{} entered zone \"{}\" (auto-process off)",
                if n == 1 { "" } else { "s" },
                zone_label
            ));
            continue;
        }

        // Validate that the target folder is set for folder-based actions.
        if let ZoneAction::MoveToFolder { target } | ZoneAction::CopyToFolder { target } = &action {
            if target.as_os_str().is_empty() {
                state.status = Some(format!(
                    "Zone \"{}\" has no target folder configured — open Zone Properties to set one.",
                    zone_label
                ));
                continue;
            }
        }

        let paths: Vec<std::path::PathBuf> = dropped_ids
            .iter()
            .filter_map(|id| {
                state
                    .session
                    .as_ref()?
                    .image_by_id(*id)
                    .map(|i| i.path.clone())
            })
            .collect();
        let allowed = state
            .session
            .as_ref()
            .map(|s| s.settings.allowed_dirs.clone())
            .unwrap_or_default();
        let results = super::file_ops::execute_zone_action(&paths, &action, &allowed);

        let mut ok_count = 0usize;
        let mut errors: Vec<String> = Vec::new();

        // Update paths for images that were successfully moved
        for (id, result) in dropped_ids.iter().zip(results.iter()) {
            match result {
                Ok(new_path) => {
                    ok_count += 1;
                    if let Some(s) = &mut state.session {
                        if let Some(img) = s.image_by_id_mut(*id) {
                            img.path = new_path.clone();
                        }
                    }
                }
                Err(e) => errors.push(e.to_string()),
            }
        }

        // Remove images from canvas if: Delete action (always), or remove_after_action flag set.
        let should_remove = matches!(action, ZoneAction::Delete) || remove_after;
        if should_remove && ok_count > 0 {
            let ok_ids: Vec<Uuid> = dropped_ids
                .iter()
                .zip(results.iter())
                .filter(|(_, r)| r.is_ok())
                .map(|(id, _)| *id)
                .collect();
            let images_to_remove: Vec<SortImage> = ok_ids
                .iter()
                .filter_map(|id| state.session.as_ref()?.image_by_id(*id).cloned())
                .collect();
            if !images_to_remove.is_empty() {
                state.execute(CanvasCommand::RemoveImages {
                    images: images_to_remove,
                });
            }
        }

        // Report outcome
        state.status = Some(if errors.is_empty() {
            format!(
                "Zone \"{}\": {} image{} processed ({})",
                zone_label,
                ok_count,
                if ok_count == 1 { "" } else { "s" },
                action.label()
            )
        } else if ok_count == 0 {
            format!("Zone \"{}\" failed: {}", zone_label, errors[0])
        } else {
            format!(
                "Zone \"{}\": {ok_count} ok, {} failed — {}",
                zone_label,
                errors.len(),
                errors[0]
            )
        });
    }
}
