use egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// ── ParticipantId & palette ──────────────────────────────────────────────────

/// Identifies a participant (human or agent) in a room. Wraps a UUID for type safety.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ParticipantId(pub Uuid);

impl ParticipantId {
    /// The local UI user. Reserved nil UUID.
    pub const HUMAN_LOCAL: ParticipantId = ParticipantId(Uuid::nil());

    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn is_human_local(self) -> bool {
        self == Self::HUMAN_LOCAL
    }
}

impl Default for ParticipantId {
    fn default() -> Self {
        Self::HUMAN_LOCAL
    }
}

/// 16-color palette assigned to agents on join. Color values picked to read clearly
/// on both light and dark canvas backgrounds. The first slot is reserved for cursors
/// where no preference is given; agents may request any palette color on join.
pub const PARTICIPANT_PALETTE: [Color32; 16] = [
    Color32::from_rgb(0xE0, 0x4A, 0x4A), // red
    Color32::from_rgb(0xE8, 0x82, 0x1A), // orange
    Color32::from_rgb(0xD4, 0xB3, 0x06), // amber
    Color32::from_rgb(0x4A, 0xA8, 0x4A), // green
    Color32::from_rgb(0x2A, 0x9D, 0x8F), // teal
    Color32::from_rgb(0x2B, 0x6C, 0xB0), // blue
    Color32::from_rgb(0x6A, 0x4C, 0xC4), // indigo
    Color32::from_rgb(0xA0, 0x36, 0xB8), // purple
    Color32::from_rgb(0xCC, 0x40, 0x80), // magenta
    Color32::from_rgb(0xCB, 0x57, 0x57), // rose
    Color32::from_rgb(0x77, 0x88, 0x99), // slate
    Color32::from_rgb(0x33, 0x88, 0x88), // cyan
    Color32::from_rgb(0x9A, 0x7B, 0x4F), // tan
    Color32::from_rgb(0x55, 0x99, 0x55), // moss
    Color32::from_rgb(0x88, 0x55, 0xAA), // violet
    Color32::from_rgb(0x44, 0x55, 0x66), // graphite
];

/// Pick the first palette color not present in `used`. If all are taken, returns
/// palette[0] (caller may suffix the participant's name to disambiguate visually).
pub fn pick_palette_color(used: &[Color32]) -> Color32 {
    PARTICIPANT_PALETTE
        .iter()
        .copied()
        .find(|c| !used.contains(c))
        .unwrap_or(PARTICIPANT_PALETTE[0])
}

/// A participant in a room (human or agent). One per active connection.
#[derive(Clone, Debug)]
pub struct Participant {
    pub id: ParticipantId,
    pub name: String,
    pub color: Color32,
    /// Current cursor position in image-pixel space. `None` if not present
    /// (just joined, or hasn't moved their cursor onto the canvas).
    pub cursor: Option<Pos2>,
    /// Last time the participant did anything (cursor move, draw, etc.).
    pub last_active: std::time::Instant,
    /// Optional short status string ("thinking…", "drawing letter J").
    /// Cleared on disconnect.
    pub status: Option<String>,
}

impl Participant {
    pub fn new_human() -> Self {
        Self {
            id: ParticipantId::HUMAN_LOCAL,
            name: "you".to_string(),
            color: PARTICIPANT_PALETTE[5], // blue — distinct from default red stroke
            cursor: None,
            last_active: std::time::Instant::now(),
            status: None,
        }
    }
}

// ── Canvas event log ─────────────────────────────────────────────────────────

/// A single committed change to a canvas, recorded in the per-room event log
/// and broadcast to subscribed participants.
#[derive(Clone, Debug)]
pub struct CanvasEvent {
    pub seq: u64,
    pub ts: std::time::SystemTime,
    pub author: ParticipantId,
    pub kind: EventKind,
}

