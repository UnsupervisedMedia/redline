use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Instant;

use egui::{Pos2, Rect, TextureHandle, Vec2};
use uuid::Uuid;

use super::types::{
    CanvasSortSession, CanvasTool, SessionMeta, StickyNote, ThumbRequest, ThumbResult,
};
use super::undo::CanvasCommand;

pub const ZOOM_MIN: f32 = 0.02;
pub const ZOOM_MAX: f32 = 10.0;
pub const ZOOM_STEP: f32 = 0.10;
pub const VIRT_THRESHOLD: usize = 100;
pub const VIRT_BUFFER: f32 = 2.5;
#[allow(dead_code)]
pub const MAX_UNDO: usize = 50;
pub const AUTO_SAVE_SECS: u64 = 2;

// ── Tool interaction state ────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ToolInteraction {
    Idle,
    Marquee {
        start: Pos2,
        current: Pos2,
    },
    DraggingImages {
        drag_start_canvas: Pos2,
        originals: Vec<(Uuid, [f32; 2])>,
    },
    DraggingZone {
        zone_id: Uuid,
        drag_start_canvas: Pos2,
        original_rect: Rect,
    },
    ResizingZone {
        zone_id: Uuid,
        handle: u8,
        drag_start_canvas: Pos2,
        original_rect: Rect,
    },
    DraggingStack {
        stack_id: Uuid,
        drag_start_canvas: Pos2,
        original_pos: [f32; 2],
    },
    Drawing {
        start_canvas: Pos2,
        current_canvas: Pos2,
    },
    EditingNote {
        note_id: Uuid,
    },
    MovingNote {
        note_id: Uuid,
        drag_start_canvas: Pos2,
        original_rect: Rect,
    },
    ResizingNote {
        note_id: Uuid,
        handle: u8,
        drag_start_canvas: Pos2,
        original_rect: Rect,
    },
}

impl Default for ToolInteraction {
    fn default() -> Self {
        ToolInteraction::Idle
    }
}

// ── History entry ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub label: String,
}

// ── Arrange dialog ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArrangeDialog {
    pub fixed_columns: bool,
    pub count: usize,
    pub spacing: f32,
    pub selected_only: bool,
}

impl Default for ArrangeDialog {
    fn default() -> Self {
        Self {
            fixed_columns: true,
            count: 5,
            spacing: 20.0,
            selected_only: false,
        }
    }
}

// ── Zone-action picker state ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ZonePicker {
    pub pending: super::types::ZoneAction,
    pub target_path_edit: String,
    pub tags_edit: String,
    pub resize_w: String,
    pub resize_h: String,
}

impl ZonePicker {
    pub fn new(action: super::types::ZoneAction) -> Self {
        let (target_path_edit, tags_edit, resize_w, resize_h) = match &action {
            super::types::ZoneAction::MoveToFolder { target }
            | super::types::ZoneAction::CopyToFolder { target } => (
                target.to_string_lossy().into_owned(),
                String::new(),
                String::new(),
                String::new(),
            ),
            super::types::ZoneAction::AddTags { tags } => {
                (String::new(), tags.join(", "), String::new(), String::new())
            }
            super::types::ZoneAction::Resize { width, height } => (
                String::new(),
                String::new(),
                width.to_string(),
                height.to_string(),
            ),
            _ => Default::default(),
        };
        Self {
            pending: action,
            target_path_edit,
            tags_edit,
            resize_w,
            resize_h,
        }
    }
}

// ── Main state ────────────────────────────────────────────────────────────────

pub struct CanvasSortState {
    // Persisted
    pub session: Option<CanvasSortSession>,

    // Viewport
    pub zoom: f32,
    /// Top-left corner of viewport in canvas space.
    pub pan: Vec2,
    /// Set each frame from ui.available_rect_before_wrap().
    pub panel_rect: Rect,

    // Selection
    pub selected_images: HashSet<Uuid>,
    pub selected_zones: HashSet<Uuid>,
    pub selected_stacks: HashSet<Uuid>,

