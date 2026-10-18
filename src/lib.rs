// Version string stamped at build time by build.rs (git describe, or the Cargo
// version when git is unavailable). e.g. "v0.2.0" or "v0.2.0-3-g50c9393-dirty".
pub const VERSION: &str = env!("REDLINE_VERSION");

// Public module declarations for integration tests
pub mod bezier;
pub mod canvas;
pub mod canvas_sort;
pub mod command;
pub mod config;
pub mod export;
pub mod flatten;
pub mod image_ops;
pub mod lama_cleaner;
pub mod loader;
pub mod native_inpaint;
pub mod pathops;
pub mod portal_capture;
pub mod recents;
pub mod region_select;
pub mod render_xform;
pub mod scene;
pub mod session;
pub mod shortcuts;
pub mod smoothing;
pub mod stamps;
pub mod state;
pub mod svg_emit;
pub mod texture_upload;
pub mod theme;
pub mod toolbar;
pub mod tools;
pub mod transform;
pub mod types;
pub mod vectorize;
pub mod vectorize_worker;

#[cfg(feature = "agent-server")]
pub mod agent;

#[cfg(feature = "agent-server")]
pub mod text_render;

#[cfg(feature = "local-ai")]
pub mod local_ai;