/// Side-effect description for a single committed action. Mirrors the shape of
/// `Command` but uses fields callers care about for replay/notification (the
/// added shape itself, not the variant that built it).
#[derive(Clone, Debug)]
pub enum EventKind {
    ShapeAdded(Annotation),
    ShapeMoved {
        id: Uuid,
        delta: Vec2,
    },
    ShapeResized {
        id: Uuid,
        new_rect: Rect,
    },
    ShapePropChanged {
        id: Uuid,
        prop: PropChange,
    },
    ShapeTextChanged {
        id: Uuid,
        content: String,
    },
    ShapePenPointsChanged {
        id: Uuid,
        raw_points: Vec<Pos2>,
        smoothed_points: Vec<Pos2>,
    },
    ShapeDeleted {
        id: Uuid,
        deleted_author: ParticipantId,
    },
    ShapeReordered {
        id: Uuid,
        new_index: usize,
    },
    ImageReplaced,
    LayerCreated {
        id: Uuid,
        name: String,
    },
    LayerDeleted {
        id: Uuid,
    },
    NodeReparented {
        id: Uuid,
        new_parent: Uuid,
        new_index: usize,
    },
    NodeReordered {
        id: Uuid,
        new_index: usize,
    },
    Grouped {
        group_id: Uuid,
        children: Vec<Uuid>,
    },
    Ungrouped {
        group_id: Uuid,
    },
    NodePropChanged {
        id: Uuid,
        prop: NodeProp,
    },
}

// ── Shape ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogicTagKind {
    MoveHere,
    DeleteThis,
    FixText,
    AddHere,
    Important,
    Question,
    Wrong,
    Custom(String),
}

