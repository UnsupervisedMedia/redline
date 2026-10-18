#[cfg(feature = "agent-server")]
use crate::agent::AgentServer;
use crate::scene::Document;
use crate::shortcuts::{load_shortcuts, Action, KeyCombo};
use crate::theme::{get_theme, Theme, ThemeId};
use crate::types::{
    Annotation, ArrowHeads, CanvasEvent, Command, EventKind, LineStyle, LogicTagKind, Participant,
    ParticipantId,
};
use egui::{Color32, Rect, TextureHandle, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use uuid::Uuid;

// ── Canvas UID ───────────────────────────────────────────────────────────────

static NEXT_CANVAS_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A process-global, monotonically increasing document identity. Used to detect
/// when the *active document* changes (load/new/paste/capture/tab-switch), which
/// `source_gen` (a per-canvas counter that resets per image) cannot do reliably.
pub fn next_canvas_uid() -> u64 {
    NEXT_CANVAS_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// ── CanvasState ───────────────────────────────────────────────────────────────

pub struct CanvasState {
    /// Authoritative scene-graph store. `annotations` is a derived mirror —
    /// never mutate it directly; mutate `document` then call `sync_annotations()`.
    pub document: Document,
    pub annotations: Vec<Annotation>,
    /// The layer new shapes are added to. Defaults to the default layer.
    pub active_layer: Uuid,
    pub texture: Option<TextureHandle>,
    pub color_image: Option<egui::ColorImage>,
    /// Stable id used by session persistence to cache this tab's encoded source
    /// PNG across autosaves. Assigned lazily by the session writer; never
    /// reused once a canvas is dropped. 0 means "not yet assigned".
    pub session_id: u64,
    /// Monotonic counter bumped whenever the *source* image (`color_image`)
    /// changes. The session writer compares it against its cache to avoid
    /// re-encoding an unchanged source image on every autosave tick.
    pub source_gen: u64,
    /// Optional path the source image was originally opened from. Preferred over
    /// re-encoding pixels when persisting a session. `None` for screen-captures
    /// or clipboard pastes, where the source is persisted as PNG bytes instead.
    pub source_path: Option<PathBuf>,
    /// Edit-revision signature `(human undo-stack len, source_gen)` captured at
    /// the last successful save (or on open). `is_saved()` compares it against
    /// the current revision to drive the saved/unsaved indicator. `None` means
    /// never saved (e.g. a fresh capture/paste with no file yet).
    pub saved_rev: Option<(usize, u64)>,
    /// Stable, globally-unique identity of the document this canvas holds. `0`
    /// means "no real document yet". Assigned by `assign_new_uid` whenever a new
    /// source image is applied. Travels with the tab (part of `CanvasState`).
    pub canvas_uid: u64,
    /// Per-participant undo stacks keyed by author. The human's binding
    /// (Ctrl+Z / Ctrl+Shift+Z) reads from `ParticipantId::HUMAN_LOCAL`.
    pub undo_stacks: HashMap<ParticipantId, Vec<Command>>,
    pub redo_stacks: HashMap<ParticipantId, Vec<Command>>,
    pub canvas_size: Vec2,
    /// Per-canvas vectorize panel state (settings live in Settings instead).
    pub vectorize_panel: VectorizePanel,
    /// Current marquee selection, if any. Lives outside annotations.
    pub selection: Option<Selection>,
    /// While this is Some, the brightness/contrast/saturation dialog is open.
    pub adjust_preview: Option<AdjustPreview>,
    /// Internal marquee interaction state (moving / resizing / drawing).
    pub marquee_state: MarqueeState,
    /// Current PDF page index (0-based). 0 when not a PDF.
    pub pdf_page: usize,
    /// Total number of PDF pages. 0 when not a PDF.
    pub pdf_pages: usize,
    /// Participants currently active in this room. Always contains at least
    /// `ParticipantId::HUMAN_LOCAL`. Agents join by adding entries; reaped
    /// after disconnect grace period (Phase 2).
    pub participants: HashMap<ParticipantId, Participant>,
    #[cfg(feature = "agent-server")]
    /// Broadcast channel for change events. `None` when no agent server is
    /// running; populated by `App::update` reconciliation when the server starts.
    pub event_broadcast: Option<tokio::sync::broadcast::Sender<crate::agent::events::ServerEvent>>,
    /// Ring-buffer of committed events for replay. Bounded at `EVENT_LOG_CAPACITY`.
    pub event_log: VecDeque<CanvasEvent>,
    /// Monotonic sequence counter; assigned to each `CanvasEvent` when it lands.
    pub next_seq: u64,

    /// Per-agent canvas-scoped memory keyed by stable AgentId role string
    /// (not by ParticipantId — ParticipantIds are minted fresh per session,
    /// while memory must survive toggle-off/on cycles).
    #[cfg(feature = "local-ai")]
    pub agent_memory: HashMap<crate::local_ai::AgentId, Vec<crate::local_ai::memory::MemoryEntry>>,

    /// Notes that all agents in this canvas can read and write.
    #[cfg(feature = "local-ai")]
    pub shared_notes: Vec<crate::local_ai::memory::SharedNote>,

    /// Per-agent invisible spatial bookmarks — not rendered in the normal
    /// canvas paint pass; revealed only via the agent panel's peek toggle.
    #[cfg(feature = "local-ai")]
    pub agent_bookmarks:
        HashMap<crate::local_ai::AgentId, Vec<crate::local_ai::memory::AgentBookmark>>,
}

/// Maximum number of events kept in `CanvasState::event_log`. Older events
/// are evicted from the front when the buffer is full; agents whose `seq`
/// has fallen below the buffer floor must rebuild state from `list_shapes`.
pub const EVENT_LOG_CAPACITY: usize = 10_000;

impl Default for CanvasState {
    fn default() -> Self {
        let mut participants = HashMap::new();
        let human = Participant::new_human();
        participants.insert(ParticipantId::HUMAN_LOCAL, human);

        let mut undo_stacks = HashMap::new();
        undo_stacks.insert(ParticipantId::HUMAN_LOCAL, Vec::new());
        let mut redo_stacks = HashMap::new();
        redo_stacks.insert(ParticipantId::HUMAN_LOCAL, Vec::new());

        let document = Document::from_flat(Vec::new());
        let active_layer = document.default_layer();

        Self {
            document,
            active_layer,
            annotations: Vec::new(),
            texture: None,
            color_image: None,
            session_id: 0,
            source_gen: 0,
            source_path: None,
            saved_rev: None,
            canvas_uid: 0,
            undo_stacks,
            redo_stacks,
            canvas_size: Vec2::ZERO,
            vectorize_panel: VectorizePanel::default(),
            selection: None,
            adjust_preview: None,
            marquee_state: MarqueeState::Idle,
            pdf_page: 0,
            pdf_pages: 0,
            participants,
            #[cfg(feature = "agent-server")]
            event_broadcast: None,
            event_log: VecDeque::new(),
            next_seq: 0,
            #[cfg(feature = "local-ai")]
            agent_memory: HashMap::new(),
            #[cfg(feature = "local-ai")]
            shared_notes: Vec::new(),
            #[cfg(feature = "local-ai")]
            agent_bookmarks: HashMap::new(),
        }
    }
}

impl CanvasState {
    /// Rebuild the derived `annotations` mirror from the authoritative document.
    /// Call after every mutation of `document`.
    pub fn sync_annotations(&mut self) {
        self.annotations = self.document.to_flat();
    }

    /// Insert `ann` as a new leaf in the default layer and resync the mirror.
    /// Returns the new leaf id (equals `ann.id`).
    pub fn add_annotation(&mut self, ann: Annotation) -> uuid::Uuid {
        let layer = if self.document.get(self.active_layer).is_some() {
            self.active_layer
        } else {
            let dl = self.document.default_layer();
            self.active_layer = dl;
            dl
        };
        let id = self.document.add_annotation(&ann, layer);
        self.sync_annotations();
        id
    }

    /// Clear every participant's undo and redo stacks. Used by image-altering
    /// operations (crop, resize canvas, replace image) that invalidate the
    /// history of every actor at once.
    pub fn clear_all_undo_redo(&mut self) {
        for stack in self.undo_stacks.values_mut() {
            stack.clear();
        }
        for stack in self.redo_stacks.values_mut() {
            stack.clear();
        }
    }

    /// Record that the source image changed so the session writer re-encodes it
    /// on the next autosave. Call from any op that replaces `color_image`.
    pub fn mark_source_changed(&mut self) {
        self.source_gen = self.source_gen.wrapping_add(1);
    }

    /// Stamp this canvas with a fresh global document identity. Call when a new
    /// source image is applied (load, new canvas, paste, capture, replace).
    pub fn assign_new_uid(&mut self) {
        self.canvas_uid = next_canvas_uid();
    }

    /// Current edit-revision signature: `(human undo-stack len, source_gen)`.
    /// Bumps when annotations/inpaint commits land (undo stack grows) or when
    /// the source image changes. Used as a cheap "has the content changed since
    /// the last save?" signal for the saved indicator.
    pub fn current_rev(&self) -> (usize, u64) {
        let undo = self
            .undo_stacks
            .get(&ParticipantId::HUMAN_LOCAL)
            .map(|s| s.len())
            .unwrap_or(0);
        (undo, self.source_gen)
    }

    /// True when the canvas content matches the last saved revision.
    pub fn is_saved(&self) -> bool {
        self.saved_rev == Some(self.current_rev())
    }

    /// Record the current revision as the saved baseline (call after a
    /// successful save or right after opening a file).
    pub fn mark_saved(&mut self) {
        self.saved_rev = Some(self.current_rev());
    }

    /// Append an event to the log, assigning the next sequence number.
    /// Evicts the oldest event if the buffer is full. Returns the assigned seq.
    pub fn record_event(&mut self, author: ParticipantId, kind: EventKind) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        let evt = CanvasEvent {
            seq,
            ts: std::time::SystemTime::now(),
            author,
            kind,
        };
        if self.event_log.len() >= EVENT_LOG_CAPACITY {
            self.event_log.pop_front();
        }
        self.event_log.push_back(evt);
        seq
    }

    /// Return events with `seq > since`. If `since` is below the buffer floor
    /// (oldest retained seq), returns `Err(current_seq)` so the caller can
    /// fall back to `list_shapes`. Overflow-safe: `since == u64::MAX` returns
    /// `Ok(empty)` without panicking.
    pub fn events_since(&self, since: u64) -> Result<Vec<CanvasEvent>, u64> {
        let floor = self.event_log.front().map(|e| e.seq).unwrap_or(0);
        if since < floor.saturating_sub(1) {
            return Err(self.next_seq.saturating_sub(1));
        }
        Ok(self
            .event_log
            .iter()
            .filter(|e| e.seq > since)
            .cloned()
            .collect())
    }
}

// ── Settings ──────────────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum SmoothingMode {
    Realtime,
    OnRelease,
}

