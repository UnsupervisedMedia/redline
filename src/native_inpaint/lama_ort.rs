//! LaMa object-removal backend running an ONNX model via the `ort` crate
//! (ONNX Runtime). Implements [`InpaintBackend`] with CUDA acceleration and a
//! silent CPU fallback when no GPU is available.
//!
//! The whole module is feature-gated behind `native-inpaint` in `mod.rs`, so no
//! per-item `cfg` attributes are needed here.

use std::path::PathBuf;
use std::sync::Mutex;

use image::imageops::FilterType;
use image::{GrayImage, Rgba, RgbaImage};
use ndarray::Array4;

use ort::execution_providers::{CUDAExecutionProvider, ExecutionProvider};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

use super::backend::{InpaintBackend, InpaintError, InpaintRequest, InpaintResult, RunCtx};
use super::imageops::{composite_masked, mask_bbox, padded_square_region, target_infer_size};

/// Masked regions whose padded square exceeds this many px on a side fall back
/// to whole-image inference (cropping that large saves nothing and risks worse
/// seams). Below it, we crop so the model sees the region at full detail.
const REGION_MAX_SIDE: u32 = 1600;

/// Input tensor names for the LaMa ONNX export.
///
/// NOTE: these are the conventional names for the common LaMa-ONNX exports
/// (`Carve/LaMa-ONNX` and friends). They are ASSUMED and must be confirmed on
/// the first real run against the actual model graph (dump with
/// `onnx.load(...).graph.input`). If the model uses different names, inference
/// will fail with an "invalid input name" error from ORT and these constants
/// should be updated.
const INPUT_IMAGE: &str = "image";
const INPUT_MASK: &str = "mask";

/// LaMa removal backend. `ort::Session::run` takes `&mut self`, but the
/// [`InpaintBackend`] trait exposes `run(&self, ..)`, so the session lives
/// behind a `Mutex` for interior mutability. `Session` is `Send + Sync`.
pub struct LamaOrt {
    session: Mutex<Session>,
}

