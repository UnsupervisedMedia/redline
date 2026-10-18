//! Native generative-fill engine: Stable Diffusion inpaint via `candle` (CUDA).
//!
//! IMPORTANT: candle-transformers has **no turnkey inpaint pipeline**. It exposes
//! only the building blocks (CLIP text encoder, VAE, UNet, schedulers). We assemble
//! the SD-1.5-inpaint denoise loop here ourselves, mirroring HuggingFace diffusers'
//! `StableDiffusionInpaintPipeline`:
//!   1. CLIP-encode prompt (+ negative for classifier-free guidance).
//!   2. VAE-encode the masked init image -> `masked_image_latents` (4ch).
//!   3. Each denoise step the inpaint UNet consumes a **9-channel** input:
//!        concat([ noisy_latents(4ch), mask(1ch @ latent res), masked_image_latents(4ch) ], dim=1)
//!   4. Run the scheduler loop with CFG, then VAE-decode.
//!
//! The [`SdCandle`] backend wraps the loaded pipeline (config + tokenizer + CLIP
//! weights + VAE + 9ch UNet + scheduler choice) behind a `Mutex` (candle model
//! state is not `Sync`) and implements [`InpaintBackend`] with the same region-crop
//! shape as `lama_ort`.
//!
//! ---
//! Resolved candle version: **0.10.2** (crates.io resolved `candle-core = "0.9"`
//! to 0.10.2; pinned `=0.10.2` in Cargo.toml). `cuda` feature builds as-is — no
//! `cudnn` feature needed.
//!
//! API names verified against the candle 0.10.2 stable-diffusion example
//! (candle-examples/examples/stable-diffusion/main.rs):
//!   - `StableDiffusionConfig::v1_5(sliced_attention_size, height, width)` — NO
//!     separate `v1_5_inpaint` constructor; the inpaint-ness comes from the 9ch
//!     UNet + 9ch input concat, not the config.
//!   - `build_clip_transformer` is a **FREE function** in the `stable_diffusion`
//!     module: `stable_diffusion::build_clip_transformer(&clip::Config, weights, &Device, DType)`
//!     — it is NOT a method on `StableDiffusionConfig` (the plan snippet was wrong).
//!   - `sd_config.build_vae(weights, &Device, DType) -> AutoEncoderKL`
//!   - `sd_config.build_unet(weights, &Device, in_channels=9, use_flash_attn, DType) -> UNet2DConditionModel`
//!   - `sd_config.build_scheduler(n_steps) -> Box<dyn Scheduler>`
//!   - `Scheduler` trait: `timesteps()`, `add_noise()`, `init_noise_sigma()`,
//!     `scale_model_input(sample, t)`, `step(&model_output, t, &sample)`.
//!   - `vae.encode(&img) -> DiagonalGaussianDistribution`; `.sample()` -> Tensor.
//!   - VAE scale factor: 0.18215 (SD 1.x / 2.x).
//!   - `unet.forward(&latent_model_input, timestep as f64, &text_embeddings)`.
//!
//! Schedulers candle exposes for SD (drives the UI dropdown). Each lives under
//! `candle_transformers::models::stable_diffusion::*` and implements the `Scheduler`
//! trait (`schedulers::Scheduler`):
//!   - `ddim::DDIMScheduler`                                  (DDIM)
//!   - `euler_ancestral_discrete::EulerAncestralDiscreteScheduler` (Euler-ancestral)
//!   - `ddpm::DDPMScheduler`                                  (DDPM)
//!   - `uni_pc::EdmDpmMultistepScheduler`                     (UniPC / EDM-DPM multistep)
//! `build_scheduler()` returns a DDIM scheduler for v1.5/v2.1. We support "ddim"
//! (via `build_scheduler`) and "euler-ancestral" (constructed directly) — see
//! [`available_schedulers`]; only those two are honest, so only those are listed.
//!
//! HF repos used (note: the legacy `runwayml/stable-diffusion-inpainting` repo was
//! removed from the Hub; the canonical live mirror is
//! `stable-diffusion-v1-5/stable-diffusion-inpainting`, which the candle 0.10.2
//! example itself uses). CLIP tokenizer comes from `openai/clip-vit-base-patch32`.
//!
//! ============================================================================
//! SDXL INPAINT — candle 0.10.2 API map (Phase 2 spike, GATE: `spike_sdxl_inpaint_runs`)
//! ============================================================================
//! GATE OUTCOME (2026-06-15, 2× RTX A4000 16 GiB): **BLOCKED on VRAM.** The full
//! SDXL-inpaint loop ASSEMBLES and RUNS with the API below — it loads both CLIP
//! encoders + VAE + 9ch UNet, encodes, and enters the denoise loop — but at 1024
//! it peaks at ~15.1 GiB and OOMs inside a UNet forward on a 16 GiB card
//! (`CUDA_ERROR_OUT_OF_MEMORY`). The one in-crate mitigation, attention slicing via
//! `sdxl(Some(0), ..)`, is BROKEN in candle 0.10.2 (rank-4 vs dims3() panic in
//! `attention::CrossAttention::sliced_attention`), so it can't rescue VRAM without
//! patching candle. Viable paths for the impl task: (a) tile/patch UNet at <1024,
//! (b) free CLIP weights before UNet load (done here) + drop VAE encoder before the
//! loop, (c) sequential CPU offload, (d) bump to a candle that fixes slicing, or
//! (e) keep SDXL out-of-process. The API map below is still fully validated up to
//! the OOM and is what the implementation lifts.
//!
//! The API below is verified against candle-transformers 0.10.2 source
//! (.../stable_diffusion/{mod,clip,unet_2d}.rs) and the 0.10.2 stable-diffusion
//! example (handles `StableDiffusionVersion::XlInpaint`).
//!
//! Config constructor:
//!   `StableDiffusionConfig::sdxl(sliced_attention_size: Option<usize>, height: Option<usize>, width: Option<usize>)`
//!     -> sets `clip = clip::Config::sdxl()` (768-d, "CLIP-L"), `clip2 = Some(clip::Config::sdxl2())`
//!        (1280-d, "OpenCLIP bigG"), `unet.cross_attention_dim = 2048` (= 768 + 1280),
//!        `use_linear_projection = true`, VAE scale 0.18215. Default H/W 1024.
//!     NOTE: there is NO `sdxl_inpaint` constructor — inpaint-ness is purely the 9ch
//!     UNet + the 9ch input concat, exactly like SD-1.5 (`build_unet(.., in_channels=9, ..)`).
//!
//! Dual text encoders (the SECOND encoder is the only structural delta from SD-1.5):
//!   Build BOTH with the SAME free fn, different config + weights:
//!     `let clip1 = stable_diffusion::build_clip_transformer(&cfg.clip,  text_encoder_weights,   dev, DType::F32)?;`
//!     `let clip2 = stable_diffusion::build_clip_transformer(cfg.clip2.as_ref().unwrap(), text_encoder_2_weights, dev, DType::F32)?;`
//!   (The example builds CLIP in F32 then casts the final embeddings to F16 — mirrored here.)
//!   Encode prompt+negative through EACH encoder (hidden states only), then concat the
//!   TWO encoders' outputs along the FEATURE dim (last dim, `D::Minus1`):
//!     emb1 = cat([uncond1, cond1], 0)   // [2, 77, 768]
//!     emb2 = cat([uncond2, cond2], 0)   // [2, 77, 1280]
//!     text_embeddings = cat([emb1, emb2], D::Minus1)  // [2, 77, 2048]  <- encoder_hidden_states
//!
//! POOLED text embeds + time_ids micro-conditioning:
//!   *** NOT AVAILABLE / NOT USED in candle 0.10.2. ***  This is the key finding.
//!     - `clip::ClipTextTransformer` exposes ONLY `forward`, `forward_with_mask`,
//!       `forward_until_encoder_layer` — ALL return hidden states. There is NO pooled /
//!       text-projection output method (no `forward_with_pooled`).
//!     - `UNet2DConditionModel::forward(xs, timestep, encoder_hidden_states)` takes EXACTLY
//!       three args; `forward_with_additional_residuals` adds only ControlNet residuals.
//!       There is NO `added_cond_kwargs` / `add_text_embeds` / `time_ids` parameter.
//!     - `UNet2DConditionModel::new` builds NO `add_embedding` block, so the real SDXL
//!       checkpoint's `add_embedding.linear_{1,2}` tensors are simply NOT loaded
//!       (VarBuilder loads only requested tensors). candle's SDXL is therefore an
//!       APPROXIMATION that omits SDXL's micro-conditioning. It still runs and denoises.
//!   => The implementation must NOT try to build/pass pooled embeds or time_ids; there is
//!      no plumbing for them in 0.10.2. (Adding it would require patching candle.)
//!
//! UNet forward (identical call shape to SD-1.5):
//!   `unet.forward(&latent_model_input, timestep as f64, &text_embeddings)`
//!   where `latent_model_input = cat([ cat([lat,lat],0) |> scale_model_input, mask_cfg(1ch), masked_latents_cfg(4ch) ], dim=1)`
//!   i.e. the SAME 9-channel inpaint concat as SD-1.5, just at latent 128×128 (1024/8).
//!
//! HF files (repo `diffusers/stable-diffusion-xl-1.0-inpainting-0.1`, fp16):
//!   unet/diffusion_pytorch_model.fp16.safetensors
//!   vae/diffusion_pytorch_model.fp16.safetensors    (XlInpaint uses its OWN vae fp16 —
//!       the madebyollin/sdxl-vae-fp16-fix override applies only to Xl/Turbo, NOT XlInpaint)
//!   text_encoder/model.fp16.safetensors             (CLIP-L)
//!   text_encoder_2/model.fp16.safetensors           (OpenCLIP bigG)
//!   tokenizer:  openai/clip-vit-large-patch14 :: tokenizer.json          (encoder 1)
//!   tokenizer2: laion/CLIP-ViT-bigG-14-laion2B-39B-b160k :: tokenizer.json (encoder 2)

