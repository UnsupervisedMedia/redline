pub mod backend;
pub mod history;
pub mod imageops;
pub mod job;
pub mod mask;
pub mod models;
pub mod state;
pub mod ui;

#[cfg(feature = "native-inpaint")]
pub mod lama_ort;

#[cfg(feature = "native-generative")]
pub mod sd_candle;

pub use state::NativeInpaintState;

/// Called each frame while `AppMode::NativeInpaint` is active.
pub fn show(ctx: &eframe::egui::Context, state: &mut crate::state::AppState) {
    state.native_inpaint.drain_msgs();
    ui::show(ctx, state);
}
