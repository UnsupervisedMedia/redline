use std::path::PathBuf;

use serde::Deserialize;
// Hashing is only exercised by the `native-inpaint` downloader (`download_to`)
// and the unit tests; unused otherwise.
#[cfg_attr(not(feature = "native-inpaint"), allow(unused_imports))]
use sha2::{Digest, Sha256};

#[cfg(feature = "native-inpaint")]
use super::backend::InpaintError;
#[cfg(feature = "native-inpaint")]
use std::path::Path;

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    #[default]
    Removal,
    Generative,
}

// A manifest DTO: individual fields are read by different feature paths
// (removal/ONNX vs generative/hf-hub) and the unit tests, so any given build
// configuration leaves some unread. (Mirrors the `size_bytes` field below.)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub kind: ModelKind,
    // Removal (ONNX) fields — present for removal models, defaulted otherwise.
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)] // parsed from the manifest; reserved for download pre-alloc / progress total
    pub size_bytes: u64,
    // Generative (hf-hub) fields — present for generative models only.
    #[serde(default)]
    pub hf_repo: Option<String>,
    #[serde(default)]
    pub base_resolution: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub models: Vec<ModelEntry>,
}

impl Manifest {
    pub fn load_embedded() -> Self {
        let raw = include_str!("models.json");
        serde_json::from_str(raw).expect("models.json is valid")
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }
}

// `model_dir`/`model_path` resolve download locations for the `native-inpaint`
// model fetcher (`ensure_model`); unused otherwise.
/// Where downloaded models live: ~/.local/share/redline/native-models/
#[cfg_attr(not(feature = "native-inpaint"), allow(dead_code))]
pub fn model_dir() -> PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("redline")
        .join("native-models")
}

#[cfg_attr(not(feature = "native-inpaint"), allow(dead_code))]
pub fn model_path(entry: &ModelEntry) -> PathBuf {
    model_dir().join(&entry.filename)
}

#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(&h.finalize())
}

// Used by the `native-inpaint` downloader and the `#[cfg(test)]` `sha256_hex`.
#[cfg_attr(not(feature = "native-inpaint"), allow(dead_code))]
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
#[derive(Debug)]
// Fields are diagnostic detail surfaced via `Debug`; the test only matches the
// variant, so dead-code analysis (which ignores derived `Debug`) flags them.
#[allow(dead_code)]
pub enum VerifyError {
    Mismatch { expected: String, got: String },
}

