use crate::scene::document::Document;
use crate::scene::node::{Node, NodeId, NodeKind, ShapeLeaf};
use crate::types::{Annotation, ShapeData};
use kurbo::Affine;
use uuid::Uuid;

/// Human-readable default name for a leaf, derived from its shape kind.
fn shape_kind_name(shape: &ShapeData) -> &'static str {
    match shape {
        ShapeData::Rect { .. } => "Rectangle",
        ShapeData::Oval { .. } => "Oval",
        ShapeData::Star { .. } => "Star",
        ShapeData::Arrow { .. } => "Arrow",
        ShapeData::Line { .. } => "Line",
        ShapeData::Pen { .. } => "Pen",
        ShapeData::BezierPath { .. } => "Path",
        ShapeData::FocusRegion { .. } => "Focus region",
        ShapeData::LogicTag { .. } => "Logic tag",
        ShapeData::Text { .. } => "Text",
        ShapeData::Question { .. } => "Question",
        ShapeData::Blur { .. } => "Blur",
        ShapeData::StepMarker { .. } => "Step",
        ShapeData::ImageLayer { .. } => "Image",
        ShapeData::Callout { .. } => "Callout",
        ShapeData::Ruler { .. } => "Ruler",
    }
}

/// Build a leaf `Node` from an `Annotation`, preserving the annotation's id and
/// moving `visible`/`locked` onto the node.
fn leaf_node_from_annotation(ann: &Annotation) -> Node {
    Node {
        id: ann.id,
        parent: None,
        children: Vec::new(),
        name: shape_kind_name(&ann.shape).to_string(),
        visible: ann.visible,
        locked: ann.locked,
        opacity: 1.0,
        transform: Affine::IDENTITY,
        kind: NodeKind::Shape(ShapeLeaf {
            author: ann.author,
            shape: ann.shape.clone(),
            color: ann.color,
            thickness: ann.thickness,
            label: ann.label.clone(),
            provisional_until: ann.provisional_until,
        }),
    }
}

impl Document {
    /// Migrate today's flat annotation list into a tree: `root → "Layer 1" →
    /// leaves in order`. Used when loading v1 sessions/sidecars and when wrapping
    /// the existing canvas in Phase 1b.
    pub fn from_flat(anns: Vec<Annotation>) -> Self {
        let mut doc = Document::new();
        let root = doc.root;
        let layer = Node {
            id: Uuid::new_v4(),
            parent: None,
            children: Vec::new(),
            name: "Layer 1".into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            transform: Affine::IDENTITY,
            kind: NodeKind::Layer,
        };
        let layer_id = doc.insert_child(root, layer, 0);
        for (i, ann) in anns.iter().enumerate() {
            doc.insert_child(layer_id, leaf_node_from_annotation(ann), i);
        }
        doc
    }

    /// Insert `ann` as a new leaf child of `parent` (appended last). Returns the id.
    pub fn add_annotation(&mut self, ann: &Annotation, parent: NodeId) -> NodeId {
        let node = leaf_node_from_annotation(ann);
        self.insert_child(parent, node, usize::MAX)
    }

    /// Reconstruct a single `Annotation` from leaf `id` (node-level visible/locked).
    /// `None` if `id` is missing or is not a Shape leaf.
    pub fn leaf_to_annotation(&self, id: NodeId) -> Option<Annotation> {
        let node = self.get(id)?;
        let leaf = self.leaf(id)?;
        Some(Annotation {
            id,
            author: leaf.author,
            shape: leaf.shape.clone(),
            color: leaf.color,
            thickness: leaf.thickness,
            label: leaf.label.clone(),
            locked: node.locked,
            visible: node.visible,
            provisional_until: leaf.provisional_until,
        })
    }

