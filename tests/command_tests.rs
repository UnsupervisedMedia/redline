use egui::{pos2, vec2, Color32, Rect};
use redline::{
    command::CommandExecutor,
    state::CanvasState,
    types::{Annotation, Command, ParticipantId, PropChange, ShapeData},
};

fn empty_canvas() -> CanvasState {
    CanvasState::default()
}

fn rect_annotation() -> Annotation {
    Annotation::new(
        ShapeData::plain_rect(Rect::from_min_size(pos2(10.0, 10.0), vec2(50.0, 30.0))),
        Color32::RED,
        2.0,
    )
}

#[test]
fn add_increases_annotation_count() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    assert_eq!(canvas.annotations.len(), 1);
}

#[test]
fn undo_add_removes_annotation() {
    let mut canvas = empty_canvas();
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 0);
}

#[test]
fn redo_add_restores_annotation() {
    let mut canvas = empty_canvas();
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    CommandExecutor::redo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 1);
}

#[test]
fn delete_removes_annotation() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Delete(id));
    assert_eq!(canvas.annotations.len(), 0);
}

#[test]
fn undo_delete_restores_annotation() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Delete(id));
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 1);
}

#[test]
fn move_translates_shape() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Move {
            id,
            delta: vec2(5.0, 0.0),
        },
    );
    let rect = canvas.annotations[0].shape.bounding_rect();
    assert!((rect.min.x - 15.0).abs() < 0.001);
}

#[test]
fn undo_move_restores_position() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Move {
            id,
            delta: vec2(5.0, 0.0),
        },
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    let rect = canvas.annotations[0].shape.bounding_rect();
    assert!((rect.min.x - 10.0).abs() < 0.001);
}

#[test]
fn execute_clears_redo_stack() {
    let mut canvas = empty_canvas();
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    // redo stack now has one entry
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    // redo stack should be cleared
    assert_eq!(canvas.redo_stacks[&ParticipantId::HUMAN_LOCAL].len(), 0);
}

#[test]
fn set_prop_color_changes_color() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation(); // starts RED
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::SetProp {
            id,
            prop: PropChange::Color(Color32::BLUE),
        },
    );
    assert_eq!(canvas.annotations[0].color, Color32::BLUE);
}

#[test]
fn undo_set_prop_restores_color() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::SetProp {
            id,
            prop: PropChange::Color(Color32::BLUE),
        },
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations[0].color, Color32::RED);
}

#[test]
fn resize_changes_rect() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    let new_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Resize { id, new_rect },
    );
    let got = canvas.annotations[0].shape.bounding_rect();
    assert!((got.min.x - 0.0).abs() < 0.001);
    assert!((got.width() - 100.0).abs() < 0.001);
}

#[test]
fn undo_resize_restores_rect() {
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    let new_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Resize { id, new_rect },
    );
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    let restored = canvas.annotations[0].shape.bounding_rect();
    assert!((restored.min.x - 10.0).abs() < 0.001);
    assert!((restored.width() - 50.0).abs() < 0.001);
}

#[test]
fn undo_batch_reverses_all_operations() {
    // Execute a Batch that adds an annotation and moves it
    // Then undo the whole batch — annotation should be gone
    let mut canvas = empty_canvas();
    let ann = rect_annotation();
    let id = ann.id;
    let batch = Command::Batch(vec![
        Command::Add(ann),
        Command::Move {
            id,
            delta: vec2(10.0, 0.0),
        },
    ]);
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, batch);
    assert_eq!(canvas.annotations.len(), 1);
    assert!((canvas.annotations[0].shape.bounding_rect().min.x - 20.0).abs() < 0.001); // 10 + 10 delta

    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 0); // Add was reversed last = annotation gone
}

