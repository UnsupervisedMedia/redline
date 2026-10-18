//! Generic structure analysis for the canvas. Converts low-level shape layouts
//! into reusable features and neutral hypotheses for the local AI.

use crate::agent::types::AnnotationDescriptor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisStrength {
    Weak,
    Strong,
}

impl AnalysisStrength {
    fn label(self) -> &'static str {
        match self {
            Self::Weak => "weak",
            Self::Strong => "strong",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisFeature {
    pub name: String,
    pub strength: AnalysisStrength,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AnalysisHypothesis {
    pub summary: String,
    pub strength: AnalysisStrength,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CanvasAnalysis {
    pub features: Vec<AnalysisFeature>,
    pub hypotheses: Vec<AnalysisHypothesis>,
}

impl CanvasAnalysis {
    pub fn render_for_prompt(&self) -> String {
        let mut lines = Vec::new();
        if !self.features.is_empty() {
            lines.push("Structured analysis:".to_string());
            for feature in self.features.iter().take(4) {
                let evidence = if feature.evidence.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", feature.evidence.join("; "))
                };
                lines.push(format!(
                    "- feature ({}) {}{}",
                    feature.strength.label(),
                    feature.name,
                    evidence
                ));
            }
        }
        if !self.hypotheses.is_empty() {
            for hypothesis in self.hypotheses.iter().take(4) {
                let evidence = if hypothesis.evidence.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", hypothesis.evidence.join("; "))
                };
                lines.push(format!(
                    "- hypothesis ({}) {}{}",
                    hypothesis.strength.label(),
                    hypothesis.summary,
                    evidence
                ));
            }
        }
        lines.join("\n")
    }

    pub fn has_strong_hypothesis(&self) -> bool {
        self.hypotheses
            .iter()
            .any(|hypothesis| hypothesis.strength == AnalysisStrength::Strong)
    }
}

pub fn analyze(shapes: &[AnnotationDescriptor]) -> CanvasAnalysis {
    let visible_shapes: Vec<&AnnotationDescriptor> =
        shapes.iter().filter(|shape| shape.visible).collect();
    let mut analysis = CanvasAnalysis::default();

    if let Some(grid) = detect_regular_grid(&visible_shapes) {
        let occupied = count_non_grid_shapes_inside(&visible_shapes, &grid);
        let total_cells = grid.rows * grid.cols;
        let empty_cells = total_cells.saturating_sub(occupied);
        analysis.features.push(AnalysisFeature {
            name: format!(
                "regular_grid rows={} cols={} occupied_cells={} empty_cells={}",
                grid.rows, grid.cols, occupied, empty_cells
            ),
            strength: AnalysisStrength::Strong,
            evidence: vec![
                format!("vertical_dividers={}", grid.verticals),
                format!("horizontal_dividers={}", grid.horizontals),
            ],
        });
        analysis.hypotheses.push(AnalysisHypothesis {
            summary: format!(
                "A structured multi-cell board or table is present with {} empty cells",
                empty_cells
            ),
            strength: AnalysisStrength::Strong,
            evidence: vec![format!("grid={}x{}", grid.rows, grid.cols)],
        });
    }

    if let Some(slot) = detect_evenly_spaced_slots(&visible_shapes) {
        analysis.features.push(AnalysisFeature {
            name: format!(
                "evenly_spaced_slots count={} axis={}",
                slot.count, slot.axis
            ),
            strength: AnalysisStrength::Strong,
            evidence: vec![
                format!("mean_span_px={:.0}", slot.mean_span),
                format!("gap_variance_px={:.1}", slot.gap_variance),
            ],
        });
        analysis.hypotheses.push(AnalysisHypothesis {
            summary: format!(
                "A fill-in sequence or slot-based task is present with {} open positions",
                slot.count
            ),
            strength: AnalysisStrength::Strong,
            evidence: vec![format!("axis={}", slot.axis)],
        });
    }

    if let Some(diagram) = detect_connector_diagram(&visible_shapes) {
        analysis.features.push(AnalysisFeature {
            name: format!(
                "connected_diagram text_like={} connectors={}",
                diagram.text_like, diagram.connectors
            ),
            strength: AnalysisStrength::Weak,
            evidence: vec![format!("boxed_regions={}", diagram.boxes)],
        });
        analysis.hypotheses.push(AnalysisHypothesis {
            summary: "A labeled diagram or flow-like structure is forming".into(),
            strength: AnalysisStrength::Weak,
            evidence: vec![
                format!("text_like={}", diagram.text_like),
                format!("connectors={}", diagram.connectors),
            ],
        });
    }

    if analysis.hypotheses.is_empty() {
        if let Some(sketch) = detect_freeform_cluster(&visible_shapes) {
            analysis.features.push(AnalysisFeature {
                name: format!(
                    "grouped_strokes count={} regions={}",
                    sketch.strokes, sketch.regions
                ),
                strength: AnalysisStrength::Weak,
                evidence: vec![format!("bbox_density={:.2}", sketch.density)],
            });
            analysis.hypotheses.push(AnalysisHypothesis {
                summary: "A freeform sketch or emerging structure is in progress".into(),
                strength: AnalysisStrength::Weak,
                evidence: vec![format!("stroke_count={}", sketch.strokes)],
            });
        }
    }

    analysis
}

struct GridInfo {
    rows: usize,
    cols: usize,
    verticals: usize,
    horizontals: usize,
    board_rect: egui::Rect,
}

fn detect_regular_grid(shapes: &[&AnnotationDescriptor]) -> Option<GridInfo> {
    let mut verticals = Vec::new();
    let mut horizontals = Vec::new();
    for shape in shapes {
        if !matches!(shape.kind.as_str(), "line" | "pen") {
            continue;
        }
        let rect = shape.bounding_rect;
        let w = rect.width().abs().max(1.0);
        let h = rect.height().abs().max(1.0);
        if h > w * 2.0 {
            verticals.push(*shape);
        } else if w > h * 2.0 {
            horizontals.push(*shape);
        }
    }
    if verticals.len() < 2 || horizontals.len() < 2 {
        return None;
    }

    verticals.sort_by(|a, b| {
        a.bounding_rect
            .center()
            .x
            .total_cmp(&b.bounding_rect.center().x)
    });
    horizontals.sort_by(|a, b| {
        a.bounding_rect
            .center()
            .y
            .total_cmp(&b.bounding_rect.center().y)
    });
    let x_min = verticals.first()?.bounding_rect.center().x;
    let x_max = verticals.last()?.bounding_rect.center().x;
    let y_min = horizontals.first()?.bounding_rect.center().y;
    let y_max = horizontals.last()?.bounding_rect.center().y;
    if x_max - x_min < 40.0 || y_max - y_min < 40.0 {
        return None;
    }

    Some(GridInfo {
        rows: horizontals.len() + 1,
        cols: verticals.len() + 1,
        verticals: verticals.len(),
        horizontals: horizontals.len(),
        board_rect: egui::Rect::from_min_max(egui::pos2(x_min, y_min), egui::pos2(x_max, y_max)),
    })
}

fn count_non_grid_shapes_inside(shapes: &[&AnnotationDescriptor], grid: &GridInfo) -> usize {
    shapes
        .iter()
        .filter(|shape| {
            !matches!(shape.kind.as_str(), "line" | "pen")
                && grid.board_rect.contains(shape.bounding_rect.center())
                && !matches!(shape.kind.as_str(), "question" | "highlighter" | "callout")
        })
        .count()
}

struct SlotInfo {
    count: usize,
    axis: &'static str,
    mean_span: f32,
    gap_variance: f32,
}

fn detect_evenly_spaced_slots(shapes: &[&AnnotationDescriptor]) -> Option<SlotInfo> {
    let mut candidates: Vec<&AnnotationDescriptor> = shapes
        .iter()
        .copied()
        .filter(|shape| matches!(shape.kind.as_str(), "line" | "rect" | "rounded_rect"))
        .filter(|shape| {
            let rect = shape.bounding_rect;
            let w = rect.width().abs();
            let h = rect.height().abs().max(1.0);
            w > 18.0 && w > h * 3.0
        })
        .collect();
    if candidates.len() < 3 {
        return None;
    }

    candidates.sort_by(|a, b| {
        a.bounding_rect
            .center()
            .x
            .total_cmp(&b.bounding_rect.center().x)
    });
    let y_centers: Vec<f32> = candidates
        .iter()
        .map(|shape| shape.bounding_rect.center().y)
        .collect();
    let min_y = y_centers.iter().copied().fold(f32::INFINITY, f32::min);
    let max_y = y_centers.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if max_y - min_y > 30.0 {
        return None;
    }

    let widths: Vec<f32> = candidates
        .iter()
        .map(|shape| shape.bounding_rect.width().abs())
        .collect();
    let mean_span = widths.iter().sum::<f32>() / widths.len() as f32;
    let max_dev = widths
        .iter()
        .map(|width| (width - mean_span).abs())
        .fold(0.0, f32::max);
    if max_dev > mean_span * 0.35 {
        return None;
    }

    let mut gaps = Vec::new();
    for pair in candidates.windows(2) {
        let a = pair[0].bounding_rect.max.x;
        let b = pair[1].bounding_rect.min.x;
        gaps.push((b - a).max(0.0));
    }
    let mean_gap = gaps.iter().sum::<f32>() / gaps.len() as f32;
    let gap_variance =
        gaps.iter().map(|gap| (gap - mean_gap).abs()).sum::<f32>() / gaps.len() as f32;
    if gap_variance > mean_span * 0.5 {
        return None;
    }

    Some(SlotInfo {
        count: candidates.len(),
        axis: "horizontal",
        mean_span,
        gap_variance,
    })
}

struct DiagramInfo {
    text_like: usize,
    connectors: usize,
    boxes: usize,
}

fn detect_connector_diagram(shapes: &[&AnnotationDescriptor]) -> Option<DiagramInfo> {
    let text_like = shapes
        .iter()
        .filter(|shape| matches!(shape.kind.as_str(), "text" | "question"))
        .count();
    let connectors = shapes
        .iter()
        .filter(|shape| matches!(shape.kind.as_str(), "arrow" | "line"))
        .count();
    let boxes = shapes
        .iter()
        .filter(|shape| matches!(shape.kind.as_str(), "rect" | "rounded_rect" | "oval"))
        .count();
    if connectors >= 2 && (text_like >= 1 || boxes >= 2) {
        Some(DiagramInfo {
            text_like,
            connectors,
            boxes,
        })
    } else {
        None
    }
}

struct SketchInfo {
    strokes: usize,
    regions: usize,
    density: f32,
}

fn detect_freeform_cluster(shapes: &[&AnnotationDescriptor]) -> Option<SketchInfo> {
    let strokes: Vec<&AnnotationDescriptor> = shapes
        .iter()
        .copied()
        .filter(|shape| matches!(shape.kind.as_str(), "line" | "pen" | "arrow"))
        .collect();
    if strokes.len() < 3 {
        return None;
    }
    let union = strokes
        .iter()
        .fold(None, |acc: Option<egui::Rect>, shape| {
            Some(match acc {
                Some(rect) => rect.union(shape.bounding_rect),
                None => shape.bounding_rect,
            })
        })?;
    let area = union.width().abs().max(1.0) * union.height().abs().max(1.0);
    let density = strokes.len() as f32 / area.max(1.0) * 10_000.0;
    Some(SketchInfo {
        strokes: strokes.len(),
        regions: 1,
        density,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ParticipantId;
    use uuid::Uuid;

    fn descriptor(kind: &str, min: [f32; 2], max: [f32; 2]) -> AnnotationDescriptor {
        AnnotationDescriptor {
            id: Uuid::new_v4(),
            kind: kind.into(),
            bounding_rect: egui::Rect::from_min_max(
                egui::pos2(min[0], min[1]),
                egui::pos2(max[0], max[1]),
            ),
            color: egui::Color32::BLACK,
            label: None,
            locked: false,
            visible: true,
            author: ParticipantId::new(),
            layer: None,
            layer_name: None,
        }
    }

    #[test]
    fn detects_grid_as_neutral_board_hypothesis() {
        let analysis = analyze(&[
            descriptor("line", [100.0, 80.0], [110.0, 320.0]),
            descriptor("line", [220.0, 80.0], [230.0, 320.0]),
            descriptor("line", [40.0, 140.0], [300.0, 150.0]),
            descriptor("line", [40.0, 250.0], [300.0, 260.0]),
        ]);
        assert!(analysis
            .features
            .iter()
            .any(|feature| feature.name.contains("regular_grid")));
        assert!(analysis
            .hypotheses
            .iter()
            .any(|hypothesis| hypothesis.summary.contains("multi-cell board")));
    }

    #[test]
    fn detects_evenly_spaced_slots() {
        let analysis = analyze(&[
            descriptor("line", [50.0, 100.0], [90.0, 104.0]),
            descriptor("line", [110.0, 100.0], [150.0, 104.0]),
            descriptor("line", [170.0, 100.0], [210.0, 104.0]),
            descriptor("line", [230.0, 100.0], [270.0, 104.0]),
        ]);
        assert!(analysis
            .features
            .iter()
            .any(|feature| feature.name.contains("evenly_spaced_slots")));
        assert!(analysis
            .hypotheses
            .iter()
            .any(|hypothesis| hypothesis.summary.contains("slot-based")));
    }

    #[test]
    fn rejects_single_line_as_strong_hypothesis() {
        let analysis = analyze(&[descriptor("line", [40.0, 140.0], [300.0, 150.0])]);
        assert!(!analysis.has_strong_hypothesis());
    }
}
