use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use eframe::egui;

use super::job::NativeMsg;

/// Outcome of comparing the panel's synced binding against the current canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    /// The active document changed (or first bind): clear per-image state.
    Reset,
    /// Same document but content changed: re-flatten the working composite.
    Reflatten,
    /// Nothing changed.
    NoChange,
}

/// Decide how the Native Inpaint panel should re-sync to the canvas, given the
/// last-synced `(uid, rev)` and the current `(uid, rev)`.
pub fn decide_sync(
    synced_uid: Option<u64>,
    synced_rev: (usize, u64),
    cur_uid: u64,
    cur_rev: (usize, u64),
) -> SyncAction {
    if synced_uid != Some(cur_uid) {
        SyncAction::Reset
    } else if synced_rev != cur_rev {
        SyncAction::Reflatten
    } else {
        SyncAction::NoChange
    }
}

/// Which masking tool is active in the panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MaskTool {
    /// Freehand disc brush.
    Brush,
    /// Magic wand: flood-select a connected same-colour region on click.
    Wand,
}

pub struct NativeInpaintState {
    pub tx: mpsc::Sender<NativeMsg>,
    pub rx: mpsc::Receiver<NativeMsg>,
    pub last_error: Option<String>,
    /// Panel-visible, non-error confirmation (e.g. "result committed").
    pub last_status: Option<String>,
    /// True while a worker is running.
    pub running: bool,
    /// Download progress 0..=1 (None when not downloading).
    pub progress: Option<f32>,
    /// Inference progress 0..=1 (None when not inferring).
    pub infer_progress: Option<f32>,
    /// Current stage label shown during a removal.
    pub stage: Option<String>,
    /// Compute device the backend reported ("GPU (CUDA)" / "CPU").
    pub device: Option<String>,
    /// Most recent finished result, ready for the UI to commit as a layer.
    pub result: Option<image::RgbaImage>,
    /// The flattened canvas composite (base image + annotations/shapes + prior
    /// inpaint result layers) that the panel shows and feeds to the next
    /// removal. Re-derived whenever the active document or its content changes.
    pub flattened: Option<image::RgbaImage>,
    /// Bumped whenever `flattened` changes, so the UI can cache the base texture.
    pub flattened_rev: u64,
    /// The mask the user is painting (lazily created to match the source size).
    pub mask: Option<super::mask::MaskBuffer>,
    /// Brush radius in image pixels.
    pub brush_radius: f32,
    /// True = paint, false = erase.
    pub paint_mode: bool,
    /// Active masking tool.
    pub tool: MaskTool,
    /// Magic-wand colour tolerance (0..=255).
    pub wand_tolerance: u8,
    /// When true, releasing a brush stroke auto-runs the current action
    /// (Remove / Generate). Off by default.
    pub auto_apply: bool,
    /// Set by the panel's Save button; the bin's app loop honors it by calling
    /// the (bin-only) toolbar save, then clears it.
    pub request_save: bool,
    /// Last brush point (image coords) for stroke interpolation; `None` between strokes.
    pub last_paint: Option<(f32, f32)>,
    /// View zoom (1.0 = fit-to-window) and pan offset (screen px).
    pub zoom: f32,
    pub pan: egui::Vec2,
    /// The canvas `canvas_uid` the mask/flattened image are bound to; when the
    /// active document changes, this no longer matches and we reset (avoids
    /// editing/showing a stale image). `None` until first synced.
    pub synced_uid: Option<u64>,
    /// The canvas `current_rev()` last reflected by `flattened`; a change means
    /// the document content moved and we re-flatten.
    pub synced_rev: (usize, u64),
    /// The `canvas_uid` the running job was started against — its result is only
    /// committed if the canvas still shows that document.
    pub job_uid: Option<u64>,
    /// Cancel flag for the in-flight worker (replaced per job).
    pub cancel: Arc<AtomicBool>,
    /// True once the user has asked to cancel the current job (suppresses commit).
    pub cancel_requested: bool,
    /// Cached, already-loaded backend (model + session) reused across removals
    /// so each Remove click doesn't reload the ~200MB model / re-register the EP.
    pub backend: Option<Arc<dyn super::backend::InpaintBackend + Send + Sync>>,
    /// The model id the cached `backend` was loaded for; a change forces a reload.
    pub backend_model_id: Option<String>,
    /// Selected model id (manifest entry).
    pub model_id: String,
    /// Generative prompt (used when the selected model is generative).
    pub gen_prompt: String,
    /// Generative negative prompt.
    pub gen_negative: String,
    /// Generative sampling steps.
    pub gen_steps: u32,
    /// Generative denoising strength (0..=1).
    pub gen_strength: f32,
    /// Generative classifier-free guidance scale (CFG).
    pub gen_guidance: f32,
    /// Generative RNG seed.
    pub gen_seed: u64,
    /// Selected scheduler id.
    pub gen_scheduler: String,
    /// Scheduler ids offered in the dropdown (refreshed when a backend loads).
    pub gen_schedulers: Vec<String>,
    /// Cached base texture + the token (flattened_rev) it reflects.
    pub base_tex: Option<egui::TextureHandle>,
    pub base_tex_token: u64,
    /// Cached mask-overlay texture + the mask version it reflects.
    pub mask_tex: Option<egui::TextureHandle>,
    pub mask_tex_version: u64,
    /// Unified undo/redo history for this panel (mask edits + applied results).
    pub history: super::history::InpaintHistory,
    /// Snapshot of the mask taken at the start of the current brush stroke; used
    /// to build one history patch when the stroke is released. `None` between
    /// strokes.
    pub stroke_before: Option<image::GrayImage>,
}

