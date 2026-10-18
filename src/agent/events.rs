//! Server events broadcast from the main thread to per-session MCP subscribers.

use serde::Serialize;
use uuid::Uuid;

use crate::agent::types::AnnotationDescriptor;
use crate::types::{CanvasEvent, EventKind, ParticipantId};

/// One change a subscriber should know about. Carries enough info to either
/// inform UX directly or trigger an MCP resource-update notification.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    ShapeAdded {
        room_id: Uuid,
        seq: u64,
        author: ParticipantId,
        shape: AnnotationDescriptor,
    },
    ShapeChanged {
        room_id: Uuid,
        seq: u64,
        author: ParticipantId,
        shape_id: Uuid,
        /// One of: "moved" | "resized" | "prop_changed" | "text_changed" | "pen_points_changed" | "reordered"
        change: String,
    },
    ShapeDeleted {
        room_id: Uuid,
        seq: u64,
        author: ParticipantId,
        shape_id: Uuid,
        deleted_author: ParticipantId,
    },
    ImageReplaced {
        room_id: Uuid,
        seq: u64,
        author: ParticipantId,
    },
    /// A structural (layer/group) change. Carries the affected node id when one
    /// is well-defined and a `change` discriminator. Phase 2 broadcasts these so
    /// subscribers can refresh the layer tree via `list_layers`.
    LayerChanged {
        room_id: Uuid,
        seq: u64,
        author: ParticipantId,
        node_id: Uuid,
        /// One of: "layer_created" | "layer_deleted" | "reparented" |
        /// "reordered" | "grouped" | "ungrouped" | "prop_changed"
        change: String,
    },
}

impl ServerEvent {
    /// Translate a `CanvasEvent` (on-canvas record) into a `ServerEvent`
    /// (over-wire form). The `find_shape` closure is used to look up the
    /// post-mutation annotation for ShapeAdded events.
    pub fn from_canvas_event(
        room_id: Uuid,
        evt: &CanvasEvent,
        find_shape: impl FnOnce(Uuid) -> Option<AnnotationDescriptor>,
    ) -> Self {
        match &evt.kind {
            EventKind::ShapeAdded(ann) => {
                let descriptor = find_shape(ann.id)
                    .unwrap_or_else(|| crate::agent::handlers::annotation_to_descriptor(ann));
                ServerEvent::ShapeAdded {
                    room_id,
                    seq: evt.seq,
                    author: evt.author,
                    shape: descriptor,
                }
            }
            EventKind::ShapeMoved { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "moved".into(),
            },
            EventKind::ShapeResized { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "resized".into(),
            },
            EventKind::ShapePropChanged { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "prop_changed".into(),
            },
            EventKind::ShapeTextChanged { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "text_changed".into(),
            },
            EventKind::ShapePenPointsChanged { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "pen_points_changed".into(),
            },
            EventKind::ShapeReordered { id, .. } => ServerEvent::ShapeChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                change: "reordered".into(),
            },
            EventKind::ShapeDeleted { id, deleted_author } => ServerEvent::ShapeDeleted {
                room_id,
                seq: evt.seq,
                author: evt.author,
                shape_id: *id,
                deleted_author: *deleted_author,
            },
            EventKind::ImageReplaced => ServerEvent::ImageReplaced {
                room_id,
                seq: evt.seq,
                author: evt.author,
            },
            EventKind::LayerCreated { id, .. } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *id,
                change: "layer_created".into(),
            },
            EventKind::LayerDeleted { id } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *id,
                change: "layer_deleted".into(),
            },
            EventKind::NodeReparented { id, .. } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *id,
                change: "reparented".into(),
            },
            EventKind::NodeReordered { id, .. } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *id,
                change: "reordered".into(),
            },
            EventKind::Grouped { group_id, .. } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *group_id,
                change: "grouped".into(),
            },
            EventKind::Ungrouped { group_id } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *group_id,
                change: "ungrouped".into(),
            },
            EventKind::NodePropChanged { id, .. } => ServerEvent::LayerChanged {
                room_id,
                seq: evt.seq,
                author: evt.author,
                node_id: *id,
                change: "prop_changed".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Annotation;

    #[test]
    fn from_canvas_event_shape_added() {
        let ann = Annotation::new(
            crate::types::ShapeData::plain_rect(egui::Rect::ZERO),
            egui::Color32::RED,
            1.0,
        );
        let id = ann.id;
        let evt = CanvasEvent {
            seq: 42,
            ts: std::time::SystemTime::now(),
            author: ParticipantId::HUMAN_LOCAL,
            kind: EventKind::ShapeAdded(ann),
        };
        let room = Uuid::nil();
        let server_evt = ServerEvent::from_canvas_event(room, &evt, |_| None);
        assert!(matches!(
            server_evt,
            ServerEvent::ShapeAdded { seq: 42, ref shape, .. } if shape.id == id
        ));
    }

    #[test]
    fn from_canvas_event_shape_moved() {
        let id = Uuid::new_v4();
        let evt = CanvasEvent {
            seq: 7,
            ts: std::time::SystemTime::now(),
            author: ParticipantId::HUMAN_LOCAL,
            kind: EventKind::ShapeMoved {
                id,
                delta: egui::vec2(1.0, 2.0),
            },
        };
        let server_evt = ServerEvent::from_canvas_event(Uuid::nil(), &evt, |_| None);
        assert!(matches!(
            server_evt,
            ServerEvent::ShapeChanged { shape_id, ref change, .. } if shape_id == id && change == "moved"
        ));
    }
}
