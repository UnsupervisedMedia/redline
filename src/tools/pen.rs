use crate::{
    smoothing::catmull_rom,
    state::{freehand_fit_tol, AppState, ToolState},
    types::{Annotation, Command, ShapeData},
};
use egui::{Color32, Pos2, Response};

const SEGMENTS_PER_SPAN: usize = 8;

/// Decide what a finished freehand stroke commits. A normal stroke (≥3 points)
/// becomes a tidy editable `BezierPath` via curve-fit; degenerate (≤2-point)
/// strokes stay a `Pen` for backward-compat with the agent draw tool and v1
/// sidecar import. Pure helper so it is testable without an egui `Response`.
pub fn commit_freehand(
    raw_points: Vec<Pos2>,
    tol: f32,
    smoothing: bool,
    color: Color32,
    width: f32,
) -> Option<Command> {
    if raw_points.len() < 2 {
        return None;
    }
    if raw_points.len() < 3 {
        let smoothed = if smoothing {
            catmull_rom(&raw_points, SEGMENTS_PER_SPAN)
        } else {
            raw_points.clone()
        };
        return Some(Command::Add(Annotation::new(
            ShapeData::Pen {
                raw_points,
                smoothed_points: smoothed,
            },
            color,
            width,
        )));
    }
    let (nodes, closed) = crate::tools::path_edit::points_to_bezier(&raw_points, tol);
    if nodes.len() < 2 {
        // Fit failed/degenerate — fall back to a raw Pen.
        let smoothed = if smoothing {
            catmull_rom(&raw_points, SEGMENTS_PER_SPAN)
        } else {
            raw_points.clone()
        };
        return Some(Command::Add(Annotation::new(
            ShapeData::Pen {
                raw_points,
                smoothed_points: smoothed,
            },
            color,
            width,
        )));
    }
    Some(Command::Add(Annotation::new(
        ShapeData::BezierPath {
            nodes,
            closed,
            fill: None,
        },
        color,
        width,
    )))
}

pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    // drag_started must be checked FIRST: on the first drag frame, egui fires both
    // drag_started() and dragged() simultaneously. We must initialise DrawingPen before
    // the dragged() branch tries to accumulate into it.
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            state.tool_state = ToolState::DrawingPen {
                raw_points: vec![pos],
            };
        }
    }

    // Accumulate only on pure drag frames (not on the release frame).
    if response.dragged() && !response.drag_stopped() {
        if let Some(pos) = response.interact_pointer_pos() {
            let pos = state.unproject(pos);
            if let ToolState::DrawingPen { ref mut raw_points } = state.tool_state {
                if raw_points
                    .last()
                    .map_or(true, |&last| last.distance(pos) > 1.5)
                {
                    raw_points.push(pos);
                }
            }
        }
    }

    if response.drag_stopped() {
        if let ToolState::DrawingPen { raw_points } =
            std::mem::replace(&mut state.tool_state, ToolState::Idle)
        {
            return commit_freehand(
                raw_points,
                freehand_fit_tol(&state.settings),
                state.settings.pen_smoothing,
                state.settings.stroke_color,
                state.settings.stroke_width,
            );
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freehand_commits_bezier_for_normal_stroke() {
        let pts: Vec<Pos2> = (0..30)
            .map(|i| Pos2::new(i as f32, (i as f32 * 0.3).sin() * 5.0))
            .collect();
        let cmd = commit_freehand(pts, 2.0, true, Color32::RED, 3.0).unwrap();
        match cmd {
            Command::Add(a) => assert!(matches!(a.shape, ShapeData::BezierPath { .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn freehand_two_points_stays_pen() {
        let cmd = commit_freehand(
            vec![Pos2::ZERO, Pos2::new(1.0, 1.0)],
            2.0,
            true,
            Color32::RED,
            3.0,
        )
        .unwrap();
        match cmd {
            Command::Add(a) => assert!(matches!(a.shape, ShapeData::Pen { .. })),
            _ => panic!(),
        }
    }

    #[test]
    fn freehand_empty_is_none() {
        assert!(commit_freehand(vec![], 2.0, true, Color32::RED, 3.0).is_none());
    }
}
