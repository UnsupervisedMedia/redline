use crate::{
    state::{AppState, CanvasState},
    types::{
        Annotation, AnnotationExport, ArrowHeads, FocusRegionExport, LineStyle, LogicTagKind,
        NormalizedRect, ParticipantId, ShapeData, SidecarExport,
    },
};
use base64::Engine;
use chrono::Utc;
use egui::{Color32, Pos2, Rect, Vec2};

/// Returns `None` for identity (omit from JSON), `Some(coeffs)` otherwise.
fn aff_to_arr(a: kurbo::Affine) -> Option<[f64; 6]> {
    if a == kurbo::Affine::IDENTITY {
        None
    } else {
        Some(a.as_coeffs())
    }
}

pub fn build_sidecar(canvas: &CanvasState) -> SidecarExport {
    let sz = canvas.canvas_size;
    let now = Utc::now().to_rfc3339();

    let focus_region = canvas
        .annotations
        .iter()
        .find(|a| matches!(a.shape, ShapeData::FocusRegion { .. }))
        .map(|a| {
            let rect = a.shape.bounding_rect();
            let nr = NormalizedRect::from_rect(rect, sz);
            FocusRegionExport {
                x: rect.min.x,
                y: rect.min.y,
                w: rect.width(),
                h: rect.height(),
                normalized: nr,
            }
        });

    let annotations: Vec<AnnotationExport> = canvas
        .annotations
        .iter()
        .filter(|a| !matches!(a.shape, ShapeData::FocusRegion { .. }))
        .map(|a| {
            let mut exp = annotation_to_export(a, sz);
            exp.transform = canvas
                .document
                .get(a.id)
                .and_then(|n| aff_to_arr(n.transform));
            exp
        })
        .collect();

    // Build clipboard text summary
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref fr) = focus_region {
        parts.push(format!(
            "Focus region at [x:{} y:{} w:{} h:{}]",
            fr.normalized.x, fr.normalized.y, fr.normalized.w, fr.normalized.h
        ));
    }
    for ann in &annotations {
        let coords_str = ann
            .coords
            .as_ref()
            .map(|c| format!("[x:{} y:{} w:{} h:{}]", c.x, c.y, c.w, c.h))
            .unwrap_or_default();
        let tag_str = ann
            .tag
            .as_deref()
            .map(|t| format!(" intent:{}", t))
            .unwrap_or_default();
        let label_str = ann
            .label
            .as_deref()
            .map(|l| format!(" '{}'", l))
            .unwrap_or_default();
        let content_str = ann
            .content
            .as_deref()
            .map(|c| format!(" \"{}\"", c))
            .unwrap_or_default();
        parts.push(format!(
            "{} at {}{}{}{}",
            ann.kind, coords_str, tag_str, label_str, content_str
        ));
    }
    let clipboard_text = parts.join(". ");

    // Sidecar v2: describe the layer/group tree so it survives a save→import.
    let layers: Vec<crate::types::LayerExport> = canvas
        .document
        .layers()
        .iter()
        .filter_map(|lid| {
            let lnode = canvas.document.get(*lid)?;
            let mut shape_ids = Vec::new();
            let mut groups = Vec::new();
            for child in &lnode.children {
                let cn = canvas.document.get(*child)?;
                match &cn.kind {
                    crate::scene::NodeKind::Shape(_) => shape_ids.push(child.to_string()),
                    crate::scene::NodeKind::Group => {
                        let member_ids: Vec<String> = canvas
                            .document
                            .descendant_leaf_ids(*child)
                            .iter()
                            .map(|i| i.to_string())
                            .collect();
                        // Group members must also be listed among the layer's shapes
                        // so the importer can place them, then reparent into the group.
                        for m in &member_ids {
                            shape_ids.push(m.clone());
                        }
                        groups.push(crate::types::GroupExport {
                            name: cn.name.clone(),
                            visible: cn.visible,
                            locked: cn.locked,
                            opacity: cn.opacity,
                            shape_ids: member_ids,
                            transform: aff_to_arr(cn.transform),
                        });
                    }
                    crate::scene::NodeKind::Layer => {} // layers are never nested
                }
            }
            Some(crate::types::LayerExport {
                name: lnode.name.clone(),
                visible: lnode.visible,
                locked: lnode.locked,
                opacity: lnode.opacity,
                shape_ids,
                groups,
                transform: aff_to_arr(lnode.transform),
            })
        })
        .collect();

    SidecarExport {
        canvas_size: [sz.x, sz.y],
        captured_at: now,
        focus_region,
        annotations,
        clipboard_text,
        layers: Some(layers),
        #[cfg(feature = "local-ai")]
        agent_memory: canvas.agent_memory.clone(),
        #[cfg(feature = "local-ai")]
        shared_notes: canvas.shared_notes.clone(),
        #[cfg(feature = "local-ai")]
        agent_bookmarks: canvas.agent_bookmarks.clone(),
    }
}

