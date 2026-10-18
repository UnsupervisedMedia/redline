use egui::{Color32, Pos2, Rect, Vec2};
use uuid::Uuid;

use super::state::{CanvasSortState, ToolInteraction};
use super::types::StickyNote;

/// Minimum sticky-note size on resize (canvas units). Matches the create floor.
pub const NOTE_MIN_W: f32 = 40.0;
pub const NOTE_MIN_H: f32 = 30.0;

/// Swatch palette shown in the note control strip.
pub const NOTE_COLORS: [(&str, Color32); 4] = [
    ("Yellow", Color32::from_rgb(0xFF, 0xEE, 0x88)),
    ("Blue", Color32::from_rgb(0x88, 0xCC, 0xFF)),
    ("Green", Color32::from_rgb(0xAA, 0xE8, 0xAA)),
    ("Pink", Color32::from_rgb(0xFF, 0xAA, 0xD4)),
];

/// Top-most (last-drawn) note whose rect contains `cp`.
pub fn hit_note(notes: &[StickyNote], cp: Pos2) -> Option<Uuid> {
    notes
        .iter()
        .rev()
        .find(|n| n.rect.contains(cp))
        .map(|n| n.id)
}

/// Index of the note with `id`, if present.
pub fn note_index(notes: &[StickyNote], id: Uuid) -> Option<usize> {
    notes.iter().position(|n| n.id == id)
}

/// If a resize handle of the *selected* note is under `cp`, return (id, handle 0..7).
pub fn hit_note_handle(state: &CanvasSortState, cp: Pos2) -> Option<(Uuid, u8)> {
    let id = state.selected_note?;
    let note = state.sticky_notes.iter().find(|n| n.id == id)?;
    let grab = super::tools::ZONE_HANDLE_GRAB_PX / state.zoom;
    let handles = super::tools::zone_handles_pub(note.rect);
    for (i, h) in handles.iter().enumerate() {
        if Rect::from_center_size(*h, Vec2::splat(grab)).contains(cp) {
            return Some((id, i as u8));
        }
    }
    None
}

/// Resize `orig` by dragging `handle` (0..7: TL,T,TR,R,BR,B,BL,L) by `delta`,
/// clamped so the note never shrinks below the minimum size. The dragged edge
/// is pushed back rather than allowing inversion.
pub fn apply_note_resize(orig: Rect, handle: u8, delta: Vec2) -> Rect {
    let mut min = orig.min;
    let mut max = orig.max;
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
    // Clamp width: identify which edge moved and push it back to the minimum.
    if max.x - min.x < NOTE_MIN_W {
        match handle {
            0 | 6 | 7 => min.x = max.x - NOTE_MIN_W, // left edge moved
            2..=4 => max.x = min.x + NOTE_MIN_W,     // right edge moved
            _ => {}
        }
    }
    if max.y - min.y < NOTE_MIN_H {
        match handle {
            0..=2 => min.y = max.y - NOTE_MIN_H, // top edge moved
            4..=6 => max.y = min.y + NOTE_MIN_H, // bottom edge moved
            _ => {}
        }
    }
    Rect::from_min_max(min, max)
}

/// Painter text for a note is suppressed while it is being edited (the live
/// `TextEdit` overlay draws the text instead, avoiding a double-draw).
pub fn should_draw_note_text(note: &StickyNote) -> bool {
    !note.editing
}

// ── Floating overlays (inline editor + control strip) ───────────────────────────

/// Draw the note editor and control strip. Call once per frame after the canvas
/// painter has run, so screen rects reflect the current pan/zoom.
pub fn show_note_overlays(ctx: &egui::Context, state: &mut CanvasSortState) {
    show_note_editor(ctx, state);
    show_note_controls(ctx, state);
}

fn show_note_editor(ctx: &egui::Context, state: &mut CanvasSortState) {
    let note_id = match state.tool_state {
        ToolInteraction::EditingNote { note_id } => note_id,
        _ => return,
    };
    let idx = match note_index(&state.sticky_notes, note_id) {
        Some(i) => i,
        None => {
            state.tool_state = ToolInteraction::Idle;
            return;
        }
    };

    let screen_rect = state.rect_to_screen(state.sticky_notes[idx].rect);
    let font_size = (state.sticky_notes[idx].font_size * state.zoom).clamp(6.0, 32.0);
    let color = state.sticky_notes[idx].color;
    let want_focus = state.note_editor_focus;
    let mut commit = false;

    egui::Area::new(egui::Id::new(("cs_note_editor", note_id)))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(screen_rect);
            // egui 0.29 has no TextEdit::background_color(); set the bg via style instead.
            ui.style_mut().visuals.extreme_bg_color = color;
            let resp = ui.add_sized(
                screen_rect.size(),
                egui::TextEdit::multiline(&mut state.sticky_notes[idx].text)
                    .font(egui::FontId::proportional(font_size))
                    .frame(false)
                    .desired_width(screen_rect.width()),
            );
            if want_focus {
                resp.request_focus();
                state.note_editor_focus = false;
            }
            if resp.changed() {
                state.dirty = true;
            }
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if resp.lost_focus() || esc {
                commit = true;
            }
        });

    if commit {
        if let Some(n) = state.sticky_notes.get_mut(idx) {
            n.editing = false;
        }
        state.tool_state = ToolInteraction::Idle;
        state.dirty = true;
    }
}