impl LogicTagKind {
    pub fn label(&self) -> &str {
        match self {
            Self::MoveHere => "Move Here",
            Self::DeleteThis => "Delete This",
            Self::FixText => "Fix Text",
            Self::AddHere => "Add Here",
            Self::Important => "Important",
            Self::Question => "Question?",
            Self::Wrong => "Wrong",
            Self::Custom(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum NodeType {
    Corner,
    Smooth,
    Symmetric,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BezierNode {
    pub pos: Pos2,
    /// Offset from `pos` to the incoming control handle. `None` means no
    /// incoming curve (the first node of an open path).
    pub in_handle: Option<Vec2>,
    /// Offset from `pos` to the outgoing control handle.
    pub out_handle: Option<Vec2>,
    pub node_type: NodeType,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self::Solid
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArrowHeads {
    Single,
    Double,
}

impl Default for ArrowHeads {
    fn default() -> Self {
        Self::Single
    }
}

#[derive(Clone, Debug)]
pub enum ShapeData {
    Rect {
        rect: Rect,
        /// Corner radius in px; 0.0 = square corners.
        radius: f32,
        /// Fill color; `None` = no fill. The color's own alpha is the fill opacity.
        fill: Option<Color32>,
        /// Whether the outline is drawn at all.
        stroke: bool,
        /// Dash pattern for the outline (only meaningful when `stroke` is true).
        stroke_style: LineStyle,
    },
    Oval {
        rect: Rect,
        fill: Option<Color32>,
        stroke: bool,
        stroke_style: LineStyle,
    },
    Star {
        rect: Rect,
    },
    Arrow {
        start: Pos2,
        end: Pos2,
        style: LineStyle,
        heads: ArrowHeads,
    },
    Line {
        start: Pos2,
        end: Pos2,
        style: LineStyle,
    },
    Pen {
        raw_points: Vec<Pos2>,
        smoothed_points: Vec<Pos2>,
    },
    FocusRegion {
        rect: Rect,
    },
    LogicTag {
        pos: Pos2,
        tag: LogicTagKind,
    },
    Text {
        pos: Pos2,
        content: String,
        size: f32,
    },
    Blur {
        rect: Rect,
        block_size: f32,
    },
    StepMarker {
        pos: Pos2,
        number: u32,
        radius: f32,
    },
    /// Bitmap layer pasted on top of the canvas. Movable + resizable.
    /// `texture` is lazily created at draw time from `data`.
    ImageLayer {
        rect: Rect,
        data: Arc<image::RgbaImage>,
        texture: TextureCell,
    },
    /// Rounded rectangle with a triangular tail (speech bubble).
    Callout {
        rect: Rect,
        tail: Pos2,
    },
    /// Line annotation that displays the pixel distance between endpoints.
    Ruler {
        start: Pos2,
        end: Pos2,
    },
    /// Agent-emitted question bubble. Rendered as ghosted yellow text with
    /// a soft outline. Distinct from `Text` so humans and agents can tell
    /// "this is a question awaiting an answer" from "this is a label."
    Question {
        pos: Pos2,
        content: String,
        size: f32,
    },
    /// Multi-segment cubic Bezier path. Closed paths connect last node back
    /// to first via the last `out_handle` / first `in_handle`.
    BezierPath {
        nodes: Vec<BezierNode>,
        closed: bool,
        fill: Option<Color32>,
    },
}

/// Holds an optional lazy-loaded `TextureHandle` with a manual Debug impl so
/// that `ShapeData` can still derive Debug (TextureHandle itself doesn't).
#[derive(Clone, Default)]
pub struct TextureCell(pub Option<egui::TextureHandle>);

impl std::fmt::Debug for TextureCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(_) => f.write_str("TextureCell(loaded)"),
            None => f.write_str("TextureCell(none)"),
        }
    }
}

impl ShapeData {
    /// Plain rect: no fill, always stroked, square corners, solid line. Convenience
    /// constructor for call sites (tests, agent ops, migration) that just need "a rect".
    pub fn plain_rect(rect: Rect) -> Self {
        Self::Rect {
            rect,
            radius: 0.0,
            fill: None,
            stroke: true,
            stroke_style: LineStyle::Solid,
        }
    }

    /// Plain oval: no fill, always stroked, solid line.
    pub fn plain_oval(rect: Rect) -> Self {
        Self::Oval {
            rect,
            fill: None,
            stroke: true,
            stroke_style: LineStyle::Solid,
        }
    }

    /// Bounding rect (used by SelectTool and Resize command).
    pub fn bounding_rect(&self) -> Rect {
        match self {
            Self::Rect { rect, .. }
            | Self::Oval { rect, .. }
            | Self::Star { rect }
            | Self::FocusRegion { rect }
            | Self::ImageLayer { rect, .. }
            | Self::Blur { rect, .. } => *rect,
            Self::Arrow { start, end, .. }
            | Self::Line { start, end, .. }
            | Self::Ruler { start, end } => Rect::from_two_pos(*start, *end),
            Self::Pen { raw_points, .. } => raw_points.iter().copied().fold(
                Rect::from_pos(raw_points.first().copied().unwrap_or(Pos2::ZERO)),
                |acc, p| acc.union(Rect::from_pos(p)),
            ),
            Self::LogicTag { pos, .. } => Rect::from_center_size(*pos, egui::vec2(80.0, 24.0)),
            Self::Text { pos, content, size } => {
                let w = (content.chars().count() as f32 * *size * 0.6).max(*size);
                Rect::from_min_size(*pos, egui::vec2(w, *size * 1.3))
            }
            Self::Question { pos, content, size } => {
                let est_w = (content.chars().count() as f32) * (*size) * 0.55;
                let est_h = *size * 1.2;
                Rect::from_min_size(*pos, egui::vec2(est_w, est_h))
            }
            Self::StepMarker { pos, radius, .. } => {
                Rect::from_center_size(*pos, egui::vec2(*radius * 2.0, *radius * 2.0))
            }
            Self::Callout { rect, tail } => rect.union(Rect::from_pos(*tail)),
            Self::BezierPath { nodes, .. } => {
                if nodes.is_empty() {
                    Rect::ZERO
                } else {
                    let mut min = nodes[0].pos;
                    let mut max = nodes[0].pos;
                    for n in nodes {
                        min.x = min.x.min(n.pos.x);
                        min.y = min.y.min(n.pos.y);
                        max.x = max.x.max(n.pos.x);
                        max.y = max.y.max(n.pos.y);
                    }
                    Rect::from_min_max(min, max)
                }
            }
        }
    }

    /// Translate all geometry by `delta`.
    pub fn translate(&mut self, delta: Vec2) {
        match self {
            Self::Rect { rect, .. }
            | Self::Oval { rect, .. }
            | Self::Star { rect }
            | Self::FocusRegion { rect }
            | Self::ImageLayer { rect, .. }
            | Self::Blur { rect, .. } => *rect = rect.translate(delta),
            Self::Arrow { start, end, .. }
            | Self::Line { start, end, .. }
            | Self::Ruler { start, end } => {
                *start += delta;
                *end += delta;
            }
            Self::Pen {
                raw_points,
                smoothed_points,
            } => {
                for p in raw_points.iter_mut().chain(smoothed_points.iter_mut()) {
                    *p += delta;
                }
            }
            Self::LogicTag { pos, .. } => *pos += delta,
            Self::Text { pos, .. } => *pos += delta,
            Self::Question { pos, .. } => *pos += delta,
            Self::StepMarker { pos, .. } => *pos += delta,
            Self::Callout { rect, tail } => {
                *rect = rect.translate(delta);
                *tail += delta;
            }
            Self::BezierPath { nodes, .. } => {
                for n in nodes.iter_mut() {
                    n.pos += delta;
                }
            }
        }
    }

    /// The rect that `set_rect` consumes, i.e. the value `r` for which
    /// `set_rect(r)` is a no-op. This is the exact pre-resize geometry the
    /// Resize command must snapshot so that apply -> undo restores the shape
    /// identically (unlike `bounding_rect`, which can return a synthetic rect
    /// larger than the stored geometry — e.g. a Callout's union with its tail —
    /// and would corrupt the shape on undo).
    pub fn resize_rect(&self) -> Rect {
        match self {
            Self::Rect { rect, .. }
            | Self::Oval { rect, .. }
            | Self::Star { rect }
            | Self::FocusRegion { rect }
            | Self::ImageLayer { rect, .. }
            | Self::Blur { rect, .. }
            | Self::Callout { rect, .. } => *rect,
            Self::Arrow { start, end, .. }
            | Self::Line { start, end, .. }
            | Self::Ruler { start, end } => Rect::from_min_max(*start, *end),
            // Shapes whose `set_rect` is a no-op: any rect round-trips, so fall
            // back to the bounding rect (event-recording / display only).
            _ => self.bounding_rect(),
        }
    }

    /// Replace rect-based geometry with a new rect (used by Resize command).
    pub fn set_rect(&mut self, new_rect: Rect) {
        match self {
            Self::Rect { rect, .. }
            | Self::Oval { rect, .. }
            | Self::Star { rect }
            | Self::FocusRegion { rect }
            | Self::ImageLayer { rect, .. }
            | Self::Blur { rect, .. } => *rect = new_rect,
            Self::Arrow { start, end, .. }
            | Self::Line { start, end, .. }
            | Self::Ruler { start, end } => {
                *start = new_rect.min;
                *end = new_rect.max;
            }
            Self::Callout { rect, tail } => {
                let delta = new_rect.min - rect.min;
                *tail += delta;
                *rect = new_rect;
            }
            _ => {}
        }
    }
}

pub fn star_points(rect: Rect) -> Vec<Pos2> {
    const INNER_RATIO: f32 = 0.381_966_02;
    let center = rect.center();
    let outer_rx = rect.width().abs() * 0.5;
    let outer_ry = rect.height().abs() * 0.5;
    let mut points = Vec::with_capacity(10);
    for i in 0..10 {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let scale = if i % 2 == 0 { 1.0 } else { INNER_RATIO };
        points.push(Pos2::new(
            center.x + outer_rx * scale * angle.cos(),
            center.y + outer_ry * scale * angle.sin(),
        ));
    }
    points
}

// ── Annotation ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Annotation {
    pub id: Uuid,
    /// Who created this annotation. Defaults to `ParticipantId::HUMAN_LOCAL`
    /// for the UI user; agents set this to their participant id.
    pub author: ParticipantId,
    pub shape: ShapeData,
    pub color: Color32,
    pub thickness: f32,
    pub label: Option<String>,
    pub locked: bool,
    pub visible: bool,
    /// If `Some`, this annotation is provisional (rendered ghosted). Phase 6's
    /// auto-expiry task removes the annotation when the time passes without
    /// being either confirmed (set to `None`) or rejected (removed). `None`
    /// means the annotation is permanent.
    pub provisional_until: Option<std::time::SystemTime>,
}

impl Annotation {
    /// Create an annotation attributed to the local human user. Existing call
    /// sites continue to work unchanged.
    pub fn new(shape: ShapeData, color: Color32, thickness: f32) -> Self {
        Self::new_authored(shape, color, thickness, ParticipantId::HUMAN_LOCAL)
    }

    /// Create an annotation with an explicit author. New code paths (agent
    /// drawing tools, replay) use this.
    pub fn new_authored(
        shape: ShapeData,
        color: Color32,
        thickness: f32,
        author: ParticipantId,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            author,
            shape,
            color,
            thickness,
            label: None,
            locked: false,
            visible: true,
            provisional_until: None,
        }
    }
}

// ── Command ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum PropChange {
    Color(Color32),
    Thickness(f32),
    Label(Option<String>),
    Locked(bool),
    Visible(bool),
    StepNumber(u32),
    /// Change the font size of a Text or Question annotation.
    TextSize(f32),
}

