//! Region/full screenshot via xdg-desktop-portal — uses the OS's native
//! capture UI (GNOME's selector on GNOME, KDE's on KDE, etc.).
//!
//! The portal call runs on a worker thread; the result is delivered via
//! a channel that the egui update loop polls each frame.

use ashpd::desktop::screenshot::Screenshot;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// Diagnostic tracing for the capture pipeline — silent unless
/// `REDLINE_CAPTURE_DEBUG=1` is set. Capture crosses the window manager, the
/// desktop portal and a worker thread, so when it misbehaves on someone else's
/// desktop an ordered log of what each step did is the only useful evidence.
pub fn capture_log(msg: impl FnOnce() -> String) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    let on = *ON.get_or_init(|| {
        std::env::var("REDLINE_CAPTURE_DEBUG")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    });
    if on {
        eprintln!("[capture] {}", msg());
    }
}

/// Either a saved file (Save button) or an in-memory image (Copy button).
pub enum Capture {
    File(PathBuf),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}
pub type CaptureResult = Result<Capture, String>;

/// Try to pull a screenshot the user just put on the clipboard via GNOME's
/// "Copy to Clipboard" button. Returns `None` if clipboard has no image.
fn try_clipboard_image() -> Option<Capture> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    Some(Capture::Image {
        width: img.width as u32,
        height: img.height as u32,
        rgba: img.bytes.into_owned(),
    })
}

/// Returns (width, height) of the current clipboard image, or None.
/// Used to fingerprint the clipboard before the portal fires so we can
/// ignore stale images in the retry loop.
fn try_clipboard_dims() -> Option<(u32, u32)> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    Some((img.width as u32, img.height as u32))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Spawn a worker that asks the desktop portal for a screenshot.
///
/// `decode` makes the worker turn a returned file into pixels before handing
/// the result over. Decoding a full-desktop PNG takes seconds (67 megapixels
/// on a three-monitor rig) — doing it on the UI thread froze the app right
/// after the user clicked Capture.
///
/// On GNOME the user can either "Save" (portal returns a file URI) or "Copy"
/// (portal returns an error and the image lands on the clipboard). We try the
/// file path first and fall back to the clipboard so both buttons work.
pub fn start(interactive: bool, decode: bool) -> Receiver<CaptureResult> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        capture_log(|| format!("portal request: interactive={interactive} decode={decode}"));
        // Snapshot the clipboard *before* the portal runs so we can skip
        // any image that was already there (avoids returning a stale
        // screenshot on the second attempt when the first attempt's image
        // is still on the clipboard).
        let pre_dims = try_clipboard_dims();

        let portal = pollster::block_on(async move {
            Screenshot::request()
                .interactive(interactive)
                .modal(false)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .response()
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    let uri = s.uri().as_str().to_string();
                    uri.strip_prefix("file://")
                        .map(|p| PathBuf::from(percent_decode(p)))
                        .ok_or_else(|| format!("non-file URI: {}", uri))
                })
        });

        let result: CaptureResult = match &portal {
            Ok(path) => {
                capture_log(|| format!("portal returned file {}", path.display()));
                Ok(Capture::File(path.clone()))
            }
            // Only the interactive portal UI has a "Copy to Clipboard" button,
            // so only there does an error plausibly mean "the shot is on the
            // clipboard". Waiting 4 s on the clipboard after a non-interactive
            // failure just delayed an error we already knew about.
            Err(portal_err) if !interactive => {
                capture_log(|| {
                    format!("portal error ({portal_err}); non-interactive, no clipboard fallback")
                });
                Err(portal_err.clone())
            }
            Err(portal_err) => {
                capture_log(|| format!("portal error ({portal_err}); trying the clipboard"));
                // On GNOME 44+, pressing Enter copies the screenshot to the
                // clipboard instead of saving to a file, so the portal errors
                // out.  GNOME writes the clipboard asynchronously — on first
                // use this can take several seconds, so we poll for up to 4 s.
                // We skip any image whose dimensions match what was on the
                // clipboard before the portal ran (stale data guard), but
                // after 1 s we accept anything so same-size captures still work.
                let mut got = None;
                for attempt in 0..80u32 {
                    if let Some(c) = try_clipboard_image() {
                        let is_fresh = match (pre_dims, &c) {
                            (None, _) => true,
                            (Some((pw, ph)), Capture::Image { width, height, .. }) => {
                                *width != pw || *height != ph || attempt >= 20
                            }
                            _ => true,
                        };
                        if is_fresh {
                            if let Capture::Image { width, height, .. } = &c {
                                capture_log(|| {
                                    format!("clipboard image {width}x{height} (attempt {attempt})")
                                });
                            }
                            got = Some(c);
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                match got {
                    Some(c) => Ok(c),
                    None => Err(format!("{portal_err} (clipboard also empty after 4s)")),
                }
            }
        };
        let result = if decode {
            result.and_then(|c| match c {
                Capture::File(path) => match image::open(&path) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        Ok(Capture::Image {
                            width: rgba.width(),
                            height: rgba.height(),
                            rgba: rgba.into_raw(),
                        })
                    }
                    Err(e) => Err(format!("reading {}: {e}", path.display())),
                },
                other => Ok(other),
            })
        } else {
            result
        };
        capture_log(|| match &result {
            Ok(Capture::File(p)) => format!("worker done: file {}", p.display()),
            Ok(Capture::Image { width, height, .. }) => {
                format!("worker done: image {width}x{height}")
            }
            Err(e) => format!("worker done: error {e}"),
        });
        let _ = tx.send(result);
    });
    rx
}