impl PartialEq for SmoothingMode {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Realtime, Self::Realtime) | (Self::OnRelease, Self::OnRelease)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::WebP => "webp",
            ExportFormat::Bmp => "bmp",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG (lossless)",
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::WebP => "WebP (lossless)",
            ExportFormat::Bmp => "BMP",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
#[allow(dead_code)]
pub struct Settings {
    pub smoothing_mode: SmoothingMode,
    pub pen_smoothing: bool,
    pub stroke_color: Color32,
    pub stroke_width: f32,
    pub accent_color: Color32,
    pub text_size: f32,
    pub blur_block: f32,
    pub recent_colors: Vec<Color32>,
    pub magnifier: bool,
    pub line_style: LineStyle,
    pub arrow_heads: ArrowHeads,
    pub flatten_export: bool,
    pub auto_save: bool,
    /// Directory template for auto-saved captures (strftime + leading `~`).
    #[serde(default = "default_auto_save_dir_template")]
    pub auto_save_dir_template: String,
    /// File-stem template for auto-saved captures (strftime; extension added).
    #[serde(default = "default_auto_save_name_template")]
    pub auto_save_name_template: String,
    /// Copy every finished capture straight to the clipboard — the common
    /// "grab it and paste it somewhere" flow, with no extra keystroke.
    #[serde(default)]
    pub auto_copy_on_capture: bool,
    /// When true, open tabs + annotations are persisted on exit and restored on
    /// next launch. Opt-out: defaults to true. (`#[serde(default)]` on the
    /// struct makes this additive for older settings.json files.)
    pub restore_session: bool,
    #[serde(skip)] // session UI state — always starts closed
    pub show_recents: bool,
    pub capture_delay_s: u32,
    pub hide_redline_on_capture: bool,
    /// Use RedLine's own region overlay (live dimensions, magnifier, pixel
    /// nudging) instead of the desktop portal's selector. Defaults on where we
    /// can position a borderless overlay; Wayland compositors don't allow
    /// absolute window placement, so there we stay on the portal.
    #[serde(default = "default_region_selector_inapp")]
    pub region_selector_inapp: bool,
    pub step_radius: f32,
    pub export_format: ExportFormat,
    pub jpeg_quality: u8,
    #[serde(skip)] // persisted separately in shortcuts.json
    pub shortcuts: HashMap<Action, KeyCombo>,
    pub show_grid: bool,
    pub grid_size: f32,
    pub marquee_shape: SelKind,
    pub fill_tolerance: u8,
    pub text_bg: bool,
    #[serde(skip)] // session UI state — always starts closed
    pub show_layers: bool,
    /// When true, the toggleable bottom Pathfinder strip is shown (`View ▸ Pathfinder`).
    #[serde(default)]
    pub show_pathfinder: bool,
    /// When true, the numeric Transform panel is shown (`View ▸ Transform`).
    #[serde(default)]
    pub show_transform: bool,
    /// Which of the 9 reference-point grid cells is active in the Transform panel.
    /// Encoded as `row * 3 + col` where row/col ∈ {0,1,2}; 0 = top-left, 4 = center, 8 = bottom-right.
    /// Defaults to 4 (center).
    #[serde(default = "default_transform_ref_point")]
    pub transform_ref_point: u8,
    pub corner_radius: f32,
    /// Whether the Rect/Oval tools' shape is filled. Independent of `stroke`.
    pub shape_fill_enabled: bool,
    /// Fill color for the Rect/Oval tools; the color's own alpha is the fill opacity.
    pub shape_fill_color: Color32,
    /// Whether the Rect/Oval tools' outline is drawn.
    pub shape_stroke_enabled: bool,
    pub theme_id: ThemeId,
    #[cfg(feature = "agent-server")]
    pub agent_server_enabled: bool,
    /// When true, the AI / Agents toolbar section is expanded. Stays collapsed by
    /// default so AI/agent controls are hidden until the user opts in; once the
    /// user reveals the section it persists across sessions.
    #[cfg(any(feature = "agent-server", feature = "local-ai"))]
    pub ai_section_expanded: bool,
    /// Default color the local AI uses for its annotations (Question bubbles,
    /// proposed shapes) when the model doesn't pick its own. The model can
    /// still override per-action via the `color` arg.
    #[cfg(feature = "local-ai")]
    pub local_ai_color: Color32,
    /// Persisted SVG-conversion knob settings.
    pub vectorize: VectorizeSettings,
}

fn default_transform_ref_point() -> u8 {
    4
} // center of the 3×3 grid

/// Wayland forbids absolute window positioning, so an overlay pinned to 0,0
/// across the virtual desktop can't be built there — fall back to the portal.
pub fn default_region_selector_inapp() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none()
}

fn default_auto_save_dir_template() -> String {
    crate::recents::DEFAULT_DIR_TEMPLATE.to_string()
}

fn default_auto_save_name_template() -> String {
    crate::recents::DEFAULT_NAME_TEMPLATE.to_string()
}

impl Settings {
    pub fn theme(&self) -> Theme {
        get_theme(self.theme_id)
    }
}

