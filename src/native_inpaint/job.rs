use std::sync::mpsc;

use super::backend::{InpaintBackend, InpaintResult};

/// Messages from inference worker threads back to the UI thread.
// Several variants are only constructed by the feature-gated inference workers
// (`native-inpaint` / `native-generative`); dead in a default build.
#[cfg_attr(
    not(any(feature = "native-inpaint", feature = "native-generative")),
    allow(dead_code)
)]
pub enum NativeMsg {
    /// Model download progress, 0.0..=1.0.
    DownloadProgress(f32),
    /// A backend (model + session) finished loading; cache it for reuse so the
    /// next Remove click skips the ~200MB reload / EP re-registration. Carries the
    /// model id the worker actually LOADED, so the UI caches it under the correct
    /// label even if the model picker changed mid-job.
    BackendReady {
        backend: std::sync::Arc<dyn InpaintBackend + Send + Sync>,
        model_id: String,
    },
    /// Compute device the backend is using ("GPU (CUDA)" / "CPU"); persistent badge.
    Device(String),
    /// Current stage label during a removal (e.g. "Inferring").
    Stage(String),
    /// Inference progress, 0.0..=1.0.
    InferProgress(f32),
    /// Non-fatal informational notice for the UI (e.g. CPU-fallback warning).
    Notice(String),
    /// Inference finished successfully.
    Done(InpaintResult),
    /// A load-only request (`spawn_load`) finished: the model is resident and
    /// cached, but no inference was run. Clears `running` without a result.
    LoadDone,
    /// The removal was cancelled by the user before finishing.
    Cancelled,
    /// Any failure (download, checksum, inference).
    Error(String),
}

/// Guards a worker closure so the UI never hangs if the worker panics.
///
/// If the closure exits (including via panic unwind) without having sent a
/// terminal `Done`/`Error` message, `completed` stays `false` and `Drop`
/// sends a synthetic `Error` so `NativeInpaintState.running` can clear.
#[cfg(feature = "native-inpaint")]
struct CompletionGuard {
    tx: std::sync::mpsc::Sender<NativeMsg>,
    completed: bool,
}

#[cfg(feature = "native-inpaint")]
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.tx.send(NativeMsg::Error(
                "removal worker stopped unexpectedly (crash)".to_string(),
            ));
        }
    }
}

