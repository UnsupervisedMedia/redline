use crate::state::AppState;
use egui::Context;
use image::DynamicImage;
use std::path::Path;

pub fn load_file(path: &Path, state: &mut AppState, ctx: &Context) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        load_pdf(path, state, ctx);
    } else if ext == "json" {
        // Sidecar-only drop: apply over the current canvas.
        match crate::export::import_sidecar(path, &mut state.canvas) {
            Ok(()) => state.status_msg = Some("Sidecar loaded".into()),
            Err(e) => state.status_msg = Some(e),
        }
    } else {
        load_image(path, state, ctx);
        try_auto_import_sidecar(path, state);
    }
}

pub fn load_image(path: &Path, state: &mut AppState, ctx: &Context) {
    match image::open(path) {
        Ok(img) => {
            let label = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Image")
                .to_string();
            state.push_current_as_tab_and_reset(label);
            apply_image(img, state, ctx);
            // Persist this tab by reference: the session manager stores the
            // path instead of re-encoding the pixels into a PNG blob. (PDFs go
            // through load_pdf and intentionally stay blob-persisted, since the
            // source is a specific rendered page, not a directly re-openable
            // raster file.)
            state.canvas.source_path = Some(path.to_path_buf());
            state.canvas.mark_source_changed();
            // A freshly opened file is "saved" — establish the baseline so the
            // saved indicator starts green and Save overwrites this path.
            state.canvas.mark_saved();
        }
        Err(e) => state.status_msg = Some(format!("Failed to load image: {e}")),
    }
}

/// If `<image>.redline.json` exists beside the loaded image, auto-load it.
fn try_auto_import_sidecar(image_path: &Path, state: &mut AppState) {
    let mut candidate = image_path.to_path_buf();
    if let Some(stem) = image_path.file_stem().and_then(|s| s.to_str()) {
        candidate.set_file_name(format!("{stem}.redline.json"));
        if candidate.exists() {
            match crate::export::import_sidecar(&candidate, &mut state.canvas) {
                Ok(()) => state.status_msg = Some("Image + sidecar loaded".into()),
                Err(e) => eprintln!("sidecar import: {e}"),
            }
        }
    }
}

pub fn apply_image(img: DynamicImage, state: &mut AppState, ctx: &Context) {
    let rgba = img.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    let pixels = rgba.into_raw();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels);
    state.view_zoom = 1.0;
    state.view_pan = egui::Vec2::ZERO;
    state.tool_state = crate::state::ToolState::Idle;
    state.canvas.selection = None;
    state.canvas.adjust_preview = None;
    state.canvas.marquee_state = crate::state::MarqueeState::Idle;
    state.last_img_draw_rect = None;
    state.canvas.canvas_size = egui::vec2(width as f32, height as f32);
    state.canvas.color_image = Some(color_image.clone());
    state.canvas.assign_new_uid();
    state.canvas.texture = Some(crate::texture_upload::load_texture_fitted(
        ctx,
        "canvas",
        color_image,
        egui::TextureOptions::default(),
    ));
    state.canvas.document = crate::scene::Document::from_flat(Vec::new());
    state.canvas.sync_annotations();
    state.canvas.clear_all_undo_redo();
    state.status_msg = None;
}

#[cfg(feature = "pdf")]
fn load_pdf(path: &Path, state: &mut AppState, ctx: &Context) {
    use pdfium_render::prelude::*;

    let bindings = match Pdfium::bind_to_statically_linked_library() {
        Ok(b) => b,
        Err(e) => {
            state.status_msg = Some(format!("pdfium init error: {e}"));
            return;
        }
    };
    let pdfium = Pdfium::new(bindings);

    let doc = match pdfium.load_pdf_from_file(path, None) {
        Ok(d) => d,
        Err(e) => {
            state.status_msg = Some(format!("PDF load error: {e}"));
            return;
        }
    };

    state.canvas.pdf_pages = doc.pages().len() as usize;
    state.canvas.pdf_page = state
        .canvas
        .pdf_page
        .min(state.canvas.pdf_pages.saturating_sub(1));

    let page = match doc.pages().get(state.canvas.pdf_page as u16) {
        Ok(p) => p,
        Err(e) => {
            state.status_msg = Some(format!("PDF page error: {e}"));
            return;
        }
    };

    let bitmap = match page.render_with_config(
        &PdfRenderConfig::new()
            .set_target_width(1920)
            .set_maximum_height(2700),
    ) {
        Ok(b) => b,
        Err(e) => {
            state.status_msg = Some(format!("PDF render error: {e}"));
            return;
        }
    };

    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PDF")
        .to_string();
    state.push_current_as_tab_and_reset(label);
    let img = bitmap.as_image();
    apply_image(img, state, ctx);
}

#[cfg(not(feature = "pdf"))]
fn load_pdf(_path: &Path, state: &mut AppState, _ctx: &Context) {
    state.status_msg =
        Some("PDF support not compiled in (rebuild with --features pdf)".to_string());
}