/// Map the freehand smoothing controls to a curve-fit tolerance in px. Higher
/// tolerance → fewer nodes in the committed `BezierPath`.
pub fn freehand_fit_tol(s: &Settings) -> f32 {
    let base = if s.pen_smoothing { 2.5 } else { 1.0 };
    match s.smoothing_mode {
        SmoothingMode::Realtime => base,
        SmoothingMode::OnRelease => base * 1.5,
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            smoothing_mode: SmoothingMode::OnRelease,
            pen_smoothing: true,
            stroke_color: Color32::from_rgb(255, 59, 48),
            stroke_width: 3.0,
            accent_color: Color32::from_rgb(56, 139, 253),
            text_size: 18.0,
            blur_block: 12.0,
            recent_colors: Vec::new(),
            magnifier: false,
            line_style: LineStyle::Solid,
            arrow_heads: ArrowHeads::Single,
            flatten_export: true,
            auto_save: true,
            auto_save_dir_template: default_auto_save_dir_template(),
            auto_save_name_template: default_auto_save_name_template(),
            auto_copy_on_capture: false,
            restore_session: true,
            show_recents: false,
            capture_delay_s: 0,
            hide_redline_on_capture: true,
            region_selector_inapp: default_region_selector_inapp(),
            step_radius: 18.0,
            export_format: ExportFormat::Png,
            jpeg_quality: 90,
            shortcuts: load_shortcuts(),
            show_grid: false,
            grid_size: 50.0,
            marquee_shape: SelKind::Rect,
            fill_tolerance: 30,
            text_bg: false,
            show_layers: true,
            show_pathfinder: false,
            show_transform: false,
            transform_ref_point: 4,
            corner_radius: 0.0,
            shape_fill_enabled: false,
            shape_fill_color: Color32::from_rgb(255, 204, 0),
            shape_stroke_enabled: true,
            theme_id: ThemeId::Graphite,
            // Security: the agent server binds to 127.0.0.1 with no auth and lets
            // any local process drive the canvas. Keep it strictly opt-in — the
            // user must enable it explicitly from the AI / Agents menu.
            #[cfg(feature = "agent-server")]
            agent_server_enabled: false,
            #[cfg(any(feature = "agent-server", feature = "local-ai"))]
            ai_section_expanded: false,
            #[cfg(feature = "local-ai")]
            local_ai_color: Color32::from_rgb(0xaa, 0xaa, 0x00),
            vectorize: VectorizeSettings::default(),
        }
    }
}

/// 8 built-in color presets.
pub const COLOR_PRESETS: [Color32; 8] = [
    Color32::from_rgb(255, 59, 48),   // red
    Color32::from_rgb(255, 149, 0),   // orange
    Color32::from_rgb(255, 204, 0),   // yellow
    Color32::from_rgb(52, 199, 89),   // green
    Color32::from_rgb(56, 139, 253),  // blue
    Color32::from_rgb(175, 82, 222),  // purple
    Color32::from_rgb(0, 0, 0),       // black
    Color32::from_rgb(255, 255, 255), // white
];

/// Push a color into the MRU recent-colors list (max 5, no duplicates).
pub fn push_recent_color(recents: &mut Vec<Color32>, c: Color32) {
    recents.retain(|x| *x != c);
    recents.insert(0, c);
    recents.truncate(5);
}

// ── Tool & ToolState ──────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Default)]
pub enum Tool {
    #[default]
    Select,
    Pen,
    Rect,
    Oval,
    Star,
    Stamp(&'static str),
    Arrow,
    Line,
    Text,
    Blur,
    Crop,
    Eyedropper,
    Step,
    FocusRegion,
    LogicTag(LogicTagKind),
    /// Marquee selection (rect or ellipse, controlled by `Settings::marquee_shape`).
    Marquee,
    /// Flood fill — click to fill a contiguous region with the current color.
    Fill,
    /// Freehand polygon selection — draws an irregular boundary and selects all
    /// annotations whose center falls inside.
    Lasso,
    /// Rounded rectangle with a triangular tail (speech bubble).
    Callout,
    /// Line annotation that shows the pixel distance between endpoints.
    Ruler,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelKind {
    Rect,
    Ellipse,
}

/// Transient marquee selection. Lives outside annotations — it's UI state, not
/// part of the undo stack, and never serialized.
#[derive(Clone, Copy)]
pub struct Selection {
    pub kind: SelKind,
    /// Selection rect in image-pixel space.
    pub rect: Rect,
    pub inverted: bool,
}

impl Selection {
    /// Does the mask (rect/ellipse, respecting inverted) include image-pixel (x, y)?
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let inside = match self.kind {
            SelKind::Rect => self.rect.contains(egui::pos2(x, y)),
            SelKind::Ellipse => {
                let cx = self.rect.center().x;
                let cy = self.rect.center().y;
                let rx = (self.rect.width() * 0.5).max(0.001);
                let ry = (self.rect.height() * 0.5).max(0.001);
                let nx = (x - cx) / rx;
                let ny = (y - cy) / ry;
                nx * nx + ny * ny <= 1.0
            }
        };
        if self.inverted {
            !inside
        } else {
            inside
        }
    }
}

/// Live brightness/contrast/saturation preview state. Holds the pixels as they
/// were when the dialog opened so Cancel can fully revert.
pub struct AdjustPreview {
    pub brightness: f32, // -1.0 .. 1.0
    pub contrast: f32,   // -1.0 .. 1.0
    pub saturation: f32, // -1.0 .. 1.0
    pub original_pixels: Vec<Color32>,
    pub original_size: [usize; 2],
    /// If Some, adjustments apply only within the mask.
    pub selection: Option<Selection>,
}

// ── Vectorize dialog ──────────────────────────────────────────────────────────

/// One `<path …/>` element extracted from generated SVG, with an enabled flag
/// so the user can delete individual paths without re-running the vectorizer.
#[derive(Clone)]
pub struct SvgPathEntry {
    pub element: String,
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum VectorizeMode {
    #[default]
    LineArt,
    Grayscale,
    Color,
}

/// How to produce a binary mask from the source image in Line Art mode.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum EdgeMethod {
    /// Luminance threshold — best for images that are already black line art.
    #[default]
    Threshold,
    /// Zhang-Suen morphological thinning — traces the centerline of each stroke
    /// instead of its outline. Best for replicating the original line intent.
    Skeleton,
    /// Canny edge detection — best for extracting edges from photos.
    Canny,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum SvgBackground {
    Transparent,
    LightGray,
    White,
}

impl Default for SvgBackground {
    fn default() -> Self {
        Self::White
    }
}

/// Which content to include in the exported/copied SVG.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum VectorizeEmitMode {
    /// Trace the raster image into paths (the original behaviour).
    #[default]
    TraceImage,
    /// Emit only the annotation layer as SVG elements.
    Annotations,
    /// Combine both: traced paths with the annotation layer on top.
    Combined,
}

/// Vectorize panel settings persisted across canvases and sessions.
/// Lives in `Settings` (saved to disk); the per-canvas in-flight job
/// and cached preview live in `CanvasState::vectorize_panel` instead.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VectorizeSettings {
    pub mode: VectorizeMode,
    pub edge_method: EdgeMethod,
    pub threshold: u8,
    pub blur_sigma: f32,
    pub edge_low: f32,
    pub edge_high: f32,
    pub stroke_color: Color32,
    pub stroke_width: f32,
    pub background: SvgBackground,
    pub filter_speckle: u32,
    pub path_precision: u32,
    pub corner_threshold: f32,
    pub segment_length: f32,
    pub splice_threshold: u32,
    pub svg_fill: bool,
    pub svg_fill_color: Color32,
    pub post_smooth: u8,
    pub color_count: u32,
    pub emit_mode: VectorizeEmitMode,
}

