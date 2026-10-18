//! In-app region selector.
//!
//! The desktop portal's interactive selector is whatever the running desktop
//! ships — no live dimensions, no magnifier, no pixel nudging, and a fresh
//! permission dance on some setups. This module freezes a full-screen grab
//! into a borderless always-on-top overlay and does the selection itself:
//! drag, then adjust with handles or arrow keys, Enter to accept, Esc to
//! cancel.
//!
//! Everything here that isn't drawing is pure geometry with tests — the
//! overlay is nearly impossible to cover in CI, so the math must stand alone.

use egui::{Pos2, Rect, Vec2};
use image::RgbaImage;

/// How far the window's reported outer size may differ from the area we paint
/// into and still count as "the WM has finished making us fullscreen".
const SLICE_TOL_PT: f32 = 2.0;

/// Minimum selection side in image pixels. Smaller than this is almost always
/// a stray click rather than an intended capture.
pub const MIN_SIDE_PX: u32 = 2;

/// Which part of the selection a drag is manipulating.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// Inside the selection — drag moves the whole rect.
    Move,
}

impl Handle {
    pub const CORNERS: [Handle; 4] = [
        Handle::TopLeft,
        Handle::TopRight,
        Handle::BottomRight,
        Handle::BottomLeft,
    ];
    pub const EDGES: [Handle; 4] = [Handle::Top, Handle::Right, Handle::Bottom, Handle::Left];

    /// Anchor point of this handle on `r` (the point the user grabs).
    pub fn pos_on(self, r: Rect) -> Pos2 {
        let c = r.center();
        match self {
            Handle::TopLeft => r.left_top(),
            Handle::Top => Pos2::new(c.x, r.top()),
            Handle::TopRight => r.right_top(),
            Handle::Right => Pos2::new(r.right(), c.y),
            Handle::BottomRight => r.right_bottom(),
            Handle::Bottom => Pos2::new(c.x, r.bottom()),
            Handle::BottomLeft => r.left_bottom(),
            Handle::Left => Pos2::new(r.left(), c.y),
            Handle::Move => c,
        }
    }

    pub fn cursor(self) -> egui::CursorIcon {
        match self {
            Handle::TopLeft | Handle::BottomRight => egui::CursorIcon::ResizeNwSe,
            Handle::TopRight | Handle::BottomLeft => egui::CursorIcon::ResizeNeSw,
            Handle::Top | Handle::Bottom => egui::CursorIcon::ResizeVertical,
            Handle::Left | Handle::Right => egui::CursorIcon::ResizeHorizontal,
            Handle::Move => egui::CursorIcon::Move,
        }
    }
}

/// Normalized rect between two drag points — always positive-sized, so a
/// right-to-left or bottom-to-top drag behaves identically.
pub fn rect_from_drag(a: Pos2, b: Pos2) -> Rect {
    Rect::from_two_pos(a, b)
}

/// Keep `r` inside `bounds`, shrinking rather than translating (a selection
/// must never claim pixels that aren't on screen).
pub fn clamp_rect(r: Rect, bounds: Rect) -> Rect {
    let min = Pos2::new(
        r.min.x.clamp(bounds.min.x, bounds.max.x),
        r.min.y.clamp(bounds.min.y, bounds.max.y),
    );
    let max = Pos2::new(
        r.max.x.clamp(bounds.min.x, bounds.max.x),
        r.max.y.clamp(bounds.min.y, bounds.max.y),
    );
    Rect::from_min_max(min, max)
}

/// Translate `r` by `delta`, keeping the whole rect inside `bounds` (slides
/// along the edge instead of shrinking — this is a move, not a resize).
pub fn move_rect(r: Rect, delta: Vec2, bounds: Rect) -> Rect {
    let mut moved = r.translate(delta);
    let dx = if moved.min.x < bounds.min.x {
        bounds.min.x - moved.min.x
    } else if moved.max.x > bounds.max.x {
        bounds.max.x - moved.max.x
    } else {
        0.0
    };
    let dy = if moved.min.y < bounds.min.y {
        bounds.min.y - moved.min.y
    } else if moved.max.y > bounds.max.y {
        bounds.max.y - moved.max.y
    } else {
        0.0
    };
    moved = moved.translate(Vec2::new(dx, dy));
    clamp_rect(moved, bounds)
}

/// Hit-test the handles of `r` at `pos`. Corners win over edges, edges over
/// the interior, so the grab nearest the pointer is the one you'd expect.
pub fn handle_at(r: Rect, pos: Pos2, tolerance: f32) -> Option<Handle> {
    for h in Handle::CORNERS.iter().chain(Handle::EDGES.iter()) {
        if h.pos_on(r).distance(pos) <= tolerance {
            return Some(*h);
        }
    }
    r.contains(pos).then_some(Handle::Move)
}

