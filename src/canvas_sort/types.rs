use egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ── Session ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasSortSession {
    pub id: Uuid,
    pub name: String,
    pub folder_paths: Vec<PathBuf>,
    /// Logical canvas dimensions in canvas-space units.
    pub canvas_size: [f32; 2],
    pub images: Vec<SortImage>,
    pub stacks: Vec<ImageStack>,
    pub zones: Vec<ActionZone>,
    pub settings: SessionSettings,
}

impl CanvasSortSession {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            folder_paths: Vec::new(),
            canvas_size: [20_000.0, 20_000.0],
            images: Vec::new(),
            stacks: Vec::new(),
            zones: Vec::new(),
            settings: SessionSettings::default(),
        }
    }

    pub fn canvas_size_vec(&self) -> Vec2 {
        Vec2::new(self.canvas_size[0], self.canvas_size[1])
    }

    pub fn image_by_id(&self, id: Uuid) -> Option<&SortImage> {
        self.images.iter().find(|i| i.id == id)
    }

    pub fn image_by_id_mut(&mut self, id: Uuid) -> Option<&mut SortImage> {
        self.images.iter_mut().find(|i| i.id == id)
    }

    pub fn stack_by_id(&self, id: Uuid) -> Option<&ImageStack> {
        self.stacks.iter().find(|s| s.id == id)
    }

    pub fn stack_by_id_mut(&mut self, id: Uuid) -> Option<&mut ImageStack> {
        self.stacks.iter_mut().find(|s| s.id == id)
    }

    pub fn zone_by_id(&self, id: Uuid) -> Option<&ActionZone> {
        self.zones.iter().find(|z| z.id == id)
    }

    pub fn zone_by_id_mut(&mut self, id: Uuid) -> Option<&mut ActionZone> {
        self.zones.iter_mut().find(|z| z.id == id)
    }

    /// Bounding box enclosing all images (canvas space).
    pub fn images_bounds(&self) -> Option<Rect> {
        if self.images.is_empty() {
            return None;
        }
        let mut min = Pos2::new(f32::MAX, f32::MAX);
        let mut max = Pos2::new(f32::MIN, f32::MIN);
        for img in &self.images {
            min.x = min.x.min(img.pos[0]);
            min.y = min.y.min(img.pos[1]);
            max.x = max.x.max(img.pos[0] + img.size[0]);
            max.y = max.y.max(img.pos[1] + img.size[1]);
        }
        Some(Rect::from_min_max(min, max))
    }

    /// Images not inside any stack.
    pub fn loose_image_ids(&self) -> Vec<Uuid> {
        self.images
            .iter()
            .filter(|i| i.stack_id.is_none())
            .map(|i| i.id)
            .collect()
    }

    /// Next z-index (one above the current max).
    pub fn next_z_index(&self) -> i32 {
        self.images.iter().map(|i| i.z_index).max().unwrap_or(0) + 1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionSettings {
    pub allowed_dirs: Vec<PathBuf>,
    pub thumb_size: u32,
}

impl SessionSettings {
    pub fn effective_thumb_size(&self) -> u32 {
        if self.thumb_size == 0 {
            200
        } else {
            self.thumb_size
        }
    }
}

// ── Images ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortImage {
    pub id: Uuid,
    pub path: PathBuf,
    /// SHA-256 of the file at load time.
    pub file_hash: [u8; 32],
    /// Top-left in canvas space (stored as [x, y]).
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub rotation: f32,
    pub z_index: i32,
    pub stack_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub is_locked: bool,
}

impl SortImage {
    pub fn new(path: PathBuf, file_hash: [u8; 32]) -> Self {
        Self {
            id: Uuid::new_v4(),
            path,
            file_hash,
            pos: [0.0, 0.0],
            size: [200.0, 200.0],
            rotation: 0.0,
            z_index: 0,
            stack_id: None,
            tags: Vec::new(),
            is_locked: false,
        }
    }

    pub fn canvas_pos(&self) -> Pos2 {
        Pos2::new(self.pos[0], self.pos[1])
    }

    pub fn canvas_size(&self) -> Vec2 {
        Vec2::new(self.size[0], self.size[1])
    }

    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.canvas_pos(), self.canvas_size())
    }

    pub fn center(&self) -> Pos2 {
        self.rect().center()
    }

    pub fn set_pos(&mut self, p: Pos2) {
        self.pos = [p.x, p.y];
    }
}

// ── Stacks ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageStack {
    pub id: Uuid,
    pub label: String,
    pub color: SerColor,
    pub pos: [f32; 2],
    pub spread: f32,
    pub rotation_variance: f32,
    pub is_expanded: bool,
    pub image_ids: Vec<Uuid>,
}

impl ImageStack {
    pub fn new(pos: Pos2) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: String::new(),
            color: SerColor([0x44, 0x88, 0xFF, 0xFF]),
            pos: [pos.x, pos.y],
            spread: 10.0,
            rotation_variance: 5.0,
            is_expanded: false,
            image_ids: Vec::new(),
        }
    }

    pub fn canvas_pos(&self) -> Pos2 {
        Pos2::new(self.pos[0], self.pos[1])
    }

    pub fn set_pos(&mut self, p: Pos2) {
        self.pos = [p.x, p.y];
    }

    pub fn rect(&self) -> Rect {
        let half = 100.0 + self.spread;
        Rect::from_center_size(self.canvas_pos(), Vec2::splat(half * 2.0))
    }
}

