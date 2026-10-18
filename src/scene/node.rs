use crate::types::{ParticipantId, ShapeData};
use egui::Color32;
use kurbo::Affine;
use std::time::SystemTime;
use uuid::Uuid;

/// Stable identifier for a node. Leaf nodes reuse the `Uuid` of the
/// `Annotation` they were created from, so existing by-id references keep working.
pub type NodeId = Uuid;

/// The drawable payload of a leaf node — today's `Annotation` minus the fields
/// that now live on the wrapping [`Node`] (`id`, `visible`, `locked`).
#[derive(Clone, Debug)]
pub struct ShapeLeaf {
    pub author: ParticipantId,
    pub shape: ShapeData,
    pub color: Color32,
    pub thickness: f32,
    pub label: Option<String>,
    pub provisional_until: Option<SystemTime>,
}

/// What a node *is*: a top-level layer, an ordinary group, or a drawable leaf.
#[derive(Clone, Debug)]
pub enum NodeKind {
    /// Top-level group shown as a row in the Layers panel. Only valid as a
    /// direct child of the root.
    Layer,
    /// Ordinary, nestable group.
    Group,
    /// Leaf — a single drawable shape.
    Shape(ShapeLeaf),
}

/// One node in the scene graph arena.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    /// `None` only for the document root.
    pub parent: Option<NodeId>,
    /// Child ids in z-order: later children paint on top.
    pub children: Vec<NodeId>,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Local opacity in `0.0..=1.0`.
    pub opacity: f32,
    /// Local affine transform, relative to the parent.
    pub transform: Affine,
    pub kind: NodeKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ShapeData;
    use egui::{Color32, Rect};
    use kurbo::Affine;

    fn leaf_node() -> Node {
        Node {
            id: uuid::Uuid::new_v4(),
            parent: None,
            children: Vec::new(),
            name: "Rectangle".into(),
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
    }

    #[test]
    fn node_kind_discriminates_leaf_from_group() {
        let n = leaf_node();
        assert!(matches!(n.kind, NodeKind::Shape(_)));
        let g = Node {
            kind: NodeKind::Group,
            ..leaf_node()
        };
        assert!(matches!(g.kind, NodeKind::Group));
    }

    #[test]
    fn shape_leaf_carries_payload() {
        if let NodeKind::Shape(leaf) = leaf_node().kind {
            assert_eq!(leaf.thickness, 2.0);
            assert_eq!(leaf.color, Color32::RED);
        } else {
            panic!("expected Shape");
        }
    }
}