impl Default for VectorizeSettings {
    fn default() -> Self {
        Self {
            mode: VectorizeMode::LineArt,
            edge_method: EdgeMethod::Threshold,
            threshold: 128,
            blur_sigma: 1.0,
            edge_low: 50.0,
            edge_high: 150.0,
            stroke_color: Color32::BLACK,
            stroke_width: 1.0,
            background: SvgBackground::White,
            filter_speckle: 4,
            path_precision: 8,
            corner_threshold: 60.0,
            segment_length: 4.0,
            splice_threshold: 45,
            svg_fill: false,
            svg_fill_color: Color32::WHITE,
            post_smooth: 0,
            color_count: 8,
            emit_mode: VectorizeEmitMode::default(),
        }
    }
}

/// Per-canvas vectorize panel state — open flag, in-flight job, cached preview,
/// and editable parsed paths. Travels with the tab on switch.
pub struct VectorizePanel {
    pub open: bool,
    pub dirty: bool,
    pub last_dirty: Option<std::time::Instant>,
    pub is_computing: bool,
    pub auto_preview: std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>,
    pub preview_svg: Option<Vec<u8>>,
    pub preview_error: Option<String>,
    pub preview_gen: u32,
    pub svg_header: String,
    pub svg_footer: String,
    pub svg_paths: Vec<SvgPathEntry>,
}

impl Default for VectorizePanel {
    fn default() -> Self {
        Self {
            open: false,
            dirty: true,
            last_dirty: None,
            is_computing: false,
            auto_preview: std::sync::Arc::new(std::sync::Mutex::new(None)),
            preview_svg: None,
            preview_error: None,
            preview_gen: 0,
            svg_header: String::new(),
            svg_footer: String::new(),
            svg_paths: Vec::new(),
        }
    }
}

#[derive(Default)]
pub enum ToolState {
    #[default]
    Idle,
    Drawing {
        start: egui::Pos2,
        current: egui::Pos2,
    },
    DrawingPen {
        raw_points: Vec<egui::Pos2>,
    },
    Selected {
        id: Uuid,
        drag_offset: Option<egui::Vec2>,
    },
    /// Multiple annotations selected. `drag_offset` accumulates total image-pixel
    /// delta while dragging so a single undo Command can revert the whole move.
    MultiSelected {
        ids: Vec<Uuid>,
        drag_offset: Option<egui::Vec2>,
        /// Rubber-band selection start point (image-pixel space).
        band_start: Option<egui::Pos2>,
        /// Rubber-band current cursor position (image-pixel space).
        band_current: Option<egui::Pos2>,
    },
    /// Drawing a freehand lasso selection polygon. Points are in image-pixel space.
    DrawingLasso {
        points: Vec<egui::Pos2>,
    },
    EditingText {
        pos: egui::Pos2,
        buffer: String,
        color: Color32,
        size: f32,
        /// Set when editing an existing annotation (double-click). Commit
        /// emits SetTextContent instead of Add so no delete+recreate needed.
        editing_id: Option<Uuid>,
    },
    PendingCrop {
        rect: egui::Rect,
    },
    /// Dragging a corner of an ImageLayer. `anchor` is the pinned corner
    /// (opposite to the handle being dragged), `start_rect` is the rect at
    /// the moment of press (used to emit the inverse resize on release).
    ResizingImage {
        id: Uuid,
        anchor: egui::Pos2,
        start_rect: egui::Rect,
    },
    /// On-canvas transform (scale / rotate / skew) drag via the gizmo handles.
    /// Holds the grabbed `handle`, the scale `pivot` (anchored point, image space),
    /// the selection `start_bbox` (image-space AABB at press), the live `cursor`
    /// (image space, updated each drag frame for preview), and `start` — a per-node
    /// snapshot of each selected node's `transform` so a single undoable Batch can
    /// be built on release.
    ///
    /// `skew` is **frozen at drag-start**: it is `true` when the grabbed handle is
    /// an edge handle AND Ctrl/Cmd was held at the moment of press. Storing it here
    /// — instead of re-reading the modifier at drag-end or during preview — ensures
    /// that releasing Ctrl before lifting the mouse still commits a skew (not a
    /// scale), and that the live preview matches the committed result.
    Transforming {
        handle: crate::tools::gizmo::Handle,
        pivot: egui::Pos2,
        start_bbox: egui::Rect,
        /// Cursor position (image space) at drag-start — used by rotate/skew to
        /// compute the angular / shear delta relative to the original press point.
        start_cursor: egui::Pos2,
        /// Current cursor position (image space), updated every drag event.
        cursor: egui::Pos2,
        start: Vec<(Uuid, kurbo::Affine)>,
        /// Skew mode, frozen at drag-start: true iff the handle is an edge handle
        /// AND Ctrl/Cmd was held when the drag began. Never re-read from the
        /// keyboard after this point — see variant-level doc above.
        skew: bool,
    },
}

// ── Tab ───────────────────────────────────────────────────────────────────────

pub struct Tab {
    pub label: String,
    pub canvas: CanvasState,
    pub view_zoom: f32,
    pub view_pan: egui::Vec2,
}

// ── LayerDrag ─────────────────────────────────────────────────────────────────

/// In-flight layers-panel row drag (drag-to-reorder within the same parent).
#[derive(Clone, Debug)]
pub struct LayerDrag {
    pub id: uuid::Uuid,
    pub y: f32,
}

// ── TabDrag ───────────────────────────────────────────────────────────────────

pub struct TabDrag {
    pub dragging_tab: usize,
    pub drag_x: f32,
}

// ── CanvasSizeDialog ─────────────────────────────────────────────────────────

/// Background fill chosen for a brand-new canvas. Ignored by the Resize dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CanvasBackground {
    #[default]
    White,
    Transparent,
    Black,
    Custom,
}

pub struct CanvasSizeDialog {
    pub width: u32,
    pub height: u32,
    /// Clipboard dimensions detected when the dialog opened (shown as a hint).
    pub clipboard_hint: Option<(u32, u32)>,
    // ── New Canvas only (the Resize dialog leaves these at their defaults) ──────
    /// Index of the currently selected preset category tab.
    pub category: usize,
    /// Chosen background fill for the new canvas.
    pub background: CanvasBackground,
    /// RGB used when `background == Custom`.
    pub custom_color: [u8; 3],
}

impl CanvasSizeDialog {
    /// Construct a dialog sized to `width`×`height` with New-Canvas defaults.
    pub fn new(width: u32, height: u32, clipboard_hint: Option<(u32, u32)>) -> Self {
        Self {
            width,
            height,
            clipboard_hint,
            category: 0,
            background: CanvasBackground::default(),
            custom_color: [255, 255, 255],
        }
    }

    /// Resolve the chosen background to a concrete RGBA fill color.
    pub fn fill_rgba(&self) -> [u8; 4] {
        match self.background {
            CanvasBackground::White => [255, 255, 255, 255],
            CanvasBackground::Transparent => [0, 0, 0, 0],
            CanvasBackground::Black => [0, 0, 0, 255],
            CanvasBackground::Custom => [
                self.custom_color[0],
                self.custom_color[1],
                self.custom_color[2],
                255,
            ],
        }
    }
}

// ── SnapMode ─────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum SnapMode {
    Full,
    Region,
}

// ── Toast ─────────────────────────────────────────────────────────────────────

/// A transient status message with a lifetime. Status text used to sit in the
/// corner until something else replaced it, so a stale "Copied to clipboard"
/// could stay on screen for an hour; a toast states its business and leaves.
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub shown_at: std::time::Instant,
    /// Failures stay up long enough to actually be read.
    pub is_error: bool,
}

/// How long a toast stays fully opaque before it starts fading.
const TOAST_HOLD_S: f32 = 3.2;
const TOAST_ERROR_HOLD_S: f32 = 8.0;
/// Fade duration after the hold.
const TOAST_FADE_S: f32 = 0.9;

impl Toast {
    pub fn new(text: String) -> Self {
        let is_error = looks_like_error(&text);
        Self {
            text,
            shown_at: std::time::Instant::now(),
            is_error,
        }
    }