// ── Action zones ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionZone {
    pub id: Uuid,
    pub label: String,
    pub color: SerColor,
    pub rect: SerRect,
    pub action: ZoneAction,
    pub opacity: f32,
    pub is_pinned: bool,
    pub is_screen_locked: bool,
    pub screen_pos: Option<[f32; 2]>,
    pub move_contents: bool,
    pub auto_process: bool,
    /// If true, remove the image from the canvas after the zone action runs.
    /// If false (default), the image stays and can be moved to another zone.
    pub remove_after_action: bool,
    pub keeper_mode: bool,
    pub max_images: Option<usize>,
    pub contained_image_ids: Vec<Uuid>,
}

impl ActionZone {
    pub fn new(rect: Rect, action: ZoneAction) -> Self {
        let color = action.default_color();
        let label = action.default_label().to_string();
        Self {
            id: Uuid::new_v4(),
            label,
            color,
            rect: rect.into(),
            action,
            opacity: 0.3,
            is_pinned: false,
            is_screen_locked: false,
            screen_pos: None,
            move_contents: false,
            auto_process: true,
            remove_after_action: true,
            keeper_mode: false,
            max_images: None,
            contained_image_ids: Vec::new(),
        }
    }

    pub fn canvas_rect(&self) -> Rect {
        self.rect.into()
    }

    pub fn set_rect(&mut self, r: Rect) {
        self.rect = r.into();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ZoneAction {
    MoveToFolder { target: PathBuf },
    CopyToFolder { target: PathBuf },
    AddTags { tags: Vec<String> },
    Resize { width: u32, height: u32 },
    ConvertFormat { format: ImageFileFormat },
    Delete,
    DuplicateCheck,
    SimilarityGroup,
    FaceDetection,
}

impl ZoneAction {
    pub fn label(&self) -> &'static str {
        self.default_label()
    }

    pub fn default_label(&self) -> &'static str {
        match self {
            ZoneAction::MoveToFolder { .. } => "Move to Folder",
            ZoneAction::CopyToFolder { .. } => "Copy to Folder",
            ZoneAction::AddTags { .. } => "Add Tags",
            ZoneAction::Resize { .. } => "Resize",
            ZoneAction::ConvertFormat { .. } => "Convert Format",
            ZoneAction::Delete => "Delete",
            ZoneAction::DuplicateCheck => "Duplicate Check",
            ZoneAction::SimilarityGroup => "Similarity Group",
            ZoneAction::FaceDetection => "Face Detection",
        }
    }

    pub fn default_color(&self) -> SerColor {
        match self {
            ZoneAction::MoveToFolder { .. } => SerColor([0x22, 0x88, 0x44, 0xFF]),
            ZoneAction::CopyToFolder { .. } => SerColor([0x22, 0x66, 0xCC, 0xFF]),
            ZoneAction::AddTags { .. } => SerColor([0x88, 0x44, 0xCC, 0xFF]),
            ZoneAction::Resize { .. } => SerColor([0xFF, 0x88, 0x22, 0xFF]),
            ZoneAction::ConvertFormat { .. } => SerColor([0xFF, 0xCC, 0x22, 0xFF]),
            ZoneAction::Delete => SerColor([0xCC, 0x22, 0x22, 0xFF]),
            ZoneAction::DuplicateCheck => SerColor([0x44, 0x88, 0xFF, 0xFF]),
            ZoneAction::SimilarityGroup => SerColor([0x44, 0xCC, 0xFF, 0xFF]),
            ZoneAction::FaceDetection => SerColor([0xFF, 0x44, 0x88, 0xFF]),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            ZoneAction::MoveToFolder { .. } => "📁",
            ZoneAction::CopyToFolder { .. } => "📋",
            ZoneAction::AddTags { .. } => "🏷",
            ZoneAction::Resize { .. } => "⇲",
            ZoneAction::ConvertFormat { .. } => "🔄",
            ZoneAction::Delete => "🗑",
            ZoneAction::DuplicateCheck => "🔍",
            ZoneAction::SimilarityGroup => "⚡",
            ZoneAction::FaceDetection => "👤",
        }
    }

    pub fn all_templates() -> Vec<ZoneAction> {
        vec![
            ZoneAction::MoveToFolder {
                target: PathBuf::new(),
            },
            ZoneAction::CopyToFolder {
                target: PathBuf::new(),
            },
            ZoneAction::AddTags { tags: Vec::new() },
            ZoneAction::Resize {
                width: 0,
                height: 0,
            },
            ZoneAction::ConvertFormat {
                format: ImageFileFormat::Jpeg,
            },
            ZoneAction::Delete,
            ZoneAction::DuplicateCheck,
            ZoneAction::SimilarityGroup,
            ZoneAction::FaceDetection,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImageFileFormat {
    Png,
    Jpeg,
    Webp,
    Bmp,
}

impl std::fmt::Display for ImageFileFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageFileFormat::Png => write!(f, "png"),
            ImageFileFormat::Jpeg => write!(f, "jpg"),
            ImageFileFormat::Webp => write!(f, "webp"),
            ImageFileFormat::Bmp => write!(f, "bmp"),
        }
    }
}