    // Sticky notes (persisted via sidecar) + runtime note selection/editor state
    pub sticky_notes: Vec<StickyNote>,
    /// Currently selected sticky note (separate from image/zone/stack selection).
    pub selected_note: Option<Uuid>,
    /// One-shot: request keyboard focus for the note editor on its next frame.
    pub note_editor_focus: bool,

    // Textures
    pub textures: std::collections::HashMap<Uuid, TextureHandle>,
    pub thumb_pending: HashSet<Uuid>,
    pub thumb_failed: HashSet<Uuid>,
    pub thumb_tx: Option<mpsc::SyncSender<ThumbRequest>>,
    pub thumb_rx: Option<mpsc::Receiver<ThumbResult>>,

    // Tools
    pub tool: CanvasTool,
    pub tool_state: ToolInteraction,

    // Undo / redo
    pub undo_stack: Vec<CanvasCommand>,
    pub redo_stack: Vec<CanvasCommand>,
    pub history_log: Vec<HistoryEntry>,

    // Panels visibility
    pub show_minimap: bool,
    pub show_search: bool,
    pub show_history: bool,
    pub show_stack_panel: bool,

    // Search
    pub search_query: String,

    // Entropy
    pub entropy_enabled: bool,
    pub entropy_frequency_mins: f32,
    pub entropy_intensity_pct: f32,
    pub entropy_last_fired: Option<Instant>,

    // Session management
    pub session_list: Vec<SessionMeta>,
    pub show_session_list: bool,
    pub new_session_name: String,
    pub new_session_folder: String,

    // Virtualization
    pub virtualization: bool,

    // Status bar message
    pub status: Option<String>,

    // Sticky-collect mode: images being collected
    pub collecting: Vec<Uuid>,

    // Arrange dialog
    pub arrange_dialog: Option<ArrangeDialog>,

    // Zone picker (shown when drawing a new zone)
    pub zone_picker: Option<ZonePicker>,

    // Zone config dialog
    pub zone_edit_id: Option<Uuid>,
    /// Working copy of the zone being edited — persists across frames.
    pub zone_edit_draft: Option<super::types::ActionZone>,
    /// Last folder chosen via Browse in the zone config dialog.
    pub zone_last_folder: Option<std::path::PathBuf>,

    // Right-click canvas position captured on secondary click, used by context menu
    pub right_click_canvas_pos: Option<Pos2>,

    // Auto-save
    pub last_save: Option<Instant>,
    pub dirty: bool,

    // Folder-load status
    pub loading_folder: bool,
    pub load_progress: usize,
    pub load_total: usize,
}

impl Default for CanvasSortState {
    fn default() -> Self {
        Self {
            session: None,
            zoom: 0.05,
            pan: Vec2::ZERO,
            panel_rect: Rect::NOTHING,
            selected_images: HashSet::new(),
            selected_zones: HashSet::new(),
            selected_stacks: HashSet::new(),
            sticky_notes: Vec::new(),
            selected_note: None,
            note_editor_focus: false,
            textures: std::collections::HashMap::new(),
            thumb_pending: HashSet::new(),
            thumb_failed: HashSet::new(),
            thumb_tx: None,
            thumb_rx: None,
            tool: CanvasTool::Select,
            tool_state: ToolInteraction::Idle,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            history_log: Vec::new(),
            show_minimap: true,
            show_search: false,
            show_history: false,
            show_stack_panel: false,
            search_query: String::new(),
            entropy_enabled: false,
            entropy_frequency_mins: 30.0,
            entropy_intensity_pct: 50.0,
            entropy_last_fired: None,
            session_list: Vec::new(),
            show_session_list: true,
            new_session_name: String::new(),
            new_session_folder: String::new(),
            virtualization: false,
            status: None,
            collecting: Vec::new(),
            arrange_dialog: None,
            zone_picker: None,
            zone_edit_id: None,
            zone_edit_draft: None,
            zone_last_folder: None,
            right_click_canvas_pos: None,
            last_save: None,
            dirty: false,
            loading_folder: false,
            load_progress: 0,
            load_total: 0,
        }
    }
}