/// Apply a drag of `handle` to `pos`. Corner/edge drags move only the grabbed
/// side(s); the result is re-normalized so dragging past the opposite edge
/// flips cleanly instead of producing a negative rect.
pub fn drag_handle(r: Rect, handle: Handle, pos: Pos2, bounds: Rect) -> Rect {
    let (mut l, mut t, mut rt, mut b) = (r.left(), r.top(), r.right(), r.bottom());
    match handle {
        Handle::TopLeft => {
            l = pos.x;
            t = pos.y;
        }
        Handle::Top => t = pos.y,
        Handle::TopRight => {
            rt = pos.x;
            t = pos.y;
        }
        Handle::Right => rt = pos.x,
        Handle::BottomRight => {
            rt = pos.x;
            b = pos.y;
        }
        Handle::Bottom => b = pos.y,
        Handle::BottomLeft => {
            l = pos.x;
            b = pos.y;
        }
        Handle::Left => l = pos.x,
        Handle::Move => return r,
    }
    clamp_rect(rect_from_drag(Pos2::new(l, t), Pos2::new(rt, b)), bounds)
}

/// Convert an overlay-space rect to integer image pixels.
///
/// `view` is the rect the grab slice is painted into (overlay space) and
/// `slice` is the part of the grab that slice shows, in image pixels — the
/// overlay only covers one monitor, so on a multi-monitor desktop the visible
/// slice is an offset window into the full-desktop grab. Deriving the scale
/// from the painted rect (rather than `pixels_per_point`) also keeps this
/// correct on HiDPI and when the texture had to be downscaled for the GPU.
/// Returns `None` when the selection is degenerate.
pub fn to_image_px(sel: Rect, view: Rect, slice: [u32; 4]) -> Option<[u32; 4]> {
    if view.width() <= 0.0 || view.height() <= 0.0 {
        return None;
    }
    let [ox, oy, sw, sh] = slice;
    let sx = sw as f32 / view.width();
    let sy = sh as f32 / view.height();
    let to_px = |v: f32, scale: f32, origin: u32, max: u32| -> u32 {
        let local = ((v * scale).round().max(0.0) as u32).min(max);
        origin.saturating_add(local)
    };
    let x0 = to_px(sel.min.x - view.min.x, sx, ox, sw);
    let y0 = to_px(sel.min.y - view.min.y, sy, oy, sh);
    let x1 = to_px(sel.max.x - view.min.x, sx, ox, sw);
    let y1 = to_px(sel.max.y - view.min.y, sy, oy, sh);
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w < MIN_SIDE_PX || h < MIN_SIDE_PX {
        return None;
    }
    Some([x0, y0, w, h])
}

/// The part of a full-desktop grab that a monitor-sized overlay shows, from the
/// overlay window's absolute position and size in logical points. Clamped to
/// the grab, and falls back to the whole grab when the window manager doesn't
/// report a position (Wayland).
pub fn slice_for_window(
    outer: Option<Rect>,
    pixels_per_point: f32,
    img_size: [u32; 2],
) -> [u32; 4] {
    let full = [0, 0, img_size[0], img_size[1]];
    let Some(outer) = outer else { return full };
    let ppp = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let x = (outer.min.x * ppp).round().max(0.0) as u32;
    let y = (outer.min.y * ppp).round().max(0.0) as u32;
    let w = (outer.width() * ppp).round().max(0.0) as u32;
    let h = (outer.height() * ppp).round().max(0.0) as u32;
    if w == 0 || h == 0 || x >= img_size[0] || y >= img_size[1] {
        return full;
    }
    [x, y, w.min(img_size[0] - x), h.min(img_size[1] - y)]
}

/// Whether the window manager's reported `outer` rect can be trusted to
/// describe the area the overlay is painting into.
///
/// On X11 `outer_rect` keeps reporting the pre-fullscreen windowed geometry for
/// several frames after the viewport says it is fullscreen; slicing the grab
/// with that stale rect points the overlay at the wrong pixels entirely. The
/// rect has caught up once it is the same size as the area we paint into.
pub fn outer_rect_settled(outer: Option<Rect>, screen: Rect) -> bool {
    outer.is_some_and(|o| {
        (o.width() - screen.width()).abs() <= SLICE_TOL_PT
            && (o.height() - screen.height()).abs() <= SLICE_TOL_PT
    })
}

/// Crop `img` to `[x, y, w, h]`, clamped to the image. `None` if the region
/// falls entirely outside or is degenerate.
pub fn crop(img: &RgbaImage, region: [u32; 4]) -> Option<RgbaImage> {
    let [x, y, w, h] = region;
    if x >= img.width() || y >= img.height() {
        return None;
    }
    let w = w.min(img.width() - x);
    let h = h.min(img.height() - y);
    if w == 0 || h == 0 {
        return None;
    }
    Some(image::imageops::crop_imm(img, x, y, w, h).to_image())
}

/// Live selection state while the overlay is up.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Phase {
    /// Nothing picked yet — the whole screen is dimmed, crosshair follows.
    #[default]
    Idle,
    /// Dragging out the initial rect.
    Dragging { from: Pos2 },
    /// A rect exists; handles and keyboard adjustment are live.
    Adjusting,
}

