// src/vectorize_worker.rs
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use egui::ColorImage;

use crate::state::VectorizeSettings;

pub struct VectorizeJob {
    pub image: ColorImage,
    pub settings: VectorizeSettings,
    pub response: Arc<Mutex<Option<Result<String, String>>>>,
    pub ctx: egui::Context,
}

pub struct VectorizeWorker {
    tx: mpsc::Sender<VectorizeJob>,
    _handle: thread::JoinHandle<()>,
}

impl VectorizeWorker {
    /// Spawn a long-lived worker thread that serializes vectorize jobs and
    /// drops any pending job whenever a newer one arrives (natural debouncing).
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<VectorizeJob>();
        let handle = thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                // Drain any backlog and keep only the most recent job.
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::vectorize::convert_to_svg(&job.image, &job.settings)
                }));
                let r: Result<String, String> = match result {
                    Ok(r) => r,
                    Err(_) => Err("SVG generation failed (internal panic)".into()),
                };
                if let Ok(mut slot) = job.response.lock() {
                    *slot = Some(r);
                }
                job.ctx.request_repaint();
            }
        });
        Self {
            tx,
            _handle: handle,
        }
    }

    /// Submit a job. If the worker has died (channel closed), silently drops.
    pub fn submit(&self, job: VectorizeJob) {
        let _ = self.tx.send(job);
    }
}