    pub fn hold_s(&self) -> f32 {
        if self.is_error {
            TOAST_ERROR_HOLD_S
        } else {
            TOAST_HOLD_S
        }
    }

    /// Opacity in 0..=1 at `elapsed` seconds; 0 once the toast is finished.
    pub fn opacity_at(&self, elapsed: f32) -> f32 {
        let hold = self.hold_s();
        if elapsed <= hold {
            1.0
        } else if elapsed >= hold + TOAST_FADE_S {
            0.0
        } else {
            1.0 - (elapsed - hold) / TOAST_FADE_S
        }
    }

    pub fn opacity(&self) -> f32 {
        self.opacity_at(self.shown_at.elapsed().as_secs_f32())
    }

    pub fn is_done(&self) -> bool {
        self.opacity() <= 0.0
    }

    /// Reconcile the toast with whatever the app last wrote to `status_msg`.
    /// Returns true while a toast is on screen (the caller should keep
    /// repainting so the fade animates).
    pub fn sync(status_msg: &mut Option<String>, toast: &mut Option<Toast>) -> bool {
        match status_msg.as_deref() {
            Some(msg) => {
                let changed = toast.as_ref().map(|t| t.text != msg).unwrap_or(true);
                if changed {
                    *toast = Some(Toast::new(msg.to_string()));
                }
            }
            None => *toast = None,
        }
        if toast.as_ref().map(|t| t.is_done()).unwrap_or(false) {
            // Clear the source too, so the same message can toast again later.
            *toast = None;
            *status_msg = None;
        }
        toast.is_some()
    }
}

/// Heuristic severity check — writers use plain strings, not levels.
fn looks_like_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "fail",
        "error",
        "cannot",
        "can't",
        "unavailable",
        "no such",
        "denied",
        "unable",
        "bad ",
        "wrong",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

// ── AppMode ───────────────────────────────────────────────────────────────────

/// Top-level application mode — selects which major UI is displayed.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum AppMode {
    #[default]
    Annotate,
    // SVG editor is scaffolded but deferred to a future phase.
    SvgEditor,
    CanvasSort,
    LamaCleaner,
    NativeInpaint,
}

impl AppMode {
    /// File-based tools operate on the active document (`AppState.canvas`) and
    /// share the open file tabs + the shared File menu: Annotate (RedLine),
    /// Native Inpaint, and the (placeholder) SVG editor.
    ///
    /// Folder-based / standalone tools (Canvas Sort, Lama-Cleaner) manage their
    /// own image set and do not participate in the tab system.
    pub fn is_file_based(self) -> bool {
        matches!(
            self,
            AppMode::Annotate | AppMode::NativeInpaint | AppMode::SvgEditor
        )
    }

    /// Whether the file-tab bar should accept input in this mode. File-based
    /// workspaces drive the tabs; folder workspaces show them disabled.
    pub fn file_tabs_interactive(self) -> bool {
        self.is_file_based()
    }
}

// ── AppState ──────────────────────────────────────────────────────────────────

pub struct AppState {
    pub canvas: CanvasState,
    pub current_tool: Tool,
    pub tool_state: ToolState,
    /// Transient Layers-panel rename buffer: `(node id, edit text)` while a
    /// layer/group/shape name is being edited inline. `None` when not renaming.
    pub layer_rename: Option<(Uuid, String)>,
    pub settings: Settings,
    pub status_msg: Option<String>,
    /// Live toast derived from `status_msg`. Writers all over the app just set
    /// `status_msg`; `Toast::sync` notices the change and (re)starts the timer,
    /// so no call site has to know about the toast at all.
    pub toast: Option<Toast>,
    /// Set by UI that can't reach the clipboard directly (the empty-state card);
    /// app.rs consumes it on the next frame and runs the normal paste path.
    pub request_paste: bool,
    /// Pending portal-screenshot worker; polled each frame in app.rs.
    pub pending_capture: Option<std::sync::mpsc::Receiver<crate::portal_capture::CaptureResult>>,
    /// When the pending grab was started, for the watchdog that gives up on a
    /// portal that never answers — without it a wedged request leaves every
    /// capture button silently dead for the rest of the session.
    pub pending_capture_since: Option<std::time::Instant>,
    /// Lazily-initialized worker thread for raster→SVG conversion.
    pub vectorize_worker: Option<crate::vectorize_worker::VectorizeWorker>,
    /// Cropped image awaiting a texture upload on the next frame (needs Context).
    pub pending_texture_refresh: Option<image::DynamicImage>,
    /// Last image draw rect (screen coords) — tools use this to map screen↔image-pixel.
    pub last_img_draw_rect: Option<Rect>,
    /// Recent capture history — persisted to data_dir JSON.
    pub recent_captures: Vec<RecentEntry>,
    /// Startup capture request from --capture CLI flag.
    pub startup_capture: Option<SnapMode>,
    /// File named on the command line, opened on the first frame.
    pub startup_file: Option<std::path::PathBuf>,
    /// The in-app region overlay, while a region capture is being framed.
    pub region_select: Option<crate::region_select::RegionSelect>,
    /// True when the in-flight full-screen grab is destined for the region
    /// overlay rather than straight onto the canvas.
    pub region_pending: bool,
    /// Canvas zoom (1.0 = fit-to-window, >1 = zoomed in). Annotations stay in
    /// base-rect coords; rendering/input apply the transform.
    pub view_zoom: f32,
    /// Canvas pan offset in screen pixels (applied on top of zoom).
    pub view_pan: Vec2,
    /// When a timed capture is scheduled, this holds (deadline, mode). The
    /// update loop checks each frame and fires once the deadline passes.
    pub capture_scheduled: Option<(std::time::Instant, SnapMode)>,
    /// Window has been minimized for capture; restore it when capture finishes.
    pub window_hidden_for_capture: bool,
    /// Window is hidden; fire the portal after this deadline (lets WM hide settle).
    pub capture_hide_pending: Option<(std::time::Instant, SnapMode)>,
    /// The grab currently in flight: (mode, whether the portal call was
    /// interactive). The error path needs both to decide whether a failure
    /// looks like the missing screenshot permission.
    pub capture_attempt: Option<(SnapMode, bool)>,
    /// The in-flight grab is a throwaway whose only job is to make GNOME show
    /// its "allow screenshots?" dialog while our window still has focus.
    pub capture_priming: bool,
    /// A priming grab has already been tried this session — only ever once, so
    /// a genuine Deny doesn't turn into a dialog loop.
    pub capture_primed: bool,
    /// Fire the priming grab after this deadline, with the real mode to run
    /// once permission is granted (lets the un-minimize + focus settle first).
    pub capture_prime_pending: Option<(std::time::Instant, SnapMode)>,

    // ── Tabs ──────────────────────────────────────────────────────────────────
    /// Label for the currently active tab.
    pub tab_label: String,
    /// Inactive tabs, stored in logical display order around `active_tab_pos`.
    /// Logical order: inactive_tabs[0..active_tab_pos], active, inactive_tabs[active_tab_pos..]
    pub inactive_tabs: Vec<Tab>,
    /// Position of the active tab in the full logical tab sequence.
    pub active_tab_pos: usize,
    /// Auto-increment counter for "Screenshot N" labels.
    pub screenshot_count: u32,
    /// Ongoing tab drag-to-reorder state.
    pub tab_drag: Option<TabDrag>,
    /// Ongoing layers-panel row drag-to-reorder state.
    pub layer_drag: Option<LayerDrag>,
    /// When Some(buf), the active tab label is being edited inline.
    pub tab_rename_buf: Option<String>,

    // ── Shortcuts editor UI state ──────────────────────────────────────────────
    pub show_shortcuts_window: bool,
    /// When Some(action), the next keypress will be recorded as that action's shortcut.
    pub shortcut_recording: Option<Action>,

    // ── Canvas size dialogs ───────────────────────────────────────────────────
    /// Open "New Canvas" dialog.
    pub new_canvas_dialog: Option<CanvasSizeDialog>,
    /// Open "Resize Canvas" dialog.
    pub resize_canvas_dialog: Option<CanvasSizeDialog>,