pub struct RegionSelect {
    /// The frozen full-desktop grab, in physical pixels.
    pub shot: std::sync::Arc<RgbaImage>,
    /// The part of `shot` this overlay covers, in image pixels. Resolved on the
    /// first frame from where the window manager actually put the overlay.
    pub slice: Option<[u32; 4]>,
    /// Texture for the visible slice, uploaded once the slice is known.
    pub texture: Option<egui::TextureHandle>,
    pub phase: Phase,
    /// Current selection in overlay-space logical points.
    pub sel: Option<Rect>,
    /// Which handle a pointer drag is currently manipulating.
    pub active_handle: Option<Handle>,
    /// Pointer offset within the rect when a Move drag started.
    pub move_grab: Option<Vec2>,
    /// Overlay size in logical points, set from the window each frame.
    pub overlay_size: Vec2,
    pub show_magnifier: bool,
    /// When the overlay was created — the fullscreen watchdog counts from here.
    pub started: std::time::Instant,
    /// Last time a Fullscreen/Focus nudge was sent to the window manager.
    /// Re-sending every frame fights the WM, so nudges are rate-limited.
    pub last_nudge: Option<std::time::Instant>,
    /// True once the window manager has confirmed the overlay is fullscreen.
    pub fullscreen_confirmed: bool,
    /// When fullscreen stopped being reported after it had been granted. A
    /// frame or two of `false` is normal churn on X11, so only a sustained
    /// loss counts as the user leaving the overlay.
    pub fullscreen_lost_since: Option<std::time::Instant>,
}

impl RegionSelect {
    pub fn new(shot: RgbaImage, overlay_size: Vec2) -> Self {
        Self {
            shot: std::sync::Arc::new(shot),
            slice: None,
            texture: None,
            phase: Phase::Idle,
            sel: None,
            active_handle: None,
            move_grab: None,
            overlay_size,
            show_magnifier: true,
            started: std::time::Instant::now(),
            last_nudge: None,
            fullscreen_confirmed: false,
            fullscreen_lost_since: None,
        }
    }

    /// The visible slice, defaulting to the whole grab until the window
    /// manager has told us where the overlay landed.
    pub fn slice(&self) -> [u32; 4] {
        self.slice
            .unwrap_or([0, 0, self.shot.width(), self.shot.height()])
    }

