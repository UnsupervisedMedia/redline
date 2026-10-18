//! MCP tool handler functions. Each handler is a dispatch arm that turns an
//! AgentOp into a side effect (mutation of AppState) and replies via the op's
//! oneshot channel.

use uuid::Uuid;

use crate::agent::types::AnnotationDescriptor;
use crate::agent::types::{AgentError, AgentOp, RoomDescriptor};
use crate::state::AppState;

/// Dispatch a single AgentOp on the main thread.
pub fn dispatch(op: AgentOp, state: &mut AppState) {
    match op {
        AgentOp::ListRooms { reply, .. } => {
            let mut rooms = Vec::new();
            // Active tab
            rooms.push(RoomDescriptor {
                id: room_id_for_tab_index(state.active_tab_pos),
                label: state.tab_label.clone(),
                canvas_size: state.canvas.canvas_size,
                participant_count: state.canvas.participants.len(),
                last_seq: state.canvas.next_seq.saturating_sub(1),
            });
            // Inactive tabs
            for (i, tab) in state.inactive_tabs.iter().enumerate() {
                let logical_idx = if i < state.active_tab_pos { i } else { i + 1 };
                rooms.push(RoomDescriptor {
                    id: room_id_for_tab_index(logical_idx),
                    label: tab.label.clone(),
                    canvas_size: tab.canvas.canvas_size,
                    participant_count: tab.canvas.participants.len(),
                    last_seq: tab.canvas.next_seq.saturating_sub(1),
                });
            }
            let _ = reply.send(Ok(rooms));
        }
        AgentOp::JoinRoom {
            room_id,
            name,
            preferred_color,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };

            let used: Vec<_> = canvas.participants.values().map(|p| p.color).collect();
            let color = match preferred_color {
                Some(c) if !used.contains(&c) => c,
                _ => crate::types::pick_palette_color(&used),
            };

            let participant = crate::types::Participant {
                id: crate::types::ParticipantId::new(),
                name,
                color,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            };
            let participant_id = participant.id;
            canvas.participants.insert(participant_id, participant);
            canvas.undo_stacks.insert(participant_id, Vec::new());
            canvas.redo_stacks.insert(participant_id, Vec::new());

            let current_shapes = canvas
                .annotations
                .iter()
                .map(annotation_to_descriptor)
                .collect();

            let _ = reply.send(Ok(crate::agent::types::JoinRoomResult {
                participant_id,
                assigned_color: color,
                last_seq: canvas.next_seq.saturating_sub(1),
                current_shapes,
            }));
        }
        AgentOp::ListShapes { room_id, reply, .. } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let doc = &canvas.document;
            let shapes: Vec<_> = canvas
                .annotations
                .iter()
                .map(|a| {
                    let mut d = annotation_to_descriptor(a);
                    if let Some(lid) = layer_of(doc, a.id) {
                        d.layer = Some(lid);
                        d.layer_name = doc.get(lid).map(|n| n.name.clone());
                    }
                    // Report the node's local transform as decomposed fields.
                    if let Some(node) = doc.get(a.id) {
                        d.transform = Some(decompose_to_wire(node.transform));
                    }
                    d
                })
                .collect();
            let _ = reply.send(Ok(shapes));
        }
        AgentOp::DrawArrow {
            room_id,
            actor,
            start,
            end,
            color,
            thickness,
            style,
            heads,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Arrow {
                start,
                end,
                style,
                heads,
            };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;

            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );

            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawRect {
            room_id,
            actor,
            rect,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::plain_rect(rect);
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawPen {
            room_id,
            actor,
            points,
            smooth,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let smoothed_points = if smooth && points.len() >= 2 {
                crate::smoothing::catmull_rom(&points, 8)
            } else {
                points.clone()
            };
            let shape = crate::types::ShapeData::Pen {
                raw_points: points,
                smoothed_points,
            };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawText {
            room_id,
            actor,
            pos,
            content,
            size,
            color,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Text { pos, content, size };
            let mut ann = crate::types::Annotation::new_authored(shape, color, 1.0, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::SnapshotPng {
            room_id,
            scale,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            // Snapshot the texture into an Arc so the worker owns its own copy.
            // Note: egui::ColorImage holds a Vec<Color32>, so this clone is a
            // deep memcpy of the pixel buffer on the main thread. Acceptable
            // for now; if it shows up in frame timing, hand the texture down
            // via Arc from the upload path instead of cloning here.
            let texture = canvas
                .color_image
                .as_ref()
                .map(|ci| std::sync::Arc::new(ci.clone()));
            crate::agent::snapshot::spawn_with_reply(
                room_id,
                texture,
                canvas.annotations.clone(),
                canvas.canvas_size,
                scale,
                reply,
            );
        }
        AgentOp::ChangesSince {
            room_id,
            since,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let result = match canvas.events_since(since) {
                Ok(evts) => crate::agent::types::ChangesSinceResult {
                    events: evts.iter().map(canvas_event_to_wire).collect(),
                    truncated: false,
                    current_seq: canvas.next_seq.saturating_sub(1),
                },
                Err(current_seq) => crate::agent::types::ChangesSinceResult {
                    events: vec![],
                    truncated: true,
                    current_seq,
                },
            };
            let _ = reply.send(Ok(result));
        }
        AgentOp::DeleteShape {
            room_id,
            actor,
            shape_id,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Delete(shape_id),
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::MoveShape {
            room_id,
            actor,
            shape_id,
            delta,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            // Differentiate missing vs locked BEFORE calling execute, since execute
            // returns false for both.
            let locked = match canvas.annotations.iter().find(|a| a.id == shape_id) {
                None => {
                    let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                    return;
                }
                Some(ann) => ann.locked,
            };
            if locked {
                let _ = reply.send(Err(AgentError::ShapeLocked(shape_id)));
                return;
            }
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Move {
                    id: shape_id,
                    delta,
                },
            );
            if !applied {
                // After the pre-check, false from execute indicates a race (shape
                // disappeared between check and execute) — still report ShapeNotFound.
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::ResizeShape {
            room_id,
            actor,
            shape_id,
            new_rect,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            // Differentiate missing vs locked BEFORE calling execute, since execute
            // returns false for both.
            let locked = match canvas.annotations.iter().find(|a| a.id == shape_id) {
                None => {
                    let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                    return;
                }
                Some(ann) => ann.locked,
            };
            if locked {
                let _ = reply.send(Err(AgentError::ShapeLocked(shape_id)));
                return;
            }
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Resize {
                    id: shape_id,
                    new_rect,
                },
            );
            if !applied {
                // After the pre-check, false from execute indicates a race (shape
                // disappeared between check and execute) — still report ShapeNotFound.
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::SetProp {
            room_id,
            actor,
            shape_id,
            prop,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::SetProp { id: shape_id, prop },
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::SetTextContent {
            room_id,
            actor,
            shape_id,
            content,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            // Pre-check: distinguish missing shape from wrong shape kind.
            match canvas.annotations.iter().find(|a| a.id == shape_id) {
                None => {
                    let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                    return;
                }
                Some(ann) => {
                    if !matches!(ann.shape, crate::types::ShapeData::Text { .. }) {
                        let _ = reply.send(Err(AgentError::InvalidArgument {
                            field: "shape_id".into(),
                            expected: "Text shape".into(),
                            got: format!("{:?}", std::mem::discriminant(&ann.shape)),
                        }));
                        return;
                    }
                }
            }
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::SetTextContent {
                    id: shape_id,
                    content,
                },
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::ReplacePenPoints {
            room_id,
            actor,
            shape_id,
            points,
            smooth,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            // Pre-check: distinguish missing shape from wrong shape kind.
            match canvas.annotations.iter().find(|a| a.id == shape_id) {
                None => {
                    let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                    return;
                }
                Some(ann) => {
                    if !matches!(ann.shape, crate::types::ShapeData::Pen { .. }) {
                        let _ = reply.send(Err(AgentError::InvalidArgument {
                            field: "shape_id".into(),
                            expected: "Pen shape".into(),
                            got: format!("{:?}", std::mem::discriminant(&ann.shape)),
                        }));
                        return;
                    }
                }
            }
            let smoothed = if smooth && points.len() >= 2 {
                crate::smoothing::catmull_rom(&points, 8)
            } else {
                points.clone()
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::ReplacePenPoints {
                    id: shape_id,
                    raw_points: points,
                    smoothed_points: smoothed,
                },
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::BringToFront {
            room_id,
            actor,
            shape_id,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::BringToFront(shape_id),
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::SendToBack {
            room_id,
            actor,
            shape_id,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::SendToBack(shape_id),
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::ReorderTo {
            room_id,
            actor,
            shape_id,
            index,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let applied = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::ReorderTo {
                    id: shape_id,
                    index,
                },
            );
            if !applied {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::RenumberSteps {
            room_id,
            actor,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let _ = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::RenumberSteps,
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(seq));
        }
        AgentOp::Undo {
            room_id,
            actor,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let did_undo = crate::command::CommandExecutor::undo(canvas, actor);
            if !did_undo {
                let _ = reply.send(Err(AgentError::NothingToUndo));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(seq));
        }
        AgentOp::Redo {
            room_id,
            actor,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let did_redo = crate::command::CommandExecutor::redo(canvas, actor);
            if !did_redo {
                let _ = reply.send(Err(AgentError::NothingToRedo));
                return;
            }
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(seq));
        }
        AgentOp::DrawOval {
            room_id,
            actor,
            rect,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::plain_oval(rect);
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawLine {
            room_id,
            actor,
            start,
            end,
            style,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Line { start, end, style };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawStar {
            room_id,
            actor,
            rect,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Star { rect };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawCallout {
            room_id,
            actor,
            rect,
            tail,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Callout { rect, tail };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawRoundedRect {
            room_id,
            actor,
            rect,
            radius,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Rect {
                rect,
                radius,
                fill: Some(egui::Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    20,
                )),
                stroke: true,
                stroke_style: crate::types::LineStyle::Solid,
            };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawHighlighter {
            room_id,
            actor,
            rect,
            color,
            thickness,
            label,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let shape = crate::types::ShapeData::Rect {
                rect,
                radius: 0.0,
                fill: Some(egui::Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    120,
                )),
                stroke: false,
                stroke_style: crate::types::LineStyle::Solid,
            };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawAsk {
            room_id,
            actor,
            anchor,
            content,
            size,
            color,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let pos = anchor.unwrap_or_else(|| {
                egui::pos2(canvas.canvas_size.x * 0.5, canvas.canvas_size.y * 0.5)
            });
            let shape = crate::types::ShapeData::Question { pos, content, size };
            let ann = crate::types::Annotation::new_authored(shape, color, 1.0, actor);
            // Asks are not provisional — they're permanent until human deletes them.
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        AgentOp::DrawPropose {
            room_id,
            actor,
            shape,
            color,
            thickness,
            label,
            expires_in,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let mut ann = crate::types::Annotation::new_authored(shape, color, thickness, actor);
            ann.label = label;
            ann.provisional_until = Some(std::time::SystemTime::now() + expires_in);
            let shape_id = ann.id;
            crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Add(ann),
            );
            let seq = canvas.next_seq.saturating_sub(1);
            let _ = reply.send(Ok(crate::agent::types::DrawResult { shape_id, seq }));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::RecordToolWish {
            room_id,
            agent_id,
            content,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            canvas.agent_memory.entry(agent_id).or_default().push(
                crate::local_ai::memory::MemoryEntry {
                    ts: std::time::SystemTime::now(),
                    kind: crate::local_ai::memory::MemoryKind::ToolWish,
                    content,
                },
            );
            let _ = reply.send(Ok(()));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::RecordDecision {
            room_id,
            agent_id,
            content,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            canvas.agent_memory.entry(agent_id).or_default().push(
                crate::local_ai::memory::MemoryEntry {
                    ts: std::time::SystemTime::now(),
                    kind: crate::local_ai::memory::MemoryKind::Decision,
                    content,
                },
            );
            let _ = reply.send(Ok(()));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::RecordLesson {
            room_id,
            agent_id,
            content,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            canvas.agent_memory.entry(agent_id).or_default().push(
                crate::local_ai::memory::MemoryEntry {
                    ts: std::time::SystemTime::now(),
                    kind: crate::local_ai::memory::MemoryKind::Lesson,
                    content,
                },
            );
            let _ = reply.send(Ok(()));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::RecordPrivateNote {
            room_id,
            agent_id,
            content,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            canvas.agent_memory.entry(agent_id).or_default().push(
                crate::local_ai::memory::MemoryEntry {
                    ts: std::time::SystemTime::now(),
                    kind: crate::local_ai::memory::MemoryKind::Private,
                    content,
                },
            );
            let _ = reply.send(Ok(()));
        }
        AgentOp::ListExpiredProvisionals { room_id, reply, .. } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let now = std::time::SystemTime::now();
            let ids: Vec<Uuid> = canvas
                .annotations
                .iter()
                .filter_map(|a| match a.provisional_until {
                    Some(t) if t <= now => Some(a.id),
                    _ => None,
                })
                .collect();
            let _ = reply.send(Ok(ids));
        }
        AgentOp::GetSelectedShapeIds { room_id, reply, .. } => {
            // Verify the room exists.
            if find_canvas_mut(state, room_id).is_none() {
                let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                return;
            }
            let ids = match &state.tool_state {
                crate::state::ToolState::Selected { id, .. } => vec![*id],
                crate::state::ToolState::MultiSelected { ids, .. } => ids.clone(),
                _ => vec![],
            };
            let _ = reply.send(Ok(ids));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::LoadAgentMemory {
            room_id,
            agent_id,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let entries = canvas.agent_memory.entry(agent_id).or_default();
            let (upgraded_entries, upgraded) =
                crate::local_ai::memory::upgrade_legacy_entries(std::mem::take(entries));
            *entries = upgraded_entries.clone();
            if upgraded {
                // Keep the migrated data in memory so future loads and compression
                // stop treating legacy decision entries as lessons.
            }
            let _ = reply.send(Ok(upgraded_entries));
        }
        #[cfg(feature = "local-ai")]
        AgentOp::CompressAgentMemory {
            room_id,
            agent_id,
            summary_text,
            reply,
            ..
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let existing = canvas.agent_memory.remove(&agent_id).unwrap_or_default();
            let updated =
                crate::local_ai::memory::replace_decisions_with_summary(existing, summary_text);
            canvas.agent_memory.insert(agent_id, updated);
            let _ = reply.send(Ok(()));
        }
        AgentOp::ListLayers { room_id, reply } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let doc = &canvas.document;
            let layers = doc
                .layers()
                .iter()
                .filter_map(|lid| {
                    let n = doc.get(*lid)?;
                    Some(crate::agent::types::LayerDescriptor {
                        id: *lid,
                        name: n.name.clone(),
                        visible: n.visible,
                        locked: n.locked,
                        opacity: n.opacity,
                        shape_ids: doc.descendant_leaf_ids(*lid),
                        transform: Some(decompose_to_wire(n.transform)),
                    })
                })
                .collect();
            let _ = reply.send(Ok(layers));
        }
        AgentOp::CreateLayer {
            room_id,
            actor,
            name,
            reply,
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let id = uuid::Uuid::new_v4();
            let ok = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::CreateLayer { id, name },
            );
            let _ = reply.send(if ok {
                Ok(id)
            } else {
                Err(AgentError::Internal {
                    reason: "create_layer failed".into(),
                })
            });
        }
        AgentOp::MoveToLayer {
            room_id,
            actor,
            shape_id,
            layer_id,
            index,
            reply,
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            if canvas.document.get(shape_id).is_none() {
                let _ = reply.send(Err(AgentError::ShapeNotFound(shape_id)));
            } else if canvas.document.get(layer_id).is_none() {
                let _ = reply.send(Err(AgentError::ShapeNotFound(layer_id)));
            } else if !canvas.document.can_reparent(shape_id, layer_id) {
                // Reject corrupting moves (cycle / layer-under-non-root / root)
                // with an explicit error rather than a silent no-op.
                let _ = reply.send(Err(AgentError::InvalidArgument {
                    field: "layer_id".into(),
                    expected: "a target that does not create a cycle or misplace a layer".into(),
                    got: "target inside the moved subtree, or a non-root parent for a layer".into(),
                }));
            } else {
                let idx = index.unwrap_or(usize::MAX);
                let ok = crate::command::CommandExecutor::execute(
                    canvas,
                    actor,
                    crate::types::Command::MoveNodeToLayer {
                        id: shape_id,
                        layer_id,
                        index: idx,
                    },
                );
                let _ = reply.send(if ok {
                    Ok(())
                } else {
                    Err(AgentError::Internal {
                        reason: "move failed".into(),
                    })
                });
            }
        }
        AgentOp::SetLayerProp {
            room_id,
            actor,
            layer_id,
            prop,
            reply,
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            if canvas.document.get(layer_id).is_none() {
                let _ = reply.send(Err(AgentError::ShapeNotFound(layer_id)));
            } else {
                let ok = crate::command::CommandExecutor::execute(
                    canvas,
                    actor,
                    crate::types::Command::SetNodeProp { id: layer_id, prop },
                );
                let _ = reply.send(if ok {
                    Ok(())
                } else {
                    Err(AgentError::Internal {
                        reason: "set_layer_prop failed".into(),
                    })
                });
            }
        }
        AgentOp::GroupShapes {
            room_id,
            actor,
            shape_ids,
            name,
            reply,
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let ok = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Group {
                    ids: shape_ids.clone(),
                    name,
                },
            );
            // Group's new id = the (shared) parent of the first child after grouping.
            let gid = shape_ids
                .first()
                .and_then(|c| canvas.document.get(*c))
                .and_then(|n| n.parent);
            let _ = reply.send(match (ok, gid) {
                (true, Some(g)) => Ok(g),
                _ => Err(AgentError::InvalidArgument {
                    field: "shape_ids".into(),
                    expected: "shapes sharing one parent".into(),
                    got: "mixed/invalid parents".into(),
                }),
            });
        }
        AgentOp::Ungroup {
            room_id,
            actor,
            group_id,
            reply,
        } => {
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            let ok = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::Ungroup { group_id },
            );
            let _ = reply.send(if ok {
                Ok(())
            } else {
                Err(AgentError::ShapeNotFound(group_id))
            });
        }
        AgentOp::SetTransform {
            room_id,
            actor,
            node_id,
            translate,
            scale,
            rotate_deg,
            skew_deg,
            reply,
        } => {
            // Semantics: the params describe the desired **absolute local transform**
            // components. We recompose them into a kurbo::Affine via
            // `transform::recompose(Decomposed{..})` and set it as the node's local
            // transform via `SetNodeProp { Transform(..) }`. This is NOT a delta —
            // it fully replaces the current local transform. The decompose/recompose
            // cycle is symmetric, so `list_shapes`/`list_layers` will report back
            // exactly what was set.
            let canvas = match find_canvas_mut(state, room_id) {
                Some(c) => c,
                None => {
                    let _ = reply.send(Err(AgentError::RoomNotFound(room_id)));
                    return;
                }
            };
            if canvas.document.get(node_id).is_none() {
                let _ = reply.send(Err(AgentError::ShapeNotFound(node_id)));
                return;
            }
            let new_transform = crate::transform::recompose(crate::transform::Decomposed {
                tx: translate[0],
                ty: translate[1],
                scale_x: scale[0],
                scale_y: scale[1],
                rotation: rotate_deg.to_radians(),
                skew: skew_deg.to_radians(),
            });
            let ok = crate::command::CommandExecutor::execute(
                canvas,
                actor,
                crate::types::Command::SetNodeProp {
                    id: node_id,
                    prop: crate::types::NodeProp::Transform(new_transform),
                },
            );
            let _ = reply.send(if ok {
                Ok(())
            } else {
                Err(AgentError::Internal {
                    reason: "set_transform failed".into(),
                })
            });
        }
    }
}

/// Stable per-session UUID for a tab at logical position `idx`. Rebuilt after
/// a restart (acceptable for Phase 2; Phase 2.5 may upgrade to persistent IDs).
pub fn room_id_for_tab_index(idx: usize) -> Uuid {
    let ns = Uuid::from_u128(0x4B4C4E5252443A41_45474E5443525352);
    Uuid::new_v5(&ns, idx.to_le_bytes().as_ref())
}

// Silence unused-import warning until other tasks use it.
#[allow(dead_code)]
fn _ensure_error_in_scope(e: AgentError) -> AgentError {
    e
}

/// Look up a canvas (active or inactive tab) by stable room id.
pub fn find_canvas_mut(
    state: &mut AppState,
    room_id: Uuid,
) -> Option<&mut crate::state::CanvasState> {
    if room_id_for_tab_index(state.active_tab_pos) == room_id {
        return Some(&mut state.canvas);
    }
    for (i, tab) in state.inactive_tabs.iter_mut().enumerate() {
        let logical_idx = if i < state.active_tab_pos { i } else { i + 1 };
        if room_id_for_tab_index(logical_idx) == room_id {
            return Some(&mut tab.canvas);
        }
    }
    None
}

/// Convert a CanvasEvent to a wire-friendly ServerEventWire.
pub fn canvas_event_to_wire(e: &crate::types::CanvasEvent) -> crate::agent::types::ServerEventWire {
    use crate::types::EventKind;
    let ts_ms =
        e.ts.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
    let (kind, shape_id) = match &e.kind {
        EventKind::ShapeAdded(ann) => ("shape_added", Some(ann.id)),
        EventKind::ShapeMoved { id, .. } => ("shape_moved", Some(*id)),
        EventKind::ShapeResized { id, .. } => ("shape_resized", Some(*id)),
        EventKind::ShapePropChanged { id, .. } => ("shape_prop_changed", Some(*id)),
        EventKind::ShapeTextChanged { id, .. } => ("shape_text_changed", Some(*id)),
        EventKind::ShapePenPointsChanged { id, .. } => ("shape_pen_points_changed", Some(*id)),
        EventKind::ShapeReordered { id, .. } => ("shape_reordered", Some(*id)),
        EventKind::ShapeDeleted { id, .. } => ("shape_deleted", Some(*id)),
        EventKind::ImageReplaced => ("image_replaced", None),
        EventKind::LayerCreated { id, .. } => ("layer_created", Some(*id)),
        EventKind::LayerDeleted { id } => ("layer_deleted", Some(*id)),
        EventKind::NodeReparented { id, .. } => ("node_reparented", Some(*id)),
        EventKind::NodeReordered { id, .. } => ("node_reordered", Some(*id)),
        EventKind::Grouped { group_id, .. } => ("grouped", Some(*group_id)),
        EventKind::Ungrouped { group_id } => ("ungrouped", Some(*group_id)),
        EventKind::NodePropChanged { id, .. } => ("node_prop_changed", Some(*id)),
    };
    crate::agent::types::ServerEventWire {
        seq: e.seq,
        ts_ms,
        author: e.author,
        kind: kind.to_string(),
        shape_id,
    }
}

/// Convert an Annotation to a wire AnnotationDescriptor.
pub fn annotation_to_descriptor(a: &crate::types::Annotation) -> AnnotationDescriptor {
    use crate::types::ShapeData;
    let kind = match &a.shape {
        ShapeData::Rect { .. } => "rect",
        ShapeData::Oval { .. } => "oval",
        ShapeData::Star { .. } => "star",
        ShapeData::Arrow { .. } => "arrow",
        ShapeData::Line { .. } => "line",
        ShapeData::Pen { .. } => "pen",
        ShapeData::FocusRegion { .. } => "focus_region",
        ShapeData::LogicTag { .. } => "logic_tag",
        ShapeData::Text { .. } => "text",
        ShapeData::Question { .. } => "question",
        ShapeData::Blur { .. } => "blur",
        ShapeData::StepMarker { .. } => "step",
        ShapeData::ImageLayer { .. } => "image_layer",
        ShapeData::Callout { .. } => "callout",
        ShapeData::Ruler { .. } => "ruler",
        ShapeData::BezierPath { .. } => "bezier_path",
    }
    .to_string();
    AnnotationDescriptor {
        id: a.id,
        kind,
        bounding_rect: a.shape.bounding_rect(),
        color: a.color,
        label: a.label.clone(),
        locked: a.locked,
        visible: a.visible,
        author: a.author,
        layer: None,
        layer_name: None,
        transform: None,
    }
}

/// Convert a `kurbo::Affine` local transform to a wire-safe `TransformDecomposed`,
/// with angles in degrees.
fn decompose_to_wire(a: kurbo::Affine) -> crate::agent::types::TransformDecomposed {
    let d = crate::transform::decompose(a);
    crate::agent::types::TransformDecomposed {
        translate: [d.tx, d.ty],
        scale: [d.scale_x, d.scale_y],
        rotate_deg: d.rotation.to_degrees(),
        skew_deg: d.skew.to_degrees(),
    }
}

/// The top-level Layer ancestor of `id`, if any.
fn layer_of(doc: &crate::scene::Document, id: uuid::Uuid) -> Option<uuid::Uuid> {
    let mut cur = Some(id);
    let mut last_layer = None;
    while let Some(c) = cur {
        let n = doc.get(c)?;
        if matches!(n.kind, crate::scene::NodeKind::Layer) {
            last_layer = Some(c);
        }
        cur = n.parent;
    }
    last_layer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::AgentOp;
    use crate::state::AppState;
    use tokio::sync::oneshot;

    #[test]
    fn list_rooms_returns_active_tab() {
        let mut state = AppState::default();
        state.tab_label = "Test Canvas".to_string();
        state.canvas.canvas_size = egui::vec2(800.0, 600.0);

        let (reply_tx, reply_rx) = oneshot::channel();
        let op = AgentOp::ListRooms { reply: reply_tx };

        dispatch(op, &mut state);

        let result = reply_rx.blocking_recv().expect("reply").expect("ok");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].label, "Test Canvas");
        assert_eq!(result[0].canvas_size, egui::vec2(800.0, 600.0));
        assert_eq!(result[0].participant_count, 1);
        assert_eq!(result[0].last_seq, 0);
    }

    #[test]
    fn join_room_assigns_participant_and_color() {
        let mut state = AppState::default();
        state.tab_label = "T".to_string();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (reply_tx, reply_rx) = oneshot::channel();
        let op = AgentOp::JoinRoom {
            room_id,
            name: "claude".into(),
            preferred_color: None,
            reply: reply_tx,
        };
        dispatch(op, &mut state);

        let result = reply_rx.blocking_recv().unwrap().unwrap();
        assert_ne!(
            result.participant_id,
            crate::types::ParticipantId::HUMAN_LOCAL
        );
        assert_eq!(state.canvas.participants.len(), 2);
        assert!(state
            .canvas
            .participants
            .contains_key(&result.participant_id));
        assert!(state
            .canvas
            .undo_stacks
            .contains_key(&result.participant_id));
    }

    #[test]
    fn join_room_unknown_id_returns_room_not_found() {
        let mut state = AppState::default();
        let bogus = uuid::Uuid::new_v4();
        let (reply_tx, reply_rx) = oneshot::channel();
        let op = AgentOp::JoinRoom {
            room_id: bogus,
            name: "x".into(),
            preferred_color: None,
            reply: reply_tx,
        };
        dispatch(op, &mut state);
        let err = reply_rx.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, AgentError::RoomNotFound(_)));
    }

    #[test]
    fn list_shapes_returns_descriptors_for_current_annotations() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let ann = Annotation::new(
            ShapeData::plain_rect(egui::Rect::from_min_size(
                egui::pos2(10.0, 20.0),
                egui::vec2(50.0, 30.0),
            )),
            egui::Color32::RED,
            2.0,
        );
        let expected_id = ann.id;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(AgentOp::ListShapes { room_id, reply: tx }, &mut state);

        let result = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, expected_id);
        assert_eq!(result[0].kind, "rect");
        assert_eq!(result[0].color, egui::Color32::RED);
        assert_eq!(result[0].author, ParticipantId::HUMAN_LOCAL);
    }

    #[test]
    fn list_layers_and_create_layer_dispatch() {
        use crate::types::ParticipantId;
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);
        // Initially one layer.
        let (tx, rx) = oneshot::channel();
        dispatch(AgentOp::ListLayers { room_id, reply: tx }, &mut state);
        let layers = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(layers.len(), 1);
        // Create one.
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::CreateLayer {
                room_id,
                actor: ParticipantId::HUMAN_LOCAL,
                name: "Ink".into(),
                reply: tx,
            },
            &mut state,
        );
        let new_id = rx.blocking_recv().unwrap().unwrap();
        assert!(state.canvas.document.get(new_id).is_some());
        assert_eq!(state.canvas.document.layers().len(), 2);
    }

    #[test]
    fn move_to_layer_into_descendant_returns_error_and_keeps_doc_intact() {
        use crate::types::ParticipantId;
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);
        // layer -> outer(Group) -> inner(Group), with a shape inside inner.
        let layer = state.canvas.document.default_layer();
        let outer = state
            .canvas
            .document
            .create_group(layer, "outer", usize::MAX);
        let inner = state
            .canvas
            .document
            .create_group(outer, "inner", usize::MAX);
        let shape = state.canvas.document.insert_child(
            inner,
            crate::scene::Node {
                id: uuid::Uuid::new_v4(),
                parent: None,
                children: Vec::new(),
                name: "s".into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                transform: kurbo::Affine::IDENTITY,
                kind: crate::scene::NodeKind::Shape(crate::scene::ShapeLeaf {
                    author: ParticipantId::HUMAN_LOCAL,
                    shape: crate::types::ShapeData::plain_rect(egui::Rect::ZERO),
                    color: egui::Color32::RED,
                    thickness: 2.0,
                    label: None,
                    provisional_until: None,
                }),
            },
            usize::MAX,
        );
        assert!(state.canvas.document.check_invariants().is_ok());
        let leaves_before = state
            .canvas
            .document
            .descendant_leaf_ids(state.canvas.document.root);

        // Attempt to move `outer` into its own descendant `inner` — must error.
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::MoveToLayer {
                room_id,
                actor: ParticipantId::HUMAN_LOCAL,
                shape_id: outer,
                layer_id: inner,
                index: None,
                reply: tx,
            },
            &mut state,
        );
        let err = rx.blocking_recv().unwrap().unwrap_err();
        assert!(
            matches!(err, AgentError::InvalidArgument { ref field, .. } if field == "layer_id")
        );
        // Document intact: invariants hold, subtree not orphaned, shape present.
        assert!(state.canvas.document.check_invariants().is_ok());
        assert_eq!(
            state.canvas.document.get(outer).unwrap().parent,
            Some(layer)
        );
        assert!(state.canvas.document.get(shape).is_some());
        assert_eq!(
            state
                .canvas
                .document
                .descendant_leaf_ids(state.canvas.document.root),
            leaves_before
        );
    }

    #[test]
    fn list_shapes_reports_layer_for_each_shape() {
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);
        let a = crate::types::Annotation::new(
            crate::types::ShapeData::plain_rect(egui::Rect::ZERO),
            egui::Color32::RED,
            2.0,
        );
        state.canvas.add_annotation(a);
        let (tx, rx) = oneshot::channel();
        dispatch(AgentOp::ListShapes { room_id, reply: tx }, &mut state);
        let shapes = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].layer, Some(state.canvas.document.default_layer()));
    }

    #[test]
    fn draw_arrow_adds_arrow_attributed_to_actor() {
        use crate::types::{ArrowHeads, LineStyle, ParticipantId, ShapeData};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        // Pretend an agent already joined.
        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::BLUE,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawArrow {
                room_id,
                actor: agent,
                start: egui::pos2(0.0, 0.0),
                end: egui::pos2(100.0, 100.0),
                color: egui::Color32::BLUE,
                thickness: 2.0,
                style: LineStyle::Solid,
                heads: ArrowHeads::Single,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, agent);
        assert!(matches!(ann.shape, ShapeData::Arrow { .. }));
        assert_eq!(state.canvas.event_log.len(), 1);
        assert_eq!(state.canvas.event_log[0].seq, result.seq);
        assert_eq!(state.canvas.event_log[0].author, agent);
    }

    #[test]
    fn draw_rect_adds_rect_attributed_to_actor() {
        use crate::types::{ParticipantId, ShapeData};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::BLUE,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawRect {
                room_id,
                actor: agent,
                rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(50.0, 30.0)),
                color: egui::Color32::BLUE,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, agent);
        assert!(matches!(ann.shape, ShapeData::Rect { .. }));
    }

    #[test]
    fn draw_pen_adds_pen_attributed_to_actor() {
        use crate::types::{ParticipantId, ShapeData};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::GREEN,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawPen {
                room_id,
                actor: agent,
                points: vec![
                    egui::pos2(0.0, 0.0),
                    egui::pos2(10.0, 10.0),
                    egui::pos2(20.0, 5.0),
                ],
                smooth: true,
                color: egui::Color32::GREEN,
                thickness: 3.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, agent);
        assert!(matches!(ann.shape, ShapeData::Pen { .. }));
    }

    #[test]
    fn snapshot_png_returns_valid_png_bytes() {
        let mut state = AppState::default();
        state.canvas.canvas_size = egui::vec2(10.0, 10.0);
        let room_id = room_id_for_tab_index(state.active_tab_pos);
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SnapshotPng {
                room_id,
                scale: 1.0,
                reply: tx,
            },
            &mut state,
        );
        // The rayon worker runs on a background thread; blocking_recv waits for it.
        let bytes = rx.blocking_recv().unwrap().expect("png bytes");
        assert!(
            bytes.starts_with(&[0x89, b'P', b'N', b'G']),
            "expected PNG magic header, got {:?}",
            &bytes[..bytes.len().min(8)]
        );
    }

    #[test]
    fn draw_text_adds_text_attributed_to_actor() {
        use crate::types::{ParticipantId, ShapeData};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::RED,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawText {
                room_id,
                actor: agent,
                pos: egui::pos2(100.0, 200.0),
                content: "Hello, world!".into(),
                size: 18.0,
                color: egui::Color32::RED,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, agent);
        assert!(matches!(ann.shape, ShapeData::Text { .. }));
    }

    #[test]
    fn changes_since_returns_shape_added_event() {
        use crate::types::{ArrowHeads, LineStyle, ParticipantId};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::BLUE,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        // Start next_seq at 1 so the draw produces seq=1, making it possible
        // to request events with since=0 (events_since filters seq > since).
        state.canvas.next_seq = 1;

        // Draw a shape so there's at least one event in the log.
        let (draw_tx, draw_rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawArrow {
                room_id,
                actor: agent,
                start: egui::pos2(0.0, 0.0),
                end: egui::pos2(50.0, 50.0),
                color: egui::Color32::BLUE,
                thickness: 2.0,
                style: LineStyle::Solid,
                heads: ArrowHeads::Single,
                label: None,
                reply: draw_tx,
            },
            &mut state,
        );
        let draw_result = draw_rx.blocking_recv().unwrap().unwrap();
        assert_eq!(draw_result.seq, 1, "sanity: draw_result.seq should be 1");

        // Request events since 0 — events_since returns seq > since, so seq=1 qualifies.
        let (cs_tx, cs_rx) = oneshot::channel();
        dispatch(
            AgentOp::ChangesSince {
                room_id,
                since: 0,
                reply: cs_tx,
            },
            &mut state,
        );
        let result = cs_rx.blocking_recv().unwrap().unwrap();
        assert!(!result.truncated);
        assert!(!result.events.is_empty(), "expected at least one event");
        assert_eq!(result.events[0].kind, "shape_added");
        assert_eq!(result.events[0].seq, draw_result.seq);
    }

    #[test]
    fn changes_since_future_seq_returns_empty() {
        use crate::types::{ArrowHeads, LineStyle, ParticipantId};
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let agent = ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::BLUE,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());

        // Draw a shape so the event log is non-empty.
        let (draw_tx, draw_rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawArrow {
                room_id,
                actor: agent,
                start: egui::pos2(0.0, 0.0),
                end: egui::pos2(50.0, 50.0),
                color: egui::Color32::BLUE,
                thickness: 2.0,
                style: LineStyle::Solid,
                heads: ArrowHeads::Single,
                label: None,
                reply: draw_tx,
            },
            &mut state,
        );
        let draw_result = draw_rx.blocking_recv().unwrap().unwrap();

        // Ask for events after the draw seq — should return empty, not truncated.
        let (cs_tx, cs_rx) = oneshot::channel();
        dispatch(
            AgentOp::ChangesSince {
                room_id,
                since: draw_result.seq, // seq > draw_result.seq → empty
                reply: cs_tx,
            },
            &mut state,
        );
        let result = cs_rx.blocking_recv().unwrap().unwrap();
        assert!(!result.truncated);
        assert!(result.events.is_empty(), "expected empty events vec");
    }

    #[test]
    fn changes_since_unknown_room_returns_error() {
        let mut state = AppState::default();
        let bogus = uuid::Uuid::new_v4();
        let (cs_tx, cs_rx) = oneshot::channel();
        dispatch(
            AgentOp::ChangesSince {
                room_id: bogus,
                since: 0,
                reply: cs_tx,
            },
            &mut state,
        );
        let err = cs_rx.blocking_recv().unwrap().unwrap_err();
        assert!(matches!(err, AgentError::RoomNotFound(_)));
    }

    #[test]
    fn delete_shape_removes_existing_shape() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DeleteShape {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                reply: tx,
            },
            &mut state,
        );
        let result = rx.blocking_recv().unwrap().expect("deletion succeeded");
        assert_eq!(result.shape_id, shape_id);
        assert!(state
            .canvas
            .annotations
            .iter()
            .find(|a| a.id == shape_id)
            .is_none());
    }

    #[test]
    fn delete_shape_missing_returns_shape_not_found() {
        use crate::types::ParticipantId;

        let mut state = AppState::default();
        let bogus_id = uuid::Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DeleteShape {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor: ParticipantId::HUMAN_LOCAL,
                shape_id: bogus_id,
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::ShapeNotFound(id)) => assert_eq!(id, bogus_id),
            other => panic!("expected ShapeNotFound, got {other:?}"),
        }
    }

    #[test]
    fn move_shape_missing_returns_shape_not_found() {
        use crate::types::ParticipantId;
        use egui::Vec2;

        let mut state = AppState::default();
        let bogus_id = uuid::Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::MoveShape {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor: ParticipantId::HUMAN_LOCAL,
                shape_id: bogus_id,
                delta: Vec2::new(5.0, 5.0),
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::ShapeNotFound(id)) => assert_eq!(id, bogus_id),
            other => panic!("expected ShapeNotFound, got {other:?}"),
        }
    }

    #[test]
    fn set_prop_changes_color() {
        use crate::types::{Annotation, ParticipantId, PropChange, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SetProp {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                prop: PropChange::Color(Color32::BLUE),
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("set_prop succeeded");

        let updated = state
            .canvas
            .annotations
            .iter()
            .find(|a| a.id == shape_id)
            .unwrap();
        assert_eq!(updated.color, Color32::BLUE);
    }

    #[test]
    fn set_text_content_changes_text() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::Text {
                pos: pos2(10.0, 10.0),
                content: "old".into(),
                size: 18.0,
            },
            Color32::BLACK,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SetTextContent {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                content: "new".into(),
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv()
            .unwrap()
            .expect("set_text_content succeeded");

        let updated = state
            .canvas
            .annotations
            .iter()
            .find(|a| a.id == shape_id)
            .unwrap();
        if let ShapeData::Text { content, .. } = &updated.shape {
            assert_eq!(content, "new");
        } else {
            panic!("expected Text shape");
        }
    }

    #[test]
    fn replace_pen_points_updates_points() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::Pen {
                raw_points: vec![pos2(0.0, 0.0), pos2(1.0, 1.0)],
                smoothed_points: vec![pos2(0.0, 0.0), pos2(1.0, 1.0)],
            },
            Color32::BLACK,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        state.canvas.add_annotation(ann);

        let new_points = vec![pos2(5.0, 5.0), pos2(10.0, 10.0), pos2(15.0, 5.0)];

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::ReplacePenPoints {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                points: new_points.clone(),
                smooth: false,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv()
            .unwrap()
            .expect("replace_pen_points succeeded");

        let updated = state
            .canvas
            .annotations
            .iter()
            .find(|a| a.id == shape_id)
            .unwrap();
        if let ShapeData::Pen { raw_points, .. } = &updated.shape {
            assert_eq!(raw_points, &new_points);
        } else {
            panic!("expected Pen shape");
        }
    }

    #[test]
    fn bring_to_front_moves_to_end() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let a = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let a_id = a.id;
        let b = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(20.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );
        state.canvas.add_annotation(a);
        state.canvas.add_annotation(b);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::BringToFront {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id: a_id,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv()
            .unwrap()
            .expect("bring_to_front succeeded");

        assert_eq!(
            state.canvas.annotations.last().unwrap().id,
            a_id,
            "shape A should be last (front)"
        );
    }

    #[test]
    fn send_to_back_moves_to_start() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let a = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let b = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(20.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );
        let b_id = b.id;
        state.canvas.add_annotation(a);
        state.canvas.add_annotation(b);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SendToBack {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id: b_id,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("send_to_back succeeded");

        assert_eq!(
            state.canvas.annotations.first().unwrap().id,
            b_id,
            "shape B should be first (back)"
        );
    }

    #[test]
    fn reorder_to_places_shape_at_index() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let a = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let b = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(20.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );
        let c = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(40.0, 0.0), vec2(10.0, 10.0))),
            Color32::GREEN,
            1.0,
            actor,
        );
        let a_id = a.id;
        state.canvas.add_annotation(a);
        state.canvas.add_annotation(b);
        state.canvas.add_annotation(c);

        // Move A (currently at index 0) to index 1
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::ReorderTo {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id: a_id,
                index: 1,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("reorder_to succeeded");

        assert_eq!(
            state.canvas.annotations[1].id, a_id,
            "shape A should be at index 1"
        );
    }

    #[test]
    fn undo_returns_nothing_to_undo_for_empty_stack() {
        use crate::types::ParticipantId;
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::Undo {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor: ParticipantId::HUMAN_LOCAL,
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::NothingToUndo) => {}
            other => panic!("expected NothingToUndo, got {other:?}"),
        }
    }

    #[test]
    fn redo_returns_nothing_to_redo_for_empty_stack() {
        use crate::types::ParticipantId;
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::Redo {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor: ParticipantId::HUMAN_LOCAL,
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::NothingToRedo) => {}
            other => panic!("expected NothingToRedo, got {other:?}"),
        }
    }

    #[test]
    fn undo_reverts_a_draw() {
        use crate::types::{Annotation, Command, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        // Use execute (not direct push) so the undo stack is populated.
        let applied =
            crate::command::CommandExecutor::execute(&mut state.canvas, actor, Command::Add(ann));
        assert!(applied);
        assert_eq!(state.canvas.annotations.len(), 1);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::Undo {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("undo succeeded");
        assert!(
            state
                .canvas
                .annotations
                .iter()
                .find(|a| a.id == shape_id)
                .is_none(),
            "annotation should be removed after undo"
        );
    }

    #[test]
    fn move_shape_locked_returns_shape_locked() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32, Vec2};
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let mut ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        ann.locked = true;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::MoveShape {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                delta: Vec2::new(5.0, 5.0),
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::ShapeLocked(id)) => assert_eq!(id, shape_id),
            other => panic!("expected ShapeLocked, got {other:?}"),
        }
    }

    #[test]
    fn set_text_content_wrong_kind_returns_invalid_argument() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        // Rect shape, not Text — SetTextContent should reject.
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLACK,
            1.0,
            actor,
        );
        let shape_id = ann.id;
        state.canvas.add_annotation(ann);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SetTextContent {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                shape_id,
                content: "irrelevant".into(),
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::InvalidArgument { field, .. }) => assert_eq!(field, "shape_id"),
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    // ── set_transform tests ───────────────────────────────────────────────────

    /// set_transform changes the local transform of an existing node and creates
    /// an undo entry so the change is reversible.
    #[test]
    fn set_transform_changes_node_transform_and_is_undoable() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(50.0, 50.0))),
            Color32::RED,
            1.0,
            actor,
        );
        let node_id = ann.id;
        state.canvas.add_annotation(ann);

        let room_id = room_id_for_tab_index(state.active_tab_pos);

        // Apply a 45° rotation with a 2× uniform scale and a translation.
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SetTransform {
                room_id,
                actor,
                node_id,
                translate: [10.0, 20.0],
                scale: [2.0, 2.0],
                rotate_deg: 45.0,
                skew_deg: 0.0,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("set_transform ok");

        // Verify the transform was applied to the document node.
        let node = state.canvas.document.get(node_id).unwrap();
        let d = crate::transform::decompose(node.transform);
        assert!((d.tx - 10.0).abs() < 1e-6, "tx: {}", d.tx);
        assert!((d.ty - 20.0).abs() < 1e-6, "ty: {}", d.ty);
        assert!((d.scale_x - 2.0).abs() < 1e-5, "sx: {}", d.scale_x);
        assert!((d.scale_y - 2.0).abs() < 1e-5, "sy: {}", d.scale_y);
        assert!(
            (d.rotation.to_degrees() - 45.0).abs() < 1e-4,
            "rot deg: {}",
            d.rotation.to_degrees()
        );

        // Undo must exist for this actor: undo restores identity transform.
        assert!(
            !state.canvas.undo_stacks.get(&actor).unwrap().is_empty(),
            "undo stack must be non-empty after set_transform"
        );
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::Undo {
                room_id,
                actor,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("undo ok");

        let node_after = state.canvas.document.get(node_id).unwrap();
        let d2 = crate::transform::decompose(node_after.transform);
        assert!((d2.tx).abs() < 1e-6, "tx after undo: {}", d2.tx);
        assert!((d2.ty).abs() < 1e-6, "ty after undo: {}", d2.ty);
        assert!(
            (d2.scale_x - 1.0).abs() < 1e-5,
            "sx after undo: {}",
            d2.scale_x
        );
        assert!(
            (d2.rotation).abs() < 1e-5,
            "rot after undo: {}",
            d2.rotation
        );
    }

    /// set_transform with a missing node id must return ShapeNotFound.
    #[test]
    fn set_transform_missing_node_returns_shape_not_found() {
        let mut state = AppState::default();
        let room_id = room_id_for_tab_index(state.active_tab_pos);
        let bogus_id = uuid::Uuid::new_v4();

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::SetTransform {
                room_id,
                actor: crate::types::ParticipantId::HUMAN_LOCAL,
                node_id: bogus_id,
                translate: [0.0, 0.0],
                scale: [1.0, 1.0],
                rotate_deg: 0.0,
                skew_deg: 0.0,
                reply: tx,
            },
            &mut state,
        );
        match rx.blocking_recv().unwrap() {
            Err(AgentError::ShapeNotFound(id)) => assert_eq!(id, bogus_id),
            other => panic!("expected ShapeNotFound, got {other:?}"),
        }
    }

    /// list_shapes must include the transform field with correct decomposed values
    /// for a node that has a non-identity local transform.
    #[test]
    fn list_shapes_reports_transform_for_transformed_node() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(100.0, 100.0),
            )),
            Color32::BLUE,
            1.0,
            actor,
        );
        let node_id = ann.id;
        state.canvas.add_annotation(ann);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        // Set a known non-identity transform on the node directly so we can assert exact values.
        let want = crate::transform::recompose(crate::transform::Decomposed {
            tx: 5.0,
            ty: -3.0,
            scale_x: 1.5,
            scale_y: 0.8,
            rotation: 30_f64.to_radians(),
            skew: 0.0,
        });
        state.canvas.document.get_mut(node_id).unwrap().transform = want;

        // list_shapes
        let (tx, rx) = oneshot::channel();
        dispatch(AgentOp::ListShapes { room_id, reply: tx }, &mut state);
        let shapes = rx.blocking_recv().unwrap().unwrap();
        assert_eq!(shapes.len(), 1);

        let t = shapes[0]
            .transform
            .as_ref()
            .expect("transform must be present");
        assert!(
            (t.translate[0] - 5.0).abs() < 1e-4,
            "tx: {}",
            t.translate[0]
        );
        assert!(
            (t.translate[1] - -3.0).abs() < 1e-4,
            "ty: {}",
            t.translate[1]
        );
        assert!((t.scale[0] - 1.5).abs() < 1e-4, "sx: {}", t.scale[0]);
        assert!((t.scale[1] - 0.8).abs() < 1e-4, "sy: {}", t.scale[1]);
        assert!(
            (t.rotate_deg - 30.0).abs() < 1e-3,
            "rot_deg: {}",
            t.rotate_deg
        );
        assert!(t.skew_deg.abs() < 1e-4, "skew_deg: {}", t.skew_deg);

        // Also verify the JSON includes the transform field (wire format check).
        let json = serde_json::to_string(&shapes[0]).unwrap();
        assert!(
            json.contains("\"transform\""),
            "JSON must include transform: {json}"
        );
        assert!(
            json.contains("\"rotate_deg\""),
            "JSON must include rotate_deg: {json}"
        );
    }

    /// list_layers must include the transform field so agents can read back a
    /// layer's local transform after calling set_transform on a layer node.
    #[test]
    fn list_layers_reports_transform() {
        use crate::types::ParticipantId;

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        // Create a layer via command so it lives in the document.
        let layer_id = uuid::Uuid::new_v4();
        crate::command::CommandExecutor::execute(
            &mut state.canvas,
            actor,
            crate::types::Command::CreateLayer {
                id: layer_id,
                name: "test-layer".into(),
            },
        );

        // Set a non-identity transform directly on the layer node.
        let want = crate::transform::recompose(crate::transform::Decomposed {
            tx: 10.0,
            ty: -5.0,
            scale_x: 2.0,
            scale_y: 0.5,
            rotation: 45_f64.to_radians(),
            skew: 0.0,
        });
        state.canvas.document.get_mut(layer_id).unwrap().transform = want;

        // list_layers
        let (tx, rx) = oneshot::channel();
        dispatch(AgentOp::ListLayers { room_id, reply: tx }, &mut state);
        let layers = rx.blocking_recv().unwrap().unwrap();

        let layer = layers
            .iter()
            .find(|l| l.id == layer_id)
            .expect("layer must appear in list");
        let t = layer
            .transform
            .as_ref()
            .expect("transform must be present on layer descriptor");
        assert!(
            (t.translate[0] - 10.0).abs() < 1e-4,
            "tx: {}",
            t.translate[0]
        );
        assert!(
            (t.translate[1] - -5.0).abs() < 1e-4,
            "ty: {}",
            t.translate[1]
        );
        assert!((t.scale[0] - 2.0).abs() < 1e-4, "sx: {}", t.scale[0]);
        assert!((t.scale[1] - 0.5).abs() < 1e-4, "sy: {}", t.scale[1]);
        assert!(
            (t.rotate_deg - 45.0).abs() < 1e-3,
            "rot_deg: {}",
            t.rotate_deg
        );
        assert!(t.skew_deg.abs() < 1e-4, "skew_deg: {}", t.skew_deg);

        // Wire format check.
        let json = serde_json::to_string(layer).unwrap();
        assert!(
            json.contains("\"transform\""),
            "JSON must include transform: {json}"
        );
        assert!(
            json.contains("\"rotate_deg\""),
            "JSON must include rotate_deg: {json}"
        );
    }

    fn make_agent(state: &mut AppState) -> crate::types::ParticipantId {
        let agent = crate::types::ParticipantId::new();
        state.canvas.participants.insert(
            agent,
            crate::types::Participant {
                id: agent,
                name: "claude".into(),
                color: egui::Color32::BLUE,
                cursor: None,
                last_active: std::time::Instant::now(),
                status: None,
            },
        );
        state.canvas.undo_stacks.insert(agent, Vec::new());
        state.canvas.redo_stacks.insert(agent, Vec::new());
        agent
    }

    #[test]
    fn draw_oval_adds_oval_attributed_to_actor() {
        use crate::types::ShapeData;
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawOval {
                room_id,
                actor,
                rect: egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(60.0, 40.0)),
                color: egui::Color32::RED,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_oval ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(ann.shape, ShapeData::Oval { .. }));
    }

    #[test]
    fn draw_line_adds_line_attributed_to_actor() {
        use crate::types::{LineStyle, ShapeData};
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawLine {
                room_id,
                actor,
                start: egui::pos2(0.0, 0.0),
                end: egui::pos2(100.0, 50.0),
                style: LineStyle::Solid,
                color: egui::Color32::GREEN,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_line ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(ann.shape, ShapeData::Line { .. }));
    }

    #[test]
    fn draw_star_adds_star_attributed_to_actor() {
        use crate::types::ShapeData;
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawStar {
                room_id,
                actor,
                rect: egui::Rect::from_min_max(egui::pos2(20.0, 20.0), egui::pos2(80.0, 80.0)),
                color: egui::Color32::YELLOW,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_star ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(ann.shape, ShapeData::Star { .. }));
    }

    #[test]
    fn draw_callout_adds_callout_attributed_to_actor() {
        use crate::types::ShapeData;
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawCallout {
                room_id,
                actor,
                rect: egui::Rect::from_min_max(egui::pos2(50.0, 50.0), egui::pos2(200.0, 120.0)),
                tail: egui::pos2(30.0, 150.0),
                color: egui::Color32::BLUE,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_callout ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(ann.shape, ShapeData::Callout { .. }));
    }

    #[test]
    fn draw_rounded_rect_adds_rounded_rect_attributed_to_actor() {
        use crate::types::ShapeData;
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawRoundedRect {
                room_id,
                actor,
                rect: egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(100.0, 60.0)),
                radius: 8.0,
                color: egui::Color32::RED,
                thickness: 2.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_rounded_rect ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(
            ann.shape,
            ShapeData::Rect { radius, fill: Some(_), stroke: true, .. } if radius > 0.0
        ));
    }

    #[test]
    fn draw_highlighter_adds_highlighter_attributed_to_actor() {
        use crate::types::ShapeData;
        let mut state = AppState::default();
        let actor = make_agent(&mut state);
        let room_id = room_id_for_tab_index(state.active_tab_pos);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawHighlighter {
                room_id,
                actor,
                rect: egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(300.0, 120.0)),
                color: egui::Color32::YELLOW,
                thickness: 20.0,
                label: None,
                reply: tx,
            },
            &mut state,
        );

        let result = rx.blocking_recv().unwrap().expect("draw_highlighter ok");
        assert_eq!(state.canvas.annotations.len(), 1);
        let ann = &state.canvas.annotations[0];
        assert_eq!(ann.id, result.shape_id);
        assert_eq!(ann.author, actor);
        assert!(matches!(
            ann.shape,
            ShapeData::Rect {
                fill: Some(_),
                stroke: false,
                ..
            }
        ));
    }

    #[test]
    fn renumber_steps_sequences_markers() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, Color32};

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;
        let a = Annotation::new_authored(
            ShapeData::StepMarker {
                pos: pos2(10.0, 10.0),
                number: 5,
                radius: 12.0,
            },
            Color32::BLACK,
            1.0,
            actor,
        );
        let b = Annotation::new_authored(
            ShapeData::StepMarker {
                pos: pos2(20.0, 20.0),
                number: 9,
                radius: 12.0,
            },
            Color32::BLACK,
            1.0,
            actor,
        );
        state.canvas.add_annotation(a);
        state.canvas.add_annotation(b);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::RenumberSteps {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor,
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv()
            .unwrap()
            .expect("renumber_steps succeeded");

        let numbers: Vec<u32> = state
            .canvas
            .annotations
            .iter()
            .filter_map(|a| {
                if let ShapeData::StepMarker { number, .. } = a.shape {
                    Some(number)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            numbers,
            vec![1, 2],
            "step markers should be renumbered to 1, 2"
        );
    }

    #[cfg(feature = "local-ai")]
    #[test]
    fn record_tool_wish_appends_memory_entry() {
        use std::time::Duration;
        use tokio::sync::oneshot;
        let mut state = AppState::default();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::RecordToolWish {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                agent_id: "local-ai".into(),
                content: "I wish I had draw_table".into(),
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("recorded");
        let entries = state
            .canvas
            .agent_memory
            .get("local-ai")
            .expect("agent_memory entry");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].kind,
            crate::local_ai::memory::MemoryKind::ToolWish
        );
        assert_eq!(entries[0].content, "I wish I had draw_table");
    }

    #[test]
    fn draw_propose_sets_provisional_until() {
        use crate::types::{ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};
        use std::time::Duration;
        use tokio::sync::oneshot;
        let mut state = AppState::default();
        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::DrawPropose {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                actor: ParticipantId::HUMAN_LOCAL,
                shape: ShapeData::plain_rect(egui::Rect::from_min_size(
                    pos2(0.0, 0.0),
                    vec2(10.0, 10.0),
                )),
                color: Color32::BLUE,
                thickness: 1.0,
                label: None,
                expires_in: Duration::from_secs(30),
                reply: tx,
            },
            &mut state,
        );
        rx.blocking_recv().unwrap().expect("proposed");
        let ann = state.canvas.annotations.last().unwrap();
        assert!(
            ann.provisional_until.is_some(),
            "ghost annotation must have provisional_until set"
        );
    }

    #[test]
    fn list_expired_provisionals_returns_passed_only() {
        use crate::types::{Annotation, ParticipantId, ShapeData};
        use egui::{pos2, vec2, Color32};
        use std::time::{Duration as StdDuration, SystemTime};
        use tokio::sync::oneshot;

        let mut state = AppState::default();
        let actor = ParticipantId::HUMAN_LOCAL;

        let mut expired = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );
        expired.provisional_until = Some(SystemTime::now() - StdDuration::from_secs(5));
        let expired_id = expired.id;

        let mut future = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(20.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );
        future.provisional_until = Some(SystemTime::now() + StdDuration::from_secs(60));

        let permanent = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::from_min_size(pos2(40.0, 0.0), vec2(10.0, 10.0))),
            Color32::BLUE,
            1.0,
            actor,
        );

        state.canvas.add_annotation(expired);
        state.canvas.add_annotation(future);
        state.canvas.add_annotation(permanent);

        let (tx, rx) = oneshot::channel();
        dispatch(
            AgentOp::ListExpiredProvisionals {
                room_id: room_id_for_tab_index(state.active_tab_pos),
                reply: tx,
            },
            &mut state,
        );
        let ids = rx.blocking_recv().unwrap().expect("listed");
        assert_eq!(ids, vec![expired_id]);
    }
}
