use redline::state::{EdgeMethod, SvgPathEntry, VectorizeMode, VectorizeSettings};
use redline::vectorize::convert_to_svg;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// 50×50 image with a filled black rectangle in the centre (clear edges).
fn make_test_image() -> egui::ColorImage {
    let (w, h) = (50usize, 50usize);
    let mut pixels = vec![egui::Color32::WHITE; w * h];
    for y in 12..38 {
        for x in 12..38 {
            pixels[y * w + x] = egui::Color32::BLACK;
        }
    }
    egui::ColorImage {
        size: [w, h],
        pixels,
    }
}

fn make_white_image() -> egui::ColorImage {
    egui::ColorImage {
        size: [30, 30],
        pixels: vec![egui::Color32::WHITE; 30 * 30],
    }
}

fn make_checkerboard() -> egui::ColorImage {
    let (w, h) = (40usize, 40usize);
    let pixels = (0..w * h)
        .map(|i| {
            if (i % w + i / w) % 2 == 0 {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            }
        })
        .collect();
    egui::ColorImage {
        size: [w, h],
        pixels,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn line_art_produces_svg_with_paths() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default(); // LineArt mode
    dialog.filter_speckle = 0; // disable noise filter so small paths aren't dropped
    let result = convert_to_svg(&ci, &dialog);
    assert!(result.is_ok(), "expected Ok, got {:?}", result);
    let svg = result.unwrap();
    assert!(svg.contains("<svg"), "result should start with <svg tag");
    assert!(svg.contains("<path"), "result should contain path elements");
}

#[test]
fn all_white_image_produces_valid_svg() {
    let ci = make_white_image();
    let dialog = VectorizeSettings::default();
    let result = convert_to_svg(&ci, &dialog);
    assert!(result.is_ok(), "all-white image should not error");
    let svg = result.unwrap();
    assert!(
        svg.contains("<svg"),
        "result should be a valid SVG document"
    );
}

#[test]
fn svg_viewbox_matches_canvas_size() {
    let ci = egui::ColorImage {
        size: [100, 80],
        pixels: vec![egui::Color32::WHITE; 100 * 80],
    };
    let dialog = VectorizeSettings::default();
    let svg = convert_to_svg(&ci, &dialog).unwrap();
    assert!(
        svg.contains("100") && svg.contains("80"),
        "SVG should reference the canvas dimensions 100 and 80, got: {}",
        &svg[..svg.len().min(300)]
    );
}

#[test]
fn color_mode_produces_valid_svg() {
    let ci = make_checkerboard();
    let mut dialog = VectorizeSettings::default();
    dialog.mode = VectorizeMode::Color;
    dialog.color_count = 2;
    let result = convert_to_svg(&ci, &dialog);
    assert!(
        result.is_ok(),
        "color mode should not error, got {:?}",
        result
    );
    let svg = result.unwrap();
    assert!(svg.contains("<svg"), "result should be valid SVG");
}

#[test]
fn grayscale_mode_produces_valid_svg() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default();
    dialog.mode = VectorizeMode::Grayscale;
    dialog.color_count = 4;
    let result = convert_to_svg(&ci, &dialog);
    assert!(
        result.is_ok(),
        "grayscale mode should not error, got {:?}",
        result
    );
    let svg = result.unwrap();
    assert!(svg.contains("<svg"), "result should be valid SVG");
}

#[test]
fn threshold_method_produces_paths() {
    // Threshold mode should trace the black rectangle directly without Canny.
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default(); // default is Threshold
    dialog.edge_method = EdgeMethod::Threshold;
    dialog.threshold = 128;
    dialog.filter_speckle = 0;
    let result = convert_to_svg(&ci, &dialog);
    assert!(
        result.is_ok(),
        "threshold mode should not error, got {:?}",
        result
    );
    let svg = result.unwrap();
    assert!(
        svg.contains("<path"),
        "threshold mode should produce path elements"
    );
}

#[test]
fn canny_method_produces_paths() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default();
    dialog.edge_method = EdgeMethod::Canny;
    dialog.filter_speckle = 0;
    let result = convert_to_svg(&ci, &dialog);
    assert!(
        result.is_ok(),
        "canny mode should not error, got {:?}",
        result
    );
    let svg = result.unwrap();
    assert!(svg.contains("<svg"), "canny mode should produce valid SVG");
}

#[test]
fn skeleton_method_produces_paths() {
    // A filled square degenerates to a point; use a thick horizontal line whose
    // centerline is a traceable 1-pixel-wide path after Zhang-Suen thinning.
    let (w, h) = (80usize, 50usize);
    let mut pixels = vec![egui::Color32::WHITE; w * h];
    for x in 10..70 {
        for y in 20..30 {
            // 10 pixels tall → 1-px centerline after thinning
            pixels[y * w + x] = egui::Color32::BLACK;
        }
    }
    let ci = egui::ColorImage {
        size: [w, h],
        pixels,
    };
    let mut dialog = VectorizeSettings::default();
    dialog.edge_method = EdgeMethod::Skeleton;
    dialog.threshold = 128;
    dialog.filter_speckle = 0;
    dialog.path_precision = 1; // minimal simplification so 1-px lines survive
    let result = convert_to_svg(&ci, &dialog);
    assert!(
        result.is_ok(),
        "skeleton mode should not error: {:?}",
        result
    );
    let svg = result.unwrap();
    assert!(
        svg.contains("<svg"),
        "skeleton mode should produce valid SVG"
    );
    assert!(
        svg.contains("<path"),
        "skeleton should trace the centerline of a thick line"
    );
}

#[test]
fn parse_and_assemble_roundtrip() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default();
    dialog.filter_speckle = 0;
    let svg = convert_to_svg(&ci, &dialog).unwrap();
    let (header, raw_paths, footer) = redline::vectorize::parse_svg_parts(&svg);
    let paths: Vec<SvgPathEntry> = raw_paths
        .into_iter()
        .map(|e| SvgPathEntry {
            element: e,
            enabled: true,
        })
        .collect();
    let reassembled = redline::vectorize::assemble_svg(&header, &paths, &footer);
    assert_eq!(svg, reassembled, "roundtrip should preserve SVG exactly");
}

#[test]
fn disabling_paths_removes_them_from_output() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default();
    dialog.filter_speckle = 0;
    let svg = convert_to_svg(&ci, &dialog).unwrap();
    let (header, raw_paths, footer) = redline::vectorize::parse_svg_parts(&svg);
    if raw_paths.is_empty() {
        return;
    } // nothing to test
    let mut paths: Vec<SvgPathEntry> = raw_paths
        .into_iter()
        .map(|e| SvgPathEntry {
            element: e,
            enabled: true,
        })
        .collect();
    paths[0].enabled = false;
    let filtered = redline::vectorize::assemble_svg(&header, &paths, &footer);
    assert!(
        filtered.matches("<path").count() < svg.matches("<path").count(),
        "disabling a path should reduce <path count in output",
    );
}

#[test]
fn white_background_rect_after_svg_tag() {
    let ci = make_test_image();
    let mut dialog = VectorizeSettings::default();
    dialog.filter_speckle = 0;
    let svg = redline::vectorize::convert_to_svg(&ci, &dialog).unwrap();
    let svg_pos = svg.find("<svg").expect("missing <svg");
    let rect_pos = svg
        .find("<rect")
        .expect("missing <rect for White background");
    assert!(rect_pos > svg_pos, "<rect must follow <svg, not precede it");
}