#[test]
fn execute_emits_event_with_actor_and_seq() {
    use redline::types::{EventKind, ParticipantId};
    let mut canvas = CanvasState::default();
    let ann = rect_annotation();
    let id = ann.id;
    let agent = ParticipantId::new();
    canvas.undo_stacks.insert(agent, vec![]);
    canvas.redo_stacks.insert(agent, vec![]);

    CommandExecutor::execute(&mut canvas, agent, Command::Add(ann));

    assert_eq!(canvas.event_log.len(), 1);
    let evt = &canvas.event_log[0];
    assert_eq!(evt.seq, 0);
    assert_eq!(evt.author, agent);
    assert!(matches!(&evt.kind, EventKind::ShapeAdded(a) if a.id == id));
}

#[test]
fn execute_routes_inverse_to_actor_undo_stack() {
    use redline::types::ParticipantId;
    let mut canvas = CanvasState::default();
    let agent = ParticipantId::new();
    canvas.undo_stacks.insert(agent, vec![]);
    canvas.redo_stacks.insert(agent, vec![]);

    CommandExecutor::execute(&mut canvas, agent, Command::Add(rect_annotation()));

    assert_eq!(canvas.undo_stacks[&agent].len(), 1);
    assert_eq!(canvas.undo_stacks[&ParticipantId::HUMAN_LOCAL].len(), 0);
}

#[test]
fn undo_pops_only_actor_stack() {
    use redline::types::ParticipantId;
    let mut canvas = CanvasState::default();
    let agent = ParticipantId::new();
    canvas.undo_stacks.insert(agent, vec![]);
    canvas.redo_stacks.insert(agent, vec![]);

    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    CommandExecutor::execute(&mut canvas, agent, Command::Add(rect_annotation()));
    assert_eq!(canvas.annotations.len(), 2);

    CommandExecutor::undo(&mut canvas, agent);
    assert_eq!(canvas.annotations.len(), 1);
    assert_eq!(canvas.undo_stacks[&ParticipantId::HUMAN_LOCAL].len(), 1);
}

#[test]
fn set_text_content_round_trips() {
    use egui::pos2;
    use redline::types::{EventKind, ParticipantId, ShapeData};
    let mut canvas = CanvasState::default();
    let text_ann = Annotation::new(
        ShapeData::Text {
            pos: pos2(10.0, 10.0),
            content: "hello".to_string(),
            size: 18.0,
        },
        Color32::RED,
        1.0,
    );
    let id = text_ann.id;
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(text_ann),
    );

    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::SetTextContent {
            id,
            content: "world".to_string(),
        },
    );

    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    assert!(matches!(&ann.shape, ShapeData::Text { content, .. } if content == "world"));

    // Undo restores prior content.
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    assert!(matches!(&ann.shape, ShapeData::Text { content, .. } if content == "hello"));

    // Redo reapplies.
    CommandExecutor::redo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    assert!(matches!(&ann.shape, ShapeData::Text { content, .. } if content == "world"));

    // Event log should have: Add, SetTextContent, Undo (which emits SetTextContent
    // reverting), Redo (which emits SetTextContent reapplying) = 4 events total.
    assert_eq!(canvas.event_log.len(), 4);
    let text_changes: Vec<_> = canvas
        .event_log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ShapeTextChanged { .. }))
        .collect();
    assert_eq!(text_changes.len(), 3);
}

#[test]
fn set_text_content_on_non_text_shape_is_noop() {
    use redline::types::ParticipantId;
    let mut canvas = CanvasState::default();
    let rect = rect_annotation();
    let id = rect.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(rect));

    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::SetTextContent {
            id,
            content: "doesnt apply".to_string(),
        },
    );

    // No panic; shape unchanged.
    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    assert!(matches!(ann.shape, redline::types::ShapeData::Rect { .. }));
}