fn annotation_to_export(a: &Annotation, sz: Vec2) -> AnnotationExport {
    let coords = if sz.x > 0.0 && sz.y > 0.0 {
        Some(NormalizedRect::from_rect(a.shape.bounding_rect(), sz))
    } else {
        None
    };
    let mut out = AnnotationExport {
        id: a.id.to_string(),
        kind: String::new(),
        coords,
        color: color_to_hex(a.color),
        label: a.label.clone(),
        tag: None,
        author: Some(if a.author.is_human_local() {
            "human-local".to_string()
        } else {
            a.author.0.to_string()
        }),
        thickness: Some(a.thickness),
        start: None,
        end: None,
        points: None,
        pos: None,
        rect: None,
        content: None,
        text_size: None,
        block_size: None,
        style: None,
        heads: None,
        number: None,
        radius: None,
        fill: None,
        fill_opacity: None,
        stroke: None,
        tail: None,
        image_png: None,
        transform: None,
    };
    match &a.shape {
        ShapeData::Rect {
            rect,
            radius,
            fill,
            stroke,
            stroke_style,
        } => {
            out.kind = "rect".into();
            out.rect = Some(rect_to_arr(*rect));
            out.radius = Some(*radius);
            out.fill = fill.map(fill_color_to_hex);
            out.fill_opacity = fill.map(|c| c.a());
            out.stroke = Some(*stroke);
            out.style = Some(*stroke_style);
        }
        ShapeData::Oval {
            rect,
            fill,
            stroke,
            stroke_style,
        } => {
            out.kind = "oval".into();
            out.rect = Some(rect_to_arr(*rect));
            out.fill = fill.map(fill_color_to_hex);
            out.fill_opacity = fill.map(|c| c.a());
            out.stroke = Some(*stroke);
            out.style = Some(*stroke_style);
        }
        ShapeData::Star { rect } => {
            out.kind = "star".into();
            out.rect = Some(rect_to_arr(*rect));
        }
        ShapeData::Arrow {
            start,
            end,
            style,
            heads,
        } => {
            out.kind = "arrow".into();
            out.start = Some([start.x, start.y]);
            out.end = Some([end.x, end.y]);
            out.style = Some(*style);
            out.heads = Some(*heads);
        }
        ShapeData::Line { start, end, style } => {
            out.kind = "line".into();
            out.start = Some([start.x, start.y]);
            out.end = Some([end.x, end.y]);
            out.style = Some(*style);
        }
        ShapeData::Pen { raw_points, .. } => {
            out.kind = "pen".into();
            out.points = Some(raw_points.iter().map(|p| [p.x, p.y]).collect());
        }
        ShapeData::LogicTag { pos, tag } => {
            out.kind = "logic_tag".into();
            out.tag = Some(tag.label().to_string());
            out.pos = Some([pos.x, pos.y]);
        }
        ShapeData::Text { pos, content, size } => {
            out.kind = "text".into();
            out.pos = Some([pos.x, pos.y]);
            out.content = Some(content.clone());
            out.text_size = Some(*size);
        }
        ShapeData::Question { pos, content, size } => {
            out.kind = "question".into();
            out.pos = Some([pos.x, pos.y]);
            out.content = Some(content.clone());
            out.text_size = Some(*size);
        }
        ShapeData::Blur { rect, block_size } => {
            out.kind = "blur".into();
            out.rect = Some(rect_to_arr(*rect));
            out.block_size = Some(*block_size);
        }
        ShapeData::StepMarker {
            pos,
            number,
            radius,
        } => {
            out.kind = "step".into();
            out.pos = Some([pos.x, pos.y]);
            out.number = Some(*number);
            out.radius = Some(*radius);
        }
        ShapeData::Callout { rect, tail } => {
            out.kind = "callout".into();
            out.rect = Some(rect_to_arr(*rect));
            out.tail = Some([tail.x, tail.y]);
        }
        ShapeData::Ruler { start, end } => {
            out.kind = "ruler".into();
            out.start = Some([start.x, start.y]);
            out.end = Some([end.x, end.y]);
        }
        ShapeData::ImageLayer { rect, data, .. } => {
            out.kind = "image_layer".into();
            out.rect = Some(rect_to_arr(*rect));
            out.image_png = encode_image_layer_png(data.as_ref());
        }
        ShapeData::FocusRegion { .. } => unreachable!(),
        ShapeData::BezierPath { .. } => {
            out.kind = "bezier_path".into();
        }
    }
    out
}

fn rect_to_arr(r: Rect) -> [f32; 4] {
    [r.min.x, r.min.y, r.width(), r.height()]
}

