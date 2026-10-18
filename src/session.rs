//! Session restore + autosave.
//!
//! Persists the set of open tabs (their source image, name, annotations, and
//! cheap view state) to `<data_dir>/redline/session.json` so a crash or quit
//! doesn't lose in-progress work. Annotations reuse the existing sidecar
//! serialization (`build_sidecar` / `import_sidecar`); the source image is
//! stored either by *path* (preferred, when the tab was opened from a file that
//! still exists) or as a PNG blob under `<data_dir>/redline/session/`.
//!
//! Restore is gated behind `Settings::restore_session` (default true) so a user
//! can opt out. Robustness is a hard requirement: a missing/corrupt session
//! file yields an empty session silently, and an individual tab whose image
//! can't be reloaded is skipped with an `eprintln!` rather than aborting the
//! whole restore.

use crate::export::{build_sidecar, import_sidecar};
use crate::state::{AppState, CanvasState, Tab};
use crate::types::SidecarExport;
use egui::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Minimum wall-clock gap between autosave flushes while dirty.
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Per-tab cap on a persisted source-image PNG blob. A blob larger than this is
/// dropped *only when the tab has no file path to fall back on*; such a tab is
/// then skipped on restore (its annotations are not worth restoring without the
/// underlying image). 24 MiB comfortably covers a 4K screenshot while keeping
/// session.json from ballooning if many huge pasted images are open.
const MAX_BLOB_BYTES: usize = 24 * 1024 * 1024;

/// Where a tab's source pixels live for restore purposes.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub enum SourceRef {
    /// Reload via `image::open` from this path on restore.
    Path(PathBuf),
    /// Decode this PNG file (relative name under the session blob dir).
    Blob(String),
    /// Annotation-only tab with no raster source (e.g. a blank canvas). Restored
    /// as a canvas sized to `canvas_size` with no texture.
    None,
}

/// One persisted tab.
#[derive(Serialize, Deserialize, Debug)]
pub struct TabSession {
    pub label: String,
    pub source: SourceRef,
    /// Logical canvas size in pixels. Used to rebuild annotation-only canvases
    /// and to sanity-check restored sources.
    pub canvas_size: [f32; 2],
    pub view_zoom: f32,
    pub view_pan: [f32; 2],
    /// Annotations, serialized exactly like an on-disk sidecar.
    pub sidecar: SidecarExport,
}

/// The full serializable session.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct SessionState {
    pub active_tab: usize,
    pub tabs: Vec<TabSession>,
}

// ── On-disk locations ──────────────────────────────────────────────────────────

/// `<data_dir>/redline/` (created if missing). Mirrors `config.rs` / `state.rs`.
fn session_dir() -> Option<PathBuf> {
    let mut p = dirs_next::data_dir()?;
    p.push("redline");
    std::fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn session_file() -> Option<PathBuf> {
    let mut p = session_dir()?;
    p.push("session.json");
    Some(p)
}

/// `<data_dir>/redline/session/` — holds PNG blobs for pathless tabs.
fn blob_dir() -> Option<PathBuf> {
    let mut p = session_dir()?;
    p.push("session");
    std::fs::create_dir_all(&p).ok()?;
    Some(p)
}

/// Atomic write: temp file + rename, mirroring `export::write_atomic`.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = {
        let mut name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp-{}", std::process::id()));
        path.with_file_name(name)
    };
    match std::fs::write(&tmp, bytes) {
        Ok(()) => match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(path, bytes).or(Err(e))
        }
    }
}

/// Encode a `ColorImage` to PNG bytes.
fn encode_png(ci: &egui::ColorImage) -> Option<Vec<u8>> {
    let [w, h] = ci.size;
    let mut raw = Vec::with_capacity(w * h * 4);
    for px in &ci.pixels {
        raw.extend_from_slice(&[px.r(), px.g(), px.b(), px.a()]);
    }
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, raw)?;
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .ok()?;
    Some(bytes)
}

// ── Manager ─────────────────────────────────────────────────────────────────────

/// Holds debounce/dirty bookkeeping and a cache of encoded source PNGs so
/// autosave doesn't re-encode unchanged images every flush. Lives in `AppState`.
#[derive(Default)]
pub struct SessionManager {
    /// Set when annotations/tabs change; cleared on flush.
    dirty: bool,
    /// Last successful (or attempted) flush time.
    last_flush: Option<Instant>,
    /// Next session_id to hand out to a canvas (1-based; 0 == unassigned).
    next_id: u64,
    /// session_id -> (source_gen the blob was encoded at, blob filename).
    blob_cache: HashMap<u64, (u64, String)>,
}

