use egui::ColorImage;
use image::GrayImage;

use crate::state::{EdgeMethod, SvgBackground, SvgPathEntry, VectorizeMode, VectorizeSettings};

// ── Public entry point ────────────────────────────────────────────────────────

pub fn convert_to_svg(ci: &ColorImage, settings: &VectorizeSettings) -> Result<String, String> {
    let (w, h) = (ci.size[0], ci.size[1]);
    if w == 0 || h == 0 {
        return Err("Canvas is empty".into());
    }
    let svg = match settings.mode {
        VectorizeMode::LineArt => line_art_to_svg(ci, settings),
        VectorizeMode::Grayscale => fill_to_svg(ci, settings, vtracer::ColorMode::Color, true),
        VectorizeMode::Color => fill_to_svg(ci, settings, vtracer::ColorMode::Color, false),
    }?;
    if settings.post_smooth > 0 {
        Ok(smooth_svg(&svg, settings.post_smooth))
    } else {
        Ok(svg)
    }
}

// ── Line Art ──────────────────────────────────────────────────────────────────

fn line_art_to_svg(ci: &ColorImage, settings: &VectorizeSettings) -> Result<String, String> {
    match settings.edge_method {
        EdgeMethod::Threshold => threshold_to_svg(ci, settings),
        EdgeMethod::Skeleton => skeleton_to_svg(ci, settings),
        EdgeMethod::Canny => canny_to_svg(ci, settings),
    }
}

/// Luminance threshold → vtracer Binary. Best for images that are already black
/// line art; preserves stroke width without doubling edges.
fn threshold_to_svg(ci: &ColorImage, settings: &VectorizeSettings) -> Result<String, String> {
    let gray = to_gray_image(ci);
    let blurred: GrayImage = if settings.blur_sigma > 0.1 {
        image::imageops::blur(&gray, settings.blur_sigma)
    } else {
        gray
    };
    // threshold(): pixels < thresh → 0 (black), pixels >= thresh → 255 (white).
    // Dark ink (low luma) becomes 0; vtracer Binary traces dark regions. No
    // inversion needed — the orientation already matches.
    let binary = imageproc::contrast::threshold(
        &blurred,
        settings.threshold,
        imageproc::contrast::ThresholdType::Binary,
    );
    let rgba = to_rgba_from_gray(&binary);
    let (w, h) = (ci.size[0], ci.size[1]);
    let vt_img = vtracer::ColorImage {
        pixels: rgba,
        width: w,
        height: h,
    };
    let config = build_vtracer_config(settings, vtracer::ColorMode::Binary);
    let svg_file = vtracer::convert(vt_img, config).map_err(|e| e.to_string())?;
    Ok(apply_line_art_style(&svg_file.to_string(), settings))
}

/// Zhang-Suen morphological thinning → vtracer Binary. Reduces each stroke to a
/// 1-pixel skeleton so the traced paths follow the center of the original lines.
fn skeleton_to_svg(ci: &ColorImage, settings: &VectorizeSettings) -> Result<String, String> {
    let gray = to_gray_image(ci);
    let blurred: GrayImage = if settings.blur_sigma > 0.1 {
        image::imageops::blur(&gray, settings.blur_sigma)
    } else {
        gray
    };
    let binary = imageproc::contrast::threshold(
        &blurred,
        settings.threshold,
        imageproc::contrast::ThresholdType::Binary,
    );
    let skeleton = zhang_suen_thin(&binary);
    let rgba = to_rgba_from_gray(&skeleton);
    let (w, h) = (ci.size[0], ci.size[1]);
    let vt_img = vtracer::ColorImage {
        pixels: rgba,
        width: w,
        height: h,
    };
    let config = build_vtracer_config(settings, vtracer::ColorMode::Binary);
    let svg_file = vtracer::convert(vt_img, config).map_err(|e| e.to_string())?;
    Ok(apply_line_art_style(&svg_file.to_string(), settings))
}

