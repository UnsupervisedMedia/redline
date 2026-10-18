use egui::Color32;
use redline::command::CommandExecutor;
use redline::state::CanvasState;
use redline::types::ParticipantId;
use redline::types::{Annotation, Command, ShapeData};

#[test]
fn fresh_canvas_state_has_human_participant() {
    let canvas = CanvasState::default();
    assert!(canvas
        .participants
        .contains_key(&ParticipantId::HUMAN_LOCAL));
    assert_eq!(canvas.participants.len(), 1);
}

#[test]
fn fresh_canvas_state_has_empty_event_log_and_zero_seq() {
    let canvas = CanvasState::default();
    assert!(canvas.event_log.is_empty());
    assert_eq!(canvas.next_seq, 0);
}

#[test]
fn fresh_canvas_state_has_per_participant_stacks() {
    let canvas = CanvasState::default();
    assert!(canvas.undo_stacks.contains_key(&ParticipantId::HUMAN_LOCAL));
    assert!(canvas.undo_stacks[&ParticipantId::HUMAN_LOCAL].is_empty());
    assert!(canvas.redo_stacks.contains_key(&ParticipantId::HUMAN_LOCAL));
    assert!(canvas.redo_stacks[&ParticipantId::HUMAN_LOCAL].is_empty());
}

#[test]
fn clear_all_undo_redo_clears_every_participant() {
    let mut canvas = CanvasState::default();
    let agent = ParticipantId::new();
    canvas.undo_stacks.insert(agent, vec![]);
    canvas.clear_all_undo_redo();
    assert!(canvas.undo_stacks.values().all(|v| v.is_empty()));
    assert!(canvas.redo_stacks.values().all(|v| v.is_empty()));
}

#[test]
fn human_and_agent_undo_stacks_are_independent() {
    let mut canvas = CanvasState::default();
    let agent = ParticipantId::new();

    // Human draws a red rectangle.
    let human_rect = Annotation::new(
        ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(50.0, 50.0),
        )),
        Color32::RED,
        1.0,
    );
    let human_id = human_rect.id;
    CommandExecutor::execute(
        &mut canvas,
        ParticipantId::HUMAN_LOCAL,
        Command::Add(human_rect),
    );

    // Agent draws a blue rectangle.
    let agent_rect = Annotation::new_authored(
        ShapeData::plain_rect(egui::Rect::from_min_size(
            egui::pos2(60.0, 0.0),
            egui::vec2(50.0, 50.0),
        )),
        Color32::BLUE,
        1.0,
        agent,
    );
    let agent_id = agent_rect.id;
    CommandExecutor::execute(&mut canvas, agent, Command::Add(agent_rect));

    assert_eq!(canvas.annotations.len(), 2);

    // Agent undoes — only their shape disappears.
    CommandExecutor::undo(&mut canvas, agent);
    assert_eq!(canvas.annotations.len(), 1);
    assert_eq!(canvas.annotations[0].id, human_id);

    // Human undoes — their shape disappears too.
    CommandExecutor::undo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 0);

    // Both redo independently.
    CommandExecutor::redo(&mut canvas, ParticipantId::HUMAN_LOCAL);
    assert_eq!(canvas.annotations.len(), 1);
    assert_eq!(canvas.annotations[0].id, human_id);

    CommandExecutor::redo(&mut canvas, agent);
    assert_eq!(canvas.annotations.len(), 2);
    assert!(canvas.annotations.iter().any(|a| a.id == agent_id));
}

#[test]
fn event_log_ring_buffer_evicts_oldest() {
    use redline::state::EVENT_LOG_CAPACITY;
    let mut canvas = CanvasState::default();

    // Push EVENT_LOG_CAPACITY + 5 commands to overflow the ring buffer.
    for _ in 0..(EVENT_LOG_CAPACITY + 5) {
        let ann = Annotation::new(ShapeData::plain_rect(egui::Rect::ZERO), Color32::RED, 1.0);
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    }

    assert_eq!(canvas.event_log.len(), EVENT_LOG_CAPACITY);
    // Oldest retained seq is 5 (we evicted seqs 0..4).
    assert_eq!(canvas.event_log.front().unwrap().seq, 5);
    assert_eq!(
        canvas.event_log.back().unwrap().seq,
        (EVENT_LOG_CAPACITY + 4) as u64
    );
}

#[test]
fn events_since_returns_correct_slice() {
    let mut canvas = CanvasState::default();
    for _ in 0..10 {
        let ann = Annotation::new(ShapeData::plain_rect(egui::Rect::ZERO), Color32::RED, 1.0);
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    }

    // Asking for events since seq=7 returns seqs 8 and 9 (>seq).
    let evts = canvas.events_since(7).unwrap();
    assert_eq!(evts.len(), 2);
    assert_eq!(evts[0].seq, 8);
    assert_eq!(evts[1].seq, 9);

    // Asking for events since the latest seq returns empty.
    let evts = canvas.events_since(9).unwrap();
    assert!(evts.is_empty());
}

#[test]
fn events_since_returns_err_when_below_buffer_floor() {
    use redline::state::EVENT_LOG_CAPACITY;
    let mut canvas = CanvasState::default();

    // Overflow the ring buffer.
    for _ in 0..(EVENT_LOG_CAPACITY + 100) {
        let ann = Annotation::new(ShapeData::plain_rect(egui::Rect::ZERO), Color32::RED, 1.0);
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
    }

    // Asking for events since seq=0 (long since evicted) returns Err with current seq.
    let result = canvas.events_since(0);
    assert!(result.is_err());
    let current_seq = result.unwrap_err();
    assert_eq!(current_seq, (EVENT_LOG_CAPACITY + 99) as u64);
}