impl SessionManager {
    /// Mark the session dirty so the next eligible flush persists it.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Assign a stable session id to a canvas if it doesn't have one yet.
    fn ensure_id(&mut self, canvas: &mut CanvasState) -> u64 {
        if canvas.session_id == 0 {
            self.next_id += 1;
            canvas.session_id = self.next_id;
        }
        self.next_id = self.next_id.max(canvas.session_id);
        canvas.session_id
    }
}

/// Debounced autosave entry point — call once per frame from `App::update`.
/// Flushes at most every `FLUSH_INTERVAL` and only when dirty.
pub fn maybe_autosave(state: &mut AppState) {
    if !state.settings.restore_session || !state.session.dirty {
        return;
    }
    let now = Instant::now();
    let due = state
        .session
        .last_flush
        .map(|t| now.duration_since(t) >= FLUSH_INTERVAL)
        .unwrap_or(true);
    if due {
        save_now(state);
    }
}

/// Force an immediate flush (used on clean exit via `App::save`). Clears dirty.
pub fn save_now(state: &mut AppState) {
    if !state.settings.restore_session {
        return;
    }
    state.session.last_flush = Some(Instant::now());
    state.session.dirty = false;

    let Some(session) = build_session(state) else {
        return;
    };
    let Some(path) = session_file() else {
        return;
    };
    match serde_json::to_vec_pretty(&session) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(&path, &bytes) {
                eprintln!("session: failed to write {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("session: serialize failed: {e}"),
    }
}

/// Build the `SessionState` from the live `AppState`, encoding/caching source
/// blobs as needed. Walks tabs in logical order so `active_tab` lines up.
fn build_session(state: &mut AppState) -> Option<SessionState> {
    let count = state.tab_count();
    let active = state.active_tab_pos;
    let mut tabs = Vec::with_capacity(count);

    for i in 0..count {
        // Pull the i-th tab's (canvas, label, view) into scope. The active tab
        // lives directly on AppState; inactive ones in `inactive_tabs`.
        let tab_session = if i == active {
            let id = state.session.ensure_id(&mut state.canvas);
            let source = persist_source(&mut state.session, id, &state.canvas);
            let sidecar = build_sidecar(&state.canvas);
            TabSession {
                label: state.tab_label.clone(),
                source,
                canvas_size: [state.canvas.canvas_size.x, state.canvas.canvas_size.y],
                view_zoom: state.view_zoom,
                view_pan: [state.view_pan.x, state.view_pan.y],
                sidecar,
            }
        } else {
            let idx = if i < active { i } else { i - 1 };
            // Split borrow: take id allocation first, then read.
            let id = {
                let canvas = &mut state.inactive_tabs[idx].canvas;
                state.session.ensure_id(canvas)
            };
            let tab = &state.inactive_tabs[idx];
            let source = persist_source(&mut state.session, id, &tab.canvas);
            let sidecar = build_sidecar(&tab.canvas);
            TabSession {
                label: tab.label.clone(),
                source,
                canvas_size: [tab.canvas.canvas_size.x, tab.canvas.canvas_size.y],
                view_zoom: tab.view_zoom,
                view_pan: [tab.view_pan.x, tab.view_pan.y],
                sidecar,
            }
        };
        tabs.push(tab_session);
    }

    Some(SessionState {
        active_tab: active.min(count.saturating_sub(1)),
        tabs,
    })
}

/// Decide how to persist a tab's source image, writing/reusing a PNG blob when
/// there's no usable file path. Cheap when nothing changed: the blob is only
/// re-encoded when `source_gen` differs from the cached value.
fn persist_source(mgr: &mut SessionManager, id: u64, canvas: &CanvasState) -> SourceRef {
    // Prefer a still-existing source path — no bytes to write.
    if let Some(p) = &canvas.source_path {
        if p.exists() {
            return SourceRef::Path(p.clone());
        }
    }
    // No usable path: persist pixels as a PNG blob, reusing the cache when the
    // source hasn't changed since the last encode.
    let Some(ci) = &canvas.color_image else {
        return SourceRef::None;
    };
    if let Some((gen, name)) = mgr.blob_cache.get(&id) {
        if *gen == canvas.source_gen {
            // Confirm the file still exists; otherwise fall through to re-encode.
            if blob_dir().map(|d| d.join(name).exists()).unwrap_or(false) {
                return SourceRef::Blob(name.clone());
            }
        }
    }
    let Some(bytes) = encode_png(ci) else {
        eprintln!("session: failed to encode source image for tab {id}");
        return SourceRef::None;
    };
    if bytes.len() > MAX_BLOB_BYTES {
        eprintln!(
            "session: source image for tab {id} is {} bytes (> {MAX_BLOB_BYTES} cap); skipping blob",
            bytes.len()
        );
        return SourceRef::None;
    }
    let name = format!("img-{id}.png");
    let Some(dir) = blob_dir() else {
        return SourceRef::None;
    };
    let path = dir.join(&name);
    if let Err(e) = write_atomic(&path, &bytes) {
        eprintln!("session: failed to write blob {path:?}: {e}");
        return SourceRef::None;
    }
    mgr.blob_cache.insert(id, (canvas.source_gen, name.clone()));
    SourceRef::Blob(name)
}

// ── Restore ─────────────────────────────────────────────────────────────────────

/// Load the persisted session from disk. Returns `None` (and removes nothing)
/// when the file is missing or corrupt — the caller then starts fresh silently.
pub fn load_session() -> Option<SessionState> {
    let path = session_file()?;
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<SessionState>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("session: ignoring corrupt session.json: {e}");
            None
        }
    }
}

