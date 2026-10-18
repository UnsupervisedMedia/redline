use crate::{
    state::AppState,
    tools::drag,
    types::{Annotation, Command, ShapeData},
};
use egui::{Rect, Response};

pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    drag::handle_drag_response(
        response,
        state,
        |_, end, _| end,
        |state, drag| {
            let rect = Rect::from_two_pos(drag.start, drag.current);
            if rect.area() < 16.0 {
                return None;
            }
            let ann = Annotation::new(
                ShapeData::Blur {
                    rect,
                    block_size: state.settings.blur_block,
                },
                state.settings.stroke_color,
                state.settings.stroke_width,
            );
            Some(Command::Add(ann))
        },
    )
}