/// A property change on a *container or leaf* node (Layer / Group / Shape),
/// addressed by node id. Distinct from [`PropChange`], which targets leaf-only
/// drawing attributes. Used by `Command::SetNodeProp` and the Layers panel.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeProp {
    Name(String),
    Visible(bool),
    Locked(bool),
    /// Local opacity, `0.0..=1.0`.
    Opacity(f32),
    /// Local affine transform, relative to the parent.
    Transform(kurbo::Affine),
}

/// Wraps a ColorImage so it can live in Command (ColorImage lacks Debug).
#[derive(Clone)]
pub struct ImageSnapshot(pub egui::ColorImage);

impl std::fmt::Debug for ImageSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageSnapshot({}×{}px)", self.0.size[0], self.0.size[1])
    }
}

/// Boolean pathfinder operation kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Unite,
    Subtract,
    Intersect,
    Exclude,
}

#[derive(Clone, Debug)]
pub enum Command {
    Add(Annotation),
    Delete(Uuid),
    Move {
        id: Uuid,
        delta: Vec2,
    },
    Resize {
        id: Uuid,
        new_rect: Rect,
    },
    SetProp {
        id: Uuid,
        prop: PropChange,
    },
    Batch(Vec<Command>),
    /// Replace the canvas pixel buffer (used by fill tool). The payload is the
    /// image to restore on undo; apply() swaps it with the current image.
    ReplaceImage(ImageSnapshot),
    /// Move annotation to front (end of vec = renders on top). Undo records original index.
    BringToFront(Uuid),
    /// Move annotation to back (start of vec = renders behind all). Undo records original index.
    SendToBack(Uuid),
    /// Reorder an annotation to a specific index. Used as the undo primitive for
    /// BringToFront and SendToBack.
    ReorderTo {
        id: Uuid,
        index: usize,
    },
    /// Renumber all StepMarker annotations sequentially (1, 2, 3, …) in draw order.
    RenumberSteps,
    /// Replace the text content of a `Text` annotation. No-op on other shape kinds.
    /// Inverse: `SetTextContent` with the prior content.
    SetTextContent {
        id: Uuid,
        content: String,
    },
    /// Replace the points of a `Pen` annotation. No-op on other shape kinds.
    /// Inverse: `ReplacePenPoints` with the prior raw and smoothed points.
    ReplacePenPoints {
        id: Uuid,
        raw_points: Vec<Pos2>,
        smoothed_points: Vec<Pos2>,
    },
    /// Create a new empty layer (last child of root) with id `id` and `name`.
    /// Inverse: `DeleteLayer { id }`.
    CreateLayer {
        id: Uuid,
        name: String,
    },
    /// Delete the layer subtree rooted at `id`. The inverse re-inserts the
    /// captured subtree at its original index under root.
    DeleteLayer {
        id: Uuid,
    },
    /// Reparent leaf/group `id` into `layer_id` at `index`. Inverse reparents back
    /// to the prior parent/index.
    MoveNodeToLayer {
        id: Uuid,
        layer_id: Uuid,
        index: usize,
    },
    /// Reorder `id` within its current parent to `index`. Inverse: prior index.
    ReorderNode {
        id: Uuid,
        index: usize,
    },
    /// Group the given leaf/group ids (shared parent) into a new Group. The new
    /// group's id is assigned by `apply` and reported via the event. Inverse: Ungroup.
    Group {
        ids: Vec<Uuid>,
        name: String,
    },
    /// Ungroup: reparent the group's children back to the group's parent at the
    /// group's index, then delete the now-empty group. Inverse: Group.
    Ungroup {
        group_id: Uuid,
    },
    /// Set a Name/Visible/Locked/Opacity property on a Layer/Group/Shape node.
    /// Inverse: `SetNodeProp` with the prior value.
    SetNodeProp {
        id: Uuid,
        prop: NodeProp,
    },
    /// Internal inverse of `DeleteLayer`: re-insert a captured subtree under
    /// `parent` at `index`. Not emitted by the UI or MCP directly.
    RestoreSubtree {
        parent: Uuid,
        index: usize,
        nodes: Vec<crate::scene::Node>,
    },

