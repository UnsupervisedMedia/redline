use egui::{Modifiers, Pos2, Response};

use crate::state::{AppState, ToolState};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragSession {
    pub start: Pos2,
    pub current: Pos2,
    /// Keyboard modifiers active at the moment the drag was committed.
    pub modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragEvent {
    Start(Pos2),
    Update {
        pos: Pos2,
        modifiers: Modifiers,
    },
    Stop,
    #[allow(dead_code)]
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragOutcome {
    None,
    Completed(DragSession),
    Cancelled,
}

pub fn apply_drag_event(
    tool_state: &mut ToolState,
    event: DragEvent,
    transform: impl Fn(Pos2, Pos2, Modifiers) -> Pos2,
) -> DragOutcome {
    match event {
        DragEvent::Start(pos) => {
            *tool_state = ToolState::Drawing {
                start: pos,
                current: pos,
            };
            DragOutcome::None
        }
        DragEvent::Update { pos, modifiers } => {
            if let ToolState::Drawing { start, current } = tool_state {
                *current = transform(*start, pos, modifiers);
            }
            DragOutcome::None
        }
        DragEvent::Stop => {
            if let ToolState::Drawing { start, current } =
                std::mem::replace(tool_state, ToolState::Idle)
            {
                DragOutcome::Completed(DragSession {
                    start,
                    current,
                    modifiers: Modifiers::NONE,
                })
            } else {
                DragOutcome::None
            }
        }
        DragEvent::Cancel => {
            if matches!(tool_state, ToolState::Drawing { .. }) {
                *tool_state = ToolState::Idle;
                DragOutcome::Cancelled
            } else {
                DragOutcome::None
            }
        }
    }
}

pub fn handle_drag_response<T>(
    response: &Response,
    state: &mut AppState,
    transform: impl Fn(Pos2, Pos2, Modifiers) -> Pos2,
    mut finish: impl FnMut(&mut AppState, DragSession) -> Option<T>,
) -> Option<T> {
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            apply_drag_event(&mut state.tool_state, DragEvent::Start(pos), &transform);
        }
    }

    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            let modifiers = response.ctx.input(|i| i.modifiers);
            apply_drag_event(
                &mut state.tool_state,
                DragEvent::Update { pos, modifiers },
                &transform,
            );
        }
    }

    if response.drag_stopped() {
        let modifiers = response.ctx.input(|i| i.modifiers);
        match apply_drag_event(&mut state.tool_state, DragEvent::Stop, transform) {
            DragOutcome::Completed(mut drag) => {
                drag.modifiers = modifiers;
                finish(state, drag)
            }
            DragOutcome::None | DragOutcome::Cancelled => None,
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, Modifiers, Rect};

    use super::{apply_drag_event, DragEvent, DragOutcome};
    use crate::{
        state::ToolState,
        types::{Annotation, Command, LineStyle, ShapeData},
    };

    fn identity_transform(
        _start: egui::Pos2,
        end: egui::Pos2,
        _modifiers: Modifiers,
    ) -> egui::Pos2 {
        end
    }

    fn snap_line_transform(start: egui::Pos2, end: egui::Pos2, modifiers: Modifiers) -> egui::Pos2 {
        if !modifiers.shift {
            return end;
        }
        let delta = end - start;
        if delta.length() < 1.0 {
            return end;
        }
        let angle = delta.y.atan2(delta.x);
        let step = std::f32::consts::PI / 12.0;
        let snapped = (angle / step).round() * step;
        let dist = delta.length();
        start + egui::Vec2::new(snapped.cos() * dist, snapped.sin() * dist)
    }

    #[test]
    fn rect_drag_sequence_commits_rect_command() {
        let mut tool_state = ToolState::Idle;
        apply_drag_event(
            &mut tool_state,
            DragEvent::Start(pos2(10.0, 20.0)),
            identity_transform,
        );
        apply_drag_event(
            &mut tool_state,
            DragEvent::Update {
                pos: pos2(40.0, 55.0),
                modifiers: Modifiers::NONE,
            },
            identity_transform,
        );

        let outcome = apply_drag_event(&mut tool_state, DragEvent::Stop, identity_transform);
        let DragOutcome::Completed(drag) = outcome else {
            panic!("expected completed drag");
        };

        let cmd = Command::Add(Annotation::new(
            ShapeData::plain_rect(Rect::from_two_pos(drag.start, drag.current)),
            egui::Color32::WHITE,
            3.0,
        ));

        match cmd {
            Command::Add(ann) => match ann.shape {
                ShapeData::Rect { rect, .. } => {
                    assert_eq!(rect.min, pos2(10.0, 20.0));
                    assert_eq!(rect.max, pos2(40.0, 55.0));
                }
                other => panic!("unexpected shape: {other:?}"),
            },
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(matches!(tool_state, ToolState::Idle));
    }

    #[test]
    fn line_drag_with_shift_snaps_before_commit() {
        let mut tool_state = ToolState::Idle;
        apply_drag_event(
            &mut tool_state,
            DragEvent::Start(pos2(0.0, 0.0)),
            snap_line_transform,
        );
        apply_drag_event(
            &mut tool_state,
            DragEvent::Update {
                pos: pos2(10.0, 1.0),
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
            },
            snap_line_transform,
        );

        let DragOutcome::Completed(drag) =
            apply_drag_event(&mut tool_state, DragEvent::Stop, snap_line_transform)
        else {
            panic!("expected completed drag");
        };

        let shape = ShapeData::Line {
            start: drag.start,
            end: drag.current,
            style: LineStyle::Solid,
        };
        match shape {
            ShapeData::Line { end, .. } => {
                assert!((end.y - 0.0).abs() < 0.001);
                assert!(end.x > 10.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn cancel_clears_drawing_without_completion() {
        let mut tool_state = ToolState::Idle;
        apply_drag_event(
            &mut tool_state,
            DragEvent::Start(pos2(5.0, 5.0)),
            identity_transform,
        );

        let outcome = apply_drag_event(&mut tool_state, DragEvent::Cancel, identity_transform);

        assert_eq!(outcome, DragOutcome::Cancelled);
        assert!(matches!(tool_state, ToolState::Idle));
        let stop = apply_drag_event(&mut tool_state, DragEvent::Stop, identity_transform);
        assert_eq!(stop, DragOutcome::None);
    }
}