    pub fn bounds(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.overlay_size)
    }

    /// Selection in image pixels, given the rect the slice is painted into.
    pub fn selection_px(&self, view: Rect) -> Option<[u32; 4]> {
        to_image_px(self.sel?, view, self.slice())
    }

    /// Crop the frozen grab to the current selection.
    pub fn cropped(&self, view: Rect) -> Option<RgbaImage> {
        crop(&self.shot, self.selection_px(view)?)
    }

    /// Select the whole screen — the "actually I want all of it" escape hatch.
    pub fn select_all(&mut self) {
        self.sel = Some(self.bounds());
        self.phase = Phase::Adjusting;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn bounds() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))
    }

    #[test]
    fn drag_normalizes_in_every_direction() {
        let a = rect_from_drag(pos2(100.0, 100.0), pos2(40.0, 30.0));
        assert_eq!(a.min, pos2(40.0, 30.0));
        assert_eq!(a.max, pos2(100.0, 100.0));
        assert!(a.width() > 0.0 && a.height() > 0.0);
    }

    #[test]
    fn clamp_keeps_selection_on_screen() {
        let r = Rect::from_min_max(pos2(-50.0, -20.0), pos2(1200.0, 900.0));
        let c = clamp_rect(r, bounds());
        assert_eq!(c, bounds());
    }

    #[test]
    fn move_slides_along_the_edge_without_shrinking() {
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 50.0));
        let moved = move_rect(r, vec2(-500.0, 0.0), bounds());
        assert_eq!(moved.min.x, 0.0, "should stop at the left edge");
        assert_eq!(moved.width(), 100.0, "move must not resize");
        assert_eq!(moved.height(), 50.0);

        let moved = move_rect(r, vec2(5000.0, 5000.0), bounds());
        assert_eq!(moved.max.x, 1000.0);
        assert_eq!(moved.max.y, 800.0);
        assert_eq!(moved.width(), 100.0);
    }

    #[test]
    fn corners_win_over_edges_and_interior() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 200.0));
        assert_eq!(handle_at(r, pos2(100.0, 100.0), 8.0), Some(Handle::TopLeft));
        assert_eq!(handle_at(r, pos2(200.0, 100.0), 8.0), Some(Handle::Top));
        assert_eq!(handle_at(r, pos2(200.0, 200.0), 8.0), Some(Handle::Move));
        assert_eq!(handle_at(r, pos2(600.0, 600.0), 8.0), None);
    }

    #[test]
    fn dragging_a_corner_past_the_opposite_one_flips_cleanly() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(100.0, 100.0));
        let out = drag_handle(r, Handle::TopLeft, pos2(400.0, 380.0), bounds());
        assert!(out.width() > 0.0 && out.height() > 0.0);
        assert_eq!(out.min, pos2(200.0, 200.0));
        assert_eq!(out.max, pos2(400.0, 380.0));
    }

    #[test]
    fn edge_drag_moves_only_that_edge_and_stays_in_bounds() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(100.0, 100.0));
        let out = drag_handle(r, Handle::Right, pos2(5000.0, 0.0), bounds());
        assert_eq!(out.left(), 100.0);
        assert_eq!(out.top(), 100.0);
        assert_eq!(out.bottom(), 200.0);
        assert_eq!(out.right(), 1000.0, "clamped to the screen edge");
    }

    #[test]
    fn image_pixels_scale_from_the_painted_rect() {
        // HiDPI: 4000x2560 physical pixels painted into 2000x1280 points.
        let view = Rect::from_min_size(Pos2::ZERO, vec2(2000.0, 1280.0));
        let sel = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 70.0));
        assert_eq!(
            to_image_px(sel, view, [0, 0, 4000, 2560]).unwrap(),
            [20, 40, 200, 100]
        );
    }

    #[test]
    fn offset_views_are_accounted_for() {
        let view = Rect::from_min_size(pos2(50.0, 30.0), vec2(800.0, 600.0));
        let sel = Rect::from_min_max(pos2(150.0, 130.0), pos2(250.0, 230.0));
        assert_eq!(
            to_image_px(sel, view, [0, 0, 800, 600]).unwrap(),
            [100, 100, 100, 100]
        );
    }

    #[test]
    fn degenerate_selections_are_rejected() {
        let view = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let tiny = Rect::from_min_max(pos2(10.0, 10.0), pos2(10.4, 10.4));
        assert!(to_image_px(tiny, view, [0, 0, 800, 600]).is_none());
        assert!(to_image_px(tiny, Rect::NOTHING, [0, 0, 800, 600]).is_none());
    }

    #[test]
    fn selection_is_clipped_to_the_image() {
        let view = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let sel = Rect::from_min_max(pos2(700.0, 500.0), pos2(2000.0, 2000.0));
        assert_eq!(
            to_image_px(sel, view, [0, 0, 800, 600]).unwrap(),
            [700, 500, 100, 100]
        );
    }

    #[test]
    fn crop_extracts_the_right_pixels() {
        let mut img = RgbaImage::new(10, 10);
        img.put_pixel(4, 5, image::Rgba([1, 2, 3, 255]));
        let out = crop(&img, [4, 5, 2, 2]).unwrap();
        assert_eq!(out.dimensions(), (2, 2));
        assert_eq!(out.get_pixel(0, 0), &image::Rgba([1, 2, 3, 255]));
    }

    #[test]
    fn crop_clamps_and_rejects_out_of_range() {
        let img = RgbaImage::new(10, 10);
        assert_eq!(crop(&img, [8, 8, 50, 50]).unwrap().dimensions(), (2, 2));
        assert!(crop(&img, [20, 0, 5, 5]).is_none());
        assert!(crop(&img, [0, 0, 0, 5]).is_none());
    }

    #[test]
    fn selection_on_a_second_monitor_maps_into_the_desktop_grab() {
        // Overlay covers the right-hand 800x600 monitor of a 1600x600 desktop.
        let slice = [800, 0, 800, 600];
        let view = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let sel = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 120.0));
        assert_eq!(
            to_image_px(sel, view, slice).unwrap(),
            [810, 20, 100, 100],
            "selection must be offset into the second monitor's pixels"
        );
    }

    #[test]
    fn window_slice_is_derived_from_position_and_scale() {
        let outer = Rect::from_min_size(pos2(1280.0, 0.0), vec2(1280.0, 720.0));
        assert_eq!(
            slice_for_window(Some(outer), 2.0, [5120, 1440]),
            [2560, 0, 2560, 1440]
        );
    }

    #[test]
    fn window_slice_falls_back_to_the_whole_grab() {
        // No reported position (Wayland), or a nonsense one.
        assert_eq!(slice_for_window(None, 1.0, [800, 600]), [0, 0, 800, 600]);
        let off = Rect::from_min_size(pos2(9000.0, 0.0), vec2(100.0, 100.0));
        assert_eq!(
            slice_for_window(Some(off), 1.0, [800, 600]),
            [0, 0, 800, 600]
        );
    }

    #[test]
    fn window_slice_is_clamped_to_the_grab() {
        let outer = Rect::from_min_size(pos2(700.0, 500.0), vec2(400.0, 400.0));
        assert_eq!(
            slice_for_window(Some(outer), 1.0, [800, 600]),
            [700, 500, 100, 100]
        );
    }

    #[test]
    fn stale_windowed_rects_are_not_trusted_for_slicing() {
        // Painting area of a fullscreen 1440x2523pt overlay.
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 2523.0));
        // What X11 reports for several frames after the fullscreen flip: the
        // old windowed geometry. Slicing from this lands on the wrong pixels.
        let stale = Rect::from_min_size(pos2(2585.0, 25.0), vec2(1280.0, 857.0));
        assert!(!outer_rect_settled(Some(stale), screen));
        assert!(!outer_rect_settled(None, screen));

        let settled = Rect::from_min_size(pos2(2560.0, 37.0), vec2(1440.0, 2523.0));
        assert!(outer_rect_settled(Some(settled), screen));
        // Sub-point rounding differences still count as settled.
        let rounded = Rect::from_min_size(pos2(2560.0, 37.0), vec2(1441.0, 2522.0));
        assert!(outer_rect_settled(Some(rounded), screen));
    }

    #[test]
    fn select_all_covers_the_whole_overlay() {
        let mut rs = RegionSelect::new(RgbaImage::new(200, 100), vec2(100.0, 50.0));
        rs.select_all();
        assert_eq!(rs.sel.unwrap(), rs.bounds());
        assert_eq!(rs.selection_px(rs.bounds()).unwrap(), [0, 0, 200, 100]);
    }
}