/// Spawn the full "ensure model → load → infer" pipeline on a worker thread.
/// `model_id` selects the manifest entry; `cancel` lets the UI abort.
#[cfg(feature = "native-inpaint")]
pub fn spawn_removal(
    cached: Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>>,
    model_id: String,
    image: image::RgbaImage,
    mask: image::GrayImage,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    use super::backend::{InpaintError, InpaintRequest, RunCtx};
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let mut guard = CompletionGuard {
            tx: tx.clone(),
            completed: false,
        };

        if cancel.load(Ordering::Relaxed) {
            guard.completed = true;
            let _ = tx.send(NativeMsg::Cancelled);
            return;
        }

        // Resolve a backend: reuse the cached one, or load it once (and cache it).
        let backend: std::sync::Arc<dyn InpaintBackend + Send + Sync> = match cached {
            Some(b) => b,
            None => {
                let tx2 = tx.clone();
                let path = match super::models::ensure_model(&model_id, move |p| {
                    let _ = tx2.send(NativeMsg::DownloadProgress(p));
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        guard.completed = true;
                        let _ = tx.send(NativeMsg::Error(e.to_string()));
                        return;
                    }
                };
                let loaded = match super::lama_ort::LamaOrt::load(path) {
                    Ok(b) => b,
                    Err(e) => {
                        guard.completed = true;
                        let _ = tx.send(NativeMsg::Error(e.to_string()));
                        return;
                    }
                };
                // Surface the compute device as a persistent badge. The EP
                // registers with soft-fail, so if CUDA isn't available ORT
                // silently uses CPU.
                let on_gpu = super::lama_ort::cuda_available();
                let _ = tx.send(NativeMsg::Device(
                    if on_gpu { "GPU (CUDA)" } else { "CPU" }.to_string(),
                ));
                if !on_gpu {
                    let _ = tx.send(NativeMsg::Notice(
                        "CUDA GPU unavailable — running on CPU (slower).".to_string(),
                    ));
                }
                let backend: std::sync::Arc<dyn InpaintBackend + Send + Sync> =
                    std::sync::Arc::new(loaded);
                // Hand a clone to the UI so subsequent removals reuse it, labelled
                // with the id this worker actually loaded.
                let _ = tx.send(NativeMsg::BackendReady {
                    backend: backend.clone(),
                    model_id: model_id.clone(),
                });
                backend
            }
        };

        let req = InpaintRequest {
            image,
            mask,
            prompt: None,
            params: Default::default(),
        };
        let tx3 = tx.clone();
        let mut on_progress = |frac: f32, stage: &str| {
            let _ = tx3.send(NativeMsg::InferProgress(frac));
            let _ = tx3.send(NativeMsg::Stage(stage.to_string()));
        };
        let mut ctx = RunCtx {
            on_progress: &mut on_progress,
            cancel: &cancel,
        };
        let out = backend.run(req, &mut ctx);
        guard.completed = true;
        match out {
            Ok(res) => {
                let _ = tx.send(NativeMsg::Done(res));
            }
            Err(InpaintError::Cancelled) => {
                let _ = tx.send(NativeMsg::Cancelled);
            }
            Err(e) => {
                let _ = tx.send(NativeMsg::Error(e.to_string()));
            }
        }
    });
}

#[cfg(all(test, feature = "native-inpaint"))]
mod guard_tests {
    use super::*;

    #[test]
    fn drop_without_completion_sends_crash_error() {
        let (tx, rx) = mpsc::channel();
        {
            let guard = CompletionGuard {
                tx,
                completed: false,
            };
            drop(guard);
        }
        match rx.try_recv() {
            Ok(NativeMsg::Error(e)) => assert!(e.contains("unexpectedly")),
            other => panic!(
                "expected crash Error, got {:?}",
                other.map(|_| "non-error msg")
            ),
        }
    }

    #[test]
    fn completed_guard_sends_nothing() {
        let (tx, rx) = mpsc::channel();
        {
            let guard = CompletionGuard {
                tx,
                completed: true,
            };
            drop(guard);
        }
        assert!(
            rx.try_recv().is_err(),
            "completed guard must not send a message"
        );
    }
}

/// Guards a generative worker closure so the UI never hangs if the worker panics.
#[cfg(feature = "native-generative")]
struct GenCompletionGuard {
    tx: std::sync::mpsc::Sender<NativeMsg>,
    completed: bool,
}

#[cfg(feature = "native-generative")]
impl Drop for GenCompletionGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.tx.send(NativeMsg::Error(
                "generative worker stopped unexpectedly (crash)".to_string(),
            ));
        }
    }
}

