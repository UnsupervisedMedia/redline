use crate::{
    state::AppState,
    types::{Command, ImageSnapshot},
};
use egui::{Color32, Response};

pub fn handle(response: &Response, state: &mut AppState) -> Option<Command> {
    if !response.clicked() {
        return None;
    }
    let click = response.interact_pointer_pos()?;
    let img_pos = state.unproject(click);
    let ci = state.canvas.color_image.as_ref()?;
    let (w, h) = (ci.size[0], ci.size[1]);
    let x = img_pos.x as i32;
    let y = img_pos.y as i32;
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return None;
    }

    let fill_color = state.settings.stroke_color;
    let tolerance = state.settings.fill_tolerance;
    let target = ci.pixels[y as usize * w + x as usize];

    // Nothing to do if click is already the fill color (within tolerance).
    if color_dist(target, fill_color) <= tolerance {
        return None;
    }

    let old_image = ci.clone();
    let mut new_image = old_image.clone();
    flood_fill(
        &mut new_image,
        x as usize,
        y as usize,
        fill_color,
        target,
        tolerance,
    );

    state.canvas.color_image = Some(new_image);
    state.canvas.texture = None;
    Some(Command::ReplaceImage(ImageSnapshot(old_image)))
}

fn flood_fill(
    image: &mut egui::ColorImage,
    x: usize,
    y: usize,
    fill: Color32,
    target: Color32,
    tolerance: u8,
) {
    let w = image.size[0];
    let h = image.size[1];
    let mut stack = vec![(x, y)];
    while let Some((cx, cy)) = stack.pop() {
        let idx = cy * w + cx;
        // skip already-filled pixels (avoids re-processing and duplicate pushes)
        if image.pixels[idx] == fill {
            continue;
        }
        if color_dist(image.pixels[idx], target) > tolerance {
            continue;
        }
        image.pixels[idx] = fill;
        if cx > 0 {
            stack.push((cx - 1, cy));
        }
        if cx + 1 < w {
            stack.push((cx + 1, cy));
        }
        if cy > 0 {
            stack.push((cx, cy - 1));
        }
        if cy + 1 < h {
            stack.push((cx, cy + 1));
        }
    }
}

fn color_dist(a: Color32, b: Color32) -> u8 {
    let dr = (a.r() as i32 - b.r() as i32).unsigned_abs() as u8;
    let dg = (a.g() as i32 - b.g() as i32).unsigned_abs() as u8;
    let db = (a.b() as i32 - b.b() as i32).unsigned_abs() as u8;
    dr.max(dg).max(db)
}
