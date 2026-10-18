use std::path::PathBuf;
use std::sync::mpsc;

use rayon::prelude::*;
use uuid::Uuid;

use super::types::{ThumbRequest, ThumbResult};

/// Worker entry-point. Runs on a background thread and processes thumbnail
/// requests using rayon for parallelism.
pub fn run_worker(rx: mpsc::Receiver<ThumbRequest>, tx: mpsc::SyncSender<ThumbResult>) {
    // Collect requests in batches and process them with rayon.
    // We drain whatever is queued up before processing to maximise parallelism.
    let mut batch: Vec<ThumbRequest> = Vec::new();
    loop {
        // Block until at least one request arrives.
        match rx.recv() {
            Ok(req) => batch.push(req),
            Err(_) => break, // sender dropped → exit
        }
        // Drain the rest of the batch without blocking.
        while let Ok(req) = rx.try_recv() {
            batch.push(req);
        }
        // Process the batch in parallel.
        let results: Vec<ThumbResult> = batch
            .par_drain(..)
            .map(|req| generate_thumb(req.image_id, &req.path, req.size))
            .collect();
        for res in results {
            if tx.send(res).is_err() {
                return; // receiver dropped → exit
            }
        }
    }
}

fn generate_thumb(id: Uuid, path: &PathBuf, size: u32) -> ThumbResult {
    // Try format-guessing first (reads magic bytes, works without extensions).
    // Fall back to extension-based open so both paths are attempted.
    let decoded = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.decode().ok())
        .or_else(|| image::open(path).ok());
    let rgba = decoded.map(|img| img.thumbnail(size, size).to_rgba8());
    ThumbResult { image_id: id, rgba }
}

/// Generate (or return cached) thumbnail path for a given session + file hash.
pub fn thumb_cache_path(session_id: Uuid, file_hash: &[u8; 32]) -> PathBuf {
    let hex = hex_encode(file_hash);
    let prefix = &hex[..2];
    super::persistence::thumb_dir(session_id)
        .join(prefix)
        .join(format!("{hex}.jpg"))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Save a thumbnail to disk cache.
pub fn save_thumb_to_cache(path: &PathBuf, rgba: &image::RgbaImage) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rgb = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
    rgb.save(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}