// ── Sticky notes (not persisted) ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StickyNote {
    pub id: Uuid,
    pub rect: Rect,
    pub text: String,
    pub color: Color32,
    pub font_size: f32,
    pub editing: bool,
}

impl StickyNote {
    pub fn new(rect: Rect) -> Self {
        Self {
            id: Uuid::new_v4(),
            rect,
            text: String::new(),
            color: Color32::from_rgb(0xFF, 0xEE, 0x88),
            font_size: 14.0,
            editing: true,
        }
    }
}

/// serde-safe, persisted form of a sticky note. The `editing` flag is runtime
/// only and intentionally absent here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickyNoteData {
    pub id: Uuid,
    pub rect: SerRect,
    pub text: String,
    pub color: SerColor,
    pub font_size: f32,
}

impl From<&StickyNote> for StickyNoteData {
    fn from(n: &StickyNote) -> Self {
        Self {
            id: n.id,
            rect: n.rect.into(),
            text: n.text.clone(),
            color: n.color.into(),
            font_size: n.font_size,
        }
    }
}

impl StickyNoteData {
    pub fn into_note(self) -> StickyNote {
        StickyNote {
            id: self.id,
            rect: self.rect.into(),
            text: self.text,
            color: self.color.into(),
            font_size: self.font_size,
            editing: false,
        }
    }
}

// ── Session metadata ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: Uuid,
    pub name: String,
    pub image_count: usize,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
}

// ── Worker messages ───────────────────────────────────────────────────────────

pub struct ThumbRequest {
    pub image_id: Uuid,
    pub path: PathBuf,
    pub size: u32,
}

pub struct ThumbResult {
    pub image_id: Uuid,
    /// None means the file failed to load — don't retry.
    pub rgba: Option<image::RgbaImage>,
}

// ── Sort mode ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortMode {
    NameAsc,
    NameDesc,
    DateNewest,
    DateOldest,
    SizeLargest,
    SizeSmallest,
    TypeAsc,
    Random,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::NameAsc => "Name A→Z",
            SortMode::NameDesc => "Name Z→A",
            SortMode::DateNewest => "Date: Newest",
            SortMode::DateOldest => "Date: Oldest",
            SortMode::SizeLargest => "Size: Largest",
            SortMode::SizeSmallest => "Size: Smallest",
            SortMode::TypeAsc => "File Type",
            SortMode::Random => "Shuffle",
        }
    }
}

// ── Canvas tool ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasTool {
    Select,
    DrawZone,
    Annotation,
    StickyCollect,
}

// ── serde-friendly geometry wrappers ─────────────────────────────────────────

/// RGBA bytes — serde-safe substitute for egui::Color32.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SerColor(pub [u8; 4]);

impl From<SerColor> for Color32 {
    fn from(c: SerColor) -> Self {
        Color32::from_rgba_unmultiplied(c.0[0], c.0[1], c.0[2], c.0[3])
    }
}

impl From<Color32> for SerColor {
    fn from(c: Color32) -> Self {
        SerColor(c.to_array())
    }
}

/// `[min_x, min_y, max_x, max_y]` — serde-safe substitute for egui::Rect.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SerRect(pub [f32; 4]);

impl From<Rect> for SerRect {
    fn from(r: Rect) -> Self {
        SerRect([r.min.x, r.min.y, r.max.x, r.max.y])
    }
}

impl From<SerRect> for Rect {
    fn from(s: SerRect) -> Self {
        Rect::from_min_max(Pos2::new(s.0[0], s.0[1]), Pos2::new(s.0[2], s.0[3]))
    }
}

#[cfg(test)]
mod sticky_note_tests {
    use super::*;

    #[test]
    fn note_data_round_trip_drops_editing_flag() {
        let mut n = StickyNote::new(Rect::from_min_max(
            Pos2::new(1.0, 2.0),
            Pos2::new(61.0, 42.0),
        ));
        n.text = "hello".into();
        n.font_size = 18.0;
        n.editing = true; // runtime-only

        let data = StickyNoteData::from(&n);
        let back = data.into_note();

        assert_eq!(back.id, n.id);
        assert_eq!(back.text, "hello");
        assert_eq!(back.font_size, 18.0);
        assert_eq!(back.rect, n.rect);
        assert_eq!(back.color, n.color);
        assert!(!back.editing, "loaded notes are never in editing mode");
    }

    #[test]
    fn note_data_bincode_round_trip() {
        let n = StickyNote::new(Rect::from_min_max(Pos2::ZERO, Pos2::new(80.0, 50.0)));
        let data = vec![StickyNoteData::from(&n)];
        let bytes = bincode::serialize(&data).unwrap();
        let decoded: Vec<StickyNoteData> = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].id, n.id);
    }
}
