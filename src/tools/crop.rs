use crate::{
    state::{AppState, ToolState},
    tools::drag,
    types::Command,
};
use egui::{Rect, Response};

/// Drag to select a crop region. On release, sit in PendingCrop state so the
/// user can confirm or cancel. The canvas draws the Apply/Cancel buttons.
pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    // Don't reset a pending crop if we're already showing confirm buttons.
    if matches!(state.tool_state, ToolState::PendingCrop { .. }) {
        return None;
    }

    drag::handle_drag_response(
        response,
        state,
        |_, end, _| end,
        |state, drag| {
            let rect = Rect::from_two_pos(drag.start, drag.current);
            if rect.area() < 16.0 {
                state.tool_state = ToolState::Idle;
            } else {
                state.tool_state = ToolState::PendingCrop { rect };
            }
            None::<Command>
        },
    );
    None
}