// ── Overlay UI ────────────────────────────────────────────────────────────────

use egui::{Color32, Stroke};

/// What the overlay wants the app to do after this frame.
pub enum Outcome {
    /// Still selecting — keep the overlay up.
    Pending,
    /// User accepted; here are the cropped pixels.
    Accepted(RgbaImage),
    /// User backed out (Esc, right-click, or the window was closed).
    Cancelled,
    /// The window manager never gave us a visible fullscreen overlay — the
    /// caller should fall back to using the grab as-is rather than leaving the
    /// user staring at a desktop with no RedLine window on it.
    Unavailable,
}

/// How long to wait for the window manager to actually map the overlay
/// fullscreen before giving up on it. Long enough for a slow un-minimize,
/// short enough that the user isn't left staring at a missing window.
const FULLSCREEN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2500);
/// How long fullscreen may stay un-reported, once granted, before the overlay
/// treats it as the user leaving.
const FULLSCREEN_GRACE: std::time::Duration = std::time::Duration::from_millis(750);
/// Minimum gap between fullscreen/focus nudges to the WM.
const NUDGE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

const HANDLE_R: f32 = 5.0;
const GRAB_TOL: f32 = 11.0;
const MAGNIFIER_SIDE: f32 = 132.0;
/// Screen pixels shown across the magnifier — smaller means more zoom.
const MAGNIFIER_SPAN_PX: f32 = 17.0;

/// Draw the selection overlay for this frame and report what the user did.
///
/// The overlay takes over the whole main window (which is put into fullscreen
/// while it is up) rather than opening a window of its own: a separate
/// virtual-desktop-sized viewport turned out to be unreliable — the GL backend
/// can fail to create or map it, silently, leaving no selector at all.
pub fn show(ctx: &egui::Context, rs: &mut RegionSelect) -> Outcome {
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    if fullscreen {
        rs.fullscreen_confirmed = true;
        rs.fullscreen_lost_since = None;
    } else if rs.fullscreen_confirmed {
        // The WM took fullscreen away after we had it (the user hit the WM's
        // own unfullscreen, or the compositor restarted). Treat a sustained
        // loss as a cancel rather than painting a windowed overlay.
        let since = *rs
            .fullscreen_lost_since
            .get_or_insert_with(std::time::Instant::now);
        if since.elapsed() > FULLSCREEN_GRACE {
            crate::portal_capture::capture_log(|| {
                "overlay: fullscreen lost after it was granted — cancelling".into()
            });
            return Outcome::Cancelled;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    } else {
        // Not fullscreen (yet). The window was very likely minimized a moment
        // ago for the grab, and un-minimize + fullscreen + focus can all be
        // dropped by the WM while the window is still unmapped — which used to
        // leave the app alive with no window on screen and no way to capture.
        // Nudge, rate-limited, and give up rather than hang.
        if rs.started.elapsed() > FULLSCREEN_TIMEOUT {
            crate::portal_capture::capture_log(|| {
                format!(
                    "overlay: window never went fullscreen within {:?} — falling back",
                    FULLSCREEN_TIMEOUT
                )
            });
            return Outcome::Unavailable;
        }
        let due = rs
            .last_nudge
            .map(|t| t.elapsed() >= NUDGE_INTERVAL)
            .unwrap_or(true);
        if due {
            rs.last_nudge = Some(std::time::Instant::now());
            crate::portal_capture::capture_log(|| {
                format!(
                    "overlay: nudging WM for fullscreen ({:?} in)",
                    rs.started.elapsed()
                )
            });
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        // Don't draw yet: the slice this overlay shows is derived from where
        // the WM finally puts the window, and a texture uploaded from a guess
        // would only have to be thrown away.
        return Outcome::Pending;
    }
    let mut outcome = Outcome::Pending;
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::BLACK))
        .show(ctx, |ui| {
            outcome = draw(ui, rs);
        });
    outcome
}

/// Put the window back the way it was when the selector closes.
pub fn leave_fullscreen(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
}