    // ── Stamp preview cache ────────────────────────────────────────────────────
    pub stamp_preview_texture: Option<TextureHandle>,
    pub stamp_preview_key: Option<String>,

    // ── File-dialog memory ────────────────────────────────────────────────────
    /// Directory the user last opened a file from (Open / Import Sidecar).
    pub last_open_dir: Option<PathBuf>,
    /// Directory the user last saved a file to.
    pub last_save_dir: Option<PathBuf>,

    // ── Global annotation visibility ──────────────────────────────────────────
    /// When false, all annotations are hidden (before/after comparison). Ephemeral
    /// session state — not persisted. Toggle with Ctrl+H.
    pub annotations_visible: bool,

    // ── Agent server (feature = "agent-server") ───────────────────────────────
    #[cfg(feature = "agent-server")]
    /// The running agent server, if enabled in Settings. `None` when disabled.
    pub agent_server: Option<AgentServer>,
    /// Channel the main thread drains each frame. `None` when no server is running.
    #[cfg(feature = "agent-server")]
    pub agent_op_rx: Option<tokio::sync::mpsc::Receiver<crate::agent::types::AgentOp>>,
    /// Local AI agent handle, when enabled. Holds the cancel token, scratchpad,
    /// counters, budget pool, and the JoinHandle for the agent's tokio task.
    #[cfg(feature = "local-ai")]
    pub local_ai: Option<crate::local_ai::agent::LocalAiHandle>,
    /// When true, render the local AI control panel on the right side of the
    /// window. Toggled from the toolbar / hotkey (Phase 5.6+).
    #[cfg(feature = "local-ai")]
    pub local_ai_panel_open: bool,

    // ── Session persistence ───────────────────────────────────────────────────
    /// Debounced autosave + source-blob cache state. See `session.rs`.
    pub session: crate::session::SessionManager,

    // ── App-level mode ────────────────────────────────────────────────────────
    pub app_mode: AppMode,
    pub canvas_sort: crate::canvas_sort::state::CanvasSortState,
    pub lama_cleaner: crate::lama_cleaner::LamaCleanerState,
    pub native_inpaint: crate::native_inpaint::NativeInpaintState,
}

#[derive(Default, Clone, Copy)]
pub enum MarqueeState {
    #[default]
    Idle,
    Drawing {
        start: egui::Pos2,
    },
    Moving {
        start_rect: egui::Rect,
        press_img: egui::Pos2,
    },
    Resizing {
        anchor: egui::Pos2,
    },
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            canvas: CanvasState::default(),
            current_tool: Tool::default(),
            tool_state: ToolState::default(),
            layer_rename: None,
            settings: Settings::default(),
            status_msg: None,
            toast: None,
            request_paste: false,
            pending_capture: None,
            vectorize_worker: None,
            pending_texture_refresh: None,
            last_img_draw_rect: None,
            recent_captures: Vec::new(),
            startup_capture: None,
            startup_file: None,
            region_select: None,
            pending_capture_since: None,
            region_pending: false,
            view_zoom: 1.0,
            view_pan: Vec2::ZERO,
            capture_scheduled: None,
            window_hidden_for_capture: false,
            capture_hide_pending: None,
            capture_attempt: None,
            capture_priming: false,
            capture_primed: false,
            capture_prime_pending: None,
            tab_label: String::new(),
            inactive_tabs: Vec::new(),
            active_tab_pos: 0,
            screenshot_count: 0,
            tab_drag: None,
            layer_drag: None,
            tab_rename_buf: None,
            show_shortcuts_window: false,
            shortcut_recording: None,
            new_canvas_dialog: None,
            resize_canvas_dialog: None,
            stamp_preview_texture: None,
            stamp_preview_key: None,
            last_open_dir: None,
            last_save_dir: None,
            annotations_visible: true,
            #[cfg(feature = "agent-server")]
            agent_server: None,
            #[cfg(feature = "agent-server")]
            agent_op_rx: None,
            #[cfg(feature = "local-ai")]
            local_ai: None,
            // Opt-in: the local AI panel stays closed until the user reveals it
            // from the AI / Agents menu.
            #[cfg(feature = "local-ai")]
            local_ai_panel_open: false,
            session: crate::session::SessionManager::default(),
            app_mode: AppMode::default(),
            canvas_sort: crate::canvas_sort::state::CanvasSortState::default(),
            lama_cleaner: crate::lama_cleaner::LamaCleanerState::default(),
            native_inpaint: crate::native_inpaint::NativeInpaintState::default(),
        }
    }
}

impl AppState {
    pub fn has_active_tool_action(&self) -> bool {
        matches!(
            self.tool_state,
            ToolState::Drawing { .. }
                | ToolState::DrawingPen { .. }
                | ToolState::DrawingLasso { .. }
                | ToolState::PendingCrop { .. }
                | ToolState::ResizingImage { .. }
                | ToolState::Transforming { .. }
        )
    }

    pub fn cancel_active_tool_action(&mut self) -> bool {
        if self.has_active_tool_action() {
            self.tool_state = ToolState::Idle;
            true
        } else {
            false
        }
    }

    pub fn set_current_tool(&mut self, tool: Tool) {
        if self.current_tool != tool {
            self.cancel_active_tool_action();
            self.current_tool = tool;
        }
    }

    /// View-rect: the on-screen rect where the texture actually renders after
    /// zoom/pan. `None` while no image is loaded.
    pub fn view_rect(&self) -> Option<Rect> {
        let base = self.last_img_draw_rect?;
        Some(Rect::from_min_size(
            base.min + self.view_pan,
            base.size() * self.view_zoom,
        ))
    }

    /// Project a point from image-pixel space → screen space.
    /// Annotations are stored in image pixels (0..canvas_size), so this is
    /// window-size-invariant: resize doesn't move annotations relative to image.
    pub fn project(&self, img_pos: egui::Pos2) -> egui::Pos2 {
        let Some(base) = self.last_img_draw_rect else {
            return img_pos;
        };
        let cs = self.canvas.canvas_size;
        if cs.x < f32::EPSILON || cs.y < f32::EPSILON {
            return img_pos;
        }
        let frac = egui::Vec2::new(img_pos.x / cs.x, img_pos.y / cs.y);
        base.min + frac * base.size() * self.view_zoom + self.view_pan
    }

    /// Inverse of `project`: screen → image-pixel space.
    pub fn unproject(&self, screen: egui::Pos2) -> egui::Pos2 {
        let Some(base) = self.last_img_draw_rect else {
            return screen;
        };
        let cs = self.canvas.canvas_size;
        if cs.x < f32::EPSILON || cs.y < f32::EPSILON || self.view_zoom.abs() < f32::EPSILON {
            return screen;
        }
        let d = screen - base.min - self.view_pan;
        let frac = d / (base.size() * self.view_zoom);
        egui::Pos2::new(frac.x * cs.x, frac.y * cs.y)
    }

    /// Total number of tabs (active + inactive).
    pub fn tab_count(&self) -> usize {
        self.inactive_tabs.len() + 1
    }

    /// Whether the shared file-tab bar should be rendered. Shown whenever a
    /// document is loaded or any inactive tab exists — in every app mode, so
    /// switching to a folder workspace never hides the open files.
    pub fn should_show_tab_bar(&self) -> bool {
        self.canvas.texture.is_some() || !self.inactive_tabs.is_empty()
    }

    /// Label for the tab at logical position `i`.
    pub fn tab_label_at(&self, i: usize) -> &str {
        if i == self.active_tab_pos {
            &self.tab_label
        } else {
            let idx = if i < self.active_tab_pos { i } else { i - 1 };
            &self.inactive_tabs[idx].label
        }
    }