use std::sync::Mutex;

use candle_core::{DType, Device, Error as CandleError, IndexOp, Module, Tensor};
use candle_transformers::models::stable_diffusion::{
    self, clip::ClipTextTransformer, schedulers::Scheduler, unet_2d::UNet2DConditionModel,
    vae::AutoEncoderKL, StableDiffusionConfig,
};
use hf_hub::api::sync::Api;
use image::imageops::FilterType;
use image::{GrayImage, Rgba, RgbaImage};
use tokenizers::Tokenizer;

use super::backend::{InpaintBackend, InpaintError, InpaintRequest, InpaintResult, RunCtx};
use super::imageops::{composite_masked, mask_bbox, padded_square_region};

const CLIP_TOKENIZER_REPO: &str = "openai/clip-vit-base-patch32";
const VAE_SCALE: f64 = 0.18215;

/// SDXL tokenizer repos (see the SDXL API map in the file header).
const SDXL_TOKENIZER_REPO: &str = "openai/clip-vit-large-patch14"; // CLIP-L (encoder 1)
const SDXL_TOKENIZER2_REPO: &str = "laion/CLIP-ViT-bigG-14-laion2B-39B-b160k"; // bigG (encoder 2)

/// Masked regions whose padded square exceeds this many px on a side fall back to
/// whole-image inference. Mirrors `lama_ort::REGION_MAX_SIDE`. SD always resizes
/// to `base_res` for inference, so this only governs how much surrounding context
/// the crop captures before that resize.
const REGION_MAX_SIDE: u32 = 2048;

/// Text-encoder set for a pipeline. SD-1.5 has a single CLIP (768-d); SDXL has two
/// (CLIP-L 768-d + OpenCLIP bigG 1280-d) whose hidden states are concatenated on the
/// feature dim to a 2048-d `encoder_hidden_states`. This is the ONLY structural delta
/// between the two families' inference; the 9ch concat / CFG / scheduler / VAE are shared.
enum TextEncoders {
    /// SD-1.5: single CLIP, single tokenizer.
    Sd15 {
        tokenizer: Tokenizer,
        clip: ClipTextTransformer,
    },
    /// SDXL: dual CLIP encoders + their two tokenizers.
    Sdxl {
        tokenizer1: Tokenizer,
        tokenizer2: Tokenizer,
        clip1: ClipTextTransformer,
        clip2: ClipTextTransformer,
    },
}

/// Loaded SD-inpaint pipeline. Held behind a `Mutex` in [`SdCandle`] because the
/// candle model handles are not `Sync` and `forward` takes shared refs that share
/// CUDA state.
struct Pipeline {
    config: StableDiffusionConfig,
    /// One or two CLIP encoders depending on model family. Built once at load time.
    encoders: TextEncoders,
    vae: AutoEncoderKL,
    unet: UNet2DConditionModel,
    device: Device,
    dtype: DType,
}

/// Stable Diffusion inpaint backend (candle, CUDA-only).
#[allow(dead_code)] // wired into the backend picker in a later Phase 2 task
pub struct SdCandle {
    pipe: Mutex<Pipeline>,
    base_res: u32,
    scheduler: String,
}