#[test]
fn replace_pen_points_round_trips() {
    use egui::pos2;
    use redline::types::{EventKind, ParticipantId, ShapeData};

    let mut canvas = CanvasState::default();
    let original_pts = vec![pos2(0.0, 0.0), pos2(10.0, 10.0)];
    let pen_ann = Annotation::new(
        ShapeData::Pen {
            raw_points: original_pts.clone(),
            smoothed_points: original_pts.clone(),
        },
        Color32::RED,
        1.0,
    );
    let id = pen_ann.id;
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(pen_ann),
    );

    let new_pts = vec![pos2(5.0, 5.0), pos2(15.0, 15.0), pos2(25.0, 25.0)];
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::ReplacePenPoints {
            id,
            raw_points: new_pts.clone(),
            smoothed_points: new_pts.clone(),
        },
    );

    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    match &ann.shape {
        ShapeData::Pen { raw_points, .. } => assert_eq!(*raw_points, new_pts),
        _ => panic!("expected Pen"),
    }

    // Undo restores original points.
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    match &ann.shape {
        ShapeData::Pen { raw_points, .. } => assert_eq!(*raw_points, original_pts),
        _ => panic!("expected Pen"),
    }

    // Event log should contain a ShapePenPointsChanged event for forward + undo.
    let pen_changes: Vec<_> = canvas
        .event_log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ShapePenPointsChanged { .. }))
        .collect();
    assert_eq!(pen_changes.len(), 2);
}

#[test]
fn replace_pen_points_on_non_pen_shape_is_noop() {
    use redline::types::ParticipantId;
    let mut canvas = CanvasState::default();
    let rect = rect_annotation();
    let id = rect.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(rect));

    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::ReplacePenPoints {
            id,
            raw_points: vec![],
            smoothed_points: vec![],
        },
    );

    let ann = canvas.annotations.iter().find(|a| a.id == id).unwrap();
    assert!(matches!(ann.shape, redline::types::ShapeData::Rect { .. }));
}

#[test]
fn undo_restores_history_when_apply_fails() {
    use egui::vec2;

    let mut canvas = CanvasState::default();
    let actor = ParticipantId::HUMAN_LOCAL;

    // Add a shape and lock it.
    let ann = rect_annotation();
    let id = ann.id;
    canvas.annotations.push(ann);
    canvas
        .annotations
        .iter_mut()
        .find(|a| a.id == id)
        .unwrap()
        .locked = true;

    // Manually push a Move command onto the actor's undo stack —
    // pretending the actor moved the shape before it was locked.
    canvas
        .undo_stacks
        .entry(actor)
        .or_default()
        .push(Command::Move {
            id,
            delta: vec2(5.0, 0.0),
        });

    // Sanity: stack has 1 entry.
    assert_eq!(
        canvas.undo_stacks.get(&actor).map(|s| s.len()).unwrap_or(0),
        1
    );

    // Try to undo. Apply will fail because shape is locked.
    let undone = CommandExecutor::undo(&mut canvas, actor);
    assert!(!undone, "undo should report false when apply fails");

    // The undo stack should still have the Move command — restored.
    assert_eq!(
        canvas.undo_stacks.get(&actor).map(|s| s.len()).unwrap_or(0),
        1,
        "undo stack should be restored after apply failure"
    );
}

#[test]
fn redo_restores_history_when_apply_fails() {
    use egui::vec2;

    let mut canvas = CanvasState::default();
    let actor = ParticipantId::HUMAN_LOCAL;

    // Add a shape and lock it.
    let ann = rect_annotation();
    let id = ann.id;
    canvas.annotations.push(ann);
    canvas
        .annotations
        .iter_mut()
        .find(|a| a.id == id)
        .unwrap()
        .locked = true;

    // Manually push a Move command onto the actor's redo stack —
    // pretending the actor had undone a move before the shape was locked.
    canvas
        .redo_stacks
        .entry(actor)
        .or_default()
        .push(Command::Move {
            id,
            delta: vec2(5.0, 0.0),
        });

    // Sanity: redo stack has 1 entry.
    assert_eq!(
        canvas.redo_stacks.get(&actor).map(|s| s.len()).unwrap_or(0),
        1
    );

    // Try to redo. Apply will fail because shape is locked.
    let redone = CommandExecutor::redo(&mut canvas, actor);
    assert!(!redone, "redo should report false when apply fails");

    // The redo stack should still have the Move command — restored.
    assert_eq!(
        canvas.redo_stacks.get(&actor).map(|s| s.len()).unwrap_or(0),
        1,
        "redo stack should be restored after apply failure"
    );
}