fn color_to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Like `color_to_hex`, but for colors that may carry alpha < 255 (e.g. shape
/// `fill`). `Color32` stores premultiplied components, so `.r()/.g()/.b()`
/// alone would bake the alpha into the hex; unmultiply first so the hex is
/// the straight sRGB color and `fill_opacity` (stored separately) carries the
/// alpha.
fn fill_color_to_hex(c: Color32) -> String {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn hex_to_color(s: &str) -> Color32 {
    let s = s.trim_start_matches('#');
    let parse = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
    if s.len() >= 6 {
        Color32::from_rgb(parse(0), parse(2), parse(4))
    } else {
        Color32::WHITE
    }
}

/// Reconstruct an Annotation from its AnnotationExport. Returns None if the
/// export lacks enough data (e.g. old sidecar without shape-specific fields).
pub fn export_to_annotation(e: &AnnotationExport) -> Option<Annotation> {
    let color = hex_to_color(&e.color);
    let thickness = e.thickness.unwrap_or(3.0);
    let rect_from =
        |r: &[f32; 4]| Rect::from_min_size(Pos2::new(r[0], r[1]), Vec2::new(r[2], r[3]));
    let pos_from = |p: &[f32; 2]| Pos2::new(p[0], p[1]);
    let fill_from = |hex: &str, a: u8| {
        let base = hex_to_color(hex);
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
    };

    let shape = match e.kind.as_str() {
        "rect" => ShapeData::Rect {
            rect: rect_from(e.rect.as_ref()?),
            radius: e.radius.unwrap_or(0.0),
            fill: match (e.fill.as_deref(), e.fill_opacity) {
                (Some(hex), Some(a)) => Some(fill_from(hex, a)),
                _ => None,
            },
            stroke: e.stroke.unwrap_or(true),
            stroke_style: e.style.unwrap_or(LineStyle::Solid),
        },
        "oval" => ShapeData::Oval {
            rect: rect_from(e.rect.as_ref()?),
            fill: match (e.fill.as_deref(), e.fill_opacity) {
                (Some(hex), Some(a)) => Some(fill_from(hex, a)),
                _ => None,
            },
            stroke: e.stroke.unwrap_or(true),
            stroke_style: e.style.unwrap_or(LineStyle::Solid),
        },
        "star" => ShapeData::Star {
            rect: rect_from(e.rect.as_ref()?),
        },
        "arrow" => ShapeData::Arrow {
            start: pos_from(e.start.as_ref()?),
            end: pos_from(e.end.as_ref()?),
            style: e.style.unwrap_or(LineStyle::Solid),
            heads: e.heads.unwrap_or(ArrowHeads::Single),
        },
        "line" => ShapeData::Line {
            start: pos_from(e.start.as_ref()?),
            end: pos_from(e.end.as_ref()?),
            style: e.style.unwrap_or(LineStyle::Solid),
        },
        "pen" => {
            let pts: Vec<Pos2> = e.points.as_ref()?.iter().map(|p| pos_from(p)).collect();
            if pts.len() < 2 {
                return None;
            }
            ShapeData::Pen {
                smoothed_points: crate::smoothing::catmull_rom(&pts, 8),
                raw_points: pts,
            }
        }
        "logic_tag" => {
            let tag_label = e.tag.as_deref().unwrap_or("");
            let kind = match tag_label {
                "Move Here" => LogicTagKind::MoveHere,
                "Delete This" => LogicTagKind::DeleteThis,
                "Fix Text" => LogicTagKind::FixText,
                "Add Here" => LogicTagKind::AddHere,
                "Important" => LogicTagKind::Important,
                "Question?" => LogicTagKind::Question,
                "Wrong" => LogicTagKind::Wrong,
                other => LogicTagKind::Custom(other.to_string()),
            };
            ShapeData::LogicTag {
                pos: pos_from(e.pos.as_ref()?),
                tag: kind,
            }
        }
        "text" => ShapeData::Text {
            pos: pos_from(e.pos.as_ref()?),
            content: e.content.clone().unwrap_or_default(),
            size: e.text_size.unwrap_or(18.0),
        },
        "question" => ShapeData::Question {
            pos: pos_from(e.pos.as_ref()?),
            content: e.content.clone().unwrap_or_default(),
            size: e.text_size.unwrap_or(18.0),
        },
        "blur" => ShapeData::Blur {
            rect: rect_from(e.rect.as_ref()?),
            block_size: e.block_size.unwrap_or(12.0),
        },
        "step" => ShapeData::StepMarker {
            pos: pos_from(e.pos.as_ref()?),
            number: e.number.unwrap_or(1),
            radius: e.radius.unwrap_or(18.0),
        },
        // Legacy kind strings from before the Rect/Oval consolidation — recognized
        // on import only. New exports never emit these; see export_to_annotation
        // migration notes in the design spec.
        "highlighter" => ShapeData::Rect {
            rect: rect_from(e.rect.as_ref()?),
            radius: 0.0,
            fill: Some(Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                120,
            )),
            stroke: false,
            stroke_style: LineStyle::Solid,
        },
        "callout" => {
            let rect = rect_from(e.rect.as_ref()?);
            let tail = e
                .tail
                .as_ref()
                .map(|t| Pos2::new(t[0], t[1]))
                .unwrap_or_else(|| Pos2::new(rect.min.x + 20.0, rect.max.y + 30.0));
            ShapeData::Callout { rect, tail }
        }
        "rounded_rect" => ShapeData::Rect {
            rect: rect_from(e.rect.as_ref()?),
            radius: e.radius.unwrap_or(8.0),
            fill: Some(Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                20,
            )),
            stroke: true,
            stroke_style: LineStyle::Solid,
        },
        "redact" => ShapeData::Rect {
            rect: rect_from(e.rect.as_ref()?),
            radius: 0.0,
            fill: Some(Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                255,
            )),
            stroke: false,
            stroke_style: LineStyle::Solid,
        },
        "redact_oval" => ShapeData::Oval {
            rect: rect_from(e.rect.as_ref()?),
            fill: Some(Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                255,
            )),
            stroke: false,
            stroke_style: LineStyle::Solid,
        },
        "ruler" => ShapeData::Ruler {
            start: pos_from(e.start.as_ref()?),
            end: pos_from(e.end.as_ref()?),
        },
        "image_layer" => ShapeData::ImageLayer {
            rect: rect_from(e.rect.as_ref()?),
            data: std::sync::Arc::new(decode_image_layer_png(e.image_png.as_deref()?)?),
            texture: crate::types::TextureCell::default(),
        },
        _ => return None,
    };

    let author = match e.author.as_deref() {
        None | Some("human-local") => ParticipantId::HUMAN_LOCAL,
        Some(s) => match uuid::Uuid::parse_str(s) {
            Ok(u) => ParticipantId(u),
            Err(_) => ParticipantId::HUMAN_LOCAL, // unrecognized → fall back
        },
    };
    let mut ann = Annotation::new_authored(shape, color, thickness, author);
    ann.label = e.label.clone();
    // Preserve the original id so by-id references (sidecar v2 layer/group
    // membership, MCP/event references) survive a save→import round-trip.
    if let Ok(id) = e.id.parse::<uuid::Uuid>() {
        ann.id = id;
    }
    Some(ann)
}