    /// Reconstruct a flat annotation list in paint order. The inverse of
    /// `from_flat` for id/visible/locked/payload. Uses each leaf's *own* node
    /// flags (not the effective ones) so a single-layer document round-trips.
    /// For a multi-layer document this flattens all layers into one list in
    /// paint order and intentionally does NOT fold in layer-level
    /// visible/locked/opacity — it is a lossy v1 compatibility bridge.
    pub fn to_flat(&self) -> Vec<Annotation> {
        self.leaves_in_paint_order()
            .into_iter()
            .map(|lv| Annotation {
                id: lv.id,
                author: lv.leaf.author,
                shape: lv.leaf.shape.clone(),
                color: lv.leaf.color,
                thickness: lv.leaf.thickness,
                label: lv.leaf.label.clone(),
                locked: lv.own_locked,
                visible: lv.own_visible,
                provisional_until: lv.leaf.provisional_until,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::scene::document::Document;
    use crate::scene::node::{Node, NodeKind, ShapeLeaf};
    use crate::types::{Annotation, ShapeData};
    use egui::{Color32, Rect};
    use kurbo::Affine;

    fn ann(name_rect: Rect, visible: bool, locked: bool) -> Annotation {
        let mut a = Annotation::new(ShapeData::plain_rect(name_rect), Color32::RED, 2.0);
        a.visible = visible;
        a.locked = locked;
        a
    }

    #[test]
    fn add_annotation_and_read_back_as_annotation() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let a = crate::types::Annotation::new(
            crate::types::ShapeData::plain_rect(egui::Rect::ZERO),
            egui::Color32::RED,
            3.0,
        );
        let id = a.id;
        doc.add_annotation(&a, layer);
        let back = doc.leaf_to_annotation(id).expect("present");
        assert_eq!(back.id, id);
        assert_eq!(back.thickness, 3.0);
        assert_eq!(back.color, egui::Color32::RED);
    }

    #[test]
    fn from_flat_builds_root_layer_and_leaves() {
        let anns = vec![
            ann(
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                true,
                false,
            ),
            ann(
                Rect::from_min_max(egui::pos2(2.0, 0.0), egui::pos2(3.0, 1.0)),
                false,
                true,
            ),
        ];
        let ids: Vec<_> = anns.iter().map(|a| a.id).collect();
        let doc = Document::from_flat(anns);
        assert!(doc.check_invariants().is_ok());
        // Root has exactly one Layer child; that layer holds both leaves in order.
        let order: Vec<_> = doc.leaves_in_paint_order().iter().map(|lv| lv.id).collect();
        assert_eq!(order, ids);
    }

    #[test]
    fn flat_round_trips_through_the_tree() {
        let anns = vec![
            ann(
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                true,
                false,
            ),
            ann(
                Rect::from_min_max(egui::pos2(2.0, 0.0), egui::pos2(3.0, 1.0)),
                false,
                true,
            ),
        ];
        let snapshot: Vec<_> = anns
            .iter()
            .map(|a| (a.id, a.visible, a.locked, a.color))
            .collect();
        let doc = Document::from_flat(anns);
        let back = doc.to_flat();
        let round: Vec<_> = back
            .iter()
            .map(|a| (a.id, a.visible, a.locked, a.color))
            .collect();
        assert_eq!(
            round, snapshot,
            "id/visible/locked/color must survive the round-trip in order"
        );
    }

    #[test]
    fn to_flat_flattens_multiple_layers_in_paint_order() {
        let mut doc = Document::new();

        // Helper closure to build a leaf Node inline.
        let make_leaf = |name: &str| -> Node {
            Node {
                id: uuid::Uuid::new_v4(),
                parent: None,
                children: Vec::new(),
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                transform: Affine::IDENTITY,
                kind: NodeKind::Shape(ShapeLeaf {
                    author: crate::types::ParticipantId::HUMAN_LOCAL,
                    shape: ShapeData::plain_rect(Rect::ZERO),
                    color: Color32::RED,
                    thickness: 2.0,
                    label: None,
                    provisional_until: None,
                }),
            }
        };
        let make_layer = |name: &str| -> Node {
            Node {
                id: uuid::Uuid::new_v4(),
                parent: None,
                children: Vec::new(),
                name: name.into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                transform: Affine::IDENTITY,
                kind: NodeKind::Layer,
            }
        };

        let layer1 = make_layer("Layer 1");
        let layer2 = make_layer("Layer 2");
        let layer1_id = doc.insert_child(doc.root, layer1, 0);
        let layer2_id = doc.insert_child(doc.root, layer2, 1);

        let leaf1 = make_leaf("leaf1");
        let leaf2 = make_leaf("leaf2");
        let leaf1_id = leaf1.id;
        let leaf2_id = leaf2.id;
        doc.insert_child(layer1_id, leaf1, 0);
        doc.insert_child(layer2_id, leaf2, 0);

        let flat = doc.to_flat();
        let ids: Vec<_> = flat.iter().map(|a| a.id).collect();
        assert_eq!(
            ids,
            vec![leaf1_id, leaf2_id],
            "leaves should appear in layer order"
        );
    }
}