    /// Weld two OPEN BezierPath leaves `a`,`b` into one. Inverse: `ReplaceLeaves`.
    WeldPaths {
        a: Uuid,
        b: Uuid,
    },
    /// Boolean pathfinder op on the closed BezierPath leaves among `ids`.
    /// Inverse: `ReplaceLeaves`.
    BooleanOp {
        op: BoolOp,
        ids: Vec<Uuid>,
    },
    /// Flatten (union) all closed vector leaves under `layer_id`. Inverse: `ReplaceLeaves`.
    FlattenLayer {
        layer_id: Uuid,
    },
    /// Composite all leaf-visible annotations onto the base image to produce one
    /// flat raster image, removing the now-merged leaves. Undoable: the inverse
    /// is a Batch of `ReplaceImage(old)` + `RestoreSubtree(...)`. Emitted by the
    /// layers panel's Flatten button; never produced as an inverse.
    FlattenToImage,
    /// Internal swap primitive backing the path-op inverses: remove the leaves in
    /// `remove`, then re-add each `(Annotation, parent, index)` in `add` at its
    /// captured slot. Its own inverse is the symmetric `ReplaceLeaves`. Not emitted
    /// by the UI or MCP directly.
    ReplaceLeaves {
        remove: Vec<Uuid>,
        add: Vec<(Annotation, Uuid, usize)>,
    },
}

