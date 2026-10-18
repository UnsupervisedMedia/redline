use std::path::{Path, PathBuf};

use super::types::ZoneAction;

#[derive(Debug)]
pub enum FileOpError {
    NotAllowed(PathBuf),
    SourceNotFound(PathBuf),
    Io(std::io::Error),
    Image(image::ImageError),
}

impl std::fmt::Display for FileOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileOpError::NotAllowed(p) => write!(f, "path not in allowed dirs: {}", p.display()),
            FileOpError::SourceNotFound(p) => write!(f, "source not found: {}", p.display()),
            FileOpError::Io(e) => write!(f, "IO: {e}"),
            FileOpError::Image(e) => write!(f, "image: {e}"),
        }
    }
}

impl From<std::io::Error> for FileOpError {
    fn from(e: std::io::Error) -> Self {
        FileOpError::Io(e)
    }
}

impl From<image::ImageError> for FileOpError {
    fn from(e: image::ImageError) -> Self {
        FileOpError::Image(e)
    }
}

/// Check that `target` is within one of the `allowed` directories.
fn check_allowed(target: &Path, allowed: &[PathBuf]) -> Result<(), FileOpError> {
    // If no allowed dirs are configured, accept any absolute path.
    if allowed.is_empty() {
        return Ok(());
    }
    for dir in allowed {
        if target.starts_with(dir) {
            return Ok(());
        }
    }
    Err(FileOpError::NotAllowed(target.to_owned()))
}

/// Execute a zone action on the given image paths.
/// Returns the new paths of the images (may change for move/convert).
pub fn execute_zone_action(
    image_paths: &[PathBuf],
    action: &ZoneAction,
    allowed_dirs: &[PathBuf],
) -> Vec<Result<PathBuf, FileOpError>> {
    image_paths
        .iter()
        .map(|src| execute_single(src, action, allowed_dirs))
        .collect()
}

fn execute_single(
    src: &Path,
    action: &ZoneAction,
    allowed: &[PathBuf],
) -> Result<PathBuf, FileOpError> {
    if !src.exists() {
        return Err(FileOpError::SourceNotFound(src.to_owned()));
    }
    match action {
        ZoneAction::MoveToFolder { target } => {
            check_allowed(target, allowed)?;
            std::fs::create_dir_all(target)?;
            let dest = unique_dest(target, src);
            if std::fs::rename(src, &dest).is_err() {
                // Cross-device rename; fall back to copy+delete.
                std::fs::copy(src, &dest)?;
                std::fs::remove_file(src)?;
            }
            Ok(dest)
        }

        ZoneAction::CopyToFolder { target } => {
            check_allowed(target, allowed)?;
            std::fs::create_dir_all(target)?;
            let dest = unique_dest(target, src);
            std::fs::copy(src, &dest)?;
            Ok(dest)
        }

        ZoneAction::AddTags { .. } => {
            // Tags are metadata only — no filesystem change.
            Ok(src.to_owned())
        }

        ZoneAction::Resize { width, height } => {
            check_allowed(src.parent().unwrap_or(Path::new(".")), allowed)?;
            let img = image::open(src)?;
            let resized = img.resize_exact(*width, *height, image::imageops::FilterType::Lanczos3);
            resized.save(src)?;
            Ok(src.to_owned())
        }

        ZoneAction::ConvertFormat { format } => {
            check_allowed(src.parent().unwrap_or(Path::new(".")), allowed)?;
            let img = image::open(src)?;
            let ext = format.to_string();
            let dest = src.with_extension(&ext);
            img.save(&dest)?;
            // Remove original if different path.
            if dest != src {
                let _ = std::fs::remove_file(src);
            }
            Ok(dest)
        }

        ZoneAction::Delete => {
            check_allowed(src.parent().unwrap_or(Path::new(".")), allowed)?;
            std::fs::remove_file(src)?;
            Ok(src.to_owned())
        }

        // AI actions are placeholders — no-op for now.
        ZoneAction::DuplicateCheck | ZoneAction::SimilarityGroup | ZoneAction::FaceDetection => {
            Ok(src.to_owned())
        }
    }
}

/// Produce a destination path that doesn't collide with an existing file.
fn unique_dest(dir: &Path, src: &Path) -> PathBuf {
    let name = src.file_name().unwrap_or_default();
    let dest = dir.join(name);
    if !dest.exists() {
        return dest;
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
    let mut n = 1u32;
    loop {
        let candidate = if ext.is_empty() {
            dir.join(format!("{stem}_{n}"))
        } else {
            dir.join(format!("{stem}_{n}.{ext}"))
        };
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Scan a folder for images and return (path, sha256_hash) pairs.
/// Runs synchronously; call from a background thread or rayon for parallelism.
pub fn scan_folder(dir: &Path) -> Vec<(PathBuf, [u8; 32])> {
    use rayon::prelude::*;
    use sha2::{Digest, Sha256};

    let extensions = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "heif"];
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = entry.into_path();
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if extensions.contains(&ext.to_lowercase().as_str()) {
                paths.push(p);
            }
        }
    }

    paths
        .par_iter()
        .map(|p| {
            let hash = std::fs::read(p)
                .map(|bytes| {
                    let mut h = Sha256::new();
                    h.update(&bytes);
                    h.finalize().into()
                })
                .unwrap_or([0u8; 32]);
            (p.clone(), hash)
        })
        .collect()
}