#[allow(dead_code)] // wired into the backend picker in a later Phase 2 task
impl SdCandle {
    /// Load the SD-inpaint pipeline. Requires a CUDA device. Resolves weights via
    /// the HF Hub (cached after first download), builds the config, tokenizer(s),
    /// CLIP encoder(s), VAE and 9-channel inpaint UNet, all in F16.
    ///
    /// Model family is detected from the HF repo id: an "xl" in the id => SDXL
    /// (dual CLIP encoders, cross-attn 2048, run at `base_res` = 768 — 1024 OOMs on
    /// a 16 GiB card); otherwise SD-1.5 (single CLIP). `base_res` only selects the
    /// working square resolution; it no longer drives the family choice.
    pub fn load(hf_repo: &str, base_res: u32, scheduler: &str) -> Result<Self, InpaintError> {
        // Pick the CUDA device with the MOST free VRAM so SD/SDXL land on the best
        // card on multi-GPU / partially-busy boxes. Best-effort: any failure falls
        // back to device 0. See [`select_cuda_device`].
        let ordinal = select_cuda_device();
        let device = match Device::new_cuda(ordinal) {
            Ok(d) if d.is_cuda() => d,
            // Fall back to device 0 if the selected ordinal failed for any reason.
            _ => match Device::cuda_if_available(0) {
                Ok(d) if d.is_cuda() => d,
                _ => return Err(InpaintError::CudaUnavailable),
            },
        };
        let dtype = DType::F16;

        // SDXL repos contain "xl" (e.g. stable-diffusion-xl-1.0-inpainting); SD-1.5
        // does not. This is the cleanest self-contained signal.
        let is_sdxl = hf_repo.to_lowercase().contains("xl");

        let api = Api::new().map_err(|e| InpaintError::DownloadFailed(e.to_string()))?;
        let repo = api.model(hf_repo.to_string());
        let dl = |path: &str| -> Result<std::path::PathBuf, InpaintError> {
            repo.get(path)
                .map_err(|e| InpaintError::DownloadFailed(format!("{path}: {e}")))
        };
        let dl_tok = |repo_id: &str| -> Result<Tokenizer, InpaintError> {
            let p = api
                .model(repo_id.to_string())
                .get("tokenizer.json")
                .map_err(|e| {
                    InpaintError::DownloadFailed(format!("{repo_id}/tokenizer.json: {e}"))
                })?;
            Tokenizer::from_file(&p).map_err(|e| InpaintError::Inference(e.to_string()))
        };

        let unet_weights = dl("unet/diffusion_pytorch_model.fp16.safetensors")?;
        let vae_weights = dl("vae/diffusion_pytorch_model.fp16.safetensors")?;

        let res = base_res as usize;

        let (config, encoders) = if is_sdxl {
            // SDXL: dual encoders, cross-attn 2048. `None` => NO attention slicing
            // (candle 0.10.2's sliced_attention panics on a rank-4 reshape).
            let config = StableDiffusionConfig::sdxl(None, Some(res), Some(res));
            let clip_weights = dl("text_encoder/model.fp16.safetensors")?;
            let clip2_weights = dl("text_encoder_2/model.fp16.safetensors")?;
            let tokenizer1 = dl_tok(SDXL_TOKENIZER_REPO)?;
            let tokenizer2 = dl_tok(SDXL_TOKENIZER2_REPO)?;

            // Build both CLIP encoders ONCE at load (safetensors parsed once each).
            // candle's example builds CLIP in F32 then casts the final embeddings to
            // F16; we mirror that for numerical parity with the validated spike.
            let clip1 = stable_diffusion::build_clip_transformer(
                &config.clip,
                &clip_weights,
                &device,
                DType::F32,
            )
            .map_err(|e| InpaintError::Inference(e.to_string()))?;
            let clip2_cfg = config
                .clip2
                .as_ref()
                .ok_or_else(|| InpaintError::Inference("sdxl config missing clip2".to_string()))?;
            let clip2 = stable_diffusion::build_clip_transformer(
                clip2_cfg,
                &clip2_weights,
                &device,
                DType::F32,
            )
            .map_err(|e| InpaintError::Inference(e.to_string()))?;

            (
                config,
                TextEncoders::Sdxl {
                    tokenizer1,
                    tokenizer2,
                    clip1,
                    clip2,
                },
            )
        } else {
            let config = StableDiffusionConfig::v1_5(None, Some(res), Some(res));
            let clip_weights = dl("text_encoder/model.fp16.safetensors")?;
            let tokenizer = dl_tok(CLIP_TOKENIZER_REPO)?;
            // Build the CLIP text transformer once at load time (parses safetensors once).
            let clip = stable_diffusion::build_clip_transformer(
                &config.clip,
                &clip_weights,
                &device,
                dtype,
            )
            .map_err(|e| InpaintError::Inference(e.to_string()))?;
            (config, TextEncoders::Sd15 { tokenizer, clip })
        };

        let vae = config
            .build_vae(&vae_weights, &device, dtype)
            .map_err(|e| InpaintError::Inference(e.to_string()))?;
        // 9 input channels: inpaint UNet ([latents(4), mask(1), masked_latents(4)]).
        let unet = config
            .build_unet(&unet_weights, &device, 9, false, dtype)
            .map_err(|e| InpaintError::Inference(e.to_string()))?;

        Ok(Self {
            pipe: Mutex::new(Pipeline {
                config,
                encoders,
                vae,
                unet,
                device,
                dtype,
            }),
            base_res,
            scheduler: scheduler.to_string(),
        })
    }

    /// Scheduler ids the UI dropdown may offer. Only the ones [`build_scheduler`]
    /// below actually constructs are listed (honest dropdown).
    pub fn available_schedulers() -> Vec<String> {
        vec!["ddim".to_string(), "euler-ancestral".to_string()]
    }
}

/// Choose the CUDA device ordinal to load onto: the device with the MOST free
/// VRAM, so on multi-GPU / partially-busy boxes the (large) SD/SDXL weights land on
/// the best card. Best-effort and never panics:
///   1. If `REDLINE_CUDA_DEVICE` is set to a valid ordinal, honor it (explicit override).
///   2. Else enumerate CUDA devices via cudarc (re-exported by candle-core) and pick
///      the one whose `mem_get_info().free` is largest.
///   3. Else fall back to device 0.
fn select_cuda_device() -> usize {
    // 1. Explicit override.
    if let Ok(v) = std::env::var("REDLINE_CUDA_DEVICE") {
        if let Ok(ord) = v.trim().parse::<usize>() {
            return ord;
        }
    }

    // 2. Freest-VRAM selection via cudarc (candle re-exports it, so no new dep).
    use candle_core::cuda_backend::cudarc::driver::CudaContext;
    let count = match CudaContext::device_count() {
        Ok(c) if c > 0 => c as usize,
        _ => return 0,
    };
    let mut best = 0usize;
    let mut best_free = 0usize;
    for ord in 0..count {
        // Creating a context per device and querying free/total is cheap and lets us
        // compare cards; the context is dropped immediately. Any error => skip device.
        if let Ok(ctx) = CudaContext::new(ord) {
            if let Ok((free, _total)) = ctx.mem_get_info() {
                if free > best_free {
                    best_free = free;
                    best = ord;
                }
            }
        }
    }
    best
}

/// CLIP-encode `prompt` + `negative` through ONE encoder, returning `[2, 77, dim]`
/// with rows `[uncond, cond]` (CFG order). The encoder's own `clip::Config` supplies
/// the max length + pad token, so this works for both CLIP-L and OpenCLIP bigG.
fn encode_one(
    tokenizer: &Tokenizer,
    cfg: &stable_diffusion::clip::Config,
    model: &ClipTextTransformer,
    prompt: &str,
    negative: &str,
    device: &Device,
) -> Result<Tensor, CandleError> {
    let max = cfg.max_position_embeddings;
    let vocab = tokenizer.get_vocab(true);
    let lookup = |tok: &str| -> Result<u32, CandleError> {
        vocab
            .get(tok)
            .copied()
            .ok_or_else(|| CandleError::msg(format!("tokenizer missing token {tok:?}")))
    };
    let pad_id = match &cfg.pad_with {
        Some(p) => lookup(p.as_str())?,
        None => lookup("<|endoftext|>")?,
    };

    let tokenize = |text: &str| -> Result<Tensor, CandleError> {
        let mut tokens = tokenizer
            .encode(text, true)
            .map_err(CandleError::msg)?
            .get_ids()
            .to_vec();
        if tokens.len() > max {
            return Err(CandleError::msg(format!(
                "prompt too long: {} > {max}",
                tokens.len()
            )));
        }
        while tokens.len() < max {
            tokens.push(pad_id);
        }
        Ok(Tensor::new(tokens.as_slice(), device)?.unsqueeze(0)?)
    };

    let cond = model.forward(&tokenize(prompt)?)?;
    let uncond = model.forward(&tokenize(negative)?)?;
    // CFG order: [uncond, cond].
    Tensor::cat(&[uncond, cond], 0)
}