#[cfg(feature = "agent-server")]
#[test]
fn execute_broadcasts_when_subscriber_present() {
    use redline::agent::events::ServerEvent;
    use redline::types::ParticipantId;

    let mut canvas = CanvasState::default();
    let (broadcast_tx, mut broadcast_rx) = tokio::sync::broadcast::channel(16);
    canvas.event_broadcast = Some(broadcast_tx);

    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );

    let received = broadcast_rx
        .try_recv()
        .expect("expected one broadcast event");
    assert!(matches!(received, ServerEvent::ShapeAdded { .. }));
}

#[cfg(feature = "agent-server")]
#[test]
fn execute_does_not_broadcast_when_no_subscriber() {
    use redline::types::ParticipantId;

    let mut canvas = CanvasState::default();
    // canvas.event_broadcast remains None.
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(rect_annotation()),
    );
    // No panic, no broadcast. Just confirm event was still recorded normally.
    assert_eq!(canvas.event_log.len(), 1);
}

#[test]
fn resize_then_undo_then_redo_round_trips() {
    let mut canvas = empty_canvas();
    let actor = ParticipantId::HUMAN_LOCAL;

    // Add a rect.
    let ann = Annotation::new(
        ShapeData::plain_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
        Color32::BLACK,
        1.0,
    );
    let id = ann.id;
    assert!(CommandExecutor::execute(
        &mut canvas,
        actor,
        Command::Add(ann)
    ));
    let original_rect = canvas
        .annotations
        .iter()
        .find(|a| a.id == id)
        .unwrap()
        .shape
        .bounding_rect();

    // Resize.
    let new_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(50.0, 50.0));
    assert!(CommandExecutor::execute(
        &mut canvas,
        actor,
        Command::Resize { id, new_rect }
    ));
    assert_eq!(
        canvas
            .annotations
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .shape
            .bounding_rect(),
        new_rect
    );

    // Undo → original rect.
    assert!(CommandExecutor::undo(&mut canvas, actor));
    assert_eq!(
        canvas
            .annotations
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .shape
            .bounding_rect(),
        original_rect
    );

    // Redo → new rect.
    assert!(CommandExecutor::redo(&mut canvas, actor));
    assert_eq!(
        canvas
            .annotations
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .shape
            .bounding_rect(),
        new_rect
    );
}

// ── Phase 4: path operations (cut / weld / boolean) ──────────────────────────

use redline::types::{BezierNode, BoolOp, NodeType};

fn bezier_ann(nodes: Vec<BezierNode>, closed: bool) -> Annotation {
    Annotation::new(
        ShapeData::BezierPath {
            nodes,
            closed,
            fill: None,
        },
        Color32::RED,
        2.0,
    )
}

fn corner(x: f32, y: f32) -> BezierNode {
    BezierNode {
        pos: pos2(x, y),
        in_handle: None,
        out_handle: None,
        node_type: NodeType::Corner,
    }
}

fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<BezierNode> {
    vec![
        corner(x0, y0),
        corner(x1, y0),
        corner(x1, y1),
        corner(x0, y1),
    ]
}