/// Rebuild tabs from a `SessionState` into `state`, reloading each source image
/// and recreating its texture via `ctx`. Tabs whose image can't be reloaded are
/// skipped with an `eprintln!`. Must run where an egui `Context` is available
/// (the eframe creation closure).
pub fn restore_into(state: &mut AppState, ctx: &Context, session: SessionState) {
    if session.tabs.is_empty() {
        return;
    }
    let want_active = session.active_tab;
    let mut restored: Vec<Tab> = Vec::new();

    for (i, ts) in session.tabs.into_iter().enumerate() {
        match build_canvas(&ts, ctx) {
            Some(canvas) => restored.push(Tab {
                label: ts.label,
                canvas,
                view_zoom: ts.view_zoom,
                view_pan: egui::vec2(ts.view_pan[0], ts.view_pan[1]),
            }),
            None => eprintln!(
                "session: skipping tab {i} ('{}') — could not restore",
                ts.label
            ),
        }
    }

    if restored.is_empty() {
        return;
    }

    // Activate the requested tab if it survived; else fall back to the first.
    let active = want_active.min(restored.len() - 1);
    // Remove the active one and install it on AppState; the rest become inactive
    // in logical order around `active_tab_pos`.
    let active_tab = restored.remove(active);
    state.tab_label = active_tab.label;
    state.canvas = active_tab.canvas;
    state.view_zoom = active_tab.view_zoom;
    state.view_pan = active_tab.view_pan;
    state.inactive_tabs = restored;
    state.active_tab_pos = active;
}

/// Rebuild a single `CanvasState` from a persisted tab. `None` if the source is
/// required but unrecoverable.
fn build_canvas(ts: &TabSession, ctx: &Context) -> Option<CanvasState> {
    let mut canvas = CanvasState::default();
    canvas.canvas_size = egui::vec2(ts.canvas_size[0], ts.canvas_size[1]);

    // Recreate the raster source + texture.
    match &ts.source {
        SourceRef::Path(p) => {
            let img = match image::open(p) {
                Ok(img) => img,
                Err(e) => {
                    eprintln!("session: source path {p:?} unreadable: {e}");
                    return None;
                }
            };
            install_image(&mut canvas, img, ctx);
            canvas.source_path = Some(p.clone());
        }
        SourceRef::Blob(name) => {
            let dir = blob_dir()?;
            let bytes = match std::fs::read(dir.join(name)) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("session: blob {name} unreadable: {e}");
                    return None;
                }
            };
            let img = match image::load_from_memory(&bytes) {
                Ok(img) => img,
                Err(e) => {
                    eprintln!("session: blob {name} corrupt: {e}");
                    return None;
                }
            };
            install_image(&mut canvas, img, ctx);
        }
        SourceRef::None => {
            // Annotation-only / blank canvas: no texture to rebuild.
        }
    }

    // Load annotations from the embedded sidecar via a temp file so we can reuse
    // `import_sidecar` verbatim (it reads from a path).
    if let Ok(json) = serde_json::to_vec(&ts.sidecar) {
        let tmp =
            std::env::temp_dir().join(format!("redline-session-{}.json", uuid::Uuid::new_v4()));
        if std::fs::write(&tmp, &json).is_ok() {
            if let Err(e) = import_sidecar(&tmp, &mut canvas) {
                eprintln!("session: annotation restore failed: {e}");
            }
            let _ = std::fs::remove_file(&tmp);
        }
    }

    Some(canvas)
}