fn draw(ui: &mut egui::Ui, rs: &mut RegionSelect) -> Outcome {
    let ctx = ui.ctx().clone();
    let screen = ctx.screen_rect();
    let painter = ui.painter().clone();

    rs.overlay_size = screen.size();

    // Resolve which part of the desktop grab this overlay is showing, once the
    // window manager has actually placed the window.
    //
    // `outer_rect` lags the fullscreen transition: on X11 it keeps reporting
    // the old *windowed* geometry for several frames after `fullscreen` flips
    // true. Resolving the slice from that stale rect is what made the selector
    // useless — it painted some arbitrary off-centre crop of the desktop,
    // stretched to fill the screen, so nothing lined up with what you clicked.
    // Only trust the rect once its size matches the area we're painting into.
    if rs.slice.is_none() {
        let (outer, ppp) = ctx.input(|i| (i.viewport().outer_rect, i.pixels_per_point()));
        match (outer, outer_rect_settled(outer, screen)) {
            (Some(outer), true) => {
                let slice = slice_for_window(Some(outer), ppp, [rs.shot.width(), rs.shot.height()]);
                crate::portal_capture::capture_log(|| {
                    format!("overlay: window {outer:?} ppp={ppp} -> slice {slice:?}")
                });
                rs.slice = Some(slice);
                rs.texture = None;
            }
            (outer, _) => {
                if rs.started.elapsed() > FULLSCREEN_TIMEOUT {
                    crate::portal_capture::capture_log(|| {
                        format!(
                            "overlay: window rect never matched the screen                              (last {outer:?} vs screen {:?}) — falling back",
                            screen.size()
                        )
                    });
                    return Outcome::Unavailable;
                }
                // Hold a black frame rather than painting a guessed slice.
                painter.rect_filled(screen, 0.0, Color32::BLACK);
                ctx.request_repaint();
                return Outcome::Pending;
            }
        }
    }

    if rs.texture.is_none() {
        // Upload only the visible slice — on a multi-monitor desktop the rest
        // of the grab is off-screen anyway, and this keeps the texture small.
        let slice = rs.slice();
        let visible = crop(&rs.shot, slice).unwrap_or_else(|| (*rs.shot).clone());
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [visible.width() as usize, visible.height() as usize],
            visible.as_raw(),
        );
        rs.texture = Some(crate::texture_upload::load_texture_fitted(
            &ctx,
            format!("region-overlay-{}-{}", slice[0], slice[1]),
            ci,
            egui::TextureOptions::LINEAR,
        ));
    }
    // The slice is painted to fill the overlay exactly, so overlay space and
    // slice space differ only by this rect's scale.
    let view = screen;
    if let Some(tex) = &rs.texture {
        painter.image(
            tex.id(),
            view,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    let resp = ui.allocate_rect(screen, egui::Sense::click_and_drag());
    let pointer = ctx.pointer_latest_pos();

    // ── Input ────────────────────────────────────────────────────────────────
    let (accept, cancel, select_all, nudge, toggle_mag) = ctx.input(|i| {
        let step = if i.modifiers.shift { 10.0 } else { 1.0 };
        let mut d = Vec2::ZERO;
        if i.key_pressed(egui::Key::ArrowLeft) {
            d.x -= step;
        }
        if i.key_pressed(egui::Key::ArrowRight) {
            d.x += step;
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            d.y -= step;
        }
        if i.key_pressed(egui::Key::ArrowDown) {
            d.y += step;
        }
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::A),
            d,
            i.key_pressed(egui::Key::M),
        )
    });

    if cancel || resp.secondary_clicked() {
        crate::portal_capture::capture_log(|| {
            format!(
                "overlay: cancelled (esc={cancel} right_click={})",
                resp.secondary_clicked()
            )
        });
        return Outcome::Cancelled;
    }
    if toggle_mag {
        rs.show_magnifier = !rs.show_magnifier;
    }
    if select_all {
        rs.select_all();
    }
    if nudge != Vec2::ZERO {
        if let Some(sel) = rs.sel {
            rs.sel = Some(move_rect(sel, nudge, rs.bounds()));
        }
    }

    let bounds = rs.bounds();
    if resp.drag_started() {
        if let Some(p) = pointer {
            match rs.sel.and_then(|s| handle_at(s, p, GRAB_TOL)) {
                Some(Handle::Move) => {
                    rs.active_handle = Some(Handle::Move);
                    rs.move_grab = rs.sel.map(|s| p - s.min);
                }
                Some(h) => {
                    rs.active_handle = Some(h);
                    rs.move_grab = None;
                }
                None => {
                    rs.phase = Phase::Dragging { from: p };
                    rs.sel = Some(Rect::from_two_pos(p, p));
                    rs.active_handle = None;
                }
            }
        }
    }
    if resp.dragged() {
        if let Some(p) = pointer {
            match (rs.phase, rs.active_handle) {
                (_, Some(Handle::Move)) => {
                    if let (Some(sel), Some(grab)) = (rs.sel, rs.move_grab) {
                        let delta = (p - grab) - sel.min;
                        rs.sel = Some(move_rect(sel, delta, bounds));
                    }
                }
                (_, Some(h)) => {
                    if let Some(sel) = rs.sel {
                        rs.sel = Some(drag_handle(sel, h, p, bounds));
                    }
                }
                (Phase::Dragging { from }, None) => {
                    rs.sel = Some(clamp_rect(rect_from_drag(from, p), bounds));
                }
                _ => {}
            }
        }
    }
    if resp.drag_stopped() {
        rs.active_handle = None;
        rs.move_grab = None;
        if matches!(rs.phase, Phase::Dragging { .. }) {
            rs.phase = Phase::Adjusting;
        }
    }

    let has_sel = rs
        .sel
        .and_then(|s| to_image_px(s, view, rs.slice()))
        .is_some();
    let mut confirm = accept;

    // ── Paint ────────────────────────────────────────────────────────────────
    let dim = Color32::from_black_alpha(130);
    match rs.sel {
        Some(sel) if has_sel || matches!(rs.phase, Phase::Dragging { .. }) => {
            // Dim everything except the selection (four bands, so the selected
            // pixels stay pristine — you're judging the shot through this).
            for band in [
                Rect::from_min_max(screen.min, Pos2::new(screen.max.x, sel.min.y)),
                Rect::from_min_max(Pos2::new(screen.min.x, sel.max.y), screen.max),
                Rect::from_min_max(Pos2::new(screen.min.x, sel.min.y), sel.left_bottom()),
                Rect::from_min_max(sel.right_top(), Pos2::new(screen.max.x, sel.max.y)),
            ] {
                painter.rect_filled(band, 0.0, dim);
            }
            painter.rect_stroke(sel, 0.0, Stroke::new(1.0, Color32::from_white_alpha(220)));
            painter.rect_stroke(
                sel.expand(1.0),
                0.0,
                Stroke::new(1.0, Color32::from_black_alpha(120)),
            );
            for h in Handle::CORNERS.iter().chain(Handle::EDGES.iter()) {
                let p = h.pos_on(sel);
                painter.rect_filled(
                    Rect::from_center_size(p, Vec2::splat(HANDLE_R * 2.0)),
                    2.0,
                    Color32::WHITE,
                );
                painter.rect_stroke(
                    Rect::from_center_size(p, Vec2::splat(HANDLE_R * 2.0)),
                    2.0,
                    Stroke::new(1.0, Color32::from_black_alpha(160)),
                );
            }
            if let Some(px) = rs.selection_px(view) {
                draw_size_badge(&painter, sel, screen, px);
            }
        }
        _ => {
            painter.rect_filled(screen, 0.0, dim);
        }
    }

    // Crosshair + magnifier follow the pointer while there's no committed rect.
    if let Some(p) = pointer {
        if matches!(rs.phase, Phase::Idle | Phase::Dragging { .. }) {
            let g = Color32::from_white_alpha(70);
            painter.line_segment(
                [Pos2::new(screen.min.x, p.y), Pos2::new(screen.max.x, p.y)],
                Stroke::new(1.0, g),
            );
            painter.line_segment(
                [Pos2::new(p.x, screen.min.y), Pos2::new(p.x, screen.max.y)],
                Stroke::new(1.0, g),
            );
        }
        if rs.show_magnifier {
            draw_magnifier(&painter, rs, view, screen, p);
        }
        // Cursor feedback over handles.
        if let Some(h) = rs.sel.and_then(|s| handle_at(s, p, GRAB_TOL)) {
            ctx.set_cursor_icon(h.cursor());
        } else {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    // Explicit confirm/cancel, pinned to the selection. Keyboard-free, and
    // safer than accept-on-double-click, which fires by accident at the end of
    // a drag.
    if has_sel {
        if let Some(sel) = rs.sel {
            match draw_actions(ui, sel, screen) {
                Some(true) => confirm = true,
                Some(false) => return Outcome::Cancelled,
                None => {}
            }
        }
    }

    if confirm && has_sel {
        if let Some(img) = rs.cropped(view) {
            crate::portal_capture::capture_log(|| {
                format!("overlay: accepted {}x{}", img.width(), img.height())
            });
            return Outcome::Accepted(img);
        }
    }

    draw_hint_bar(&painter, screen, rs.sel.is_some());
    ctx.request_repaint();
    Outcome::Pending
}

/// Confirm / cancel buttons for the current selection. `Some(true)` = capture,
/// `Some(false)` = cancel.
fn draw_actions(ui: &mut egui::Ui, sel: Rect, screen: Rect) -> Option<bool> {
    let size = Vec2::new(196.0, 34.0);
    // Below the selection by default; above it, then inside, when there's no room.
    let mut origin = Pos2::new(sel.right() - size.x, sel.bottom() + 10.0);
    if origin.y + size.y > screen.max.y {
        origin.y = sel.top() - size.y - 10.0;
    }
    if origin.y < screen.min.y {
        origin.y = sel.bottom() - size.y - 10.0;
    }
    origin.x = origin
        .x
        .clamp(screen.min.x + 4.0, screen.max.x - size.x - 4.0);

    let mut result = None;
    ui.allocate_new_ui(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(origin, size))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("✓  Capture")
                            .color(Color32::from_rgb(26, 20, 6))
                            .strong(),
                    )
                    .fill(Color32::from_rgb(233, 186, 74))
                    .rounding(egui::Rounding::same(7.0))
                    .min_size(Vec2::new(0.0, 28.0)),
                )
                .on_hover_text("Capture this region (Enter)")
                .clicked()
            {
                result = Some(true);
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Cancel").color(Color32::WHITE))
                        .fill(Color32::from_rgba_unmultiplied(30, 32, 38, 235))
                        .rounding(egui::Rounding::same(7.0))
                        .min_size(Vec2::new(0.0, 28.0)),
                )
                .on_hover_text("Cancel (Esc)")
                .clicked()
            {
                result = Some(false);
            }
        },
    );
    result
}

