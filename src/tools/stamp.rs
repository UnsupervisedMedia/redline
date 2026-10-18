use egui::{Rect, Response};

use crate::{
    stamps::{make_stamp_annotation, square_rect_from_drag, stamp_by_id},
    state::AppState,
    tools::drag::{self, DragSession},
    types::{Annotation, Command},
};

const DEFAULT_STAMP_SIZE: f32 = 64.0;

fn stamp_rect_from_drag(drag: DragSession) -> Rect {
    let rect = square_rect_from_drag(drag.start, drag.current);
    if rect.area() >= 16.0 {
        Rect::from_two_pos(rect.min, rect.max)
    } else {
        Rect::from_center_size(
            drag.start,
            egui::vec2(DEFAULT_STAMP_SIZE, DEFAULT_STAMP_SIZE),
        )
    }
}

fn build_stamp_annotation(
    state: &mut AppState,
    stamp_id: &'static str,
    drag: DragSession,
) -> Option<Annotation> {
    let rect = stamp_rect_from_drag(drag);
    let Some(stamp) = stamp_by_id(stamp_id) else {
        state.status_msg = Some(format!("Stamp failed: unknown stamp `{stamp_id}`"));
        return None;
    };
    match make_stamp_annotation(stamp, rect, state.settings.stroke_color) {
        Ok(annotation) => Some(annotation),
        Err(err) => {
            state.status_msg = Some(format!("Stamp failed: {err}"));
            None
        }
    }
}

pub fn handle(
    response: &Response,
    state: &mut AppState,
    stamp_id: &'static str,
) -> Option<Command> {
    drag::handle_drag_response(
        response,
        state,
        |start, end, _| square_rect_from_drag(start, end).max,
        |state, drag| build_stamp_annotation(state, stamp_id, drag).map(Command::Add),
    )
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::{build_stamp_annotation, DEFAULT_STAMP_SIZE};
    use crate::state::AppState;

    #[test]
    fn click_places_default_sized_stamp() {
        let mut state = AppState::default();
        let stamp_id = crate::stamps::all_stamps()
            .first()
            .expect("expected at least one stamp")
            .id;

        let result = build_stamp_annotation(
            &mut state,
            stamp_id,
            crate::tools::drag::DragSession {
                start: pos2(10.0, 10.0),
                current: pos2(12.0, 12.0),
                modifiers: egui::Modifiers::NONE,
            },
        );

        let ann = result.expect("expected stamp annotation");
        match ann.shape {
            crate::types::ShapeData::ImageLayer { rect, .. } => {
                assert_eq!(rect.center(), pos2(10.0, 10.0));
                assert_eq!(rect.width(), DEFAULT_STAMP_SIZE);
                assert_eq!(rect.height(), DEFAULT_STAMP_SIZE);
            }
            other => panic!("unexpected shape: {other:?}"),
        }
        assert!(state.status_msg.is_none());
    }

    #[test]
    fn missing_stamp_sets_status_message() {
        let mut state = AppState::default();

        let result = build_stamp_annotation(
            &mut state,
            "missing-stamp",
            crate::tools::drag::DragSession {
                start: pos2(10.0, 10.0),
                current: pos2(30.0, 30.0),
                modifiers: egui::Modifiers::NONE,
            },
        );

        assert!(result.is_none());
        assert_eq!(
            state.status_msg.as_deref(),
            Some("Stamp failed: unknown stamp `missing-stamp`"),
        );
    }
}