/// Upload an image into `canvas` as source pixels + texture. Mirrors
/// `loader::apply_image` but writes straight into a `CanvasState` (no AppState).
fn install_image(canvas: &mut CanvasState, img: image::DynamicImage, ctx: &Context) {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let pixels = rgba.into_raw();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
    canvas.canvas_size = egui::vec2(w as f32, h as f32);
    canvas.texture = Some(crate::texture_upload::load_texture_fitted(
        ctx,
        "canvas",
        color_image.clone(),
        egui::TextureOptions::default(),
    ));
    canvas.color_image = Some(color_image);
    canvas.mark_source_changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SidecarExport;

    fn sample_sidecar() -> SidecarExport {
        SidecarExport {
            canvas_size: [640.0, 480.0],
            captured_at: "2026-01-01T00:00:00Z".into(),
            focus_region: None,
            annotations: Vec::new(),
            clipboard_text: String::new(),
            layers: None,
            #[cfg(feature = "local-ai")]
            agent_memory: std::collections::HashMap::new(),
            #[cfg(feature = "local-ai")]
            shared_notes: Vec::new(),
            #[cfg(feature = "local-ai")]
            agent_bookmarks: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn session_state_roundtrips() {
        let session = SessionState {
            active_tab: 1,
            tabs: vec![
                TabSession {
                    label: "First".into(),
                    source: SourceRef::Path(PathBuf::from("/tmp/a.png")),
                    canvas_size: [800.0, 600.0],
                    view_zoom: 1.5,
                    view_pan: [10.0, -4.0],
                    sidecar: sample_sidecar(),
                },
                TabSession {
                    label: "Second".into(),
                    source: SourceRef::Blob("img-2.png".into()),
                    canvas_size: [640.0, 480.0],
                    view_zoom: 1.0,
                    view_pan: [0.0, 0.0],
                    sidecar: sample_sidecar(),
                },
            ],
        };

        let json = serde_json::to_string(&session).expect("serialize");
        let back: SessionState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.active_tab, session.active_tab);
        assert_eq!(back.tabs.len(), 2);
        assert_eq!(back.tabs[0].label, "First");
        assert_eq!(
            back.tabs[0].source,
            SourceRef::Path(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(back.tabs[0].view_zoom, 1.5);
        assert_eq!(back.tabs[0].view_pan, [10.0, -4.0]);
        assert_eq!(back.tabs[0].canvas_size, [800.0, 600.0]);
        assert_eq!(back.tabs[1].source, SourceRef::Blob("img-2.png".into()));
        assert_eq!(back.tabs[1].sidecar.canvas_size, [640.0, 480.0]);
    }

    #[test]
    fn session_sidecar_preserves_second_layer() {
        // Session autosave/restore persists each tab's canvas via build_sidecar
        // and rehydrates via import_sidecar (see persist + restore paths above),
        // so it inherits sidecar v2 automatically. Verify a second layer survives.
        use crate::command::CommandExecutor;
        use crate::export::{build_sidecar, import_sidecar};
        use crate::state::CanvasState;
        use crate::types::{Command, ParticipantId};

        let mut c = CanvasState::default();
        let l2 = uuid::Uuid::new_v4();
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::CreateLayer {
                id: l2,
                name: "Layer 2".into(),
            },
        );
        assert_eq!(c.document.layers().len(), 2);

        let sc = build_sidecar(&c);
        let path = std::env::temp_dir().join(format!("redline-sess-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&sc).unwrap()).unwrap();
        let mut c2 = CanvasState::default();
        import_sidecar(&path, &mut c2).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(c2.document.layers().len(), 2);
        assert_eq!(
            c2.document.get(c2.document.layers()[1]).unwrap().name,
            "Layer 2"
        );
    }

    #[test]
    fn corrupt_session_bytes_yield_none() {
        // Garbage JSON must not panic — `from_slice` returns Err, surfaced as None
        // by the same logic `load_session` uses.
        let bad = b"{ this is not valid json ]";
        let parsed: Result<SessionState, _> = serde_json::from_slice(bad);
        assert!(parsed.is_err());
    }

    #[test]
    fn empty_session_is_default() {
        let s = SessionState::default();
        assert_eq!(s.active_tab, 0);
        assert!(s.tabs.is_empty());
    }
}
