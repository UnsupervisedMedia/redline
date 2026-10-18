//! Marquee selection tool — drag to create, drag inside to move, drag a
//! handle to resize. Selection state lives on `state.canvas.selection` (a `Selection`)
//! and the transient interaction on `state.canvas.marquee_state`.

use crate::{
    state::{AppState, MarqueeState, Selection},
    types::Command,
};
use egui::{Pos2, Rect, Response, Vec2};

/// Screen-pixel radius for grabbing a selection handle.
const HANDLE_HIT_RADIUS: f32 = 10.0;

fn inside_selection_rect(sel: &Selection, cursor_img: Pos2) -> bool {
    sel.rect.contains(cursor_img)
}

pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    // ── Begin interaction ──
    if response.drag_started() {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            let img = state.unproject(screen_pos);
            // Hit a resize handle of the current selection first.
            if let Some(sel) = state.canvas.selection {
                // Convert handle hit to screen space for consistent pixel radius.
                let r = sel.rect;
                let corners_img = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ];
                let anchors_img = [
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                    r.right_top(),
                ];
                let mut hit = None;
                for (c, a) in corners_img.iter().zip(anchors_img.iter()) {
                    let c_screen = state.project(*c);
                    if (c_screen - screen_pos).length() <= HANDLE_HIT_RADIUS {
                        hit = Some(*a);
                        break;
                    }
                }
                if let Some(anchor) = hit {
                    state.canvas.marquee_state = MarqueeState::Resizing { anchor };
                    return None;
                }
                if inside_selection_rect(&sel, img) {
                    state.canvas.marquee_state = MarqueeState::Moving {
                        start_rect: sel.rect,
                        press_img: img,
                    };
                    return None;
                }
                // Fall through: starting a new selection discards the old one.
            }
            state.canvas.marquee_state = MarqueeState::Drawing { start: img };
            let kind = state.settings.marquee_shape;
            state.canvas.selection = Some(Selection {
                kind,
                rect: Rect::from_min_max(img, img),
                inverted: false,
            });
        }
    }

    // ── While dragging ──
    if response.dragged() {
        if let Some(screen_pos) = response.interact_pointer_pos() {
            let img = state.unproject(screen_pos);
            let ms = state.canvas.marquee_state;
            match ms {
                MarqueeState::Drawing { start } => {
                    if let Some(sel) = state.canvas.selection.as_mut() {
                        sel.rect = Rect::from_two_pos(start, img);
                    }
                }
                MarqueeState::Moving {
                    start_rect,
                    press_img,
                } => {
                    let delta = img - press_img;
                    if let Some(sel) = state.canvas.selection.as_mut() {
                        sel.rect = start_rect.translate(delta);
                    }
                }
                MarqueeState::Resizing { anchor } => {
                    if let Some(sel) = state.canvas.selection.as_mut() {
                        sel.rect = Rect::from_two_pos(anchor, img);
                    }
                }
                MarqueeState::Idle => {}
            }
        }
    }

    // ── On release ──
    if response.drag_stopped() {
        // If the result is a degenerate selection, clear it.
        if let Some(sel) = state.canvas.selection {
            let (w, h) = (sel.rect.width().abs(), sel.rect.height().abs());
            if w < 2.0 || h < 2.0 {
                state.canvas.selection = None;
            } else if let Some(sel) = state.canvas.selection.as_mut() {
                // Normalize min<=max.
                sel.rect = Rect::from_min_max(
                    Pos2::new(
                        sel.rect.min.x.min(sel.rect.max.x),
                        sel.rect.min.y.min(sel.rect.max.y),
                    ),
                    Pos2::new(
                        sel.rect.min.x.max(sel.rect.max.x),
                        sel.rect.min.y.max(sel.rect.max.y),
                    ),
                );
                // Clamp to canvas.
                let cs = state.canvas.canvas_size;
                if cs.x > 0.0 && cs.y > 0.0 {
                    let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(cs.x, cs.y));
                    sel.rect = sel.rect.intersect(canvas);
                }
            }
        }
        state.canvas.marquee_state = MarqueeState::Idle;
    }

    // Match SelKind to current shape setting so toggling in the toolbar is live.
    if let Some(sel) = state.canvas.selection.as_mut() {
        sel.kind = state.settings.marquee_shape;
    }

    None
}