fn encode_image_layer_png(image: &image::RgbaImage) -> Option<String> {
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut cursor, image::ImageFormat::Png)
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn decode_image_layer_png(encoded: &str) -> Option<image::RgbaImage> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    image::load_from_memory(&bytes)
        .ok()
        .map(|img| img.to_rgba8())
}

/// Load a sidecar JSON file and populate `canvas.annotations`. Clears existing
/// annotations and the undo/redo stacks. Returns Err with a human message on
/// failure.
pub fn import_sidecar(path: &std::path::Path, canvas: &mut CanvasState) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read sidecar: {e}"))?;
    let sc: SidecarExport =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse sidecar: {e}"))?;

    let mut anns: Vec<Annotation> = Vec::new();
    if let Some(fr) = &sc.focus_region {
        let rect = Rect::from_min_size(Pos2::new(fr.x, fr.y), Vec2::new(fr.w, fr.h));
        anns.push(Annotation::new(
            ShapeData::FocusRegion { rect },
            Color32::from_rgb(227, 179, 65),
            2.0,
        ));
    }
    for ae in &sc.annotations {
        if let Some(ann) = export_to_annotation(ae) {
            anns.push(ann);
        }
    }
    match &sc.layers {
        Some(layer_specs) if !layer_specs.is_empty() => {
            canvas.document = build_document_from_layers(layer_specs, anns, &sc.annotations);
        }
        _ => {
            canvas.document = crate::scene::Document::from_flat(anns); // v1 fallback
        }
    }
    canvas.active_layer = canvas.document.default_layer();
    canvas.sync_annotations();
    canvas.clear_all_undo_redo();

    #[cfg(feature = "local-ai")]
    {
        canvas.agent_memory = sc.agent_memory.clone();
        canvas.shared_notes = sc.shared_notes.clone();
        canvas.agent_bookmarks = sc.agent_bookmarks.clone();
    }
    Ok(())
}