/// Build the scheduler the caller named. Unknown names fall back to DDIM (the
/// SD 1.5 default) rather than erroring, so a stale params string can't break a run.
fn build_scheduler(
    config: &StableDiffusionConfig,
    name: &str,
    n_steps: usize,
) -> Result<Box<dyn Scheduler>, InpaintError> {
    let r = match name {
        "euler-ancestral" => {
            use stable_diffusion::euler_ancestral_discrete::{
                EulerAncestralDiscreteScheduler, EulerAncestralDiscreteSchedulerConfig,
            };
            EulerAncestralDiscreteScheduler::new(
                n_steps,
                EulerAncestralDiscreteSchedulerConfig::default(),
            )
            .map(|s| Box::new(s) as Box<dyn Scheduler>)
        }
        // "ddim" and anything unrecognised: build_scheduler returns DDIM for v1.5.
        _ => config.build_scheduler(n_steps),
    };
    r.map_err(|e| InpaintError::Inference(e.to_string()))
}

impl SdCandle {
    /// Encode `prompt` + `negative` into the UNet's `encoder_hidden_states`, cast to
    /// the pipeline dtype (F16). SD-1.5 => `[2, 77, 768]`; SDXL => `[2, 77, 2048]`
    /// (the two encoders' hidden states concatenated on the feature dim). Row order
    /// is `[uncond, cond]` for classifier-free guidance.
    fn encode_text(pipe: &Pipeline, prompt: &str, negative: &str) -> Result<Tensor, InpaintError> {
        Self::encode_text_inner(pipe, prompt, negative)
            .map_err(|e| InpaintError::Inference(e.to_string()))
    }

    fn encode_text_inner(
        pipe: &Pipeline,
        prompt: &str,
        negative: &str,
    ) -> Result<Tensor, CandleError> {
        match &pipe.encoders {
            TextEncoders::Sd15 { tokenizer, clip } => {
                // Single encoder -> [2, 77, 768].
                encode_one(
                    tokenizer,
                    &pipe.config.clip,
                    clip,
                    prompt,
                    negative,
                    &pipe.device,
                )?
                .to_dtype(pipe.dtype)
            }
            TextEncoders::Sdxl {
                tokenizer1,
                tokenizer2,
                clip1,
                clip2,
            } => {
                // Encode through EACH encoder, then concat on the FEATURE dim:
                //   emb1 [2,77,768] (CLIP-L) ++ emb2 [2,77,1280] (bigG) -> [2,77,2048].
                let clip2_cfg = pipe
                    .config
                    .clip2
                    .as_ref()
                    .ok_or_else(|| CandleError::msg("sdxl config missing clip2"))?;
                let emb1 = encode_one(
                    tokenizer1,
                    &pipe.config.clip,
                    clip1,
                    prompt,
                    negative,
                    &pipe.device,
                )?;
                let emb2 =
                    encode_one(tokenizer2, clip2_cfg, clip2, prompt, negative, &pipe.device)?;
                use candle_core::D;
                Tensor::cat(&[&emb1, &emb2], D::Minus1)?.to_dtype(pipe.dtype)
            }
        }
    }

    /// Run the SD-inpaint denoise loop (lifted from the verified spike) on one
    /// image+mask pair at `base_res`, then composite the fill back so unmasked
    /// pixels keep their ORIGINAL values. `img`/`mask` may be any size; they are
    /// resized to `base_res` for inference and the decode is resized back.
    fn infer_region(
        &self,
        pipe: &Pipeline,
        img: &RgbaImage,
        mask: &GrayImage,
        req: &InpaintRequest,
        ctx: &mut RunCtx,
    ) -> Result<RgbaImage, InpaintError> {
        let (ow, oh) = img.dimensions();
        let res = self.base_res;
        let device = &pipe.device;
        let dtype = pipe.dtype;

        // Resize the working crop to the model's square base resolution.
        let work_img = image::imageops::resize(img, res, res, FilterType::Triangle);
        let work_mask = image::imageops::resize(mask, res, res, FilterType::Nearest);

        let map = |e: CandleError| map_candle_error(&e);

        let n_steps = req.params.steps.max(1) as usize;
        let guidance = req.params.guidance as f64;
        let prompt = req.prompt.as_deref().unwrap_or("");

        // Tensors: image in [-1,1] [1,3,res,res]; mask in {0,1} [1,1,res,res].
        let image_t = rgba_to_tensor(&work_img, device)?
            .to_dtype(dtype)
            .map_err(map)?;
        let mask_t = mask_to_tensor(&work_mask, device)?
            .to_dtype(dtype)
            .map_err(map)?;

        let text_embeddings = Self::encode_text(pipe, prompt, &req.params.negative_prompt)?;

        // Honor the per-request scheduler; fall back to the load-time default if empty.
        let scheduler_name = if req.params.scheduler.is_empty() {
            &self.scheduler
        } else {
            &req.params.scheduler
        };
        let mut scheduler = build_scheduler(&pipe.config, scheduler_name, n_steps)?;
        device.set_seed(req.params.seed).map_err(map)?;

        let lr = (res / 8) as usize; // latent resolution

        // masked_image = image * (1 - mask): zero out the region to be filled.
        let keep = mask_t
            .affine(-1.0, 1.0)
            .map_err(map)?
            .repeat(&[1, 3, 1, 1])
            .map_err(map)?;
        let masked_image = (&image_t * &keep).map_err(map)?;
        let masked_image_latents = (pipe
            .vae
            .encode(&masked_image)
            .map_err(map)?
            .sample()
            .map_err(map)?
            * VAE_SCALE)
            .map_err(map)?
            .to_dtype(dtype)
            .map_err(map)?;

        let mask_latent = mask_t
            .interpolate2d(lr, lr)
            .map_err(map)?
            .to_dtype(dtype)
            .map_err(map)?;
        let mask_cfg = Tensor::cat(&[&mask_latent, &mask_latent], 0).map_err(map)?;
        let masked_latents_cfg =
            Tensor::cat(&[&masked_image_latents, &masked_image_latents], 0).map_err(map)?;

        // Strength controls how much of the masked region is changed, mirroring
        // diffusers' `StableDiffusionInpaintPipeline.get_timesteps`:
        //   strength == 1.0 → pure-noise init, full schedule (maximal change).
        //   strength <  1.0 → start from the masked-image latents noised to an
        //     intermediate timestep, running only the schedule tail (less change).
        // Clamp to a sane minimum so at least one step always runs.
        let strength = (req.params.strength as f64).clamp(0.05, 1.0);

        let full_timesteps = scheduler.timesteps().to_vec();
        let n = full_timesteps.len();
        // t_start = floor(steps * (1 - strength)); take the schedule tail.
        let t_start = ((n as f64) * (1.0 - strength)).floor() as usize;
        let t_start = t_start.min(n.saturating_sub(1));
        let timesteps = full_timesteps[t_start..].to_vec();

        let noise = Tensor::randn(0f32, 1f32, (1, 4, lr, lr), device).map_err(map)?;
        let mut latents = if strength >= 1.0 {
            // Pure noise scaled by the scheduler sigma.
            (&noise * scheduler.init_noise_sigma())
                .map_err(map)?
                .to_dtype(dtype)
                .map_err(map)?
        } else {
            // Start from the masked-image latents noised to the first tail timestep.
            let t0 = *timesteps.first().expect("clamped to >=1 timestep");
            let init = masked_image_latents.to_dtype(DType::F32).map_err(map)?;
            scheduler
                .add_noise(&init, noise, t0)
                .map_err(map)?
                .to_dtype(dtype)
                .map_err(map)?
        };

        let total = timesteps.len();
        for (i, &t) in timesteps.iter().enumerate() {
            if ctx.is_cancelled() {
                return Err(InpaintError::Cancelled);
            }

            let model_in = Tensor::cat(&[&latents, &latents], 0).map_err(map)?;
            let model_in = scheduler.scale_model_input(model_in, t).map_err(map)?;
            // 9ch inpaint input: concat([latents(4), mask(1), masked_latents(4)], dim=1).
            let model_in =
                Tensor::cat(&[&model_in, &mask_cfg, &masked_latents_cfg], 1).map_err(map)?;

            let noise_pred = pipe
                .unet
                .forward(&model_in, t as f64, &text_embeddings)
                .map_err(map)?;
            let chunks = noise_pred.chunk(2, 0).map_err(map)?;
            let (uncond, cond) = (&chunks[0], &chunks[1]);
            let noise_pred =
                (uncond + ((cond - uncond).map_err(map)? * guidance).map_err(map)?).map_err(map)?;

            latents = scheduler.step(&noise_pred, t, &latents).map_err(map)?;

            // Progress over the denoise loop (which dominates runtime).
            let frac = 0.4 + 0.5 * ((i + 1) as f32 / total as f32);
            ctx.progress(frac.min(0.9), &format!("Step {}/{}", i + 1, total));
        }

        if ctx.is_cancelled() {
            return Err(InpaintError::Cancelled);
        }

        // VAE-decode to RGB in [0,1], shape [1,3,res,res].
        let decoded = pipe
            .vae
            .decode(&(&latents / VAE_SCALE).map_err(map)?)
            .map_err(map)?;
        let decoded = ((decoded / 2.0).map_err(map)? + 0.5)
            .map_err(map)?
            .clamp(0f32, 1f32)
            .map_err(map)?;
        let decoded = decoded
            .to_dtype(DType::F32)
            .map_err(map)?
            .to_device(&Device::Cpu)
            .map_err(map)?;

        let decoded_img = tensor_to_rgba(&decoded, res, res)?;
        // Resize the decoded patch back to the crop's own size, then composite so
        // unmasked pixels keep the ORIGINAL crop (mask-weighted lerp).
        let decoded_full = image::imageops::resize(&decoded_img, ow, oh, FilterType::Triangle);
        Ok(composite_masked(img, &decoded_full, mask))
    }
}