// ── Export ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormalizedRect {
    pub fn from_rect(rect: Rect, canvas_size: Vec2) -> Self {
        Self {
            x: (rect.min.x / canvas_size.x * 1000.0).round(),
            y: (rect.min.y / canvas_size.y * 1000.0).round(),
            w: (rect.width() / canvas_size.x * 1000.0).round(),
            h: (rect.height() / canvas_size.y * 1000.0).round(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FocusRegionExport {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub normalized: NormalizedRect,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct AnnotationExport {
    pub id: String,
    pub kind: String,
    pub coords: Option<NormalizedRect>,
    pub color: String,
    pub label: Option<String>,
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    // Shape-specific raw fields (image-pixel coordinates) — enable sidecar roundtrip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f32; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<LineStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heads: Option<ArrowHeads>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_opacity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_png: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<[f64; 6]>,
}

/// Sidecar v2: a flattened description of a group within a layer. Each group
/// lists ordered member shape ids that the importer reparents into the group.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GroupExport {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    /// Ordered shape ids that belong to this group (must also appear among the
    /// owning layer's shapes — the importer reparents them into the group).
    pub shape_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<[f64; 6]>,
}

/// Sidecar v2: a flattened description of one layer. Absent in v1 sidecars;
/// import falls back to flat→single-layer migration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LayerExport {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    /// Ordered shape ids directly in this layer (paint order). Includes ids that
    /// are later reparented into a group via `groups`.
    pub shape_ids: Vec<String>,
    #[serde(default)]
    pub groups: Vec<GroupExport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<[f64; 6]>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SidecarExport {
    pub canvas_size: [f32; 2],
    pub captured_at: String,
    pub focus_region: Option<FocusRegionExport>,
    pub annotations: Vec<AnnotationExport>,
    pub clipboard_text: String,
    /// Sidecar v2 layer tree. Absent (`None`) in v1 sidecars, which import via
    /// the flat→single-layer path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<LayerExport>>,
    #[cfg(feature = "local-ai")]
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub agent_memory: std::collections::HashMap<String, Vec<crate::local_ai::memory::MemoryEntry>>,
    #[cfg(feature = "local-ai")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_notes: Vec<crate::local_ai::memory::SharedNote>,
    #[cfg(feature = "local-ai")]
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub agent_bookmarks:
        std::collections::HashMap<String, Vec<crate::local_ai::memory::AgentBookmark>>,
}

#[cfg(test)]
mod participant_tests {
    use super::*;

    #[test]
    fn human_local_is_nil_uuid() {
        assert_eq!(ParticipantId::HUMAN_LOCAL.0, uuid::Uuid::nil());
    }

    #[test]
    fn new_participant_id_is_unique() {
        let a = ParticipantId::new();
        let b = ParticipantId::new();
        assert_ne!(a, b);
        assert_ne!(a, ParticipantId::HUMAN_LOCAL);
    }

    #[test]
    fn participant_palette_has_sixteen_colors() {
        assert_eq!(PARTICIPANT_PALETTE.len(), 16);
    }

    #[test]
    fn pick_palette_color_assigns_first_unused() {
        let used = vec![PARTICIPANT_PALETTE[0], PARTICIPANT_PALETTE[1]];
        let picked = pick_palette_color(&used);
        assert_eq!(picked, PARTICIPANT_PALETTE[2]);
    }

    #[test]
    fn pick_palette_color_wraps_when_exhausted() {
        let used: Vec<_> = PARTICIPANT_PALETTE.iter().copied().collect();
        let picked = pick_palette_color(&used);
        // All taken — function returns palette[0] (caller can dedupe by name suffix later)
        assert_eq!(picked, PARTICIPANT_PALETTE[0]);
    }

    #[test]
    fn canvas_event_struct_fields() {
        let evt = CanvasEvent {
            seq: 42,
            ts: std::time::SystemTime::now(),
            author: ParticipantId::HUMAN_LOCAL,
            kind: EventKind::ImageReplaced,
        };
        assert_eq!(evt.seq, 42);
        assert!(matches!(evt.kind, EventKind::ImageReplaced));
    }

    #[test]
    fn annotation_new_defaults_to_human_local_author() {
        let ann = Annotation::new(ShapeData::plain_rect(egui::Rect::ZERO), Color32::RED, 1.0);
        assert_eq!(ann.author, ParticipantId::HUMAN_LOCAL);
    }

    #[test]
    fn annotation_new_authored_carries_specified_author() {
        let agent = ParticipantId::new();
        let ann = Annotation::new_authored(
            ShapeData::plain_rect(egui::Rect::ZERO),
            Color32::RED,
            1.0,
            agent,
        );
        assert_eq!(ann.author, agent);
    }

    #[test]
    fn event_kind_variants_match_command_taxonomy() {
        let id = uuid::Uuid::new_v4();
        let actor = ParticipantId::HUMAN_LOCAL;
        let _ = EventKind::ShapeAdded(make_test_annotation(id, actor));
        let _ = EventKind::ShapeMoved {
            id,
            delta: egui::vec2(1.0, 0.0),
        };
        let _ = EventKind::ShapeResized {
            id,
            new_rect: egui::Rect::ZERO,
        };
        let _ = EventKind::ShapePropChanged {
            id,
            prop: PropChange::Color(Color32::RED),
        };
        let _ = EventKind::ShapeTextChanged {
            id,
            content: "x".to_string(),
        };
        let _ = EventKind::ShapePenPointsChanged {
            id,
            raw_points: vec![],
            smoothed_points: vec![],
        };
        let _ = EventKind::ShapeDeleted {
            id,
            deleted_author: actor,
        };
        let _ = EventKind::ShapeReordered { id, new_index: 0 };
        let _ = EventKind::ImageReplaced;
    }

    fn make_test_annotation(id: uuid::Uuid, author: ParticipantId) -> Annotation {
        Annotation {
            id,
            author,
            shape: ShapeData::plain_rect(egui::Rect::ZERO),
            color: Color32::RED,
            thickness: 1.0,
            label: None,
            locked: false,
            visible: true,
            provisional_until: None,
        }
    }
}