/// Zhang-Suen thinning algorithm. Input/output: GrayImage where 0 = ink, 255 = background.
/// Iteratively erodes foreground pixels until only a 1-pixel-wide skeleton remains.
fn zhang_suen_thin(binary: &GrayImage) -> GrayImage {
    let w = binary.width() as usize;
    let h = binary.height() as usize;

    // Work in bool space: true = ink (pixel 0), false = background (pixel 255)
    let mut grid: Vec<bool> = binary.pixels().map(|p| p.0[0] == 0).collect();

    for _ in 0..200 {
        let mut changed = false;

        // Sub-iteration 1 and 2 — conditions differ only in the product tests
        for pass in 0..2u8 {
            let mut remove = vec![false; w * h];
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let idx = y * w + x;
                    if !grid[idx] {
                        continue;
                    }
                    let nb = zs_neighbors(&grid, x, y, w);
                    let b = nb.iter().filter(|&&p| p).count();
                    if b < 2 || b > 6 {
                        continue;
                    }
                    // Count 0→1 transitions in the cyclic neighbor sequence
                    let a = nb.windows(2).filter(|w| !w[0] && w[1]).count()
                        + usize::from(!nb[7] && nb[0]); // wrap-around p9→p2
                    if a != 1 {
                        continue;
                    }
                    let [p2, p4, p6, p8] = [nb[0], nb[2], nb[4], nb[6]];
                    let cond = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if cond {
                        remove[idx] = true;
                    }
                }
            }
            for (i, rm) in remove.into_iter().enumerate() {
                if rm {
                    grid[i] = false;
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }

    let pixels: Vec<u8> = grid.iter().map(|&ink| if ink { 0 } else { 255 }).collect();
    GrayImage::from_raw(w as u32, h as u32, pixels).expect("consistent dimensions")
}

/// Returns the 8 clockwise neighbors starting from top: p2,p3,p4,p5,p6,p7,p8,p9.
fn zs_neighbors(grid: &[bool], x: usize, y: usize, w: usize) -> [bool; 8] {
    [
        grid[(y - 1) * w + x],     // p2 top
        grid[(y - 1) * w + x + 1], // p3 top-right
        grid[y * w + x + 1],       // p4 right
        grid[(y + 1) * w + x + 1], // p5 bottom-right
        grid[(y + 1) * w + x],     // p6 bottom
        grid[(y + 1) * w + x - 1], // p7 bottom-left
        grid[y * w + x - 1],       // p8 left
        grid[(y - 1) * w + x - 1], // p9 top-left
    ]
}

/// Canny edge detection → vtracer Binary. Best for extracting edges from photos
/// or complex shaded images.
fn canny_to_svg(ci: &ColorImage, settings: &VectorizeSettings) -> Result<String, String> {
    let gray = to_gray_image(ci);

    // Optional Gaussian pre-blur (reduces noise before edge detection)
    let blurred: GrayImage = if settings.blur_sigma > 0.1 {
        image::imageops::blur(&gray, settings.blur_sigma)
    } else {
        gray
    };

    // Canny edge detection — returns GrayImage where edges = 255, background = 0
    let edges = imageproc::edges::canny(&blurred, settings.edge_low, settings.edge_high);

    // Invert: vtracer Binary mode traces dark regions, so edges must be dark (0)
    let mut inverted = edges;
    image::imageops::invert(&mut inverted);

    let rgba = to_rgba_from_gray(&inverted);
    let (w, h) = (ci.size[0], ci.size[1]);
    let vt_img = vtracer::ColorImage {
        pixels: rgba,
        width: w,
        height: h,
    };

    let config = build_vtracer_config(settings, vtracer::ColorMode::Binary);
    let svg_file = vtracer::convert(vt_img, config).map_err(|e| e.to_string())?;
    Ok(apply_line_art_style(&svg_file.to_string(), settings))
}

// ── Grayscale / Color fill ────────────────────────────────────────────────────

fn fill_to_svg(
    ci: &ColorImage,
    settings: &VectorizeSettings,
    color_mode: vtracer::ColorMode,
    grayscale: bool,
) -> Result<String, String> {
    let (w, h) = (ci.size[0], ci.size[1]);
    let rgba = if grayscale {
        ci.pixels
            .iter()
            .flat_map(|c| {
                let lum =
                    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) as u8;
                [lum, lum, lum, c.a()]
            })
            .collect::<Vec<u8>>()
    } else {
        ci.pixels
            .iter()
            .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
            .collect::<Vec<u8>>()
    };
    let vt_img = vtracer::ColorImage {
        pixels: rgba,
        width: w,
        height: h,
    };
    // color_precision controls quantization depth (1–8 bits per channel).
    // We map color_count (2–32) to a precision that approximates the desired
    // palette size: precision = ceil(log2(color_count)).
    let color_precision = ((settings.color_count as f32).log2().ceil() as i32)
        .max(1)
        .min(8);
    let config = vtracer::Config {
        color_mode,
        filter_speckle: settings.filter_speckle as usize,
        color_precision,
        ..vtracer::Config::default()
    };
    let svg_file = vtracer::convert(vt_img, config).map_err(|e| e.to_string())?;
    Ok(svg_file.to_string())
}