impl CanvasSortState {
    // ── Coordinate transforms ─────────────────────────────────────────────

    pub fn to_screen(&self, p: Pos2) -> Pos2 {
        self.panel_rect.min + (p.to_vec2() - self.pan) * self.zoom
    }

    pub fn to_canvas(&self, p: Pos2) -> Pos2 {
        ((p - self.panel_rect.min) / self.zoom + self.pan).to_pos2()
    }

    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }

    pub fn rect_to_canvas(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_canvas(r.min), self.to_canvas(r.max))
    }

    /// Canvas-space rect currently visible in the panel.
    pub fn visible_canvas_rect(&self) -> Rect {
        self.rect_to_canvas(self.panel_rect)
    }

    // ── Zoom helpers ──────────────────────────────────────────────────────

    pub fn zoom_at(&mut self, new_zoom: f32, screen_pt: Pos2) {
        let canvas_pt = self.to_canvas(screen_pt);
        self.zoom = new_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.pan = canvas_pt.to_vec2() - (screen_pt - self.panel_rect.min) / self.zoom;
    }

    pub fn zoom_in_at(&mut self, pt: Pos2) {
        let z = (self.zoom * (1.0 + ZOOM_STEP)).min(ZOOM_MAX);
        self.zoom_at(z, pt);
    }

    pub fn zoom_out_at(&mut self, pt: Pos2) {
        let z = (self.zoom / (1.0 + ZOOM_STEP)).max(ZOOM_MIN);
        self.zoom_at(z, pt);
    }

    pub fn zoom_to_fit(&mut self) {
        let Some(bounds) = self.session.as_ref().and_then(|s| s.images_bounds()) else {
            return;
        };
        let padded = bounds.expand(bounds.size().max_elem() * 0.1 + 100.0);
        let panel = self.panel_rect;
        if panel.width() <= 0.0 || panel.height() <= 0.0 {
            return;
        }
        let zoom_x = panel.width() / padded.width();
        let zoom_y = panel.height() / padded.height();
        self.zoom = zoom_x.min(zoom_y).clamp(ZOOM_MIN, ZOOM_MAX);
        // Pan so padded center lands at panel center
        let canvas_center = padded.center().to_vec2();
        let screen_center = panel.center();
        self.pan = canvas_center - (screen_center - panel.min) / self.zoom;
    }

    pub fn center_view(&mut self) {
        self.pan = Vec2::ZERO;
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
    }

    // ── Thumbnail management ─────────────────────────────────────────────

    pub fn ensure_thumb_worker(&mut self) {
        if self.thumb_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::sync_channel::<ThumbRequest>(128);
        let (res_tx, res_rx) = mpsc::sync_channel::<ThumbResult>(128);
        std::thread::Builder::new()
            .name("thumb-worker".into())
            .spawn(move || super::thumbnail::run_worker(req_rx, res_tx))
            .expect("spawn thumb worker");
        self.thumb_tx = Some(req_tx);
        self.thumb_rx = Some(res_rx);
    }

    pub fn request_thumbnail(&mut self, id: Uuid, path: &std::path::Path, size: u32) {
        if self.textures.contains_key(&id)
            || self.thumb_pending.contains(&id)
            || self.thumb_failed.contains(&id)
        {
            return;
        }
        if let Some(tx) = &self.thumb_tx {
            if tx
                .try_send(ThumbRequest {
                    image_id: id,
                    path: path.to_owned(),
                    size,
                })
                .is_ok()
            {
                self.thumb_pending.insert(id);
            }
        }
    }

    pub fn drain_thumbnails(&mut self, ctx: &egui::Context) {
        let rx = match self.thumb_rx.take() {
            Some(r) => r,
            None => return,
        };
        let mut any = false;
        while let Ok(res) = rx.try_recv() {
            self.thumb_pending.remove(&res.image_id);
            match res.rgba {
                None => {
                    // Image failed to decode — don't request again.
                    self.thumb_failed.insert(res.image_id);
                }
                Some(rgba) => {
                    let (w, h) = (rgba.width(), rgba.height());
                    // Update canvas size to preserve aspect ratio (max 200 in either dim).
                    let max_dim = 200.0_f32;
                    let (display_w, display_h) = if w >= h {
                        (max_dim, max_dim * h as f32 / w as f32)
                    } else {
                        (max_dim * w as f32 / h as f32, max_dim)
                    };
                    if let Some(s) = &mut self.session {
                        if let Some(img) = s.image_by_id_mut(res.image_id) {
                            img.size = [display_w, display_h];
                        }
                    }
                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    let handle = crate::texture_upload::load_texture_fitted(
                        ctx,
                        format!("cs_thumb_{}", res.image_id),
                        ci,
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(res.image_id, handle);
                    any = true;
                }
            }
        }
        self.thumb_rx = Some(rx);
        if any {
            ctx.request_repaint();
        }
    }

    // ── Command execution ─────────────────────────────────────────────────

    pub fn execute(&mut self, cmd: CanvasCommand) {
        let label = cmd.label().to_string();
        super::undo::apply_command(
            &mut self.session,
            cmd,
            &mut self.undo_stack,
            &mut self.redo_stack,
        );
        self.history_log.push(HistoryEntry { label });
        if self.history_log.len() > 100 {
            self.history_log.remove(0);
        }
        self.dirty = true;
    }

    pub fn undo(&mut self) {
        if !self.undo_stack.is_empty() {
            let label = self
                .undo_stack
                .last()
                .map(|c| c.label())
                .unwrap_or("")
                .to_string();
            super::undo::undo_command(
                &mut self.session,
                &mut self.undo_stack,
                &mut self.redo_stack,
            );
            self.status = Some(format!("Undo: {label}"));
            self.dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if !self.redo_stack.is_empty() {
            let label = self
                .redo_stack
                .last()
                .map(|c| c.label())
                .unwrap_or("")
                .to_string();
            super::undo::redo_command(
                &mut self.session,
                &mut self.undo_stack,
                &mut self.redo_stack,
            );
            self.status = Some(format!("Redo: {label}"));
            self.dirty = true;
        }
    }

    // ── Selection helpers ─────────────────────────────────────────────────

    pub fn clear_selection(&mut self) {
        self.selected_images.clear();
        self.selected_zones.clear();
        self.selected_stacks.clear();
        self.selected_note = None;
    }

    /// Remove the currently selected sticky note, if any. Returns true if a
    /// note was deleted. Notes are outside the undo system, so this marks the
    /// session dirty directly for auto-save.
    pub fn delete_selected_note(&mut self) -> bool {
        if let Some(id) = self.selected_note.take() {
            self.sticky_notes.retain(|n| n.id != id);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn select_all_images(&mut self) {
        if let Some(s) = &self.session {
            self.selected_images = s.images.iter().map(|i| i.id).collect();
        }
    }

    /// Remove images under `cursor_canvas` from canvas (not disk).
    pub fn delete_selected(&mut self) {
        let ids = self.selected_images.drain().collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let images = ids
            .iter()
            .filter_map(|id| self.session.as_ref()?.image_by_id(*id).cloned())
            .collect::<Vec<_>>();
        self.execute(super::undo::CanvasCommand::RemoveImages { images });
        self.selected_images.clear();
    }

    // ── Auto-save ─────────────────────────────────────────────────────────

    pub fn maybe_auto_save(&mut self) {
        if !self.dirty {
            return;
        }
        let elapsed = self
            .last_save
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(u64::MAX);
        if elapsed >= AUTO_SAVE_SECS {
            if super::persistence::save_session(self).is_ok() {
                self.last_save = Some(Instant::now());
                self.dirty = false;
            }
        }
    }

    // ── Entropy mode ──────────────────────────────────────────────────────

    pub fn maybe_fire_entropy(&mut self) {
        if !self.entropy_enabled {
            return;
        }
        let freq_secs = (self.entropy_frequency_mins * 60.0) as u64;
        let elapsed = self
            .entropy_last_fired
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(u64::MAX);
        if elapsed < freq_secs {
            return;
        }
        self.entropy_last_fired = Some(Instant::now());
        let intensity = (self.entropy_intensity_pct / 100.0).clamp(0.0, 1.0);
        let moves: Vec<(Uuid, [f32; 2], [f32; 2])> = if let Some(s) = &self.session {
            let loose = s.loose_image_ids();
            let n = ((loose.len() as f32 * intensity) as usize)
                .max(1)
                .min(loose.len());
            loose
                .into_iter()
                .take(n)
                .filter_map(|id| {
                    let img = s.image_by_id(id)?;
                    let offset_x = (rand_offset() * 500.0) - 250.0;
                    let offset_y = (rand_offset() * 500.0) - 250.0;
                    let new_pos = [img.pos[0] + offset_x, img.pos[1] + offset_y];
                    Some((id, img.pos, new_pos))
                })
                .collect()
        } else {
            Vec::new()
        };
        if !moves.is_empty() {
            self.execute(CanvasCommand::MoveImages { moves });
        }
    }

    pub fn should_virtualize(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| self.virtualization && s.images.len() > VIRT_THRESHOLD)
            .unwrap_or(false)
    }
}

/// Simple deterministic-ish float in [0,1] from system time nanoseconds.
fn rand_offset() -> f32 {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Mix bits
    let v = ns
        .wrapping_mul(2654435761)
        .wrapping_add(ns.wrapping_shr(13));
    (v & 0xFFFF) as f32 / 65535.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_sort::types::{CanvasSortSession, SortImage};

    fn state_with_images(n: usize) -> CanvasSortState {
        let mut st = CanvasSortState::default();
        let mut session = CanvasSortSession::new("t");
        for k in 0..n {
            let mut i = SortImage::new(std::path::PathBuf::from("x.png"), [0u8; 32]);
            i.pos = [k as f32 * 10.0, 0.0];
            session.images.push(i);
        }
        st.session = Some(session);
        st
    }

    #[test]
    fn entropy_fire_is_undoable_and_marks_dirty() {
        let mut st = state_with_images(4);
        st.entropy_enabled = true;
        st.entropy_frequency_mins = 0.0; // fire immediately
        st.entropy_last_fired = None;
        st.dirty = false;
        st.undo_stack.clear();

        st.maybe_fire_entropy();

        assert!(
            !st.undo_stack.is_empty(),
            "entropy should push an undoable command"
        );
        assert!(st.dirty, "entropy should mark the session dirty");
    }

    #[test]
    fn clear_selection_also_clears_selected_note() {
        let mut st = CanvasSortState::default();
        st.selected_note = Some(Uuid::new_v4());
        st.clear_selection();
        assert!(st.selected_note.is_none());
    }

    #[test]
    fn delete_selected_note_removes_and_reports() {
        use crate::canvas_sort::types::StickyNote;
        let mut st = CanvasSortState::default();
        let note = StickyNote::new(Rect::from_min_max(Pos2::ZERO, Pos2::new(60.0, 40.0)));
        let id = note.id;
        st.sticky_notes.push(note);
        st.selected_note = Some(id);
        st.dirty = false;

        assert!(
            st.delete_selected_note(),
            "should report a deletion happened"
        );
        assert!(st.sticky_notes.is_empty(), "note should be removed");
        assert!(st.selected_note.is_none(), "selection should clear");
        assert!(st.dirty, "deletion should mark dirty");
        assert!(
            !st.delete_selected_note(),
            "second call has nothing to delete"
        );
    }
}