    /// If a canvas is loaded, park it as a background tab and reset the active
    /// canvas to empty so the next `apply_image` call starts fresh.
    pub fn push_current_as_tab_and_reset(&mut self, new_label: String) {
        // Treat annotation-only canvases as populated too — lets tests
        // build multi-tab state without a GPU texture handle.
        if self.canvas.texture.is_some() || !self.canvas.annotations.is_empty() {
            // If the vectorize panel was open, keep it open for the new image.
            let panel_was_open = self.canvas.vectorize_panel.open;
            let old_label = std::mem::replace(&mut self.tab_label, new_label);
            let old_canvas = std::mem::replace(&mut self.canvas, CanvasState::default());
            self.inactive_tabs.insert(
                self.active_tab_pos,
                Tab {
                    label: old_label,
                    canvas: old_canvas,
                    view_zoom: self.view_zoom,
                    view_pan: self.view_pan,
                },
            );
            self.active_tab_pos = self.inactive_tabs.len();
            self.view_zoom = 1.0;
            self.view_pan = Vec2::ZERO;
            self.tool_state = ToolState::default();
            self.last_img_draw_rect = None;
            if panel_was_open {
                self.canvas.vectorize_panel.open = true;
            }
        } else {
            self.tab_label = new_label;
            self.view_zoom = 1.0;
            self.view_pan = Vec2::ZERO;
            self.tool_state = ToolState::default();
            self.last_img_draw_rect = None;
        }
    }

    /// Switch the active canvas to logical tab index `n`.
    pub fn switch_to_tab(&mut self, n: usize) {
        if n == self.active_tab_pos {
            return;
        }
        let inactive_idx = if n < self.active_tab_pos { n } else { n - 1 };
        let new_tab = self.inactive_tabs.remove(inactive_idx);
        let insert_at = if n < self.active_tab_pos {
            self.active_tab_pos - 1
        } else {
            self.active_tab_pos
        };
        let old_label = std::mem::replace(&mut self.tab_label, new_tab.label);
        let old_canvas = std::mem::replace(&mut self.canvas, new_tab.canvas);
        let incoming_zoom = new_tab.view_zoom;
        let incoming_pan = new_tab.view_pan;
        self.inactive_tabs.insert(
            insert_at,
            Tab {
                label: old_label,
                canvas: old_canvas,
                view_zoom: self.view_zoom,
                view_pan: self.view_pan,
            },
        );
        self.active_tab_pos = n;
        self.view_zoom = incoming_zoom;
        self.view_pan = incoming_pan;
        self.tool_state = ToolState::default();
        self.last_img_draw_rect = None;
    }

    /// Close the tab at logical index `n`. If it is the active tab, the
    /// adjacent tab (preferring the previous one) becomes active.
    pub fn close_tab(&mut self, n: usize) {
        if n == self.active_tab_pos {
            if self.inactive_tabs.is_empty() {
                // Last tab: just blank the canvas.
                self.canvas = CanvasState::default();
                self.tab_label = String::new();
                self.active_tab_pos = 0;
                self.view_zoom = 1.0;
                self.view_pan = Vec2::ZERO;
            } else if self.active_tab_pos > 0 {
                let prev = self.inactive_tabs.remove(self.active_tab_pos - 1);
                self.view_zoom = prev.view_zoom;
                self.view_pan = prev.view_pan;
                self.canvas = prev.canvas;
                self.tab_label = prev.label;
                self.active_tab_pos -= 1;
            } else {
                let next = self.inactive_tabs.remove(0);
                self.view_zoom = next.view_zoom;
                self.view_pan = next.view_pan;
                self.canvas = next.canvas;
                self.tab_label = next.label;
                // active_tab_pos stays 0
            }
        } else {
            let inactive_idx = if n < self.active_tab_pos { n } else { n - 1 };
            self.inactive_tabs.remove(inactive_idx);
            if n < self.active_tab_pos {
                self.active_tab_pos -= 1;
            }
            // Active tab stays active — keep its zoom/pan as-is.
        }
        self.tool_state = ToolState::default();
        self.last_img_draw_rect = None;
    }

    /// Reorder the tab at logical index `from` to logical index `to`.
    /// The active tab's identity is preserved; only the display order changes.
    pub fn reorder_tab(&mut self, from: usize, to: usize) {
        let count = self.tab_count();
        if from == to || from >= count || to >= count {
            return;
        }

        // Build logical ordering: None = active tab, Some(j) = self.inactive_tabs[j].
        let mut order: Vec<Option<usize>> = (0..count)
            .map(|i| {
                if i == self.active_tab_pos {
                    None
                } else {
                    let j = if i < self.active_tab_pos { i } else { i - 1 };
                    Some(j)
                }
            })
            .collect();

        let item = order.remove(from);
        order.insert(to, item);

        let new_active_pos = order.iter().position(|x| x.is_none()).unwrap();
        let new_inactive_indices: Vec<usize> = order.iter().filter_map(|x| *x).collect();

        let n = self.inactive_tabs.len();
        let mut taken: Vec<Option<Tab>> = self.inactive_tabs.drain(..).map(Some).collect();
        let mut reordered = Vec::with_capacity(n);
        for &old_j in &new_inactive_indices {
            reordered.push(
                taken[old_j]
                    .take()
                    .expect("each old index visited exactly once in a permutation"),
            );
        }
        self.inactive_tabs = reordered;
        self.active_tab_pos = new_active_pos;
    }

    /// Convert a screen-space drag delta to image-pixel-space delta.
    pub fn screen_delta_to_img(&self, delta: egui::Vec2) -> egui::Vec2 {
        let Some(base) = self.last_img_draw_rect else {
            return delta;
        };
        let cs = self.canvas.canvas_size;
        if cs.x < f32::EPSILON || self.view_zoom.abs() < f32::EPSILON {
            return delta;
        }
        let scale = base.width() / cs.x * self.view_zoom;
        delta / scale
    }
}

/// One entry in the recent-captures list.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RecentEntry {
    pub image_path: PathBuf,
    pub sidecar_path: Option<PathBuf>,
    pub captured_at: String,
    pub width: u32,
    pub height: u32,
}

fn recent_file_path() -> Option<PathBuf> {
    let mut p = dirs_next::data_dir()?;
    p.push("redline");
    std::fs::create_dir_all(&p).ok()?;
    p.push("recent.json");
    Some(p)
}