impl LamaOrt {
    /// Build a `Session` from the ONNX file at `path`.
    ///
    /// Registers the CUDA execution provider with soft-fail semantics: if CUDA
    /// is unavailable the default registration silently ignores the failure and
    /// ORT falls back to CPU. We deliberately do NOT call `error_on_failure`.
    pub fn load(path: PathBuf) -> Result<Self, InpaintError> {
        let session = Session::builder()
            .map_err(|e| InpaintError::Inference(e.to_string()))?
            .with_execution_providers([CUDAExecutionProvider::default().build()])
            .map_err(|e| InpaintError::Inference(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| InpaintError::Inference(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| InpaintError::Inference(e.to_string()))?;

        Ok(Self {
            session: Mutex::new(session),
        })
    }

    /// Query the model's required input (H, W) for the image input.
    ///
    /// Uses the ort rc.10 metadata API: `session.inputs[i].input_type` is a
    /// `ValueType::Tensor { shape, .. }` whose `shape` derefs to `[i64]` (NCHW),
    /// with a dim of `-1` (or any non-positive value) meaning "dynamic". We read
    /// dims[2] (H) and dims[3] (W) of the image input.
    ///
    /// Returns `Some((h, w))` when both are fixed positives, else `None`
    /// (dynamic / unknown — caller rounds source size up to a multiple of 8).
    fn model_image_hw(session: &Session) -> Option<(u32, u32)> {
        // Prefer the input named like the image input; fall back to the first
        // rank-4 tensor input.
        let input = session
            .inputs
            .iter()
            .find(|i| i.name == INPUT_IMAGE)
            .or_else(|| {
                session
                    .inputs
                    .iter()
                    .find(|i| i.input_type.tensor_shape().map(|s| s.len()) == Some(4))
            })?;
        let shape = input.input_type.tensor_shape()?;
        if shape.len() != 4 {
            return None;
        }
        let h = shape[2];
        let w = shape[3];
        if h > 0 && w > 0 {
            Some((h as u32, w as u32))
        } else {
            None
        }
    }

    /// Build the NCHW f32 input tensors.
    ///
    /// - image: `(1, 3, H, W)`, channels R,G,B normalized to `0..1` (alpha dropped).
    /// - mask:  `(1, 1, H, W)`, `1.0` where the mask pixel is `> 127` else `0.0`.
    fn to_tensors(img: &RgbaImage, mask: &GrayImage) -> (Array4<f32>, Array4<f32>) {
        let (w, h) = img.dimensions();
        let (hw, ww) = (h as usize, w as usize);

        let mut image = Array4::<f32>::zeros((1, 3, hw, ww));
        let mut mask_arr = Array4::<f32>::zeros((1, 1, hw, ww));

        for y in 0..h {
            for x in 0..w {
                let p = img.get_pixel(x, y).0;
                let (xi, yi) = (x as usize, y as usize);
                image[[0, 0, yi, xi]] = p[0] as f32 / 255.0;
                image[[0, 1, yi, xi]] = p[1] as f32 / 255.0;
                image[[0, 2, yi, xi]] = p[2] as f32 / 255.0;
                let m = mask.get_pixel(x, y).0[0];
                mask_arr[[0, 0, yi, xi]] = if m > 127 { 1.0 } else { 0.0 };
            }
        }

        (image, mask_arr)
    }
}

impl LamaOrt {
    /// Run the model on one image+mask pair at the model's input size and
    /// composite the fill back at the pair's own resolution. Unmasked pixels stay
    /// byte-identical to `img`. This is the per-region primitive `run` builds on.
    fn infer_region(
        &self,
        session: &mut Session,
        img: &RgbaImage,
        mask: &GrayImage,
    ) -> Result<RgbaImage, InpaintError> {
        let (ow_orig, oh_orig) = img.dimensions();
        let model_hw = Self::model_image_hw(session);
        let (tw, th) = target_infer_size(model_hw, ow_orig, oh_orig);

        let infer_img = image::imageops::resize(img, tw, th, FilterType::Triangle);
        let infer_mask = image::imageops::resize(mask, tw, th, FilterType::Nearest);

        let (image_arr, mask_arr) = Self::to_tensors(&infer_img, &infer_mask);
        let image_tensor =
            Tensor::from_array(image_arr).map_err(|e| InpaintError::Inference(e.to_string()))?;
        let mask_tensor =
            Tensor::from_array(mask_arr).map_err(|e| InpaintError::Inference(e.to_string()))?;

        let outputs = session
            .run(ort::inputs! {
                INPUT_IMAGE => image_tensor,
                INPUT_MASK => mask_tensor,
            })
            .map_err(map_inference_error)?;

        let output = &outputs[0];
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| InpaintError::Inference(e.to_string()))?;

        if shape.len() != 4 {
            return Err(InpaintError::Inference(format!(
                "unexpected output rank: shape {shape:?}"
            )));
        }
        let oc = shape[1] as usize;
        let oh = shape[2] as usize;
        let ow = shape[3] as usize;
        if oc < 3 || oh != th as usize || ow != tw as usize {
            return Err(InpaintError::Inference(format!(
                "unexpected output shape {shape:?} (expected (1,3,{th},{tw}))"
            )));
        }

        // Scale detection: if any value in the first plane exceeds 1.5 the export
        // already emits 0..255, otherwise it is 0..1 and must be scaled.
        let plane = oh * ow;
        let already_255 = data[..plane].iter().any(|&v| v > 1.5);
        let scale = if already_255 { 1.0_f32 } else { 255.0_f32 };

        let mut infer = RgbaImage::new(tw, th);
        for y in 0..oh {
            for x in 0..ow {
                let idx = |c: usize| (c * plane) + y * ow + x;
                let r = (data[idx(0)] * scale).clamp(0.0, 255.0) as u8;
                let g = (data[idx(1)] * scale).clamp(0.0, 255.0) as u8;
                let b = (data[idx(2)] * scale).clamp(0.0, 255.0) as u8;
                infer.put_pixel(x as u32, y as u32, Rgba([r, g, b, 255]));
            }
        }

        let infer_full = image::imageops::resize(&infer, ow_orig, oh_orig, FilterType::Triangle);
        Ok(composite_masked(img, &infer_full, mask))
    }
}

impl InpaintBackend for LamaOrt {
    fn id(&self) -> &'static str {
        "lama-ort"
    }

    fn is_generative(&self) -> bool {
        false
    }