impl Default for NativeInpaintState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            last_error: None,
            last_status: None,
            running: false,
            progress: None,
            infer_progress: None,
            stage: None,
            device: None,
            result: None,
            flattened: None,
            flattened_rev: 0,
            mask: None,
            brush_radius: 24.0,
            paint_mode: true,
            tool: MaskTool::Brush,
            wand_tolerance: 24,
            auto_apply: false,
            request_save: false,
            last_paint: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            synced_uid: None,
            synced_rev: (0, 0),
            job_uid: None,
            cancel: Arc::new(AtomicBool::new(false)),
            cancel_requested: false,
            backend: None,
            backend_model_id: None,
            model_id: "lama-fp32".to_string(),
            gen_prompt: String::new(),
            gen_negative: String::new(),
            gen_steps: 25,
            gen_strength: 0.85,
            gen_guidance: 7.5,
            gen_seed: 0,
            gen_scheduler: "ddim".to_string(),
            gen_schedulers: vec!["ddim".into(), "euler-ancestral".into()],
            base_tex: None,
            base_tex_token: 0,
            mask_tex: None,
            mask_tex_version: 0,
            history: super::history::InpaintHistory::default(),
            stroke_before: None,
        }
    }
}

impl NativeInpaintState {
    pub fn drain_msgs(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                NativeMsg::DownloadProgress(p) => self.progress = Some(p),
                NativeMsg::BackendReady { backend, model_id } => {
                    self.backend = Some(backend);
                    // Label the cache with the id the worker actually LOADED, not
                    // the current picker value (which may have changed mid-job).
                    self.backend_model_id = Some(model_id);
                    #[cfg(feature = "native-generative")]
                    {
                        let list = super::sd_candle::SdCandle::available_schedulers();
                        if !list.is_empty() {
                            if !list.contains(&self.gen_scheduler) {
                                self.gen_scheduler = list[0].clone();
                            }
                            self.gen_schedulers = list;
                        }
                    }
                }
                NativeMsg::Device(d) => self.device = Some(d),
                NativeMsg::Stage(s) => self.stage = Some(s),
                NativeMsg::InferProgress(f) => self.infer_progress = Some(f),
                NativeMsg::Notice(n) => self.last_status = Some(n),
                NativeMsg::Done(res) => {
                    self.running = false;
                    self.progress = None;
                    self.infer_progress = None;
                    self.stage = None;
                    if self.cancel_requested {
                        // The user asked to cancel but the job finished first —
                        // honour the cancel and drop the result.
                        self.cancel_requested = false;
                        self.last_status = Some("Inpaint cancelled.".to_string());
                    } else {
                        self.result = Some(res.image);
                    }
                }
                NativeMsg::LoadDone => {
                    // Load-only completion: the model is resident + cached, no
                    // result to commit. Clear the in-flight flags and confirm.
                    self.running = false;
                    self.progress = None;
                    self.infer_progress = None;
                    self.stage = None;
                    self.cancel_requested = false;
                    let dev = self.device.clone().unwrap_or_else(|| "GPU".into());
                    self.last_status = Some(format!("Model loaded ({dev}) — ready."));
                }
                NativeMsg::Cancelled => {
                    self.running = false;
                    self.progress = None;
                    self.infer_progress = None;
                    self.stage = None;
                    self.result = None;
                    self.cancel_requested = false;
                    self.last_status = Some("Inpaint cancelled.".to_string());
                }
                NativeMsg::Error(e) => {
                    self.running = false;
                    self.progress = None;
                    self.infer_progress = None;
                    self.stage = None;
                    self.last_error = Some(e);
                }
            }
        }
    }

    /// Begin a cancellation request for the in-flight job.
    pub fn request_cancel(&mut self) {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel_requested = true;
            self.stage = Some("Cancelling…".to_string());
        }
    }

    /// Reset all per-image working state because the active document changed (or
    /// is being bound for the first time). Does not touch the loaded
    /// backend/device or the selected model.
    pub fn reset_for_uid(&mut self, uid: u64) {
        if self.running {
            // Abandon any in-flight work against the old document.
            self.cancel.store(true, Ordering::Relaxed);
            self.cancel_requested = true;
        }
        self.mask = None;
        self.flattened = None;
        self.flattened_rev = self.flattened_rev.wrapping_add(1);
        self.last_error = None;
        self.last_status = None;
        self.result = None;
        self.last_paint = None;
        self.stage = None;
        self.infer_progress = None;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
        self.base_tex = None;
        self.mask_tex = None;
        self.history.clear();
        self.stroke_before = None;
        self.synced_uid = Some(uid);
    }

    /// Take the finished result for committing as a layer. The committed result
    /// lands on the shared canvas; the panel re-flattens next frame to show it.
    pub fn take_committed_result(&mut self) -> Option<image::RgbaImage> {
        self.result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_inpaint::backend::InpaintResult;

    fn solid(w: u32, h: u32, v: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(w, h, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn done_then_commit_hands_back_result() {
        let mut s = NativeInpaintState::default();

        s.running = true;
        s.tx.send(NativeMsg::Done(InpaintResult {
            image: solid(4, 4, 100),
        }))
        .unwrap();
        s.drain_msgs();
        assert!(!s.running, "Done clears running");
        assert!(s.result.is_some(), "Done stores the result");

        let committed = s.take_committed_result().expect("a result to commit");
        assert_eq!(
            committed.get_pixel(0, 0),
            &image::Rgba([100, 100, 100, 255])
        );
        assert!(s.result.is_none(), "result is consumed once committed");
    }

    #[test]
    fn take_committed_result_is_none_without_a_result() {
        let mut s = NativeInpaintState::default();
        assert!(s.take_committed_result().is_none());
    }

    #[test]
    fn decide_sync_first_bind_resets() {
        assert_eq!(decide_sync(None, (0, 0), 5, (1, 1)), SyncAction::Reset);
    }

    #[test]
    fn decide_sync_uid_change_resets() {
        assert_eq!(decide_sync(Some(5), (3, 1), 6, (0, 1)), SyncAction::Reset);
    }

    #[test]
    fn decide_sync_same_uid_new_rev_reflattens() {
        assert_eq!(
            decide_sync(Some(5), (3, 1), 5, (4, 1)),
            SyncAction::Reflatten
        );
    }

    #[test]
    fn decide_sync_no_change() {
        assert_eq!(
            decide_sync(Some(5), (3, 1), 5, (3, 1)),
            SyncAction::NoChange
        );
    }

    #[test]
    fn cancelled_message_clears_state_and_drops_result() {
        let mut s = NativeInpaintState::default();
        s.running = true;
        s.cancel_requested = true;
        s.tx.send(NativeMsg::Cancelled).unwrap();
        s.drain_msgs();
        assert!(!s.running);
        assert!(s.result.is_none());
        assert!(!s.cancel_requested, "cancel request cleared on completion");
        assert_eq!(s.last_status.as_deref(), Some("Inpaint cancelled."));
    }

    #[test]
    fn done_after_cancel_request_drops_result() {
        // The job finished before it observed the cancel flag: honour the cancel.
        let mut s = NativeInpaintState::default();
        s.running = true;
        s.cancel_requested = true;
        s.tx.send(NativeMsg::Done(InpaintResult {
            image: solid(4, 4, 50),
        }))
        .unwrap();
        s.drain_msgs();
        assert!(!s.running);
        assert!(s.result.is_none(), "cancelled result must not be kept");
        assert!(!s.cancel_requested, "cancel request cleared on completion");
        assert_eq!(s.last_status.as_deref(), Some("Inpaint cancelled."));
    }

    #[test]
    fn backend_ready_labels_cache_with_loaded_model_not_picker() {
        // Regression: the worker loaded model "X", but the user switched the picker
        // to "Y" while the job ran. The cache must be labelled with the LOADED id
        // ("X") from the message payload, NOT the current picker value ("Y").
        use crate::native_inpaint::backend::{
            InpaintBackend, InpaintError, InpaintRequest, InpaintResult, RunCtx,
        };

        struct DummyBackend;
        impl InpaintBackend for DummyBackend {
            fn id(&self) -> &'static str {
                "dummy"
            }
            fn is_generative(&self) -> bool {
                false
            }
            fn run(
                &self,
                _req: InpaintRequest,
                _ctx: &mut RunCtx,
            ) -> Result<InpaintResult, InpaintError> {
                Err(InpaintError::Cancelled)
            }
        }

        let mut s = NativeInpaintState::default();
        s.model_id = "Y".to_string(); // current picker value
        let backend: Arc<dyn InpaintBackend + Send + Sync> = Arc::new(DummyBackend);
        s.tx.send(NativeMsg::BackendReady {
            backend,
            model_id: "X".to_string(), // what the worker actually loaded
        })
        .unwrap();
        s.drain_msgs();
        assert_eq!(
            s.backend_model_id.as_deref(),
            Some("X"),
            "cache must be labelled with the loaded model id, not the picker value"
        );
        assert!(s.backend.is_some(), "backend cached");
    }

    #[test]
    fn reset_for_uid_clears_mask_and_flattened() {
        let mut s = NativeInpaintState::default();
        s.flattened = Some(solid(2, 2, 1));
        s.mask = Some(super::super::mask::MaskBuffer::new(2, 2));
        s.zoom = 3.0;
        s.reset_for_uid(7);
        assert!(s.flattened.is_none());
        assert!(s.mask.is_none());
        assert_eq!(s.zoom, 1.0);
        assert_eq!(s.synced_uid, Some(7));
    }

    #[test]
    fn reset_for_uid_clears_history() {
        use crate::native_inpaint::history::InpaintAction;
        let mut s = NativeInpaintState::default();
        s.history.push(InpaintAction::AppliedResult);
        assert!(s.history.can_undo());
        s.reset_for_uid(3);
        assert!(
            !s.history.can_undo(),
            "history is per-document; reset clears it"
        );
        assert!(s.stroke_before.is_none());
    }
}