#[test]
fn boolean_unite_two_overlapping_squares_yields_one_leaf_and_undo_restores_two() {
    let mut canvas = CanvasState::default();
    let a_ann = bezier_ann(square(0.0, 0.0, 10.0, 10.0), true);
    let b_ann = bezier_ann(square(5.0, 5.0, 15.0, 15.0), true);
    let a = a_ann.id;
    let b = b_ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(a_ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(b_ann));
    let before = canvas.document.leaves_in_paint_order().len();

    assert!(CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::BooleanOp {
            op: BoolOp::Unite,
            ids: vec![a, b]
        }
    ));
    // Two inputs gone, one result leaf added.
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before - 1);
    assert!(canvas.document.leaf(a).is_none() && canvas.document.leaf(b).is_none());

    assert!(CommandExecutor::undo(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before);
    assert!(canvas.document.leaf(a).is_some() && canvas.document.leaf(b).is_some());

    // Redo returns to the single-leaf union.
    assert!(CommandExecutor::redo(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before - 1);
}

#[test]
fn boolean_intersect_disjoint_is_noop() {
    let mut canvas = CanvasState::default();
    let a_ann = bezier_ann(square(0.0, 0.0, 10.0, 10.0), true);
    let b_ann = bezier_ann(square(50.0, 50.0, 60.0, 60.0), true);
    let a = a_ann.id;
    let b = b_ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(a_ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(b_ann));
    let before = canvas.document.leaves_in_paint_order().len();
    // 0-contour result → no-op, nothing recorded, inputs untouched.
    assert!(!CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::BooleanOp {
            op: BoolOp::Intersect,
            ids: vec![a, b]
        }
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before);
    assert!(canvas.document.leaf(a).is_some() && canvas.document.leaf(b).is_some());
}

#[test]
fn boolean_on_single_qualifying_input_is_noop() {
    let mut canvas = CanvasState::default();
    let closed = bezier_ann(square(0.0, 0.0, 10.0, 10.0), true);
    let open = bezier_ann(square(5.0, 5.0, 15.0, 15.0), false);
    let a = closed.id;
    let b = open.id;
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(closed),
    );
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(open));
    // Only one closed input qualifies → fewer than 2 → no-op.
    assert!(!CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::BooleanOp {
            op: BoolOp::Unite,
            ids: vec![a, b]
        }
    ));
}

#[test]
fn weld_two_open_paths_into_one_and_undo() {
    let mut canvas = CanvasState::default();
    let a_ann = bezier_ann(vec![corner(0.0, 0.0), corner(10.0, 0.0)], false);
    let b_ann = bezier_ann(vec![corner(11.0, 0.0), corner(20.0, 0.0)], false);
    let a = a_ann.id;
    let b = b_ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(a_ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(b_ann));
    let before = canvas.document.leaves_in_paint_order().len();

    assert!(CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::WeldPaths { a, b }
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before - 1);

    assert!(CommandExecutor::undo(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before);
    assert!(canvas.document.leaf(a).is_some() && canvas.document.leaf(b).is_some());
}

#[test]
fn weld_rejects_closed_input_as_noop() {
    let mut canvas = CanvasState::default();
    let a_ann = bezier_ann(square(0.0, 0.0, 10.0, 10.0), true); // closed
    let b_ann = bezier_ann(vec![corner(11.0, 0.0), corner(20.0, 0.0)], false);
    let a = a_ann.id;
    let b = b_ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(a_ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(b_ann));
    assert!(!CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::WeldPaths { a, b }
    ));
}

#[test]
fn flatten_layer_unions_closed_leaves_and_undo_restores() {
    let mut canvas = CanvasState::default();
    let layer = canvas.document.default_layer();
    let a_ann = bezier_ann(square(0.0, 0.0, 10.0, 10.0), true);
    let b_ann = bezier_ann(square(5.0, 5.0, 15.0, 15.0), true);
    let a = a_ann.id;
    let b = b_ann.id;
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(a_ann));
    CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(b_ann));
    let before = canvas.document.leaves_in_paint_order().len();

    assert!(CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::FlattenLayer { layer_id: layer }
    ));
    // Two closed leaves united into one.
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before - 1);

    assert!(CommandExecutor::undo(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL
    ));
    assert_eq!(canvas.document.leaves_in_paint_order().len(), before);
    assert!(canvas.document.leaf(a).is_some() && canvas.document.leaf(b).is_some());
}
