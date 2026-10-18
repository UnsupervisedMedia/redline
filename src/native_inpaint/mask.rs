use image::{GrayImage, Luma, RgbaImage};

/// A paintable inpaint mask aligned to the source raster.
/// White (255) = inpaint here; black (0) = keep.
pub struct MaskBuffer {
    img: GrayImage,
    /// Number of set (non-zero) pixels, tracked incrementally so `any_set()` is
    /// O(1) instead of a full per-frame scan.
    set_count: u64,
    /// Bumped on every mutation so the UI can cache the overlay texture and only
    /// re-upload when the mask actually changed.
    version: u64,
}

impl MaskBuffer {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            img: GrayImage::from_pixel(w, h, Luma([0])),
            set_count: 0,
            version: 0,
        }
    }

    pub fn image(&self) -> &GrayImage {
        &self.img
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.img.dimensions()
    }

    /// A monotonically increasing version that changes on every mutation.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// True if at least one pixel is set (something to inpaint). O(1).
    pub fn any_set(&self) -> bool {
        self.set_count > 0
    }

    pub fn clear(&mut self) {
        if self.set_count == 0 {
            return;
        }
        for p in self.img.pixels_mut() {
            *p = Luma([0]);
        }
        self.set_count = 0;
        self.version += 1;
    }

    /// Set a single pixel, keeping `set_count` accurate. Internal helper.
    #[inline]
    fn put(&mut self, x: u32, y: u32, val: u8) {
        let cur = self.img.get_pixel(x, y).0[0];
        let was_set = cur > 0;
        let now_set = val > 0;
        if was_set != now_set {
            if now_set {
                self.set_count += 1;
            } else {
                self.set_count -= 1;
            }
        }
        if cur != val {
            self.img.put_pixel(x, y, Luma([val]));
        }
    }

    /// Paint (or erase) a filled disc centred at image coords `(cx, cy)`.
    pub fn stamp(&mut self, cx: f32, cy: f32, radius: f32, paint: bool) {
        let before = self.set_count;
        self.stamp_inner(cx, cy, radius, paint);
        if self.set_count != before {
            self.version += 1;
        }
    }

    fn stamp_inner(&mut self, cx: f32, cy: f32, radius: f32, paint: bool) {
        let (w, h) = self.img.dimensions();
        if w == 0 || h == 0 {
            return;
        }
        let r = radius.max(0.5);
        let val = if paint { 255 } else { 0 };
        let x0 = ((cx - r).floor().max(0.0)) as u32;
        let y0 = ((cy - r).floor().max(0.0)) as u32;
        let x1 = ((cx + r).ceil().min((w - 1) as f32)).max(0.0) as u32;
        let y1 = ((cy + r).ceil().min((h - 1) as f32)).max(0.0) as u32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if dx * dx + dy * dy <= r * r {
                    self.put(x, y, val);
                }
            }
        }
    }

    /// Paint (or erase) a continuous stroke from `(x0,y0)` to `(x1,y1)` by
    /// stamping overlapping discs along the segment. Without this, fast drags —
    /// which only deliver sparse pointer samples — leave gaps in the mask.
    pub fn stamp_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, paint: bool) {
        let before = self.set_count;
        let dx = x1 - x0;
        let dy = y1 - y0;
        let dist = (dx * dx + dy * dy).sqrt();
        // Step at most half the radius so consecutive discs overlap.
        let step = (radius.max(0.5) * 0.5).max(0.5);
        let n = (dist / step).ceil().max(1.0) as u32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            self.stamp_inner(x0 + dx * t, y0 + dy * t, radius, paint);
        }
        if self.set_count != before {
            self.version += 1;
        }
    }

    /// Magic-wand: flood-fill from `seed` selecting connected pixels of `img`
    /// whose colour is within `tolerance` (per-channel max distance, 0..=255) of
    /// the seed colour, and paint/erase them into the mask. 4-connected.
    /// Returns the number of pixels visited (selected).
    pub fn flood_fill(
        &mut self,
        img: &RgbaImage,
        seed: (u32, u32),
        tolerance: u8,
        paint: bool,
    ) -> u64 {
        let (w, h) = self.img.dimensions();
        if w == 0 || h == 0 || img.dimensions() != (w, h) {
            return 0;
        }
        let (sx, sy) = seed;
        if sx >= w || sy >= h {
            return 0;
        }
        let before = self.set_count;
        let target = img.get_pixel(sx, sy).0;
        let tol = tolerance as i32;
        let within = |p: &[u8; 4]| -> bool {
            (p[0] as i32 - target[0] as i32).abs() <= tol
                && (p[1] as i32 - target[1] as i32).abs() <= tol
                && (p[2] as i32 - target[2] as i32).abs() <= tol
        };

        let val = if paint { 255 } else { 0 };
        let mut visited = vec![false; (w as usize) * (h as usize)];
        let mut stack = vec![(sx, sy)];
        let mut count = 0u64;
        while let Some((x, y)) = stack.pop() {
            let idx = (y as usize) * (w as usize) + x as usize;
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            if !within(&img.get_pixel(x, y).0) {
                continue;
            }
            self.put(x, y, val);
            count += 1;
            if x > 0 {
                stack.push((x - 1, y));
            }
            if x + 1 < w {
                stack.push((x + 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if y + 1 < h {
                stack.push((x, y + 1));
            }
        }
        if self.set_count != before {
            self.version += 1;
        }
        count
    }

    /// Overwrite a `w`×`h` rectangle at `(x, y)` with `bytes` (row-major
    /// grayscale), keeping `set_count` accurate and bumping `version`. Used to
    /// apply undo/redo patches. Pixels outside the image are skipped.
    pub fn apply_region(&mut self, x: u32, y: u32, w: u32, h: u32, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), (w as usize) * (h as usize));
        let (iw, ih) = self.img.dimensions();
        for row in 0..h {
            for col in 0..w {
                let px = x + col;
                let py = y + row;
                if px < iw && py < ih {
                    self.put(px, py, bytes[(row * w + col) as usize]);
                }
            }
        }
        self.version += 1;
    }
}