impl InpaintBackend for SdCandle {
    fn id(&self) -> &'static str {
        "sd-candle"
    }

    fn is_generative(&self) -> bool {
        true
    }

    fn run(&self, req: InpaintRequest, ctx: &mut RunCtx) -> Result<InpaintResult, InpaintError> {
        let (iw, ih) = req.image.dimensions();

        if ctx.is_cancelled() {
            return Err(InpaintError::Cancelled);
        }
        ctx.progress(0.05, "Preparing");

        let pipe = self
            .pipe
            .lock()
            .map_err(|e| InpaintError::Inference(format!("pipeline lock poisoned: {e}")))?;

        // Region-crop: inpaint a padded square around the masked area so the model
        // sees it with surrounding context. Falls back to whole-image when there's
        // no mask or the region is too large to help.
        let region = mask_bbox(&req.mask).map(|bbox| {
            let (bx0, by0, bx1, by1) = bbox;
            let span = (bx1 - bx0).max(by1 - by0);
            let margin = (span / 2).clamp(32, 256);
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
                let filled = self.infer_region(&pipe, &crop_img, &crop_mask, &req, ctx)?;
                let mut out = req.image.clone();
                image::imageops::replace(&mut out, &filled, rx as i64, ry as i64);
                out
            }
            _ => self.infer_region(&pipe, &req.image, &req.mask, &req, ctx)?,
        };

        ctx.progress(1.0, "Done");
        Ok(InpaintResult { image })
    }
}

/// RGBA image -> `[1, 3, h, w]` f32 tensor in `[-1, 1]` (alpha dropped).
fn rgba_to_tensor(img: &RgbaImage, device: &Device) -> Result<Tensor, InpaintError> {
    let (w, h) = img.dimensions();
    let (hw, ww) = (h as usize, w as usize);
    let mut data = vec![0f32; 3 * hw * ww];
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x, y).0;
            let (xi, yi) = (x as usize, y as usize);
            for c in 0..3 {
                data[(c * hw + yi) * ww + xi] = p[c] as f32 / 255.0 * 2.0 - 1.0;
            }
        }
    }
    Tensor::from_vec(data, (1, 3, hw, ww), device).map_err(|e| map_candle_error(&e))
}

/// Gray mask -> `[1, 1, h, w]` f32 tensor in `{0, 1}` (1 = inpaint, pixel > 127).
fn mask_to_tensor(mask: &GrayImage, device: &Device) -> Result<Tensor, InpaintError> {
    let (w, h) = mask.dimensions();
    let (hw, ww) = (h as usize, w as usize);
    let mut data = vec![0f32; hw * ww];
    for y in 0..h {
        for x in 0..w {
            data[y as usize * ww + x as usize] = if mask.get_pixel(x, y).0[0] > 127 {
                1.0
            } else {
                0.0
            };
        }
    }
    Tensor::from_vec(data, (1, 1, hw, ww), device).map_err(|e| map_candle_error(&e))
}

/// `[1, 3, h, w]` f32 tensor in `[0, 1]` (CPU) -> opaque RGBA image.
fn tensor_to_rgba(t: &Tensor, w: u32, h: u32) -> Result<RgbaImage, InpaintError> {
    let map = |e: CandleError| map_candle_error(&e);
    let mut out = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
    let plane = t.i(0).map_err(map)?; // [3, h, w]
    let r = plane
        .i(0)
        .map_err(map)?
        .flatten_all()
        .map_err(map)?
        .to_vec1::<f32>()
        .map_err(map)?;
    let g = plane
        .i(1)
        .map_err(map)?
        .flatten_all()
        .map_err(map)?
        .to_vec1::<f32>()
        .map_err(map)?;
    let b = plane
        .i(2)
        .map_err(map)?
        .flatten_all()
        .map_err(map)?
        .to_vec1::<f32>()
        .map_err(map)?;
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            out.put_pixel(
                x,
                y,
                Rgba([to_u8(r[idx]), to_u8(g[idx]), to_u8(b[idx]), 255]),
            );
        }
    }
    Ok(out)
}