/// Spawn the SD generative-fill pipeline on a worker thread. Mirrors
/// `spawn_removal`; loads an `SdCandle` for the selected manifest entry.
#[cfg(feature = "native-generative")]
#[allow(clippy::too_many_arguments)]
pub fn spawn_generate(
    cached: Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>>,
    model_id: String,
    image: image::RgbaImage,
    mask: image::GrayImage,
    prompt: String,
    params: super::backend::InpaintParams,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    use super::backend::{InpaintError, InpaintRequest, RunCtx};
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let mut guard = GenCompletionGuard {
            tx: tx.clone(),
            completed: false,
        };

        if cancel.load(Ordering::Relaxed) {
            guard.completed = true;
            let _ = tx.send(NativeMsg::Cancelled);
            return;
        }

        // Resolve a backend: reuse the cached one, or load it from the manifest.
        let backend: std::sync::Arc<dyn InpaintBackend + Send + Sync> = match cached {
            Some(b) => b,
            None => {
                let manifest = super::models::Manifest::load_embedded();
                let entry = match manifest.get(&model_id) {
                    Some(e) => e,
                    None => {
                        guard.completed = true;
                        let _ = tx.send(NativeMsg::Error(format!("unknown model: {model_id}")));
                        return;
                    }
                };
                let repo = entry.hf_repo.clone().unwrap_or_default();
                let res = entry.base_resolution.unwrap_or(512);
                let _ = tx.send(NativeMsg::Stage("Loading model…".into()));
                let loaded = match super::sd_candle::SdCandle::load(&repo, res, &params.scheduler) {
                    Ok(b) => b,
                    Err(e) => {
                        guard.completed = true;
                        let _ = tx.send(NativeMsg::Error(e.to_string()));
                        return;
                    }
                };
                let _ = tx.send(NativeMsg::Device("GPU (CUDA)".to_string()));
                let backend: std::sync::Arc<dyn InpaintBackend + Send + Sync> =
                    std::sync::Arc::new(loaded);
                let _ = tx.send(NativeMsg::BackendReady {
                    backend: backend.clone(),
                    model_id: model_id.clone(),
                });
                backend
            }
        };

        let req = InpaintRequest {
            image,
            mask,
            prompt: Some(prompt),
            params,
        };
        let tx3 = tx.clone();
        let mut on_progress = |frac: f32, stage: &str| {
            let _ = tx3.send(NativeMsg::InferProgress(frac));
            let _ = tx3.send(NativeMsg::Stage(stage.to_string()));
        };
        let mut ctx = RunCtx {
            on_progress: &mut on_progress,
            cancel: &cancel,
        };
        let out = backend.run(req, &mut ctx);
        guard.completed = true;
        match out {
            Ok(res) => {
                let _ = tx.send(NativeMsg::Done(res));
            }
            Err(InpaintError::Cancelled) => {
                let _ = tx.send(NativeMsg::Cancelled);
            }
            Err(e) => {
                let _ = tx.send(NativeMsg::Error(e.to_string()));
            }
        }
    });
}

/// Load (only) the selected backend onto the GPU without running any inference,
/// so the user can pre-warm a model or re-load it after an Unload. Mirrors the
/// load half of `spawn_removal`/`spawn_generate`: sends `Device` + `BackendReady`
/// (so the UI caches it), then a terminal `LoadDone`.
#[cfg(feature = "native-inpaint")]
pub fn spawn_load(
    model_id: String,
    is_generative: bool,
    scheduler: String,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let mut guard = CompletionGuard {
            tx: tx.clone(),
            completed: false,
        };

        if cancel.load(Ordering::Relaxed) {
            guard.completed = true;
            let _ = tx.send(NativeMsg::Cancelled);
            return;
        }

        let _ = tx.send(NativeMsg::Stage("Loading model…".into()));
        let backend = if is_generative {
            load_generative_backend(&model_id, &scheduler, &tx)
        } else {
            load_removal_backend(&model_id, &tx)
        };
        // On failure the helper already sent an `Error`; mark complete so the
        // guard doesn't add a synthetic "crash" message on top.
        let backend = match backend {
            Some(b) => b,
            None => {
                guard.completed = true;
                return;
            }
        };
        let _ = tx.send(NativeMsg::BackendReady {
            backend,
            model_id: model_id.clone(),
        });
        guard.completed = true;
        let _ = tx.send(NativeMsg::LoadDone);
    });
}