/// Rebuild a `Document` from a v2 layer description and the imported annotations.
/// Annotations not referenced by any layer are appended to the first layer (so
/// nothing is lost). Ids in the spec that have no matching annotation are skipped.
fn build_document_from_layers(
    specs: &[crate::types::LayerExport],
    anns: Vec<Annotation>,
    ann_exports: &[crate::types::AnnotationExport],
) -> crate::scene::Document {
    use std::collections::HashMap;
    let by_id: HashMap<uuid::Uuid, Annotation> = anns.iter().map(|a| (a.id, a.clone())).collect();
    // Map annotation id -> leaf transform coeffs (only for non-identity transforms).
    let leaf_transform: HashMap<uuid::Uuid, [f64; 6]> = ann_exports
        .iter()
        .filter_map(|ae| {
            let t = ae.transform?;
            let id = ae.id.parse::<uuid::Uuid>().ok()?;
            Some((id, t))
        })
        .collect();
    let mut placed: std::collections::HashSet<uuid::Uuid> = Default::default();

    // Start from an empty document (root group only).
    let mut doc = crate::scene::Document::new();
    let mut first_layer = None;
    for spec in specs {
        let layer_id = doc.create_layer(&spec.name);
        if first_layer.is_none() {
            first_layer = Some(layer_id);
        }
        if let Some(n) = doc.get_mut(layer_id) {
            n.visible = spec.visible;
            n.locked = spec.locked;
            n.opacity = spec.opacity;
            if let Some(arr) = spec.transform {
                n.transform = kurbo::Affine::new(arr);
            }
        }
        // Group membership lookup: shape id -> group index.
        let mut group_of: HashMap<uuid::Uuid, usize> = HashMap::new();
        for (gi, g) in spec.groups.iter().enumerate() {
            for sid in &g.shape_ids {
                if let Ok(u) = sid.parse() {
                    group_of.insert(u, gi);
                }
            }
        }
        // Create group nodes up front (empty), in spec order, as layer children.
        let group_ids: Vec<uuid::Uuid> = spec
            .groups
            .iter()
            .map(|g| {
                let gid = doc.create_group(layer_id, &g.name, usize::MAX);
                if let Some(n) = doc.get_mut(gid) {
                    n.visible = g.visible;
                    n.locked = g.locked;
                    n.opacity = g.opacity;
                    if let Some(arr) = g.transform {
                        n.transform = kurbo::Affine::new(arr);
                    }
                }
                gid
            })
            .collect();
        // Place each shape: into its group if any, else directly in the layer.
        for sid in &spec.shape_ids {
            let Ok(uid) = sid.parse::<uuid::Uuid>() else {
                continue;
            };
            if placed.contains(&uid) {
                continue;
            }
            let Some(ann) = by_id.get(&uid) else {
                continue;
            };
            placed.insert(uid);
            let parent = match group_of.get(&uid) {
                Some(&gi) => group_ids[gi],
                None => layer_id,
            };
            doc.add_annotation(ann, parent);
            if let Some(&arr) = leaf_transform.get(&uid) {
                if let Some(n) = doc.get_mut(uid) {
                    n.transform = kurbo::Affine::new(arr);
                }
            }
        }
    }
    // If the spec produced no layers, ensure at least one.
    let fallback_layer = first_layer.unwrap_or_else(|| doc.create_layer("Layer 1"));
    // Append any annotations not referenced by the spec (e.g. focus region).
    for ann in &anns {
        if !placed.contains(&ann.id) {
            doc.add_annotation(ann, fallback_layer);
        }
    }
    doc
}

/// Write `bytes` to `path` as robustly as is straightforward: stream into a
/// sibling temp file, then atomically rename it into place. A failed temp write
/// or rename is reported (and the temp file cleaned up) rather than silently
/// dropping the output. Falls back to a direct write only if a temp path can't
/// be derived (e.g. a path with no parent component).
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = {
        let mut name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp-{}", std::process::id()));
        path.with_file_name(name)
    };
    match std::fs::write(&tmp, bytes) {
        Ok(()) => match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // Last-ditch direct write so we don't lose the artifact entirely.
            std::fs::write(path, bytes).or(Err(e))
        }
    }
}