/// A compact, reversible record of a rectangular mask change: the previous and
/// new grayscale bytes over a bounding rectangle. Built by [`MaskPatch::between`]
/// and applied with [`MaskBuffer::apply_region`].
#[derive(Clone)]
pub struct MaskPatch {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

impl MaskPatch {
    /// Diff two equally-sized masks into a patch covering the bounding rectangle
    /// of all differing pixels. Returns `None` if the masks are identical (or
    /// differ in size — treated as no patch).
    pub fn between(before: &GrayImage, after: &GrayImage) -> Option<MaskPatch> {
        let (w, h) = before.dimensions();
        if after.dimensions() != (w, h) {
            return None;
        }
        let (mut minx, mut miny, mut maxx, mut maxy) = (u32::MAX, u32::MAX, 0u32, 0u32);
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                if before.get_pixel(x, y).0[0] != after.get_pixel(x, y).0[0] {
                    any = true;
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
        if !any {
            return None;
        }
        let rw = maxx - minx + 1;
        let rh = maxy - miny + 1;
        let mut b = Vec::with_capacity((rw * rh) as usize);
        let mut a = Vec::with_capacity((rw * rh) as usize);
        for y in miny..=maxy {
            for x in minx..=maxx {
                b.push(before.get_pixel(x, y).0[0]);
                a.push(after.get_pixel(x, y).0[0]);
            }
        }
        Some(MaskPatch {
            x: minx,
            y: miny,
            w: rw,
            h: rh,
            before: b,
            after: a,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn new_mask_is_empty() {
        let m = MaskBuffer::new(20, 10);
        assert_eq!(m.image().dimensions(), (20, 10));
        assert!(!m.any_set());
    }

    #[test]
    fn stamp_sets_a_disc_of_pixels() {
        let mut m = MaskBuffer::new(20, 20);
        m.stamp(10.0, 10.0, 3.0, true);
        assert!(m.any_set());
        assert_eq!(m.image().get_pixel(10, 10).0[0], 255);
        assert_eq!(m.image().get_pixel(0, 0).0[0], 0);
    }

    #[test]
    fn erase_clears_pixels() {
        let mut m = MaskBuffer::new(20, 20);
        m.stamp(10.0, 10.0, 3.0, true);
        m.stamp(10.0, 10.0, 3.0, false);
        assert_eq!(m.image().get_pixel(10, 10).0[0], 0);
        assert!(!m.any_set());
    }

    #[test]
    fn clear_resets_everything() {
        let mut m = MaskBuffer::new(20, 20);
        m.stamp(10.0, 10.0, 5.0, true);
        m.clear();
        assert!(!m.any_set());
    }

    #[test]
    fn set_count_tracks_any_set_in_o1() {
        let mut m = MaskBuffer::new(20, 20);
        assert!(!m.any_set());
        m.stamp(5.0, 5.0, 2.0, true);
        assert!(m.any_set());
        // Erasing the same disc must bring set_count back to zero.
        m.stamp(5.0, 5.0, 2.0, false);
        assert!(!m.any_set());
    }

    #[test]
    fn version_bumps_only_on_actual_change() {
        let mut m = MaskBuffer::new(20, 20);
        let v0 = m.version();
        m.stamp(5.0, 5.0, 2.0, true);
        let v1 = m.version();
        assert!(v1 > v0, "painting changes the mask");
        // Erasing an already-empty corner changes nothing → version stable.
        m.stamp(0.0, 0.0, 0.5, false);
        assert_eq!(m.version(), v1, "no-op must not bump version");
    }

    #[test]
    fn stamp_line_fills_gaps_between_endpoints() {
        let mut m = MaskBuffer::new(100, 10);
        // A fast horizontal drag from x=5 to x=95 with a small brush.
        m.stamp_line(5.0, 5.0, 95.0, 5.0, 2.0, true);
        // Every point along the midline must be covered (no dotted gaps).
        for x in 5..=95 {
            assert_eq!(
                m.image().get_pixel(x, 5).0[0],
                255,
                "gap at x={x} — stroke not continuous"
            );
        }
    }

    #[test]
    fn flood_fill_selects_connected_same_color_region() {
        // Left half black, right half white; flood from the left should select
        // exactly the left half.
        let mut img = RgbaImage::from_pixel(10, 4, Rgba([0, 0, 0, 255]));
        for y in 0..4 {
            for x in 5..10 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let mut m = MaskBuffer::new(10, 4);
        let n = m.flood_fill(&img, (0, 0), 10, true);
        assert_eq!(n, 5 * 4, "selects the whole black region");
        assert_eq!(m.image().get_pixel(0, 0).0[0], 255);
        assert_eq!(m.image().get_pixel(4, 0).0[0], 255);
        assert_eq!(
            m.image().get_pixel(5, 0).0[0],
            0,
            "stops at the colour edge"
        );
        assert!(m.any_set());
    }

    #[test]
    fn flood_fill_respects_tolerance() {
        // A gradient; a tight tolerance selects only near-seed pixels.
        let mut img = RgbaImage::new(6, 1);
        for x in 0..6 {
            let v = (x * 40) as u8;
            img.put_pixel(x, 0, Rgba([v, v, v, 255]));
        }
        let mut m = MaskBuffer::new(6, 1);
        // Seed at x=0 (v=0), tolerance 50 → reaches x=0 (0) and x=1 (40) but not x=2 (80).
        let n = m.flood_fill(&img, (0, 0), 50, true);
        assert_eq!(n, 2);
        assert_eq!(m.image().get_pixel(1, 0).0[0], 255);
        assert_eq!(m.image().get_pixel(2, 0).0[0], 0);
    }

    #[test]
    fn mask_patch_between_is_none_when_unchanged() {
        let a = GrayImage::from_pixel(8, 8, Luma([0]));
        let b = a.clone();
        assert!(MaskPatch::between(&a, &b).is_none());
    }

    #[test]
    fn mask_patch_between_bounds_the_changed_region() {
        let before = GrayImage::from_pixel(8, 8, Luma([0]));
        let mut after = before.clone();
        after.put_pixel(2, 3, Luma([255]));
        after.put_pixel(4, 5, Luma([255]));
        let p = MaskPatch::between(&before, &after).expect("a change");
        assert_eq!((p.x, p.y), (2, 3), "min corner of changed bbox");
        assert_eq!((p.w, p.h), (3, 3), "covers (2..=4, 3..=5)");
        assert_eq!(p.before.len(), 9);
        assert_eq!(p.after.len(), 9);
    }

    #[test]
    fn apply_region_restores_bytes_and_set_count() {
        let mut m = MaskBuffer::new(8, 8);
        let before = m.image().clone();
        m.stamp(4.0, 4.0, 2.0, true);
        assert!(m.any_set());
        let patch = MaskPatch::between(&before, m.image()).expect("a change");
        // Undo: write the BEFORE bytes back → mask empty again.
        m.apply_region(patch.x, patch.y, patch.w, patch.h, &patch.before);
        assert!(!m.any_set(), "apply_region(before) clears the stamp");
        assert_eq!(m.image(), &before, "exact byte restore");
        // Redo: write the AFTER bytes → stamp returns.
        m.apply_region(patch.x, patch.y, patch.w, patch.h, &patch.after);
        assert!(m.any_set(), "apply_region(after) restores the stamp");
    }
}