/// Resolve + load the ONNX removal backend (LaMa). Sends `Device`/`Notice` on
/// success, `Error` on failure; returns `None` on failure (message already sent).
#[cfg(feature = "native-inpaint")]
fn load_removal_backend(
    model_id: &str,
    tx: &mpsc::Sender<NativeMsg>,
) -> Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>> {
    let tx2 = tx.clone();
    let path = match super::models::ensure_model(model_id, move |p| {
        let _ = tx2.send(NativeMsg::DownloadProgress(p));
    }) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(NativeMsg::Error(e.to_string()));
            return None;
        }
    };
    match super::lama_ort::LamaOrt::load(path) {
        Ok(b) => {
            let on_gpu = super::lama_ort::cuda_available();
            let _ = tx.send(NativeMsg::Device(
                if on_gpu { "GPU (CUDA)" } else { "CPU" }.to_string(),
            ));
            if !on_gpu {
                let _ = tx.send(NativeMsg::Notice(
                    "CUDA GPU unavailable — running on CPU (slower).".to_string(),
                ));
            }
            Some(std::sync::Arc::new(b))
        }
        Err(e) => {
            let _ = tx.send(NativeMsg::Error(e.to_string()));
            None
        }
    }
}

/// Resolve + load the SD generative backend. Sends `Device` on success, `Error`
/// on failure; returns `None` on failure (message already sent).
#[cfg(feature = "native-generative")]
fn load_generative_backend(
    model_id: &str,
    scheduler: &str,
    tx: &mpsc::Sender<NativeMsg>,
) -> Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>> {
    let manifest = super::models::Manifest::load_embedded();
    let entry = match manifest.get(model_id) {
        Some(e) => e,
        None => {
            let _ = tx.send(NativeMsg::Error(format!("unknown model: {model_id}")));
            return None;
        }
    };
    let repo = entry.hf_repo.clone().unwrap_or_default();
    let res = entry.base_resolution.unwrap_or(512);
    match super::sd_candle::SdCandle::load(&repo, res, scheduler) {
        Ok(b) => {
            let _ = tx.send(NativeMsg::Device("GPU (CUDA)".to_string()));
            Some(std::sync::Arc::new(b))
        }
        Err(e) => {
            let _ = tx.send(NativeMsg::Error(e.to_string()));
            None
        }
    }
}

/// Stub for builds with `native-inpaint` but not `native-generative`: a generative
/// load is impossible, so report it instead of failing to compile.
#[cfg(all(feature = "native-inpaint", not(feature = "native-generative")))]
fn load_generative_backend(
    _model_id: &str,
    _scheduler: &str,
    tx: &mpsc::Sender<NativeMsg>,
) -> Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>> {
    let _ = tx.send(NativeMsg::Error(
        "native-generative feature not compiled in this build".to_string(),
    ));
    None
}

/// Build-time stub so non-feature builds still compile references to spawn_load.
#[cfg(not(feature = "native-inpaint"))]
pub fn spawn_load(
    _model_id: String,
    _is_generative: bool,
    _scheduler: String,
    _cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    let _ = tx.send(NativeMsg::Error(
        "native-inpaint feature not compiled in this build".to_string(),
    ));
}

/// Build-time stub so non-feature builds still compile references to spawn_generate.
#[cfg(not(feature = "native-generative"))]
pub fn spawn_generate(
    _cached: Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>>,
    _model_id: String,
    _image: image::RgbaImage,
    _mask: image::GrayImage,
    _prompt: String,
    _params: super::backend::InpaintParams,
    _cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    let _ = tx.send(NativeMsg::Error(
        "native-generative feature not compiled in this build".to_string(),
    ));
}

/// Build-time stub so non-feature builds still compile references to spawn_removal.
#[cfg(not(feature = "native-inpaint"))]
pub fn spawn_removal(
    _cached: Option<std::sync::Arc<dyn InpaintBackend + Send + Sync>>,
    _model_id: String,
    _image: image::RgbaImage,
    _mask: image::GrayImage,
    _cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: mpsc::Sender<NativeMsg>,
) {
    let _ = tx.send(NativeMsg::Error(
        "native-inpaint feature not compiled in this build".to_string(),
    ));
}