/// Export the current canvas: writes the sidecar JSON, renders a flattened PNG
/// of the source image + all visible annotations next to it, and places that
/// PNG on the system clipboard.
///
/// Clipboard policy: arboard can only hold one payload at a time, so the
/// flattened **image** is the primary clipboard payload (the product's core
/// output). The textual annotation summary still lives in the sidecar JSON and
/// in `SidecarExport::clipboard_text`; if the image can't be produced or placed,
/// we fall back to putting that text on the clipboard instead.
pub fn export(state: &AppState) {
    let sidecar = build_sidecar(&state.canvas);
    let sidecar_json = serde_json::to_string_pretty(&sidecar).unwrap_or_default();

    let base_path = {
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
        let mut path = dirs_next::document_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        path.push(format!("screenshot-{}.redline", ts));
        path
    };
    let sidecar_path = base_path.with_extension("json");
    let png_path = base_path.with_extension("png");

    if let Err(e) = write_atomic(&sidecar_path, sidecar_json.as_bytes()) {
        eprintln!("RedLine export: failed to write sidecar {sidecar_path:?}: {e}");
    }

    // Render the flattened PNG (source image + annotations) via the shared
    // rasterizer. screen_rect drives screen→image-pixel thickness conversion;
    // fall back to a canvas-sized rect when no on-screen draw rect is known.
    let canvas = &state.canvas;
    let screen_rect = state
        .last_img_draw_rect
        .unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, canvas.canvas_size));
    let png = crate::flatten::flatten_to_png(
        &canvas.annotations,
        canvas.color_image.as_ref(),
        canvas.canvas_size,
        screen_rect,
    );

    let mut image_on_clipboard = false;
    let mut clipboard = arboard::Clipboard::new();
    match png {
        Ok((bytes, w, h)) => {
            if let Err(e) = write_atomic(&png_path, &bytes) {
                eprintln!("RedLine export: failed to write image {png_path:?}: {e}");
            }
            // Decode back to raw RGBA for arboard (it wants pixels, not PNG).
            match (&mut clipboard, image::load_from_memory(&bytes)) {
                (Ok(cb), Ok(img)) => {
                    let rgba = img.to_rgba8();
                    let data = arboard::ImageData {
                        width: w as usize,
                        height: h as usize,
                        bytes: std::borrow::Cow::from(rgba.into_raw()),
                    };
                    match cb.set_image(data) {
                        Ok(()) => image_on_clipboard = true,
                        Err(e) => eprintln!("RedLine export: clipboard set_image failed: {e}"),
                    }
                }
                (Err(e), _) => eprintln!("RedLine export: clipboard unavailable: {e}"),
                (_, Err(e)) => eprintln!("RedLine export: re-decode for clipboard failed: {e}"),
            }
        }
        Err(e) => eprintln!("RedLine export: could not flatten image: {e}"),
    }

    // If the image never made it onto the clipboard, keep the legacy behavior
    // of putting the textual summary there so the export still yields something.
    if !image_on_clipboard {
        if let Ok(ref mut cb) = clipboard {
            if let Err(e) = cb.set_text(sidecar.clipboard_text.clone()) {
                eprintln!("RedLine export: clipboard set_text failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod sidecar_v2_tests {
    use super::*;
    use crate::command::CommandExecutor;
    use crate::state::CanvasState;
    use crate::types::{Annotation, Command, LineStyle, NodeProp, ParticipantId, ShapeData};
    use egui::{Color32, Pos2, Rect, Vec2};

    #[test]
    fn sidecar_round_trips_layers_and_groups() {
        let mut c = CanvasState::default();
        c.canvas_size = Vec2::new(100.0, 100.0);
        let l1 = c.document.default_layer();
        let l2 = uuid::Uuid::new_v4();
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::CreateLayer {
                id: l2,
                name: "L2".into(),
            },
        );
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id: l1,
                prop: NodeProp::Opacity(0.5),
            },
        );
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        let aid = a.id;
        c.add_annotation(a); // into l1
        let b = Annotation::new(
            ShapeData::plain_oval(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::BLUE,
            2.0,
        );
        let bid = b.id;
        c.active_layer = l2;
        c.add_annotation(b); // into l2
        c.active_layer = l1;
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::Group {
                ids: vec![aid],
                name: "G".into(),
            },
        );

        let sc = build_sidecar(&c);
        assert!(sc.layers.is_some());
        let path = std::env::temp_dir().join(format!("redline-v2-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&sc).unwrap()).unwrap();

        let mut c2 = CanvasState::default();
        import_sidecar(&path, &mut c2).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(c2.document.layers().len(), 2);
        // l1 had opacity 0.5 (matched by name/order).
        let l1b = c2.document.layers()[0];
        assert!((c2.document.get(l1b).unwrap().opacity - 0.5).abs() < 1e-6);
        // Both shapes survive (by id).
        assert!(c2.document.leaf(aid).is_some());
        assert!(c2.document.leaf(bid).is_some());
        // aid is inside a group (its parent is not a layer).
        let pa = c2.document.get(aid).unwrap().parent.unwrap();
        assert!(matches!(
            c2.document.get(pa).unwrap().kind,
            crate::scene::NodeKind::Group
        ));
    }

    #[test]
    fn sidecar_roundtrips_node_transform() {
        use kurbo::Affine;
        let mut c = CanvasState::default();
        c.canvas_size = Vec2::new(100.0, 100.0);
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        let id = a.id;
        c.add_annotation(a);

        let t = Affine::rotate(0.6) * Affine::scale(1.5);
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id,
                prop: NodeProp::Transform(t),
            },
        );

        let sc = build_sidecar(&c);
        let path = std::env::temp_dir().join(format!(
            "redline-rt-transform-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, serde_json::to_vec(&sc).unwrap()).unwrap();

        let mut c2 = CanvasState::default();
        import_sidecar(&path, &mut c2).unwrap();
        let _ = std::fs::remove_file(&path);

        let rt = c2.document.get(id).unwrap().transform;
        let (a_coeffs, b_coeffs) = (t.as_coeffs(), rt.as_coeffs());
        for i in 0..6 {
            assert!(
                (a_coeffs[i] - b_coeffs[i]).abs() < 1e-9,
                "transform coeff {i} mismatch: expected {}, got {}",
                a_coeffs[i],
                b_coeffs[i]
            );
        }
    }

    #[test]
    fn old_sidecar_without_transform_loads_as_identity() {
        use kurbo::Affine;
        // A minimal v2 sidecar JSON with no "transform" keys at all — simulates
        // an old sidecar written before this field existed.
        let leaf_id = uuid::Uuid::new_v4();
        let id_str = leaf_id.to_string();
        // Use a regular string with explicit escape sequences to avoid the
        // r#"..."# raw-string delimiter collision with `"#` in "#ff0000".
        let json = format!(
            concat!(
                "{{",
                "\"canvas_size\":[100.0,100.0],",
                "\"captured_at\":\"2025-01-01T00:00:00Z\",",
                "\"focus_region\":null,",
                "\"annotations\":[{{",
                "\"id\":\"{id}\",",
                "\"kind\":\"rect\",\"coords\":null,",
                "\"color\":\"#ff0000\",",
                "\"label\":null,\"tag\":null,",
                "\"rect\":[0.0,0.0,10.0,10.0]",
                "}}],",
                "\"clipboard_text\":\"\",",
                "\"layers\":[{{",
                "\"name\":\"Layer 1\",\"visible\":true,\"locked\":false,\"opacity\":1.0,",
                "\"shape_ids\":[\"{id}\"],",
                "\"groups\":[]",
                "}}]",
                "}}"
            ),
            id = id_str
        );
        let path = std::env::temp_dir().join(format!(
            "redline-no-transform-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, json.as_bytes()).unwrap();

        let mut c = CanvasState::default();
        import_sidecar(&path, &mut c).unwrap();
        let _ = std::fs::remove_file(&path);

        let node_transform = c.document.get(leaf_id).unwrap().transform;
        assert_eq!(node_transform, Affine::IDENTITY);
    }

    #[test]
    fn sidecar_roundtrips_group_and_layer_transform() {
        use kurbo::Affine;
        let mut c = CanvasState::default();
        c.canvas_size = Vec2::new(100.0, 100.0);

        // Add a leaf shape into the default layer.
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        let aid = a.id;
        c.add_annotation(a);

        // Group the leaf.
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::Group {
                ids: vec![aid],
                name: "G".into(),
            },
        );

        // Find the group id by walking up from the leaf.
        let group_id = c.document.get(aid).unwrap().parent.unwrap();
        assert!(matches!(
            c.document.get(group_id).unwrap().kind,
            crate::scene::NodeKind::Group
        ));

        // Set a non-identity transform on the GROUP node.
        let t_group = Affine::rotate(0.3) * Affine::scale(2.0);
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id: group_id,
                prop: NodeProp::Transform(t_group),
            },
        );

        // Set a non-identity transform on the LAYER node.
        let layer_id = c.document.default_layer();
        let t_layer = Affine::translate((5.0, 7.0)) * Affine::rotate(-0.1);
        CommandExecutor::execute(
            &mut c,
            ParticipantId::HUMAN_LOCAL,
            Command::SetNodeProp {
                id: layer_id,
                prop: NodeProp::Transform(t_layer),
            },
        );

        // Round-trip: serialize → file → import.
        let sc = build_sidecar(&c);
        let path = std::env::temp_dir().join(format!(
            "redline-rt-group-layer-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, serde_json::to_vec(&sc).unwrap()).unwrap();

        let mut c2 = CanvasState::default();
        import_sidecar(&path, &mut c2).unwrap();
        let _ = std::fs::remove_file(&path);

        // Locate the group by walking from the leaf (group id is not preserved).
        assert!(
            c2.document.leaf(aid).is_some(),
            "leaf shape missing after import"
        );
        let imported_group_id = c2.document.get(aid).unwrap().parent.unwrap();
        assert!(
            matches!(
                c2.document.get(imported_group_id).unwrap().kind,
                crate::scene::NodeKind::Group
            ),
            "leaf's parent should be a Group after import"
        );

        // Assert group transform survived.
        let rt_group = c2.document.get(imported_group_id).unwrap().transform;
        let (eg, rg) = (t_group.as_coeffs(), rt_group.as_coeffs());
        for i in 0..6 {
            assert!(
                (eg[i] - rg[i]).abs() < 1e-9,
                "group transform coeff {i} mismatch: expected {}, got {}",
                eg[i],
                rg[i]
            );
        }

        // Locate the layer by position (layer id is not preserved across import).
        let imported_layer_id = c2.document.layers()[0];

        // Assert layer transform survived.
        let rt_layer = c2.document.get(imported_layer_id).unwrap().transform;
        let (el, rl) = (t_layer.as_coeffs(), rt_layer.as_coeffs());
        for i in 0..6 {
            assert!(
                (el[i] - rl[i]).abs() < 1e-9,
                "layer transform coeff {i} mismatch: expected {}, got {}",
                el[i],
                rl[i]
            );
        }
    }

    #[test]
    fn v1_sidecar_without_layers_imports_as_single_layer() {
        let mut c = CanvasState::default();
        c.canvas_size = Vec2::new(100.0, 100.0);
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        c.add_annotation(a);
        let mut sc = build_sidecar(&c);
        sc.layers = None; // simulate v1
        let path = std::env::temp_dir().join(format!("redline-v1-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&sc).unwrap()).unwrap();
        let mut c2 = CanvasState::default();
        import_sidecar(&path, &mut c2).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(c2.document.layers().len(), 1);
        assert_eq!(c2.annotations.len(), 1);
    }

    #[test]
    fn legacy_highlighter_kind_migrates_to_unified_rect() {
        let e = AnnotationExport {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "highlighter".into(),
            rect: Some([0.0, 0.0, 10.0, 10.0]),
            color: "#ff0000".into(),
            ..Default::default()
        };
        let ann = export_to_annotation(&e).expect("highlighter imports");
        match ann.shape {
            ShapeData::Rect {
                fill: Some(f),
                stroke,
                radius,
                ..
            } => {
                assert_eq!(f.a(), 120);
                assert!(!stroke);
                assert_eq!(radius, 0.0);
            }
            other => panic!("expected unified Rect, got {other:?}"),
        }
    }

    #[test]
    fn legacy_rounded_rect_kind_migrates_to_unified_rect() {
        let e = AnnotationExport {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "rounded_rect".into(),
            rect: Some([0.0, 0.0, 10.0, 10.0]),
            radius: Some(15.0),
            color: "#00ff00".into(),
            ..Default::default()
        };
        let ann = export_to_annotation(&e).expect("rounded_rect imports");
        match ann.shape {
            ShapeData::Rect {
                fill: Some(f),
                stroke,
                radius,
                ..
            } => {
                assert_eq!(f.a(), 20);
                assert!(stroke);
                assert_eq!(radius, 15.0);
            }
            other => panic!("expected unified Rect, got {other:?}"),
        }
    }

    #[test]
    fn legacy_redact_kind_migrates_to_unified_rect_opaque_fill() {
        let e = AnnotationExport {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "redact".into(),
            rect: Some([0.0, 0.0, 10.0, 10.0]),
            color: "#000000".into(),
            ..Default::default()
        };
        let ann = export_to_annotation(&e).expect("redact imports");
        match ann.shape {
            ShapeData::Rect {
                fill: Some(f),
                stroke,
                ..
            } => {
                assert_eq!(f.a(), 255);
                assert!(!stroke);
            }
            other => panic!("expected unified Rect, got {other:?}"),
        }
    }

    #[test]
    fn legacy_redact_oval_kind_migrates_to_unified_oval_opaque_fill() {
        let e = AnnotationExport {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "redact_oval".into(),
            rect: Some([0.0, 0.0, 10.0, 10.0]),
            color: "#000000".into(),
            ..Default::default()
        };
        let ann = export_to_annotation(&e).expect("redact_oval imports");
        match ann.shape {
            ShapeData::Oval {
                fill: Some(f),
                stroke,
                ..
            } => {
                assert_eq!(f.a(), 255);
                assert!(!stroke);
            }
            other => panic!("expected unified Oval, got {other:?}"),
        }
    }

    #[test]
    fn new_rect_export_round_trips_fill_and_stroke() {
        // `Color32::from_rgba_unmultiplied` immediately bakes its inputs into
        // premultiplied storage (gamma-correct, lossy for alpha < 255), so
        // `.r()/.g()/.b()` on the constructed color never equal the literal
        // (10, 20, 30) inputs even before any export round-trip. Compare
        // against the color's own straight-sRGB view (`to_srgba_unmultiplied`)
        // instead of the construction literals.
        let fill_color = Color32::from_rgba_unmultiplied(10, 20, 30, 77);
        let [exp_r, exp_g, exp_b, exp_a] = fill_color.to_srgba_unmultiplied();
        let ann = Annotation::new(
            ShapeData::Rect {
                rect: Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0)),
                radius: 6.0,
                fill: Some(fill_color),
                stroke: true,
                stroke_style: LineStyle::Dashed,
            },
            Color32::WHITE,
            2.0,
        );
        let exported = annotation_to_export(&ann, Vec2::new(100.0, 100.0));
        let reimported = export_to_annotation(&exported).expect("round-trips");
        match reimported.shape {
            ShapeData::Rect {
                radius,
                fill: Some(f),
                stroke,
                stroke_style,
                ..
            } => {
                assert_eq!(radius, 6.0);
                let [got_r, got_g, got_b, got_a] = f.to_srgba_unmultiplied();
                assert_eq!(got_r, exp_r);
                assert_eq!(got_g, exp_g);
                assert_eq!(got_b, exp_b);
                assert_eq!(got_a, exp_a);
                assert!(stroke);
                assert_eq!(stroke_style, LineStyle::Dashed);
            }
            other => panic!("expected unified Rect, got {other:?}"),
        }
    }
}
