use std::fmt;

/// A single inpaint request. `prompt` is unused in Phase 1 (removal) but kept so
/// the generative backend can reuse this type unchanged.
#[allow(dead_code)] // `prompt`/`params` are Phase 2 (generative) stubs
pub struct InpaintRequest {
    pub image: image::RgbaImage,
    pub mask: image::GrayImage, // white (255) = inpaint, black (0) = keep
    pub prompt: Option<String>,
    pub params: InpaintParams,
}

pub struct InpaintResult {
    pub image: image::RgbaImage,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // most fields drive Phase 2 generative fill / future tiling
pub struct InpaintParams {
    pub model_id: String,
    /// Images whose max dimension exceeds this are tiled before inference.
    pub tile_threshold_px: u32,
    /// Generative-only (Phase 2); ignored by removal.
    pub steps: u32,
    pub strength: f32,
    pub seed: u64,
    /// Generative-only (Phase 2); ignored by removal.
    pub negative_prompt: String,
    /// Classifier-free guidance scale (CFG). Generative-only.
    pub guidance: f32,
    /// Scheduler id (must match a name the candle backend reports). Generative-only.
    pub scheduler: String,
}

impl Default for InpaintParams {
    fn default() -> Self {
        Self {
            model_id: "lama-fp32".to_string(),
            tile_threshold_px: 2048,
            steps: 20,
            strength: 0.75,
            seed: 0,
            negative_prompt: String::new(),
            guidance: 7.5,
            scheduler: "ddim".to_string(),
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)] // `CudaUnavailable`/`Cancelled` are surfaced once Phase 2 wiring lands
pub enum InpaintError {
    CudaUnavailable,
    ModelMissing(String),
    DownloadFailed(String),
    ChecksumMismatch { expected: String, got: String },
    Inference(String),
    OutOfMemory,
    Cancelled,
}

impl fmt::Display for InpaintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CudaUnavailable => write!(f, "CUDA GPU unavailable — falling back to CPU"),
            Self::ModelMissing(id) => write!(f, "model not downloaded: {id}"),
            Self::DownloadFailed(e) => write!(f, "model download failed: {e}"),
            Self::ChecksumMismatch { expected, got } => write!(
                f,
                "model checksum mismatch (expected {expected}, got {got})"
            ),
            Self::Inference(e) => write!(f, "inference error: {e}"),
            Self::OutOfMemory => write!(f, "out of GPU memory — try a smaller region"),
            Self::Cancelled => write!(f, "inpaint cancelled"),
        }
    }
}
impl std::error::Error for InpaintError {}

/// Side channel handed to a backend's `run`: report coarse progress and check
/// for cancellation. Keeps the trait simple while supporting a progress bar and
/// a Cancel button without the backend knowing about the UI.
// The progress/cancel side-channel is only driven by the feature-gated backends
// (`lama_ort` / `sd_candle`); dead in a default build.
#[cfg_attr(
    not(any(feature = "native-inpaint", feature = "native-generative")),
    allow(dead_code)
)]
pub struct RunCtx<'a> {
    /// `(fraction 0..=1, human stage label)`.
    pub on_progress: &'a mut dyn FnMut(f32, &str),
    /// Set to `true` by the UI to request cancellation; backends poll it at
    /// safe points and return `InpaintError::Cancelled`.
    pub cancel: &'a std::sync::atomic::AtomicBool,
}

#[cfg_attr(
    not(any(feature = "native-inpaint", feature = "native-generative")),
    allow(dead_code)
)]
impl RunCtx<'_> {
    pub fn progress(&mut self, frac: f32, stage: &str) {
        (self.on_progress)(frac, stage);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The seam every engine hides behind.
#[allow(dead_code)] // `id`/`is_generative` are introspection hooks for the Phase 2 backend picker
pub trait InpaintBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn is_generative(&self) -> bool;
    fn run(&self, req: InpaintRequest, ctx: &mut RunCtx) -> Result<InpaintResult, InpaintError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_human_readable() {
        assert!(InpaintError::CudaUnavailable.to_string().contains("CUDA"));
        assert!(InpaintError::ModelMissing("lama".into())
            .to_string()
            .contains("lama"));
        assert_eq!(InpaintError::Cancelled.to_string(), "inpaint cancelled");
    }

    #[test]
    fn params_default_is_removal() {
        let p = InpaintParams::default();
        assert_eq!(p.model_id, "lama-fp32");
        assert!(p.tile_threshold_px >= 512);
    }

    #[test]
    fn params_carry_generative_fields() {
        let p = InpaintParams::default();
        assert_eq!(p.negative_prompt, "");
        assert!((p.guidance - 7.5).abs() < 1e-6);
        assert_eq!(p.scheduler, "ddim");
    }
}