fn show_note_controls(ctx: &egui::Context, state: &mut CanvasSortState) {
    // No control strip while editing text.
    if matches!(state.tool_state, ToolInteraction::EditingNote { .. }) {
        return;
    }
    let note_id = match state.selected_note {
        Some(id) => id,
        None => return,
    };
    let idx = match note_index(&state.sticky_notes, note_id) {
        Some(i) => i,
        None => {
            state.selected_note = None;
            return;
        }
    };

    let screen_rect = state.rect_to_screen(state.sticky_notes[idx].rect);
    let strip_pos = egui::pos2(screen_rect.min.x, screen_rect.min.y - 30.0);
    let mut delete = false;

    egui::Area::new(egui::Id::new(("cs_note_controls", note_id)))
        .order(egui::Order::Foreground)
        .fixed_pos(strip_pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (name, col) in NOTE_COLORS {
                        if ui
                            .add(
                                egui::Button::new("  ")
                                    .fill(col)
                                    .min_size(egui::vec2(16.0, 16.0)),
                            )
                            .on_hover_text(name)
                            .clicked()
                        {
                            state.sticky_notes[idx].color = col;
                            state.dirty = true;
                        }
                    }
                    ui.separator();
                    if ui.small_button("A-").on_hover_text("Smaller").clicked() {
                        let n = &mut state.sticky_notes[idx];
                        n.font_size = (n.font_size - 2.0).max(8.0);
                        state.dirty = true;
                    }
                    ui.label(format!("{}", state.sticky_notes[idx].font_size as i32));
                    if ui.small_button("A+").on_hover_text("Larger").clicked() {
                        let n = &mut state.sticky_notes[idx];
                        n.font_size = (n.font_size + 2.0).min(48.0);
                        state.dirty = true;
                    }
                    ui.separator();
                    if ui.small_button("🗑").on_hover_text("Delete note").clicked() {
                        delete = true;
                    }
                });
            });
        });

    if delete {
        state.sticky_notes.retain(|n| n.id != note_id);
        state.selected_note = None;
        state.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_at(x: f32, y: f32, w: f32, h: f32) -> StickyNote {
        StickyNote::new(Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)))
    }

    #[test]
    fn note_index_finds_and_misses() {
        let a = note_at(0.0, 0.0, 60.0, 40.0);
        let b = note_at(100.0, 0.0, 60.0, 40.0);
        let b_id = b.id;
        let notes = vec![a, b];
        assert_eq!(note_index(&notes, b_id), Some(1));
        assert_eq!(note_index(&notes, Uuid::new_v4()), None);
    }

    #[test]
    fn hit_note_returns_topmost() {
        let lower = note_at(0.0, 0.0, 100.0, 100.0);
        let upper = note_at(0.0, 0.0, 100.0, 100.0);
        let upper_id = upper.id;
        let notes = vec![lower, upper];
        assert_eq!(hit_note(&notes, Pos2::new(50.0, 50.0)), Some(upper_id));
    }

    #[test]
    fn hit_note_misses_outside() {
        let notes = vec![note_at(0.0, 0.0, 50.0, 50.0)];
        assert_eq!(hit_note(&notes, Pos2::new(200.0, 200.0)), None);
    }

    #[test]
    fn resize_clamps_to_minimum_width() {
        let orig = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        // Drag the right edge (handle 3) far left, past the minimum.
        let r = apply_note_resize(orig, 3, Vec2::new(-500.0, 0.0));
        assert!(
            (r.width() - NOTE_MIN_W).abs() < 0.001,
            "width clamped to {NOTE_MIN_W}"
        );
        assert_eq!(r.min.x, 0.0, "left edge stays put when dragging right edge");
    }

    #[test]
    fn resize_clamps_minimum_height_from_top() {
        let orig = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        // Drag the top edge (handle 1) down past the minimum.
        let r = apply_note_resize(orig, 1, Vec2::new(0.0, 500.0));
        assert!(
            (r.height() - NOTE_MIN_H).abs() < 0.001,
            "height clamped to {NOTE_MIN_H}"
        );
        assert_eq!(
            r.max.y, 100.0,
            "bottom edge stays put when dragging top edge"
        );
    }

    #[test]
    fn resize_normal_drag_grows() {
        let orig = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let r = apply_note_resize(orig, 4, Vec2::new(50.0, 50.0)); // BR corner out
        assert_eq!(r.max, Pos2::new(150.0, 150.0));
    }

    #[test]
    fn resize_corner_clamps_both_axes() {
        let orig = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        // Drag the top-left corner (handle 0) far down-right, past both minimums.
        let r = apply_note_resize(orig, 0, Vec2::new(500.0, 500.0));
        assert!(
            (r.width() - NOTE_MIN_W).abs() < 0.001,
            "width clamped to {NOTE_MIN_W}"
        );
        assert!(
            (r.height() - NOTE_MIN_H).abs() < 0.001,
            "height clamped to {NOTE_MIN_H}"
        );
        assert_eq!(
            r.max,
            Pos2::new(100.0, 100.0),
            "bottom-right corner stays put"
        );
    }

    #[test]
    fn text_suppressed_while_editing() {
        let mut n = note_at(0.0, 0.0, 60.0, 40.0);
        n.editing = true;
        assert!(!should_draw_note_text(&n));
        n.editing = false;
        assert!(should_draw_note_text(&n));
    }
}