// ── Post-process SVG for line art style ───────────────────────────────────────

// vtracer Binary mode produces filled black paths. We replace them with
// stroked paths in the user's chosen colour, and optionally add a background.
fn apply_line_art_style(svg: &str, settings: &VectorizeSettings) -> String {
    let c = settings.stroke_color;
    let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    let stroke_attrs = if settings.svg_fill {
        let fc = settings.svg_fill_color;
        let fhex = format!("#{:02X}{:02X}{:02X}", fc.r(), fc.g(), fc.b());
        format!(
            r#"fill="{}" stroke="{}" stroke-width="{}""#,
            fhex, hex, settings.stroke_width
        )
    } else {
        format!(
            r#"fill="none" stroke="{}" stroke-width="{}""#,
            hex, settings.stroke_width
        )
    };

    // Replace both formats vtracer may emit for black fills
    let styled = svg.replace("fill=\"#000000\"", &stroke_attrs);
    let styled = styled.replace("fill=\"black\"", &stroke_attrs);

    let bg_color = match settings.background {
        SvgBackground::White => Some("#ffffff"),
        SvgBackground::LightGray => Some("#f0f0f0"),
        SvgBackground::Transparent => None,
    };
    if let Some(color) = bg_color {
        // Search from <svg to avoid matching > inside <?xml?> or <!-- comments -->
        if let Some(svg_pos) = styled.find("<svg") {
            if let Some(rel) = styled[svg_pos..].find('>') {
                let pos = svg_pos + rel;
                let mut result = styled[..=pos].to_string();
                result.push_str(&format!(
                    r#"<rect width="100%" height="100%" fill="{}"/>"#,
                    color
                ));
                result.push_str(&styled[pos + 1..]);
                return result;
            }
        }
    }
    styled
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn to_gray_image(ci: &ColorImage) -> GrayImage {
    let (w, h) = (ci.size[0] as u32, ci.size[1] as u32);
    let pixels: Vec<u8> = ci
        .pixels
        .iter()
        .map(|c| {
            let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
            lum as u8
        })
        .collect();
    GrayImage::from_raw(w, h, pixels).expect("consistent dimensions")
}

/// Convert an inverted grayscale edge image to RGBA bytes for visioncortex::ColorImage.
fn to_rgba_from_gray(gray: &GrayImage) -> Vec<u8> {
    gray.pixels()
        .flat_map(|p| {
            let v = p.0[0];
            [v, v, v, 255]
        })
        .collect()
}

fn build_vtracer_config(
    settings: &VectorizeSettings,
    color_mode: vtracer::ColorMode,
) -> vtracer::Config {
    vtracer::Config {
        color_mode,
        hierarchical: vtracer::Hierarchical::Stacked,
        filter_speckle: settings.filter_speckle as usize,
        color_precision: 6,
        layer_difference: 16,
        corner_threshold: settings.corner_threshold as i32,
        length_threshold: settings.segment_length as f64,
        max_iterations: 10,
        splice_threshold: settings.splice_threshold as i32,
        path_precision: Some(settings.path_precision),
        // mode defaults to PathSimplifyMode::Spline from visioncortex
        ..vtracer::Config::default()
    }
}

// ── SVG path editing helpers ──────────────────────────────────────────────────

/// Split an SVG string into (header, path_elements, footer).
/// `header` is everything up to the first `<path`; `footer` is everything
/// after the last `/>` closing a path; `path_elements` is each `<path…/>`.
pub fn parse_svg_parts(svg: &str) -> (String, Vec<String>, String) {
    let mut paths = Vec::new();
    let mut pos = 0usize;
    let mut first_path = None::<usize>;
    let mut last_end = 0usize;

    while let Some(rel) = svg[pos..].find("<path") {
        let start = pos + rel;
        if first_path.is_none() {
            first_path = Some(start);
        }
        match svg[start..].find("/>") {
            Some(close_rel) => {
                let end = start + close_rel + 2;
                paths.push(svg[start..end].to_string());
                last_end = end;
                pos = end;
            }
            None => break,
        }
    }

    let header = first_path.map_or_else(|| svg.to_string(), |p| svg[..p].to_string());
    let footer = if last_end > 0 {
        svg[last_end..].to_string()
    } else {
        String::new()
    };
    (header, paths, footer)
}

/// Reassemble header + enabled paths + footer into a complete SVG string.
pub fn assemble_svg(header: &str, paths: &[SvgPathEntry], footer: &str) -> String {
    let mut out = header.to_string();
    for p in paths {
        if p.enabled {
            out.push_str(&p.element);
        }
    }
    out.push_str(footer);
    out
}

// ── Post-process SVG coordinate smoothing ────────────────────────────────────

/// Apply `passes` rounds of weighted 3-point averaging to every `d="…"` attribute
/// in the SVG. Each pass replaces each interior coordinate with
/// `(prev + 2*cur + next) / 4`.
pub fn smooth_svg(svg: &str, passes: u8) -> String {
    let mut result = String::with_capacity(svg.len());
    let mut pos = 0;
    while let Some(rel) = svg[pos..].find("d=\"") {
        let attr_start = pos + rel;
        let val_start = attr_start + 3; // skip d="
        match svg[val_start..].find('"') {
            None => break,
            Some(end_rel) => {
                let val_end = val_start + end_rel;
                result.push_str(&svg[pos..val_start]);
                result.push_str(&smooth_path_d(&svg[val_start..val_end], passes));
                result.push('"');
                pos = val_end + 1; // step past closing "
            }
        }
    }
    result.push_str(&svg[pos..]);
    result
}

/// Apply `passes` rounds of weighted 3-point averaging to the coordinate numbers
/// in a single SVG path `d` attribute value.
fn smooth_path_d(d: &str, passes: u8) -> String {
    // Parse numbers (including sign and decimal point) while preserving the
    // non-numeric command characters exactly.
    struct Token {
        is_num: bool,
        text: String,
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut chars = d.chars().peekable();
    loop {
        // Skip whitespace and commas, keeping them as a separator token
        let mut sep = String::new();
        while chars
            .peek()
            .map_or(false, |c| c.is_ascii_whitespace() || *c == ',')
        {
            sep.push(chars.next().unwrap());
        }
        if !sep.is_empty() {
            tokens.push(Token {
                is_num: false,
                text: sep,
            });
        }

        match chars.peek() {
            None => break,
            Some(&c) if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' => {
                let mut num = String::new();
                // consume optional sign
                if c == '-' || c == '+' {
                    num.push(chars.next().unwrap());
                }
                while chars
                    .peek()
                    .map_or(false, |c| c.is_ascii_digit() || *c == '.')
                {
                    num.push(chars.next().unwrap());
                }
                tokens.push(Token {
                    is_num: true,
                    text: num,
                });
            }
            Some(_) => {
                // SVG path command letter or other character
                let mut cmd = String::new();
                while chars.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
                    cmd.push(chars.next().unwrap());
                }
                if cmd.is_empty() {
                    cmd.push(chars.next().unwrap());
                }
                tokens.push(Token {
                    is_num: false,
                    text: cmd,
                });
            }
        }
    }

    // Collect numeric token indices and their values
    let num_indices: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_num)
        .map(|(i, _)| i)
        .collect();

    if num_indices.len() < 3 {
        return d.to_string();
    }

    let mut values: Vec<f64> = num_indices
        .iter()
        .map(|&i| tokens[i].text.parse::<f64>().unwrap_or(0.0))
        .collect();

    for _ in 0..passes {
        let mut smoothed = values.clone();
        for j in 1..values.len() - 1 {
            smoothed[j] = (values[j - 1] + 2.0 * values[j] + values[j + 1]) / 4.0;
        }
        values = smoothed;
    }

    // Write smoothed values back into tokens
    for (k, &idx) in num_indices.iter().enumerate() {
        // Preserve up to 2 decimal places; drop trailing zeros
        let v = values[k];
        tokens[idx].text = format!("{:.2}", v)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string();
    }

    tokens.into_iter().map(|t| t.text).collect()
}
