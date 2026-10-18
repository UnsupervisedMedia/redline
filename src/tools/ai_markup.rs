use crate::{
    state::AppState,
    tools::drag,
    types::{Annotation, Command, LogicTagKind, ShapeData},
};
use egui::{Rect, Response};

pub fn handle_focus(response: &Response, state: &mut AppState) -> Option<Command> {
    drag::handle_drag_response(
        response,
        state,
        |_, end, _| end,
        |state, drag| {
            let rect = Rect::from_two_pos(drag.start, drag.current);
            if rect.area() > 4.0 {
                // Remove any existing focus region first
                let old_ids: Vec<_> = state
                    .canvas
                    .annotations
                    .iter()
                    .filter(|a| matches!(a.shape, ShapeData::FocusRegion { .. }))
                    .map(|a| a.id)
                    .collect();
                let mut cmds: Vec<Command> = old_ids.into_iter().map(Command::Delete).collect();
                let ann = Annotation::new(
                    ShapeData::FocusRegion { rect },
                    egui::Color32::from_rgb(227, 179, 65),
                    2.0,
                );
                cmds.push(Command::Add(ann));
                return Some(Command::Batch(cmds));
            }
            None
        },
    )
}

pub fn handle_tag(
    response: &Response,
    state: &mut AppState,
    kind: LogicTagKind,
) -> Option<Command> {
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            let ann = Annotation::new(
                ShapeData::LogicTag { pos, tag: kind },
                egui::Color32::from_rgb(227, 179, 65),
                1.0,
            );
            return Some(Command::Add(ann));
        }
    }
    None
}
