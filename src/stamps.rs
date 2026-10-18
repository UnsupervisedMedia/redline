use std::sync::Arc;

use egui::{Color32, Pos2, Rect};
use image::RgbaImage;

use crate::types::{Annotation, ShapeData, TextureCell};

#[derive(Clone, Copy, Debug)]
pub struct StampDef {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub uri: &'static str,
    pub svg_bytes: &'static [u8],
}

pub fn all_stamps() -> &'static [StampDef] {
    include!(concat!(env!("OUT_DIR"), "/stamp_defs.rs"))
}

pub fn stamp_by_id(id: &str) -> Option<&'static StampDef> {
    all_stamps().iter().find(|stamp| stamp.id == id)
}

pub fn stamp_categories() -> Vec<&'static str> {
    let mut categories = Vec::new();
    for stamp in all_stamps() {
        if !categories.contains(&stamp.category) {
            categories.push(stamp.category);
        }
    }
    categories
}

pub fn square_rect_from_drag(start: Pos2, end: Pos2) -> Rect {
    let delta = end - start;
    let extent = delta.x.abs().max(delta.y.abs()).max(1.0);
    Rect::from_two_pos(
        start,
        Pos2::new(
            start.x + delta.x.signum() * extent,
            start.y + delta.y.signum() * extent,
        ),
    )
}

pub fn rasterize_stamp(
    stamp: &StampDef,
    target_size: [u32; 2],
    tint: Color32,
) -> Result<RgbaImage, String> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(stamp.svg_bytes, &options)
        .map_err(|err| format!("parse SVG: {err}"))?;
    let svg_size = tree.size();
    let width = target_size[0].max(1);
    let height = target_size[1].max(1);
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "allocate pixmap".to_string())?;

    let scale = (width as f32 / svg_size.width()).min(height as f32 / svg_size.height());
    let tx = ((width as f32) - svg_size.width() * scale) * 0.5;
    let ty = ((height as f32) - svg_size.height() * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    let mut pm = pixmap.as_mut();
    resvg::render(&tree, transform, &mut pm);

    let mut rgba = RgbaImage::from_raw(width, height, pixmap.take())
        .ok_or_else(|| "build RGBA image".to_string())?;
    tint_stamp(&mut rgba, tint);
    Ok(rgba)
}

pub fn make_stamp_annotation(
    stamp: &StampDef,
    rect: Rect,
    tint: Color32,
) -> Result<Annotation, String> {
    let image = rasterize_stamp(
        stamp,
        [
            rect.width().round().max(1.0) as u32,
            rect.height().round().max(1.0) as u32,
        ],
        tint,
    )?;
    Ok(Annotation::new(
        ShapeData::ImageLayer {
            rect,
            data: Arc::new(image),
            texture: TextureCell::default(),
        },
        tint,
        1.0,
    ))
}

fn tint_stamp(image: &mut RgbaImage, tint: Color32) {
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            continue;
        }
        pixel[0] = tint.r();
        pixel[1] = tint.g();
        pixel[2] = tint.b();
    }
}

#[cfg(test)]
mod tests {
    use super::{all_stamps, make_stamp_annotation, stamp_categories};
    use egui::{pos2, vec2, Color32, Rect};

    #[test]
    fn every_stamp_can_build_annotation() {
        for stamp in all_stamps() {
            let result = make_stamp_annotation(
                stamp,
                Rect::from_min_size(pos2(0.0, 0.0), vec2(32.0, 32.0)),
                Color32::RED,
            );
            assert!(
                result.is_ok(),
                "stamp `{}` failed: {:?}",
                stamp.id,
                result.err()
            );
        }
    }

    #[test]
    fn stamp_categories_are_unique_and_nonempty() {
        let categories = stamp_categories();
        assert!(!categories.is_empty());
        for window in categories.windows(2) {
            assert_ne!(window[0], window[1]);
        }
    }
}

// ── Icon tinting ──────────────────────────────────────────────────────────────

/// egui tints images by *multiplying*, so a black-filled SVG stays black no
/// matter what colour the theme asks for — which is most of the icon set.
/// Rewrite every literal `fill`/`stroke` colour to white once, cache the
/// result, and let the tint do the colouring. Returns bytes usable directly as
/// an `egui::Image` source under the original URI.
pub fn white_svg(uri: &'static str, bytes: &'static [u8]) -> &'static [u8] {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static [u8]>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = map.get(uri) {
        return found;
    }
    let leaked: &'static [u8] = match std::str::from_utf8(bytes) {
        Ok(text) => Box::leak(whiten_svg_text(text).into_bytes().into_boxed_slice()),
        // Not UTF-8 (shouldn't happen for SVG) — pass through untouched.
        Err(_) => bytes,
    };
    map.insert(uri, leaked);
    leaked
}

/// Replace concrete colours in `fill=`/`stroke=` attributes and `fill:`/`stroke:`
/// style declarations with white. `none`, `url(...)` and `currentColor` are left
/// alone so masks, gradients and inheritance keep working.
fn whiten_svg_text(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &src[i..];
        let attr = ["fill=\"", "stroke=\"", "fill='", "stroke='"]
            .iter()
            .find(|p| rest.starts_with(**p))
            .copied();
        if let Some(prefix) = attr {
            let quote = prefix.chars().last().unwrap();
            let value_start = i + prefix.len();
            if let Some(end_off) = src[value_start..].find(quote) {
                let value = &src[value_start..value_start + end_off];
                out.push_str(prefix);
                out.push_str(&replacement(value));
                out.push(quote);
                i = value_start + end_off + 1;
                continue;
            }
        }
        let style = ["fill:", "stroke:"]
            .iter()
            .find(|p| rest.starts_with(**p))
            .copied();
        if let Some(prefix) = style {
            let value_start = i + prefix.len();
            let end_off = src[value_start..]
                .find([';', '"', '\''])
                .unwrap_or(src.len() - value_start);
            let value = src[value_start..value_start + end_off].trim();
            out.push_str(prefix);
            out.push_str(&replacement(value));
            i = value_start + end_off;
            continue;
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replacement(value: &str) -> String {
    let v = value.trim();
    if v.is_empty()
        || v.eq_ignore_ascii_case("none")
        || v.eq_ignore_ascii_case("currentColor")
        || v.eq_ignore_ascii_case("transparent")
        || v.starts_with("url(")
    {
        v.to_string()
    } else {
        "#ffffff".to_string()
    }
}

#[cfg(test)]
mod tint_tests {
    use super::whiten_svg_text;

    #[test]
    fn rewrites_literal_colors_only() {
        let src = r##"<path fill="#2e3436" stroke="none" style="fill:#000;stroke:red" d="M0 0"/>"##;
        let out = whiten_svg_text(src);
        assert!(out.contains(r##"fill="#ffffff""##), "{out}");
        assert!(out.contains(r##"stroke="none""##), "{out}");
        assert!(out.contains("fill:#ffffff;stroke:#ffffff"), "{out}");
        assert!(out.contains(r##"d="M0 0""##), "{out}");
    }

    #[test]
    fn leaves_url_and_current_color() {
        let src = r##"<g fill="url(#grad)" stroke="currentColor"/>"##;
        assert_eq!(whiten_svg_text(src), src);
    }
}
