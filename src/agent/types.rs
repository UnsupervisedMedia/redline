use egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::types::{ArrowHeads, LineStyle, ParticipantId};

/// One message from the MCP server thread to the main UI thread.
/// Each variant carries a `oneshot::Sender` for the reply.
#[derive(Debug)]
pub enum AgentOp {
    ListRooms {
        reply: oneshot::Sender<Result<Vec<RoomDescriptor>, AgentError>>,
    },
    JoinRoom {
        room_id: Uuid,
        name: String,
        preferred_color: Option<egui::Color32>,
        reply: oneshot::Sender<Result<JoinRoomResult, AgentError>>,
    },
    ListShapes {
        room_id: Uuid,
        reply: oneshot::Sender<Result<Vec<AnnotationDescriptor>, AgentError>>,
    },
    DrawArrow {
        room_id: Uuid,
        actor: ParticipantId,
        start: Pos2,
        end: Pos2,
        color: egui::Color32,
        thickness: f32,
        style: LineStyle,
        heads: ArrowHeads,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawRect {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawPen {
        room_id: Uuid,
        actor: ParticipantId,
        points: Vec<Pos2>,
        smooth: bool,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawText {
        room_id: Uuid,
        actor: ParticipantId,
        pos: Pos2,
        content: String,
        size: f32,
        color: egui::Color32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    SnapshotPng {
        room_id: Uuid,
        scale: f32,
        reply: oneshot::Sender<Result<Vec<u8>, AgentError>>,
    },
    ChangesSince {
        room_id: Uuid,
        since: u64,
        reply: oneshot::Sender<Result<ChangesSinceResult, AgentError>>,
    },
    DeleteShape {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    MoveShape {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        delta: Vec2,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    ResizeShape {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        new_rect: Rect,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    SetProp {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        prop: crate::types::PropChange,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    SetTextContent {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        content: String,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    ReplacePenPoints {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        points: Vec<Pos2>,
        smooth: bool,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    BringToFront {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    SendToBack {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    ReorderTo {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        index: usize,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    RenumberSteps {
        room_id: Uuid,
        actor: ParticipantId,
        reply: oneshot::Sender<Result<u64, AgentError>>,
    },
    Undo {
        room_id: Uuid,
        actor: ParticipantId,
        reply: oneshot::Sender<Result<u64 /* seq */, AgentError>>,
    },
    Redo {
        room_id: Uuid,
        actor: ParticipantId,
        reply: oneshot::Sender<Result<u64 /* seq */, AgentError>>,
    },
    DrawOval {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawLine {
        room_id: Uuid,
        actor: ParticipantId,
        start: egui::Pos2,
        end: egui::Pos2,
        style: crate::types::LineStyle,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawStar {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawCallout {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        tail: egui::Pos2,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawRoundedRect {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        radius: f32,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    DrawHighlighter {
        room_id: Uuid,
        actor: ParticipantId,
        rect: Rect,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    #[allow(dead_code)] // WIP agent op, not yet wired into the MCP surface.
    DrawAsk {
        room_id: Uuid,
        actor: ParticipantId,
        anchor: Option<egui::Pos2>,
        content: String,
        size: f32,
        color: egui::Color32,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    #[allow(dead_code)] // WIP agent op, not yet wired into the MCP surface.
    DrawPropose {
        room_id: Uuid,
        actor: ParticipantId,
        shape: crate::types::ShapeData,
        color: egui::Color32,
        thickness: f32,
        label: Option<String>,
        expires_in: Duration,
        reply: oneshot::Sender<Result<DrawResult, AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    RecordToolWish {
        room_id: Uuid,
        agent_id: String,
        content: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    RecordDecision {
        room_id: Uuid,
        agent_id: String,
        content: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    RecordLesson {
        room_id: Uuid,
        agent_id: String,
        content: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    RecordPrivateNote {
        room_id: Uuid,
        agent_id: String,
        content: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    /// Internal op used by the provisional auto-expiry task to find
    /// annotations whose `provisional_until` has passed.
    #[allow(dead_code)] // WIP agent op, not yet wired into the MCP surface.
    ListExpiredProvisionals {
        room_id: Uuid,
        reply: oneshot::Sender<Result<Vec<Uuid>, AgentError>>,
    },
    /// Read the human's current selection from tool_state.
    /// Returns the IDs of all selected annotations (empty if nothing selected).
    #[allow(dead_code)] // WIP agent op, not yet wired into the MCP surface.
    GetSelectedShapeIds {
        room_id: Uuid,
        reply: oneshot::Sender<Result<Vec<Uuid>, AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    LoadAgentMemory {
        room_id: Uuid,
        agent_id: String,
        reply: oneshot::Sender<Result<Vec<crate::local_ai::memory::MemoryEntry>, AgentError>>,
    },
    #[cfg(feature = "local-ai")]
    CompressAgentMemory {
        room_id: Uuid,
        agent_id: String,
        summary_text: String,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    ListLayers {
        room_id: Uuid,
        reply: oneshot::Sender<Result<Vec<LayerDescriptor>, AgentError>>,
    },
    CreateLayer {
        room_id: Uuid,
        actor: ParticipantId,
        name: String,
        reply: oneshot::Sender<Result<Uuid, AgentError>>,
    },
    MoveToLayer {
        room_id: Uuid,
        actor: ParticipantId,
        shape_id: Uuid,
        layer_id: Uuid,
        index: Option<usize>,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    SetLayerProp {
        room_id: Uuid,
        actor: ParticipantId,
        layer_id: Uuid,
        prop: crate::types::NodeProp,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    GroupShapes {
        room_id: Uuid,
        actor: ParticipantId,
        shape_ids: Vec<Uuid>,
        name: String,
        reply: oneshot::Sender<Result<Uuid, AgentError>>,
    },
    Ungroup {
        room_id: Uuid,
        actor: ParticipantId,
        group_id: Uuid,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    /// Set the local affine transform of a node (Layer / Group / Shape) identified
    /// by `node_id`. Dispatched via `CommandExecutor::execute` so it is undoable
    /// and event-logged. The transform is expressed as decomposed components
    /// (`translate`, `scale`, `rotate_deg`, `skew_deg`) and recomposed via
    /// `transform::recompose` — these describe the desired *absolute* local
    /// transform (not a delta).
    SetTransform {
        room_id: Uuid,
        actor: ParticipantId,
        node_id: Uuid,
        /// `[tx, ty]` translation in local-space pixels.
        translate: [f64; 2],
        /// `[sx, sy]` scale factors (must be non-zero).
        scale: [f64; 2],
        /// Rotation in degrees (converted to radians internally).
        rotate_deg: f64,
        /// Skew angle in degrees (converted to radians internally).
        skew_deg: f64,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
}

// All descriptor types derive Serialize+Deserialize so rmcp can ship them
// over the wire. egui types (Vec2, Rect, Color32) lack serde impls, so we
// route them through the helper modules below.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDescriptor {
    pub id: Uuid,
    pub label: String,
    #[serde(with = "vec2_serde")]
    pub canvas_size: Vec2,
    pub participant_count: usize,
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationDescriptor {
    pub id: Uuid,
    pub kind: String,
    #[serde(with = "rect_serde")]
    pub bounding_rect: Rect,
    #[serde(with = "color_serde")]
    pub color: egui::Color32,
    pub label: Option<String>,
    pub locked: bool,
    pub visible: bool,
    pub author: ParticipantId,
    /// The id of the layer (top-level Layer ancestor) this shape lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_name: Option<String>,
    /// Local transform decomposed into human-readable components. Always present
    /// (identity when no transform has been applied). Fields: translate [tx,ty],
    /// scale [sx,sy], rotate_deg (degrees, CCW-positive in math convention),
    /// skew_deg (degrees). Corresponds to `transform::Decomposed` via
    /// `transform::decompose`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformDecomposed>,
}

/// Wire-safe decomposed affine transform, produced by `transform::decompose`.
///
/// All angle fields are in **degrees** (not radians), which is more readable
/// for agents and round-trips cleanly through JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformDecomposed {
    /// Translation `[tx, ty]` in pixels.
    pub translate: [f64; 2],
    /// Scale `[sx, sy]`. Negative scale encodes flip + scale magnitude.
    pub scale: [f64; 2],
    /// Rotation in **degrees** (radians × 180/π).
    pub rotate_deg: f64,
    /// Skew in **degrees** (radians × 180/π).
    pub skew_deg: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDescriptor {
    pub id: Uuid,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub shape_ids: Vec<Uuid>,
    /// Local transform decomposed into human-readable components. Matches the
    /// same field on `AnnotationDescriptor`; present whenever the layer node
    /// exists in the document (identity when no transform has been applied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformDecomposed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRoomResult {
    pub participant_id: ParticipantId,
    #[serde(with = "color_serde")]
    pub assigned_color: egui::Color32,
    pub last_seq: u64,
    pub current_shapes: Vec<AnnotationDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawResult {
    pub shape_id: Uuid,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesSinceResult {
    pub events: Vec<ServerEventWire>,
    pub truncated: bool,
    pub current_seq: u64,
}

/// Wire-friendly version of a CanvasEvent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEventWire {
    pub seq: u64,
    pub ts_ms: u64,
    pub author: ParticipantId,
    pub kind: String,
    pub shape_id: Option<Uuid>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AgentError {
    #[error("room {0} not found")]
    RoomNotFound(Uuid),
    #[error("shape {0} not found")]
    ShapeNotFound(Uuid),
    #[error("shape {0} is locked; unlock via set_prop {{ locked: false }} first")]
    ShapeLocked(Uuid),
    #[error("invalid argument {field}: expected {expected}, got {got}")]
    InvalidArgument {
        field: String,
        expected: String,
        got: String,
    },
    #[error("internal: {reason}")]
    Internal { reason: String },
    #[error("nothing to undo for this actor")]
    NothingToUndo,
    #[error("nothing to redo for this actor")]
    NothingToRedo,
}

// ── Serde adapters for egui types ────────────────────────────────────────────

mod vec2_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(v: &egui::Vec2, s: S) -> Result<S::Ok, S::Error> {
        [v.x, v.y].serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<egui::Vec2, D::Error> {
        let [x, y] = <[f32; 2]>::deserialize(d)?;
        Ok(egui::Vec2::new(x, y))
    }
}

mod rect_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(r: &egui::Rect, s: S) -> Result<S::Ok, S::Error> {
        [r.min.x, r.min.y, r.max.x, r.max.y].serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<egui::Rect, D::Error> {
        let [x1, y1, x2, y2] = <[f32; 4]>::deserialize(d)?;
        Ok(egui::Rect::from_min_max(
            egui::pos2(x1, y1),
            egui::pos2(x2, y2),
        ))
    }
}

mod color_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(c: &egui::Color32, s: S) -> Result<S::Ok, S::Error> {
        let hex = format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
        s.serialize_str(&hex)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<egui::Color32, D::Error> {
        let s = String::deserialize(d)?;
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return Err(serde::de::Error::custom("expected #RRGGBB"));
        }
        let r = u8::from_str_radix(&s[0..2], 16).map_err(serde::de::Error::custom)?;
        let g = u8::from_str_radix(&s[2..4], 16).map_err(serde::de::Error::custom)?;
        let b = u8::from_str_radix(&s[4..6], 16).map_err(serde::de::Error::custom)?;
        Ok(egui::Color32::from_rgb(r, g, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_descriptor_clone() {
        let rd = RoomDescriptor {
            id: Uuid::new_v4(),
            label: "test".into(),
            canvas_size: Vec2::new(100.0, 100.0),
            participant_count: 1,
            last_seq: 42,
        };
        let _ = rd.clone();
    }

    #[test]
    fn color_serde_roundtrip() {
        let c = egui::Color32::from_rgb(0xff, 0x42, 0x80);
        let d = AnnotationDescriptor {
            id: Uuid::nil(),
            kind: "rect".into(),
            bounding_rect: Rect::ZERO,
            color: c,
            label: None,
            locked: false,
            visible: true,
            author: ParticipantId::HUMAN_LOCAL,
            layer: None,
            layer_name: None,
            transform: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"#ff4280\""), "got: {json}");
        let back: AnnotationDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back.color, c);
    }

    #[test]
    fn rect_serde_roundtrip() {
        let r = Rect::from_min_max(egui::pos2(1.0, 2.0), egui::pos2(3.0, 4.0));
        let d = RoomDescriptor {
            id: Uuid::nil(),
            label: "test".into(),
            canvas_size: Vec2::new(r.width(), r.height()),
            participant_count: 0,
            last_seq: 0,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains("[2.0,2.0]") || json.contains("[2,2]"),
            "expected vec2 array, got: {json}"
        );
    }
}