    fn run(&self, req: InpaintRequest, ctx: &mut RunCtx) -> Result<InpaintResult, InpaintError> {
        let (iw, ih) = req.image.dimensions();

        if ctx.is_cancelled() {
            return Err(InpaintError::Cancelled);
        }
        ctx.progress(0.05, "Preparing");

        let mut session = self
            .session
            .lock()
            .map_err(|e| InpaintError::Inference(format!("session lock poisoned: {e}")))?;

        // Region-crop: inpaint a padded square around the masked area at full
        // detail instead of squashing the whole image to 512. Falls back to
        // whole-image when there's no mask or the region is too large to help.
        let region = mask_bbox(&req.mask).map(|bbox| {
            // Margin scales with the masked area so LaMa has surrounding context.
            let (bx0, by0, bx1, by1) = bbox;
            let span = (bx1 - bx0).max(by1 - by0);
            let margin = (span / 2).clamp(16, 256);
            padded_square_region(bbox, iw, ih, margin)
        });

        if ctx.is_cancelled() {
            return Err(InpaintError::Cancelled);
        }
        ctx.progress(0.4, "Inferring");

        let image = match region {
            Some((rx, ry, rw, rh)) if rw <= REGION_MAX_SIDE && rh <= REGION_MAX_SIDE => {
                let crop_img = image::imageops::crop_imm(&req.image, rx, ry, rw, rh).to_image();
                let crop_mask = image::imageops::crop_imm(&req.mask, rx, ry, rw, rh).to_image();
                let filled = self.infer_region(&mut session, &crop_img, &crop_mask)?;
                ctx.progress(0.9, "Compositing");
                // Paste the filled region back over a copy of the original.
                let mut out = req.image.clone();
                image::imageops::replace(&mut out, &filled, rx as i64, ry as i64);
                out
            }
            _ => {
                // No mask, or region too large — whole-image inference.
                let out = self.infer_region(&mut session, &req.image, &req.mask)?;
                ctx.progress(0.9, "Compositing");
                out
            }
        };

        ctx.progress(1.0, "Done");
        Ok(InpaintResult { image })
    }
}

/// Best-effort query of whether the CUDA execution provider is available in the
/// linked ONNX Runtime, WITHOUT building a session. Uses ORT's
/// `GetAvailableProviders` via [`ExecutionProvider::is_available`].
///
/// Note: this reports whether ORT was *compiled with* CUDA support, not whether
/// a specific session/model will actually run on the GPU. It's only used to
/// surface a non-fatal "running on CPU" notice, so a conservative `false` on
/// error is fine.
pub fn cuda_available() -> bool {
    CUDAExecutionProvider::default()
        .is_available()
        .unwrap_or(false)
}

/// Map an ORT inference error: out-of-memory (case-insensitive) -> `OutOfMemory`,
/// everything else -> `Inference`.
fn map_inference_error(e: ort::Error) -> InpaintError {
    let msg = e.to_string();
    if msg.to_lowercase().contains("out of memory") {
        InpaintError::OutOfMemory
    } else {
        InpaintError::Inference(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, Rgba};

    #[test]
    #[ignore]
    fn lama_removes_masked_region_and_keeps_rest() {
        let path = crate::native_inpaint::models::ensure_model("lama-fp32", |_| {}).unwrap();
        let backend = LamaOrt::load(path).unwrap();
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.put_pixel(x, y, Rgba([x as u8 * 4, y as u8 * 4, 128, 255]));
            }
        }
        for y in 24..40 {
            for x in 24..40 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let mut mask = GrayImage::from_pixel(64, 64, Luma([0]));
        for y in 24..40 {
            for x in 24..40 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        let req = InpaintRequest {
            image: img.clone(),
            mask,
            prompt: None,
            params: Default::default(),
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut prog = |_f: f32, _s: &str| {};
        let mut ctx = RunCtx {
            on_progress: &mut prog,
            cancel: &cancel,
        };
        let out = backend.run(req, &mut ctx).unwrap().image;
        assert_eq!(out.get_pixel(0, 0), img.get_pixel(0, 0));
        assert_ne!(out.get_pixel(31, 31), &Rgba([255, 255, 255, 255]));
    }
}