#[cfg(test)]
pub fn verify_bytes(bytes: &[u8], expected_sha256: &str) -> Result<(), VerifyError> {
    let got = sha256_hex(bytes);
    if got.eq_ignore_ascii_case(expected_sha256) {
        Ok(())
    } else {
        Err(VerifyError::Mismatch {
            expected: expected_sha256.to_string(),
            got,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_and_finds_model() {
        let m = Manifest::load_embedded();
        let entry = m.get("lama-fp32").expect("lama-fp32 present");
        assert_eq!(entry.filename, "lama_fp32.onnx");
        assert!(entry.url.starts_with("https://"));
    }

    #[test]
    fn manifest_parses_generative_entry() {
        let m = Manifest::load_embedded();
        let lama = m.get("lama-fp32").expect("lama present");
        assert_eq!(lama.kind, ModelKind::Removal); // default when absent
        let sd = m.get("sd15-inpaint").expect("sd15 present");
        assert_eq!(sd.kind, ModelKind::Generative);
        assert_eq!(
            sd.hf_repo.as_deref(),
            Some("stable-diffusion-v1-5/stable-diffusion-inpainting")
        );
        assert_eq!(sd.base_resolution, Some(512));
    }

    #[test]
    fn sha256_hex_of_known_bytes() {
        let got = sha256_hex(b"abc");
        assert_eq!(
            got,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn verify_rejects_wrong_digest() {
        let r = verify_bytes(b"abc", "deadbeef");
        assert!(matches!(r, Err(VerifyError::Mismatch { .. })));
    }
}

/// Stream `url` to `dest`, calling `on_progress(fraction 0..=1)`, hashing as
/// bytes arrive and verifying sha256 before the atomic rename.
///
/// Streams straight to a unique `.part` file on disk (never buffering the whole
/// ~200MB body in RAM) and hashes incrementally, so a malicious `Content-Length`
/// can't drive a giant allocation.
#[cfg(feature = "native-inpaint")]
pub fn download_to(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    mut on_progress: impl FnMut(f32),
) -> Result<(), InpaintError> {
    use std::io::{BufWriter, Read, Write};

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| InpaintError::DownloadFailed(e.to_string()))?;
    }
    let mut resp =
        reqwest::blocking::get(url).map_err(|e| InpaintError::DownloadFailed(e.to_string()))?;
    let total = resp.content_length().unwrap_or(0);

    // Unique temp name (pid + dest name) so concurrent RedLine instances don't
    // clobber each other's partial download.
    let pid = std::process::id();
    let stem = dest.file_name().and_then(|s| s.to_str()).unwrap_or("model");
    let tmp = dest.with_file_name(format!(".{stem}.{pid}.part"));

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    {
        let file =
            std::fs::File::create(&tmp).map_err(|e| InpaintError::DownloadFailed(e.to_string()))?;
        let mut writer = BufWriter::new(file);
        let mut buf = [0u8; 65536];
        loop {
            let n = resp.read(&mut buf).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                InpaintError::DownloadFailed(e.to_string())
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            if let Err(e) = writer.write_all(&buf[..n]) {
                let _ = std::fs::remove_file(&tmp);
                return Err(InpaintError::DownloadFailed(e.to_string()));
            }
            downloaded += n as u64;
            if total > 0 {
                on_progress((downloaded as f32 / total as f32).min(1.0));
            }
        }
        if let Err(e) = writer.flush() {
            let _ = std::fs::remove_file(&tmp);
            return Err(InpaintError::DownloadFailed(e.to_string()));
        }
        if let Ok(f) = writer.into_inner() {
            f.sync_all().ok();
        }
    }

    let got = hex_encode(&hasher.finalize());
    if !got.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(InpaintError::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            got,
        });
    }

    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        InpaintError::DownloadFailed(e.to_string())
    })?;
    Ok(())
}

/// Ensure the model for `id` exists locally; download it if missing.
#[cfg(feature = "native-inpaint")]
pub fn ensure_model(id: &str, on_progress: impl FnMut(f32)) -> Result<PathBuf, InpaintError> {
    let manifest = Manifest::load_embedded();
    let entry = manifest
        .get(id)
        .ok_or_else(|| InpaintError::ModelMissing(id.to_string()))?;
    let path = model_path(entry);
    if path.exists() {
        return Ok(path);
    }
    download_to(&entry.url, &path, &entry.sha256, on_progress)?;
    Ok(path)
}

#[cfg(all(test, feature = "native-inpaint"))]
mod download_tests {
    use super::*;

    #[test]
    fn download_writes_file_and_verifies() {
        let body = b"hello-model".to_vec();
        let expected = sha256_hex(&body);
        let (addr, _join) = spawn_stub(body.clone());

        let dir = std::env::temp_dir().join(format!("redline-test-{}", expected));
        let dest = dir.join("m.bin");
        let _ = std::fs::remove_dir_all(&dir);

        let res = download_to(&format!("http://{addr}/m.bin"), &dest, &expected, |_p| {});
        assert!(res.is_ok(), "download failed: {res:?}");
        assert!(dest.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Minimal HTTP/1.1 stub: serves `body` once, returns its bound addr.
    fn spawn_stub(body: Vec<u8>) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = sock.write_all(header.as_bytes());
                let _ = sock.write_all(&body);
            }
        });
        (addr, handle)
    }
}