pub fn load_recent_captures() -> Vec<RecentEntry> {
    let Some(p) = recent_file_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&p) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save_recent_captures(entries: &[RecentEntry]) {
    let Some(p) = recent_file_path() else {
        return;
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(entries) {
        let _ = std::fs::write(p, bytes);
    }
}

/// Insert a new capture at the top, dedupe by path, cap to 20.
pub fn push_recent_capture(entries: &mut Vec<RecentEntry>, entry: RecentEntry) {
    entries.retain(|e| e.image_path != entry.image_path);
    entries.insert(0, entry);
    entries.truncate(20);
    save_recent_captures(entries);
}

/// Persisted last-used directories for Open/Save file dialogs.
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct LastDirs {
    pub open: Option<PathBuf>,
    pub save: Option<PathBuf>,
}

fn last_dirs_file_path() -> Option<PathBuf> {
    let mut p = dirs_next::data_dir()?;
    p.push("redline");
    std::fs::create_dir_all(&p).ok()?;
    p.push("last_dirs.json");
    Some(p)
}

pub fn load_last_dirs() -> LastDirs {
    let Some(p) = last_dirs_file_path() else {
        return LastDirs::default();
    };
    let Ok(bytes) = std::fs::read(&p) else {
        return LastDirs::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_last_dirs(dirs: &LastDirs) {
    let Some(p) = last_dirs_file_path() else {
        return;
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(dirs) {
        let _ = std::fs::write(p, bytes);
    }
}

/// Record a directory the user just opened from. Persists to disk.
pub fn remember_open_dir(state: &mut AppState, picked: &std::path::Path) {
    let Some(dir) = picked.parent().map(|p| p.to_path_buf()) else {
        return;
    };
    state.last_open_dir = Some(dir.clone());
    save_last_dirs(&LastDirs {
        open: Some(dir),
        save: state.last_save_dir.clone(),
    });
}

/// Record a directory the user just saved to. Persists to disk.
pub fn remember_save_dir(state: &mut AppState, picked: &std::path::Path) {
    let Some(dir) = picked.parent().map(|p| p.to_path_buf()) else {
        return;
    };
    state.last_save_dir = Some(dir.clone());
    save_last_dirs(&LastDirs {
        open: state.last_open_dir.clone(),
        save: Some(dir),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_mode_eq() {
        assert_eq!(SnapMode::Full, SnapMode::Full);
        assert_ne!(SnapMode::Full, SnapMode::Region);
    }

    #[test]
    fn freehand_fit_tol_grows_with_smoothing() {
        let on = Settings {
            pen_smoothing: true,
            ..Settings::default()
        };
        let off = Settings {
            pen_smoothing: false,
            ..Settings::default()
        };
        assert!(freehand_fit_tol(&off) < freehand_fit_tol(&on));
    }

    #[test]
    fn shape_fill_stroke_defaults() {
        let s = Settings::default();
        assert!(
            !s.shape_fill_enabled,
            "fill starts disabled — plain rect/oval is the default look"
        );
        assert!(
            s.shape_stroke_enabled,
            "stroke starts enabled — matches old Rect/Oval's always-stroked look"
        );
        assert_eq!(
            s.corner_radius, 0.0,
            "unified Rect defaults to square corners, not the old RoundedRect-only 8.0 default"
        );
    }

    /// Security: the MCP agent server must be strictly opt-in so no local process
    /// can drive the canvas the moment the app launches.
    #[cfg(feature = "agent-server")]
    #[test]
    fn agent_server_disabled_by_default() {
        assert!(!Settings::default().agent_server_enabled);
    }

    /// The AI / Agents toolbar section starts collapsed.
    #[cfg(any(feature = "agent-server", feature = "local-ai"))]
    #[test]
    fn ai_section_collapsed_by_default() {
        assert!(!Settings::default().ai_section_expanded);
    }

    /// The local AI side panel must not auto-open.
    #[cfg(feature = "local-ai")]
    #[test]
    fn local_ai_panel_closed_by_default() {
        assert!(!AppState::default().local_ai_panel_open);
    }

    #[test]
    fn canvas_uid_is_unique_and_monotonic() {
        let a = next_canvas_uid();
        let b = next_canvas_uid();
        assert!(b > a, "uids strictly increase");
    }

    #[test]
    fn fresh_canvas_has_zero_uid() {
        let c = CanvasState::default();
        assert_eq!(
            c.canvas_uid, 0,
            "a fresh canvas has no document identity yet"
        );
    }

    #[test]
    fn assign_new_uid_sets_a_nonzero_unique_value() {
        let mut c = CanvasState::default();
        c.assign_new_uid();
        let first = c.canvas_uid;
        assert_ne!(first, 0);
        c.assign_new_uid();
        assert!(c.canvas_uid > first, "reassigning advances the uid");
    }

    #[cfg(feature = "local-ai")]
    #[test]
    fn canvas_state_agent_memory_roundtrips_through_sidecar() {
        use crate::export::{build_sidecar, import_sidecar};
        use crate::local_ai::memory::{MemoryEntry, MemoryKind};
        use std::time::SystemTime;

        let mut canvas = CanvasState::default();
        canvas
            .agent_memory
            .entry("local-ai".into())
            .or_default()
            .push(MemoryEntry {
                ts: SystemTime::UNIX_EPOCH,
                kind: MemoryKind::Decision,
                content: "noticed three boxes".into(),
            });

        let sidecar = build_sidecar(&canvas);
        let json = serde_json::to_string(&sidecar).expect("serialize");
        let back: crate::types::SidecarExport = serde_json::from_str(&json).expect("deserialize");

        // Write the deserialized sidecar to a tempfile and import it.
        let path = std::env::temp_dir().join(format!("redline-test-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&back).unwrap()).unwrap();
        let mut restored = CanvasState::default();
        import_sidecar(&path, &mut restored).expect("import");
        let _ = std::fs::remove_file(&path);

        let entries = restored
            .agent_memory
            .get("local-ai")
            .expect("agent_memory key present");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "noticed three boxes");
        assert_eq!(entries[0].kind, MemoryKind::Decision);
    }
}

#[cfg(test)]
mod doc_mirror_tests {
    use super::*;
    use crate::types::{Annotation, ShapeData};
    use egui::{Color32, Rect};

    #[test]
    fn add_annotation_updates_both_document_and_mirror() {
        let mut c = CanvasState::default();
        assert!(c.annotations.is_empty());
        let a = Annotation::new(ShapeData::plain_rect(Rect::ZERO), Color32::RED, 2.0);
        let id = a.id;
        c.add_annotation(a);
        // Mirror reflects it.
        assert_eq!(c.annotations.len(), 1);
        assert_eq!(c.annotations[0].id, id);
        // Document is authoritative and holds it under the default layer.
        assert!(c.document.leaf(id).is_some());
    }

    #[test]
    fn add_annotation_targets_active_layer() {
        let mut c = CanvasState::default();
        let l2 = c.document.create_layer("Layer 2");
        c.sync_annotations();
        c.active_layer = l2;
        let a = Annotation::new(ShapeData::plain_rect(Rect::ZERO), Color32::RED, 2.0);
        let id = a.id;
        c.add_annotation(a);
        assert_eq!(c.document.get(id).unwrap().parent, Some(l2));
    }

    #[test]
    fn active_layer_defaults_to_default_layer() {
        let c = CanvasState::default();
        assert_eq!(c.active_layer, c.document.default_layer());
    }

    #[test]
    fn sync_annotations_rebuilds_mirror_from_document() {
        let mut c = CanvasState::default();
        let a = Annotation::new(ShapeData::plain_rect(Rect::ZERO), Color32::RED, 2.0);
        let id = a.id;
        c.add_annotation(a);
        // Corrupt the mirror, then resync — it must be rebuilt from the document.
        c.annotations.clear();
        c.sync_annotations();
        assert_eq!(c.annotations.len(), 1);
        assert_eq!(c.annotations[0].id, id);
    }
}

#[cfg(test)]
mod settings_tests {
    use super::Settings;

    #[test]
    fn layers_panel_shown_by_default() {
        let s = Settings::default();
        assert!(s.show_layers, "layers panel should be visible by default");
    }
}

#[cfg(test)]
mod toast_tests {
    use super::Toast;

    #[test]
    fn info_holds_then_fades_to_zero() {
        let t = Toast::new("Copied to clipboard".into());
        assert!(!t.is_error);
        assert_eq!(t.opacity_at(0.0), 1.0);
        assert_eq!(t.opacity_at(3.0), 1.0);
        assert!(t.opacity_at(3.6) > 0.0 && t.opacity_at(3.6) < 1.0);
        assert_eq!(t.opacity_at(60.0), 0.0);
    }

    #[test]
    fn errors_are_detected_and_held_longer() {
        let e = Toast::new("Capture failed: portal closed".into());
        assert!(e.is_error);
        assert_eq!(e.opacity_at(5.0), 1.0, "errors must survive the info hold");
        assert_eq!(e.opacity_at(30.0), 0.0);
    }

    #[test]
    fn sync_starts_replaces_and_clears() {
        let mut msg = Some("Saved".to_string());
        let mut toast = None;
        assert!(Toast::sync(&mut msg, &mut toast));
        let first = toast.as_ref().unwrap().shown_at;

        // Same message on the next frame: keep the original timer running.
        assert!(Toast::sync(&mut msg, &mut toast));
        assert_eq!(toast.as_ref().unwrap().shown_at, first);

        // New message: restart.
        msg = Some("Saved as foo.png".to_string());
        assert!(Toast::sync(&mut msg, &mut toast));
        assert_eq!(toast.as_ref().unwrap().text, "Saved as foo.png");

        // Cleared source: toast goes away.
        msg = None;
        assert!(!Toast::sync(&mut msg, &mut toast));
        assert!(toast.is_none());
    }
}
