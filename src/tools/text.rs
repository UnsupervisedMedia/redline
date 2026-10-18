use crate::{
    state::{AppState, ToolState},
    types::Command,
};
use egui::Response;

/// A single click enters text-editing mode at that position. The actual
/// TextEdit widget is drawn elsewhere (see `show_text_editor` in canvas.rs)
/// and commits a Command::Add when the user confirms.
pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    if response.clicked() {
        // If already editing, let show_text_editor handle commit/cancel via
        // lost_focus — don't clobber the in-progress buffer.
        if matches!(state.tool_state, ToolState::EditingText { .. }) {
            return None;
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            state.tool_state = ToolState::EditingText {
                pos,
                buffer: String::new(),
                color: state.settings.stroke_color,
                size: state.settings.text_size,
                editing_id: None,
            };
        }
    }
    None
}