/// Map a candle error: out-of-memory (case-insensitive) -> `OutOfMemory`,
/// everything else -> `Inference`.
fn map_candle_error(e: &CandleError) -> InpaintError {
    let msg = e.to_string();
    let lc = msg.to_lowercase();
    if lc.contains("out of memory") || lc.contains("oom") {
        InpaintError::OutOfMemory
    } else {
        InpaintError::Inference(msg)
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use image::{GrayImage, Luma, Rgba, RgbaImage};
    use std::sync::atomic::AtomicBool;

    const SD15_INPAINT_REPO: &str = "stable-diffusion-v1-5/stable-diffusion-inpainting";

    /// 512x512 RGB gradient fixture (opaque).
    fn gradient_image() -> RgbaImage {
        let mut img = RgbaImage::new(512, 512);
        for y in 0..512u32 {
            for x in 0..512u32 {
                let r = (x * 255 / 511) as u8;
                let g = (y * 255 / 511) as u8;
                img.put_pixel(x, y, Rgba([r, g, 128, 255]));
            }
        }
        img
    }

    /// Centred 200..312 square mask (white = inpaint).
    fn centre_square_mask() -> GrayImage {
        let mut m = GrayImage::from_pixel(512, 512, Luma([0]));
        for y in 200..312u32 {
            for x in 200..312u32 {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        m
    }

    #[test]
    #[ignore = "GPU + multi-GB weights; run explicitly with --ignored"]
    fn sd15_fills_masked_region_and_keeps_rest() {
        let b = SdCandle::load(SD15_INPAINT_REPO, 512, "ddim").unwrap();
        let img = gradient_image();
        let mask = centre_square_mask();
        let corner_orig = *img.get_pixel(8, 8);

        let mut params = crate::native_inpaint::backend::InpaintParams::default();
        params.steps = 20;
        params.seed = 42;
        let req = InpaintRequest {
            image: img.clone(),
            mask,
            prompt: Some("a red apple".to_string()),
            params,
        };
        let cancel = AtomicBool::new(false);
        let mut prog = |_f: f32, _s: &str| {};
        let mut ctx = RunCtx {
            on_progress: &mut prog,
            cancel: &cancel,
        };
        let out = b.run(req, &mut ctx).unwrap().image;

        // composite_masked guarantees unmasked pixels are byte-identical.
        assert_eq!(
            out.get_pixel(8, 8),
            &corner_orig,
            "corner must equal original"
        );
        // The masked centre must have been filled (changed from the gradient).
        assert_ne!(
            out.get_pixel(256, 256),
            img.get_pixel(256, 256),
            "masked centre must change"
        );
    }

    /// The strength knob must actually change the output: strength=1.0 (full noise,
    /// full schedule) vs strength=0.2 (mostly-preserved init, short tail) on the
    /// same seed/prompt/mask must produce DIFFERENT masked-region pixels.
    #[test]
    #[ignore = "GPU + multi-GB weights; run explicitly with --ignored"]
    fn strength_varies_masked_output() {
        let b = SdCandle::load(SD15_INPAINT_REPO, 512, "ddim").unwrap();
        let img = gradient_image();
        let mask = centre_square_mask();

        let run = |strength: f32| -> RgbaImage {
            let mut params = crate::native_inpaint::backend::InpaintParams::default();
            params.steps = 20;
            params.seed = 42;
            params.strength = strength;
            let req = InpaintRequest {
                image: img.clone(),
                mask: mask.clone(),
                prompt: Some("a red apple".to_string()),
                params,
            };
            let cancel = AtomicBool::new(false);
            let mut prog = |_f: f32, _s: &str| {};
            let mut ctx = RunCtx {
                on_progress: &mut prog,
                cancel: &cancel,
            };
            b.run(req, &mut ctx).unwrap().image
        };

        let full = run(1.0);
        let low = run(0.2);

        // Unmasked corner is byte-identical for both (composite_masked guarantee).
        assert_eq!(full.get_pixel(8, 8), img.get_pixel(8, 8));
        assert_eq!(low.get_pixel(8, 8), img.get_pixel(8, 8));
        // The masked centre must differ between the two strengths — proving the knob works.
        let differs = (200..312u32).step_by(7).any(|y| {
            (200..312u32)
                .step_by(7)
                .any(|x| full.get_pixel(x, y) != low.get_pixel(x, y))
        });
        assert!(
            differs,
            "strength=1.0 and strength=0.2 must differ in the masked region"
        );
    }

    /// Keep the original GATE spike: assemble + run the SD-1.5-inpaint denoise loop
    /// end to end on the GPU using raw candle (no backend), proving the lifted loop
    /// matches the verified source.
    #[test]
    #[ignore = "GPU + multi-GB weight download; run explicitly with --ignored"]
    fn spike_sd15_inpaint_runs() -> candle_core::Result<()> {
        let device = Device::new_cuda(0).expect(
            "CUDA device required for the generative-fill GATE spike (need nvcc + a CUDA GPU)",
        );
        let dtype = DType::F16;
        let n_steps = 8usize;
        let guidance_scale = 7.5f64;

        let api = Api::new().map_err(CandleError::msg)?;
        let repo = api.model(SD15_INPAINT_REPO.to_string());
        let unet_weights = repo
            .get("unet/diffusion_pytorch_model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let vae_weights = repo
            .get("vae/diffusion_pytorch_model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let clip_weights = repo
            .get("text_encoder/model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let tokenizer_path = api
            .model(CLIP_TOKENIZER_REPO.to_string())
            .get("tokenizer.json")
            .map_err(CandleError::msg)?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(CandleError::msg)?;

        let sd_config = StableDiffusionConfig::v1_5(None, Some(512), Some(512));
        let mut scheduler = sd_config.build_scheduler(n_steps)?;
        device.set_seed(42)?;

        // Inline encode_text mirroring the production path.
        let max = sd_config.clip.max_position_embeddings;
        let pad_id = *tokenizer.get_vocab(true).get("<|endoftext|>").unwrap();
        let tokenize = |text: &str| -> candle_core::Result<Tensor> {
            let mut tokens = tokenizer
                .encode(text, true)
                .map_err(CandleError::msg)?
                .get_ids()
                .to_vec();
            while tokens.len() < max {
                tokens.push(pad_id);
            }
            Ok(Tensor::new(tokens.as_slice(), &device)?.unsqueeze(0)?)
        };
        let text_model = stable_diffusion::build_clip_transformer(
            &sd_config.clip,
            &clip_weights,
            &device,
            dtype,
        )?;
        let cond = text_model.forward(&tokenize("a red apple")?)?;
        let uncond = text_model.forward(&tokenize("")?)?;
        let text_embeddings = Tensor::cat(&[uncond, cond], 0)?.to_dtype(dtype)?;

        let vae = sd_config.build_vae(&vae_weights, &device, dtype)?;
        let unet = sd_config.build_unet(&unet_weights, &device, 9, false, dtype)?;

        // Gradient image + centre mask as tensors.
        let mut img_data = vec![0f32; 3 * 512 * 512];
        for y in 0..512usize {
            for x in 0..512usize {
                img_data[(0 * 512 + y) * 512 + x] = x as f32 / 511.0 * 2.0 - 1.0;
                img_data[(1 * 512 + y) * 512 + x] = y as f32 / 511.0 * 2.0 - 1.0;
                img_data[(2 * 512 + y) * 512 + x] = 0.0;
            }
        }
        let image = Tensor::from_vec(img_data, (1, 3, 512, 512), &device)?.to_dtype(dtype)?;
        let mut mask_data = vec![0f32; 512 * 512];
        for y in 128..384usize {
            for x in 128..384usize {
                mask_data[y * 512 + x] = 1.0;
            }
        }
        let mask = Tensor::from_vec(mask_data, (1, 1, 512, 512), &device)?.to_dtype(dtype)?;

        let keep = mask.affine(-1.0, 1.0)?.repeat(&[1, 3, 1, 1])?;
        let masked_image = (&image * &keep)?;
        let masked_image_latents =
            (vae.encode(&masked_image)?.sample()? * VAE_SCALE)?.to_dtype(dtype)?;
        let mask_latent = mask.interpolate2d(64, 64)?.to_dtype(dtype)?;
        let mask_cfg = Tensor::cat(&[&mask_latent, &mask_latent], 0)?;
        let masked_latents_cfg = Tensor::cat(&[&masked_image_latents, &masked_image_latents], 0)?;

        let latents = Tensor::randn(0f32, 1f32, (1, 4, 64, 64), &device)?;
        let mut latents = (latents * scheduler.init_noise_sigma())?.to_dtype(dtype)?;

        let timesteps = scheduler.timesteps().to_vec();
        for &t in timesteps.iter() {
            let model_in = Tensor::cat(&[&latents, &latents], 0)?;
            let model_in = scheduler.scale_model_input(model_in, t)?;
            let model_in = Tensor::cat(&[&model_in, &mask_cfg, &masked_latents_cfg], 1)?;
            let noise_pred = unet.forward(&model_in, t as f64, &text_embeddings)?;
            let chunks = noise_pred.chunk(2, 0)?;
            let (uncond, cond) = (&chunks[0], &chunks[1]);
            let noise_pred = (uncond + ((cond - uncond)? * guidance_scale)?)?;
            latents = scheduler.step(&noise_pred, t, &latents)?;
        }

        let decoded = vae.decode(&(&latents / VAE_SCALE)?)?;
        let decoded = ((decoded / 2.0)? + 0.5)?.clamp(0f32, 1f32)?;
        let decoded = decoded.to_dtype(DType::F32)?.to_device(&Device::Cpu)?;

        let patch_mean = |t: &Tensor, y: usize, x: usize, s: usize| -> candle_core::Result<f32> {
            Ok(t.i((0, .., y..y + s, x..x + s))?
                .mean_all()?
                .to_scalar::<f32>()?)
        };
        let centre = patch_mean(&decoded, 224, 224, 64)?;
        assert!(centre.is_finite() && (0.0..=1.0).contains(&centre));
        Ok(())
    }

    const SDXL_INPAINT_REPO: &str = "diffusers/stable-diffusion-xl-1.0-inpainting-0.1";

    /// 768×768 RGB gradient fixture (opaque) — SDXL working resolution.
    fn gradient_image_768() -> RgbaImage {
        let mut img = RgbaImage::new(768, 768);
        for y in 0..768u32 {
            for x in 0..768u32 {
                let r = (x * 255 / 767) as u8;
                let g = (y * 255 / 767) as u8;
                img.put_pixel(x, y, Rgba([r, g, 128, 255]));
            }
        }
        img
    }

    /// Centred 300..468 square mask (white = inpaint) on a 768 canvas.
    fn centre_square_mask_768() -> GrayImage {
        let mut m = GrayImage::from_pixel(768, 768, Luma([0]));
        for y in 300..468u32 {
            for x in 300..468u32 {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        m
    }

    /// End-to-end SDXL inpaint through the real [`SdCandle`] backend at 768 (1024
    /// OOMs on a 16 GiB A4000). Builds dual CLIP encoders + VAE + 9ch UNet, runs the
    /// shared denoise loop, and asserts the composite guarantee: the masked centre
    /// changed while an unmasked corner stays byte-identical to the original.
    /// Record the nvidia-smi peak in the test report (must stay < ~16 GiB).
    #[test]
    #[ignore = "GPU + ~7GB SDXL weight download; run explicitly with --ignored"]
    fn sdxl_inpaint_runs_at_768() {
        let b = SdCandle::load(SDXL_INPAINT_REPO, 768, "ddim").unwrap();
        let img = gradient_image_768();
        let mask = centre_square_mask_768();
        let corner_orig = *img.get_pixel(8, 8);

        let mut params = crate::native_inpaint::backend::InpaintParams::default();
        params.steps = 8;
        params.seed = 42;
        let req = InpaintRequest {
            image: img.clone(),
            mask,
            prompt: Some("a red apple".to_string()),
            params,
        };
        let cancel = AtomicBool::new(false);
        let mut prog = |_f: f32, _s: &str| {};
        let mut ctx = RunCtx {
            on_progress: &mut prog,
            cancel: &cancel,
        };
        let out = b.run(req, &mut ctx).unwrap().image;

        // composite_masked guarantees unmasked pixels are byte-identical.
        assert_eq!(
            out.get_pixel(8, 8),
            &corner_orig,
            "corner must equal original"
        );
        // The masked centre must have been filled (changed from the gradient).
        assert_ne!(
            out.get_pixel(384, 384),
            img.get_pixel(384, 384),
            "masked centre must change"
        );
    }

    /// GATE spike — assemble + RUN the full SDXL-inpaint denoise loop end to end on
    /// the GPU with raw candle 0.10.2, proving SDXL inpaint fits in VRAM and pinning
    /// the exact API the implementation task will lift (see the SDXL API map in the
    /// file header). 1024×1024, latent 128×128, 9-channel inpaint concat, CFG, F16.
    ///
    /// DIFFERENCE vs the SD-1.5 spike: `StableDiffusionConfig::sdxl(...)`, TWO CLIP
    /// text encoders (CLIP-L 768 + OpenCLIP bigG 1280) concatenated on the feature
    /// dim to a [2,77,2048] `encoder_hidden_states`, two tokenizer repos. candle
    /// 0.10.2 has NO pooled-embeds / time_ids plumbing, so (matching candle's own
    /// example) none are passed — `unet.forward` is the same 3-arg call as SD-1.5.
    ///
    /// VRAM: record the nvidia-smi peak observed during this run in the test report.
    #[test]
    #[ignore = "GPU + ~7GB SDXL weight download; run explicitly with --ignored"]
    fn spike_sdxl_inpaint_runs() -> candle_core::Result<()> {
        use candle_core::D;

        let device = Device::new_cuda(0)
            .expect("CUDA device required for the SDXL GATE spike (need nvcc + a CUDA GPU)");
        let dtype = DType::F16;
        let n_steps = 8usize;
        let guidance_scale = 7.5f64;
        let res = 1024usize;
        let lr = res / 8; // latent resolution = 128

        let api = Api::new().map_err(CandleError::msg)?;
        let repo = api.model(SDXL_INPAINT_REPO.to_string());
        let unet_weights = repo
            .get("unet/diffusion_pytorch_model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let vae_weights = repo
            .get("vae/diffusion_pytorch_model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        // text_encoder = CLIP-L (768), text_encoder_2 = OpenCLIP bigG (1280).
        let clip_weights = repo
            .get("text_encoder/model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let clip2_weights = repo
            .get("text_encoder_2/model.fp16.safetensors")
            .map_err(CandleError::msg)?;
        let tokenizer_path = api
            .model(SDXL_TOKENIZER_REPO.to_string())
            .get("tokenizer.json")
            .map_err(CandleError::msg)?;
        let tokenizer2_path = api
            .model(SDXL_TOKENIZER2_REPO.to_string())
            .get("tokenizer.json")
            .map_err(CandleError::msg)?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(CandleError::msg)?;
        let tokenizer2 = Tokenizer::from_file(&tokenizer2_path).map_err(CandleError::msg)?;

        // GATE RESULT (2026-06-15, RTX A4000 16 GiB, cuda:0):
        //   * `None` (no slicing): RUNS through load + into the denoise loop, peaks at
        //     ~15.1 GiB / 16 GiB, then OOMs inside a UNet forward at 1024
        //     (`CUDA_ERROR_OUT_OF_MEMORY`) after ~230s. Right at the card's edge.
        //   * `Some(0)` (candle auto attention-slicing) is BROKEN in candle 0.10.2:
        //     `sliced_attention` does `Tensor::stack(.., 0)` -> rank-4, then
        //     `reshape_batch_dim_to_heads` calls dims3() -> panics
        //     "unexpected rank, expected: 3, got: 4 ([4, 5, 4096, 64])". So slicing
        //     cannot be used as the OOM mitigation without patching candle upstream.
        // Left as `None` (the only non-panicking path) so this stays a faithful,
        // compiling record of the real call shape. Expect OOM on a 16 GiB card.
        let sd_config = StableDiffusionConfig::sdxl(None, Some(res), Some(res));
        let mut scheduler = sd_config.build_scheduler(n_steps)?;
        device.set_seed(42)?;

        // Encode prompt+negative through ONE encoder -> [2, 77, dim] (uncond, cond).
        // `cfg` is that encoder's clip::Config (clip vs clip2); `tok` its tokenizer.
        let encode = |tok: &Tokenizer,
                      cfg: &stable_diffusion::clip::Config,
                      model: &stable_diffusion::clip::ClipTextTransformer|
         -> candle_core::Result<Tensor> {
            let max = cfg.max_position_embeddings;
            let pad_id = match &cfg.pad_with {
                Some(p) => *tok.get_vocab(true).get(p.as_str()).unwrap(),
                None => *tok.get_vocab(true).get("<|endoftext|>").unwrap(),
            };
            let tokenize = |text: &str| -> candle_core::Result<Tensor> {
                let mut tokens = tok
                    .encode(text, true)
                    .map_err(CandleError::msg)?
                    .get_ids()
                    .to_vec();
                while tokens.len() < max {
                    tokens.push(pad_id);
                }
                Ok(Tensor::new(tokens.as_slice(), &device)?.unsqueeze(0)?)
            };
            let cond = model.forward(&tokenize("a red apple")?)?;
            let uncond = model.forward(&tokenize("")?)?;
            // CFG order: [uncond, cond].
            Tensor::cat(&[uncond, cond], 0)
        };

        // Build CLIP transformers in F32 (mirrors candle's example), encode, then
        // concat the TWO encoders along the FEATURE dim -> [2, 77, 2048], cast F16.
        let clip1 = stable_diffusion::build_clip_transformer(
            &sd_config.clip,
            &clip_weights,
            &device,
            DType::F32,
        )?;
        let emb1 = encode(&tokenizer, &sd_config.clip, &clip1)?;
        drop(clip1); // free encoder-1 weights before loading encoder 2 (VRAM hygiene)

        let clip2_cfg = sd_config.clip2.clone().expect("sdxl config has clip2");
        let clip2 = stable_diffusion::build_clip_transformer(
            &clip2_cfg,
            &clip2_weights,
            &device,
            DType::F32,
        )?;
        let emb2 = encode(&tokenizer2, &clip2_cfg, &clip2)?;
        drop(clip2);

        let text_embeddings = Tensor::cat(&[&emb1, &emb2], D::Minus1)?.to_dtype(dtype)?;
        // Sanity: [2, 77, 2048] (768 + 1280) — the SDXL encoder_hidden_states.
        assert_eq!(
            text_embeddings.dims3()?.2,
            2048,
            "SDXL cross-attn dim must be 2048"
        );

        let vae = sd_config.build_vae(&vae_weights, &device, dtype)?;
        // 9 input channels: inpaint UNet ([latents(4), mask(1), masked_latents(4)]).
        let unet = sd_config.build_unet(&unet_weights, &device, 9, false, dtype)?;

        // 1024×1024 gradient image (in [-1,1]) + centred 384..640 square mask.
        let mut img_data = vec![0f32; 3 * res * res];
        for y in 0..res {
            for x in 0..res {
                img_data[(0 * res + y) * res + x] = x as f32 / (res as f32 - 1.0) * 2.0 - 1.0;
                img_data[(1 * res + y) * res + x] = y as f32 / (res as f32 - 1.0) * 2.0 - 1.0;
                img_data[(2 * res + y) * res + x] = 0.0;
            }
        }
        let image = Tensor::from_vec(img_data, (1, 3, res, res), &device)?.to_dtype(dtype)?;
        let mut mask_data = vec![0f32; res * res];
        for y in 384..640usize {
            for x in 384..640usize {
                mask_data[y * res + x] = 1.0;
            }
        }
        let mask = Tensor::from_vec(mask_data, (1, 1, res, res), &device)?.to_dtype(dtype)?;

        // masked_image = image * (1 - mask); VAE-encode -> masked_image_latents (4ch).
        let keep = mask.affine(-1.0, 1.0)?.repeat(&[1, 3, 1, 1])?;
        let masked_image = (&image * &keep)?;
        let masked_image_latents =
            (vae.encode(&masked_image)?.sample()? * VAE_SCALE)?.to_dtype(dtype)?;
        let mask_latent = mask.interpolate2d(lr, lr)?.to_dtype(dtype)?;
        let mask_cfg = Tensor::cat(&[&mask_latent, &mask_latent], 0)?;
        let masked_latents_cfg = Tensor::cat(&[&masked_image_latents, &masked_image_latents], 0)?;

        let latents = Tensor::randn(0f32, 1f32, (1, 4, lr, lr), &device)?;
        let mut latents = (latents * scheduler.init_noise_sigma())?.to_dtype(dtype)?;

        // Snapshot the masked-region init latents to prove they actually change.
        let centre_before = latents
            .i((
                0,
                ..,
                (lr / 2 - 4)..(lr / 2 + 4),
                (lr / 2 - 4)..(lr / 2 + 4),
            ))?
            .to_dtype(DType::F32)?
            .mean_all()?
            .to_scalar::<f32>()?;

        let timesteps = scheduler.timesteps().to_vec();
        for &t in timesteps.iter() {
            let model_in = Tensor::cat(&[&latents, &latents], 0)?;
            let model_in = scheduler.scale_model_input(model_in, t)?;
            // 9ch inpaint input: concat([latents(4), mask(1), masked_latents(4)], dim=1).
            let model_in = Tensor::cat(&[&model_in, &mask_cfg, &masked_latents_cfg], 1)?;
            // SDXL UNet forward: SAME 3-arg call as SD-1.5 (no pooled/time_ids in candle 0.10.2).
            let noise_pred = unet.forward(&model_in, t as f64, &text_embeddings)?;
            let chunks = noise_pred.chunk(2, 0)?;
            let (uncond, cond) = (&chunks[0], &chunks[1]);
            let noise_pred = (uncond + ((cond - uncond)? * guidance_scale)?)?;
            latents = scheduler.step(&noise_pred, t, &latents)?;
        }

        // VAE-decode to RGB in [0,1], [1,3,1024,1024].
        let decoded = vae.decode(&(&latents / VAE_SCALE)?)?;
        let decoded = ((decoded / 2.0)? + 0.5)?.clamp(0f32, 1f32)?;
        let decoded = decoded.to_dtype(DType::F32)?.to_device(&Device::Cpu)?;

        let patch_mean = |t: &Tensor, y: usize, x: usize, s: usize| -> candle_core::Result<f32> {
            Ok(t.i((0, .., y..y + s, x..x + s))?
                .mean_all()?
                .to_scalar::<f32>()?)
        };
        // Masked centre (≈ pixel 384..640) must be finite + in range, and the latents
        // there must have moved from their noise init (the loop actually denoised).
        let centre_after = latents
            .i((
                0,
                ..,
                (lr / 2 - 4)..(lr / 2 + 4),
                (lr / 2 - 4)..(lr / 2 + 4),
            ))?
            .to_dtype(DType::F32)?
            .mean_all()?
            .to_scalar::<f32>()?;
        let centre_px = patch_mean(&decoded, 448, 448, 128)?;
        assert!(centre_px.is_finite() && (0.0..=1.0).contains(&centre_px));
        assert_ne!(
            centre_before, centre_after,
            "masked-region latents must change across the denoise loop"
        );
        Ok(())
    }
}
