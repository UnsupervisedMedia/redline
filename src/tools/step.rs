//! Numbered step-marker tool. Each click drops a filled circle with an
//! auto-incrementing number inside, useful for annotating tutorial steps.

use crate::{
    state::AppState,
    types::{Annotation, Command, ShapeData},
};
use egui::Response;

pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    if !response.clicked() {
        return None;
    }
    let pos_view = response.interact_pointer_pos()?;
    let pos = state.unproject(pos_view);

    let next = next_step_number(&state.canvas.annotations);
    let radius = state.settings.step_radius.max(8.0);

    let ann = Annotation::new(
        ShapeData::StepMarker {
            pos,
            number: next,
            radius,
        },
        state.settings.stroke_color,
        state.settings.stroke_width,
    );
    Some(Command::Add(ann))
}

pub fn next_step_number(anns: &[Annotation]) -> u32 {
    let mut max = 0;
    for a in anns {
        if let ShapeData::StepMarker { number, .. } = a.shape {
            if number > max {
                max = number;
            }
        }
    }
    max + 1
}
