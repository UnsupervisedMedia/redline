use crate::{
    state::{push_recent_color, AppState},
    types::Command,
};
use egui::Response;

/// Click anywhere on the loaded image to sample a pixel color and set it as
/// the current stroke_color. No-op if no image is loaded or the click is
/// outside the image draw rect.
pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    if !response.clicked() {
        return None;
    }
    let Some(click) = response.interact_pointer_pos() else {
        return None;
    };
    // unproject converts screen → image-pixel space; use those coords directly.
    let img = state.unproject(click);
    let Some(ci) = state.canvas.color_image.as_ref() else {
        return None;
    };
    let (w, h) = (ci.size[0] as f32, ci.size[1] as f32);
    if img.x < 0.0 || img.y < 0.0 || img.x >= w || img.y >= h {
        return None;
    }

    let ix = img.x as usize;
    let iy = img.y as usize;
    let sampled = ci.pixels[iy * ci.size[0] + ix];

    let opaque = egui::Color32::from_rgb(sampled.r(), sampled.g(), sampled.b());
    state.settings.stroke_color = opaque;
    push_recent_color(&mut state.settings.recent_colors, opaque);
    state.status_msg = Some(format!(
        "Picked #{:02x}{:02x}{:02x}",
        opaque.r(),
        opaque.g(),
        opaque.b()
    ));
    None
}
