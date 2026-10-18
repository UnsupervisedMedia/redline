use crate::{
    state::{AppState, Tool},
    types::Command,
};
use egui::{Context, Response};

mod ai_markup;
pub mod blur;
pub mod crop;
mod drag;
pub mod eyedropper;
pub mod fill;
pub mod gizmo;
pub mod path_edit;
mod pen;
pub(crate) mod select;
mod selection;
mod shapes;
mod stamp;
pub mod step;
pub mod text;

/// Dispatch input to the active tool. Returns a command if one was completed.
pub fn handle_input(
    response: &Response,
    ctx: &Context,
    state: &mut AppState,
    canvas_rect: egui::Rect,
) -> Option<Command> {
    let tool = state.current_tool.clone();
    match tool {
        Tool::Select => select::handle(response, ctx, state, canvas_rect),
        Tool::Pen => pen::handle(response, state),
        Tool::Rect => shapes::handle_rect(response, state),
        Tool::Oval => shapes::handle_oval(response, state),
        Tool::Star => shapes::handle_star(response, state),
        Tool::Stamp(id) => stamp::handle(response, state, id),
        Tool::Arrow => shapes::handle_arrow(response, state),
        Tool::Line => shapes::handle_line(response, state),
        Tool::Text => text::handle(response, state),
        Tool::Blur => blur::handle(response, state),
        Tool::Crop => crop::handle(response, state),
        Tool::Eyedropper => eyedropper::handle(response, state),
        Tool::Step => step::handle(response, state),
        Tool::FocusRegion => ai_markup::handle_focus(response, state),
        Tool::LogicTag(kind) => ai_markup::handle_tag(response, state, kind),
        Tool::Marquee => selection::handle(response, state),
        Tool::Lasso => {
            crate::canvas::handle_lasso_input(response, state);
            None
        }
        Tool::Fill => fill::handle(response, state),
        Tool::Callout => shapes::handle_callout(response, state),
        Tool::Ruler => shapes::handle_ruler(response, state),
    }
}