/// "1280 × 720" chip pinned to the selection, flipped inside when the
/// selection is near the screen edge.
fn draw_size_badge(painter: &egui::Painter, sel: Rect, screen: Rect, px: [u32; 4]) {
    let text = format!("{} × {}", px[2], px[3]);
    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
    let pad = Vec2::new(9.0, 5.0);
    let size = galley.size() + pad * 2.0;
    let above = sel.top() - size.y - 8.0 >= screen.top();
    let origin = if above {
        Pos2::new(sel.min.x, sel.top() - size.y - 8.0)
    } else {
        Pos2::new(sel.min.x, (sel.top() + 8.0).min(screen.max.y - size.y))
    };
    let rect = Rect::from_min_size(origin, size);
    painter.rect_filled(rect, 6.0, Color32::from_rgba_unmultiplied(20, 22, 26, 235));
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_white_alpha(40)));
    painter.galley(rect.min + pad, galley, Color32::WHITE);
}

/// Pixel magnifier: the pixels under the cursor blown up, with a crosshair on
/// the exact pixel and its hex value. This is what makes pixel-tight
/// selections possible.
fn draw_magnifier(painter: &egui::Painter, rs: &RegionSelect, view: Rect, screen: Rect, p: Pos2) {
    let Some(tex) = &rs.texture else { return };
    let slice = rs.slice();
    let img_w = slice[2] as f32;
    let img_h = slice[3] as f32;
    if img_w <= 0.0 || img_h <= 0.0 || view.width() <= 0.0 {
        return;
    }
    // Keep the magnifier clear of the cursor, and flip it near screen edges.
    let side = MAGNIFIER_SIDE;
    let mut origin = p + Vec2::new(24.0, 24.0);
    if origin.x + side > screen.max.x {
        origin.x = p.x - side - 24.0;
    }
    if origin.y + side > screen.max.y {
        origin.y = p.y - side - 24.0;
    }
    let rect = Rect::from_min_size(origin, Vec2::splat(side));

    // Source UV window centred on the pointer.
    let scale_x = img_w / view.width();
    let scale_y = img_h / view.height();
    let cx = (p.x - view.min.x) * scale_x;
    let cy = (p.y - view.min.y) * scale_y;
    let half = MAGNIFIER_SPAN_PX * 0.5;
    let uv = Rect::from_min_max(
        Pos2::new((cx - half) / img_w, (cy - half) / img_h),
        Pos2::new((cx + half) / img_w, (cy + half) / img_h),
    );

    painter.rect_filled(
        rect.expand(2.0),
        8.0,
        Color32::from_rgba_unmultiplied(20, 22, 26, 240),
    );
    painter.image(tex.id(), rect, uv, Color32::WHITE);
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_white_alpha(90)));
    // Centre pixel marker.
    let cell = side / MAGNIFIER_SPAN_PX;
    painter.rect_stroke(
        Rect::from_center_size(rect.center(), Vec2::splat(cell)),
        0.0,
        Stroke::new(1.0, Color32::from_rgb(255, 210, 90)),
    );

    // Hex readout of the exact pixel under the crosshair.
    let (ix, iy) = (
        cx.round() as i64 + slice[0] as i64,
        cy.round() as i64 + slice[1] as i64,
    );
    if ix >= 0 && iy >= 0 && (ix as u32) < rs.shot.width() && (iy as u32) < rs.shot.height() {
        let px = rs.shot.get_pixel(ix as u32, iy as u32);
        let label = format!("#{:02X}{:02X}{:02X}  {ix},{iy}", px[0], px[1], px[2]);
        let font = egui::FontId::monospace(11.0);
        let galley = painter.layout_no_wrap(label, font, Color32::WHITE);
        let bar = Rect::from_min_size(
            Pos2::new(rect.min.x, rect.max.y + 4.0),
            Vec2::new(rect.width(), galley.size().y + 6.0),
        );
        painter.rect_filled(bar, 5.0, Color32::from_rgba_unmultiplied(20, 22, 26, 235));
        painter.galley(bar.min + Vec2::new(6.0, 3.0), galley, Color32::WHITE);
    }
}

/// Key legend along the bottom — the overlay has no chrome, so the shortcuts
/// have to announce themselves.
fn draw_hint_bar(painter: &egui::Painter, screen: Rect, has_sel: bool) {
    let text = if has_sel {
        "Enter or ✓ Capture     drag handles / arrow keys  adjust  (Shift ×10)     A  whole screen     M  magnifier     Esc  cancel"
    } else {
        "Drag to select a region     A  whole screen     M  magnifier     Esc  cancel"
    };
    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, Color32::from_white_alpha(230));
    let pad = Vec2::new(16.0, 9.0);
    let size = galley.size() + pad * 2.0;
    let rect = Rect::from_min_size(
        Pos2::new(
            screen.center().x - size.x * 0.5,
            screen.max.y - size.y - 34.0,
        ),
        size,
    );
    painter.rect_filled(rect, 9.0, Color32::from_rgba_unmultiplied(18, 20, 24, 232));
    painter.rect_stroke(rect, 9.0, Stroke::new(1.0, Color32::from_white_alpha(45)));
    painter.galley(rect.min + pad, galley, Color32::WHITE);
}
