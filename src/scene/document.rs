use crate::scene::node::{Node, NodeId, NodeKind, ShapeLeaf};
use kurbo::Affine;
use std::collections::HashMap;
use uuid::Uuid;

/// Hard cap on how deep any tree/parent-chain walk will descend. Real
/// documents are nowhere near this deep; the cap exists purely as
/// belt-and-suspenders so that if a cycle ever slipped past `can_reparent`
/// (the primary guard), the recursive/iterative walks degrade gracefully
/// instead of overflowing the stack or looping forever.
const MAX_TREE_DEPTH: usize = 10_000;

/// A leaf shape resolved for painting/hit-testing: its world transform and
/// effective flags already composed from its ancestors. Borrows the document.
pub struct LeafView<'a> {
    pub id: NodeId,
    pub leaf: &'a ShapeLeaf,
    pub world: Affine,
    pub effective_opacity: f32,
    pub effective_visible: bool,
    pub effective_locked: bool,
    /// The leaf node's OWN `visible` flag — not composed with ancestors.
    /// Distinct from `effective_visible`, which ANDs all ancestor flags in.
    pub own_visible: bool,
    /// The leaf node's OWN `locked` flag — not composed with ancestors.
    /// Distinct from `effective_locked`, which ORs all ancestor flags in.
    pub own_locked: bool,
    /// Paint index, back-to-front (0 = drawn first / behind).
    pub z: usize,
}

/// The scene-graph document: an arena of nodes with a single root `Group`.
#[derive(Clone, Debug)]
pub struct Document {
    pub root: NodeId,
    nodes: HashMap<NodeId, Node>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// A new document containing only an empty root `Group`.
    pub fn new() -> Self {
        let root = Uuid::new_v4();
        let mut nodes = HashMap::new();
        nodes.insert(
            root,
            Node {
                id: root,
                parent: None,
                children: Vec::new(),
                name: "Document".into(),
                visible: true,
                locked: false,
                opacity: 1.0,
                transform: Affine::IDENTITY,
                kind: NodeKind::Group,
            },
        );
        Self { root, nodes }
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// The default content layer: the first `Layer` child of root. Falls back to
    /// the root id if no layer exists (shouldn't happen for documents built via
    /// `from_flat`).
    pub fn default_layer(&self) -> NodeId {
        let root = self.nodes.get(&self.root);
        root.and_then(|r| {
            r.children
                .iter()
                .copied()
                .find(|c| matches!(self.nodes.get(c).map(|n| &n.kind), Some(NodeKind::Layer)))
        })
        .unwrap_or(self.root)
    }

    /// Index of `id` within its parent's `children`, or `None` if it has no parent.
    pub fn child_index(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes.get(&id)?.parent?;
        self.nodes
            .get(&parent)?
            .children
            .iter()
            .position(|c| *c == id)
    }

    /// Total node count in the arena — includes the root and all Group/Layer structural
    /// nodes, not just drawable Shape leaves.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        // Always at least the root, so the document is "empty" of content
        // when only the root exists.
        self.nodes.len() <= 1
    }

    /// Immutable access to a leaf's payload. `None` if `id` is missing or is a Group/Layer.
    pub fn leaf(&self, id: NodeId) -> Option<&ShapeLeaf> {
        match &self.nodes.get(&id)?.kind {
            NodeKind::Shape(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// Mutable access to a leaf's payload. `None` if `id` is missing or is a Group/Layer.
    pub fn leaf_mut(&mut self, id: NodeId) -> Option<&mut ShapeLeaf> {
        match &mut self.nodes.get_mut(&id)?.kind {
            NodeKind::Shape(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// Insert `node` as a child of `parent` at `index` (clamped to the child
    /// count). Sets the node's `parent`. Returns the inserted node's id.
    pub fn insert_child(&mut self, parent: NodeId, mut node: Node, index: usize) -> NodeId {
        let id = node.id;
        node.parent = Some(parent);
        self.nodes.insert(id, node);
        if let Some(p) = self.nodes.get_mut(&parent) {
            let idx = index.min(p.children.len());
            p.children.insert(idx, id);
        }
        id
    }

    /// Remove `id` and all its descendants. Detaches `id` from its parent's
    /// child list and returns every removed node (the subtree).
    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<Node> {
        if let Some(parent_id) = self.nodes.get(&id).and_then(|n| n.parent) {
            if let Some(p) = self.nodes.get_mut(&parent_id) {
                p.children.retain(|c| *c != id);
            }
        }
        let mut order = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            if let Some(n) = self.nodes.get(&cur) {
                stack.extend(n.children.iter().copied());
                order.push(cur);
            }
        }
        order
            .into_iter()
            .filter_map(|i| self.nodes.remove(&i))
            .collect()
    }

    /// True if `maybe_ancestor` is `node` itself or any ancestor of `node`
    /// (found by walking `node`'s parent chain). Equivalently: is `node` inside
    /// the subtree rooted at `maybe_ancestor`? A short depth cap makes this
    /// terminate even if the arena somehow already contains a cycle.
    pub fn is_descendant(&self, node: NodeId, maybe_ancestor: NodeId) -> bool {
        let mut cur = Some(node);
        let mut depth = 0usize;
        while let Some(c) = cur {
            if c == maybe_ancestor {
                return true;
            }
            depth += 1;
            if depth > MAX_TREE_DEPTH {
                break;
            }
            cur = self.nodes.get(&c).and_then(|n| n.parent);
        }
        false
    }

    /// Whether `id` may legally be reparented under `new_parent`. Returns false
    /// when the move would corrupt the document:
    /// - `id == new_parent` (a node cannot parent itself),
    /// - `new_parent` is missing from the arena,
    /// - `id` is the root (the root has no parent),
    /// - `new_parent` is `id` or sits inside the subtree rooted at `id`
    ///   (would create a cycle / orphan the subtree),
    /// - `id` is a `Layer` and `new_parent` is not the root (layers may only
    ///   sit directly under root).
    pub fn can_reparent(&self, id: NodeId, new_parent: NodeId) -> bool {
        if id == new_parent {
            return false;
        }
        if self.nodes.get(&new_parent).is_none() {
            return false;
        }
        if id == self.root {
            return false;
        }
        // `new_parent` inside the subtree rooted at `id` ⇒ cycle.
        if self.is_descendant(new_parent, id) {
            return false;
        }
        if matches!(self.nodes.get(&id).map(|n| &n.kind), Some(NodeKind::Layer))
            && new_parent != self.root
        {
            return false;
        }
        true
    }

    /// Move `id` to be a child of `new_parent` at `index` (clamped). Detaches
    /// from the current parent first.
    ///
    /// Defensive: if `can_reparent(id, new_parent)` is false (cycle, missing
    /// parent, root, or a layer under a non-root), this returns WITHOUT mutating
    /// so the document can never be corrupted. `check_invariants` remains the
    /// belt-and-suspenders safety net.
    pub fn reparent(&mut self, id: NodeId, new_parent: NodeId, index: usize) {
        if !self.can_reparent(id, new_parent) {
            return;
        }
        if let Some(old) = self.nodes.get(&id).and_then(|n| n.parent) {
            if let Some(p) = self.nodes.get_mut(&old) {
                p.children.retain(|c| *c != id);
            }
        }
        if let Some(n) = self.nodes.get_mut(&id) {
            n.parent = Some(new_parent);
        }
        if let Some(p) = self.nodes.get_mut(&new_parent) {
            let idx = index.min(p.children.len());
            p.children.insert(idx, id);
        }
    }

    /// Reorder `id` within its current parent's child list to `new_index`
    /// (clamped). No-op if `id` has no parent (e.g. the root).
    pub fn reorder_within_parent(&mut self, id: NodeId, new_index: usize) {
        let Some(parent_id) = self.nodes.get(&id).and_then(|n| n.parent) else {
            return;
        };
        if let Some(p) = self.nodes.get_mut(&parent_id) {
            if let Some(pos) = p.children.iter().position(|c| *c == id) {
                p.children.remove(pos);
                let idx = new_index.min(p.children.len());
                p.children.insert(idx, id);
            }
        }
    }

    /// Verify structural invariants. Returns `Err(reason)` on the first
    /// violation. Used in tests and (cheaply) after structural mutations.
    pub fn check_invariants(&self) -> Result<(), String> {
        let root = self
            .nodes
            .get(&self.root)
            .ok_or_else(|| "root id not present in arena".to_string())?;
        if !matches!(root.kind, NodeKind::Group) {
            return Err("root must be a Group".into());
        }
        if root.parent.is_some() {
            return Err("root must have no parent".into());
        }

        for (id, node) in &self.nodes {
            for child in &node.children {
                let c = self
                    .nodes
                    .get(child)
                    .ok_or_else(|| format!("child {child} of {id} missing from arena"))?;
                if c.parent != Some(*id) {
                    return Err(format!("child {child} does not point back to parent {id}"));
                }
            }
            if *id != self.root {
                let p = node
                    .parent
                    .ok_or_else(|| format!("non-root node {id} has no parent"))?;
                let pn = self
                    .nodes
                    .get(&p)
                    .ok_or_else(|| format!("parent {p} of {id} missing from arena"))?;
                if !pn.children.contains(id) {
                    return Err(format!("node {id} not listed in parent {p}'s children"));
                }
            }
            if matches!(node.kind, NodeKind::Layer) && node.parent != Some(self.root) {
                return Err(format!("Layer {id} must be a direct child of root"));
            }
        }

        // Reachability + cycle detection via DFS from root.
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![self.root];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur) {
                return Err(format!("cycle detected at {cur}"));
            }
            if let Some(n) = self.nodes.get(&cur) {
                stack.extend(n.children.iter().copied());
            }
        }
        if seen.len() != self.nodes.len() {
            return Err("arena contains nodes unreachable from root".into());
        }
        Ok(())
    }

    /// Test-only: insert a node directly into the arena WITHOUT linking it into
    /// any parent's child list. Used to construct deliberately-broken documents.
    #[cfg(test)]
    pub(crate) fn nodes_for_test_insert(&mut self, id: NodeId, node: Node) {
        self.nodes.insert(id, node);
    }

    /// All `Layer` children of root, in order.
    pub fn layers(&self) -> Vec<NodeId> {
        self.get(self.root)
            .map(|r| {
                r.children
                    .iter()
                    .copied()
                    .filter(|c| matches!(self.get(*c).map(|n| &n.kind), Some(NodeKind::Layer)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Create a new empty `Layer` appended as the last child of root. Returns its id.
    pub fn create_layer(&mut self, name: &str) -> NodeId {
        let node = Node {
            id: Uuid::new_v4(),
            parent: None,
            children: Vec::new(),
            name: name.to_string(),
            visible: true,
            locked: false,
            opacity: 1.0,
            transform: Affine::IDENTITY,
            kind: NodeKind::Layer,
        };
        self.insert_child(self.root, node, usize::MAX)
    }

    /// Create a new empty `Group` as a child of `parent` at `index`. Returns its id.
    pub fn create_group(&mut self, parent: NodeId, name: &str, index: usize) -> NodeId {
        let node = Node {
            id: Uuid::new_v4(),
            parent: None,
            children: Vec::new(),
            name: name.to_string(),
            visible: true,
            locked: false,
            opacity: 1.0,
            transform: Affine::IDENTITY,
            kind: NodeKind::Group,
        };
        self.insert_child(parent, node, index)
    }

    /// Wrap `ids` (which must all share one parent) into a new `Group` placed at
    /// the minimum of their current indices, reparenting them into the group in
    /// the given order. Returns the new group id, or `None` if `ids` is empty or
    /// the ids do not share a single parent.
    pub fn group(&mut self, ids: &[NodeId], name: &str) -> Option<NodeId> {
        if ids.is_empty() {
            return None;
        }
        let parent = self.get(ids[0])?.parent?;
        let mut min_index = usize::MAX;
        for id in ids {
            let n = self.get(*id)?;
            if n.parent != Some(parent) {
                return None;
            }
            if let Some(i) = self.child_index(*id) {
                min_index = min_index.min(i);
            }
        }
        let group_id = self.create_group(parent, name, min_index);
        for (i, id) in ids.iter().enumerate() {
            self.reparent(*id, group_id, i);
        }
        Some(group_id)
    }

    /// All leaf (Shape) ids reachable from `id`, in paint order. A leaf returns
    /// just itself.
    pub fn descendant_leaf_ids(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_descendant_leaves(id, &mut out);
        out
    }

    fn collect_descendant_leaves(&self, id: NodeId, out: &mut Vec<NodeId>) {
        let Some(n) = self.get(id) else { return };
        match &n.kind {
            NodeKind::Shape(_) => out.push(id),
            NodeKind::Group | NodeKind::Layer => {
                for c in &n.children {
                    self.collect_descendant_leaves(*c, out);
                }
            }
        }
    }

    /// Re-insert a subtree previously produced by `remove_subtree`. The first
    /// node in `nodes` is the subtree root; it is attached to `parent` at `index`.
    /// All nodes (including the root) are re-inserted by their stored id, so
    /// references survive a delete→undo cycle.
    pub fn insert_subtree(&mut self, parent: NodeId, index: usize, nodes: Vec<Node>) {
        let Some(root_id) = nodes.first().map(|n| n.id) else {
            return;
        };
        for mut n in nodes {
            if n.id == root_id {
                n.parent = Some(parent);
            }
            self.nodes_insert_raw(n);
        }
        if let Some(p) = self.get_mut(parent) {
            let idx = index.min(p.children.len());
            p.children.insert(idx, root_id);
        }
    }

    /// Insert a node into the arena without touching any parent's child list.
    /// Internal: used by `insert_subtree` (the child links are already stored on
    /// the nodes themselves).
    fn nodes_insert_raw(&mut self, node: Node) {
        self.nodes.insert(node.id, node);
    }

    /// The composed affine transform from document space down to `id`'s local
    /// space: the product of every ancestor's transform (root first) and `id`'s own.
    pub fn world_transform(&self, id: NodeId) -> Affine {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        // Cap the chain length: a cycle would otherwise loop forever (see
        // MAX_TREE_DEPTH). Normal documents never approach this depth.
        let mut depth = 0usize;
        while let Some(c) = cur {
            if depth >= MAX_TREE_DEPTH {
                break;
            }
            match self.nodes.get(&c) {
                Some(n) => {
                    chain.push(n.transform);
                    cur = n.parent;
                    depth += 1;
                }
                None => break,
            }
        }
        // `chain` is leaf→root; multiply root→leaf so ancestors apply first.
        let mut acc = Affine::IDENTITY;
        for t in chain.iter().rev() {
            acc *= *t;
        }
        acc
    }

    /// Effective `(visible, locked, opacity)` for `id`: visibility AND-ed,
    /// lock OR-ed, opacity multiplied across `id` and all its ancestors.
    pub fn effective_flags(&self, id: NodeId) -> (bool, bool, f32) {
        let mut visible = true;
        let mut locked = false;
        let mut opacity = 1.0_f32;
        let mut cur = Some(id);
        // Cap the chain length so a stray cycle degrades gracefully rather than
        // looping forever (see MAX_TREE_DEPTH).
        let mut depth = 0usize;
        while let Some(c) = cur {
            if depth >= MAX_TREE_DEPTH {
                break;
            }
            match self.nodes.get(&c) {
                Some(n) => {
                    visible &= n.visible;
                    locked |= n.locked;
                    opacity *= n.opacity;
                    cur = n.parent;
                    depth += 1;
                }
                None => break,
            }
        }
        (visible, locked, opacity)
    }

    /// All leaf shapes in paint order (back to front), each with its composed
    /// world transform and effective flags. This is the derived flat view that
    /// rendering, hit-testing, export, and the MCP layer consume.
    pub fn leaves_in_paint_order(&self) -> Vec<LeafView<'_>> {
        let mut out = Vec::new();
        self.collect_leaves(self.root, 0, &mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, id: NodeId, depth: usize, out: &mut Vec<LeafView<'a>>) {
        // Stop descending past a sane cap. Normal trees are shallow; this only
        // matters if a cycle ever slipped past `can_reparent`, in which case it
        // prevents unbounded recursion / stack overflow (see MAX_TREE_DEPTH).
        if depth > MAX_TREE_DEPTH {
            return;
        }
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        match &node.kind {
            NodeKind::Shape(leaf) => {
                let (visible, locked, opacity) = self.effective_flags(id);
                let z = out.len();
                out.push(LeafView {
                    id,
                    leaf,
                    world: self.world_transform(id),
                    effective_opacity: opacity,
                    effective_visible: visible,
                    effective_locked: locked,
                    own_visible: node.visible,
                    own_locked: node.locked,
                    z,
                });
            }
            NodeKind::Group | NodeKind::Layer => {
                for child in &node.children {
                    self.collect_leaves(*child, depth + 1, out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::node::{Node, NodeKind, ShapeLeaf};
    use crate::types::ShapeData;
    use egui::{Color32, Rect};
    use kurbo::Affine;

    fn shape_node(name: &str) -> Node {
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
    }

    #[test]
    fn default_layer_is_first_layer_child_of_root() {
        let doc = Document::from_flat(vec![]); // root -> "Layer 1"
        let dl = doc.default_layer();
        assert_eq!(doc.get(dl).unwrap().name, "Layer 1");
        assert!(matches!(doc.get(dl).unwrap().kind, NodeKind::Layer));
    }

    #[test]
    fn child_index_reports_position_within_parent() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let a = doc.insert_child(layer, shape_node("a"), usize::MAX);
        let b = doc.insert_child(layer, shape_node("b"), usize::MAX);
        assert_eq!(doc.child_index(a), Some(0));
        assert_eq!(doc.child_index(b), Some(1));
    }

    #[test]
    fn new_document_has_root_group() {
        let doc = Document::new();
        let root = doc.get(doc.root).expect("root present");
        assert!(matches!(root.kind, NodeKind::Group));
        assert_eq!(root.parent, None);
    }

    #[test]
    fn leaf_accessor_returns_payload_only_for_shapes() {
        let mut doc = Document::new();
        let n = shape_node("Rect");
        let id = n.id;
        doc.insert_child(doc.root, n, 0);
        assert!(doc.leaf(id).is_some());
        assert!(doc.leaf(doc.root).is_none()); // root is a Group, not a Shape
        doc.leaf_mut(id).unwrap().thickness = 9.0;
        assert_eq!(doc.leaf(id).unwrap().thickness, 9.0);
    }

    #[test]
    fn remove_subtree_detaches_and_drops_descendants() {
        let mut doc = Document::new();
        let g = Node {
            kind: NodeKind::Group,
            ..shape_node("g")
        };
        let gid = g.id;
        doc.insert_child(doc.root, g, 0);
        let child = shape_node("child");
        let cid = child.id;
        doc.insert_child(gid, child, 0);

        let removed = doc.remove_subtree(gid);
        assert_eq!(removed.len(), 2); // group + its child
        assert!(doc.get(gid).is_none());
        assert!(doc.get(cid).is_none());
        assert!(!doc.get(doc.root).unwrap().children.contains(&gid));
    }

    #[test]
    fn reparent_moves_node_between_parents() {
        let mut doc = Document::new();
        let a = Node {
            kind: NodeKind::Group,
            ..shape_node("a")
        };
        let b = Node {
            kind: NodeKind::Group,
            ..shape_node("b")
        };
        let (aid, bid) = (a.id, b.id);
        doc.insert_child(doc.root, a, 0);
        doc.insert_child(doc.root, b, 1);
        let leaf = shape_node("leaf");
        let lid = leaf.id;
        doc.insert_child(aid, leaf, 0);

        doc.reparent(lid, bid, 0);
        assert_eq!(doc.get(lid).unwrap().parent, Some(bid));
        assert!(doc.get(bid).unwrap().children.contains(&lid));
        assert!(!doc.get(aid).unwrap().children.contains(&lid));
    }

    #[test]
    fn reorder_within_parent_changes_z_order() {
        let mut doc = Document::new();
        let mut ids = Vec::new();
        for i in 0..3 {
            let n = shape_node(&format!("n{i}"));
            ids.push(n.id);
            doc.insert_child(doc.root, n, i);
        }
        // Move the first child to the end.
        doc.reorder_within_parent(ids[0], 2);
        assert_eq!(
            doc.get(doc.root).unwrap().children,
            vec![ids[1], ids[2], ids[0]]
        );
    }

    #[test]
    fn fresh_and_mutated_documents_satisfy_invariants() {
        let mut doc = Document::new();
        assert!(doc.check_invariants().is_ok());
        let g = Node {
            kind: NodeKind::Group,
            ..shape_node("g")
        };
        let gid = g.id;
        doc.insert_child(doc.root, g, 0);
        doc.insert_child(gid, shape_node("leaf"), 0);
        assert!(doc.check_invariants().is_ok());
    }

    #[test]
    fn invariant_check_catches_parent_child_mismatch() {
        let mut doc = Document::new();
        let n = shape_node("orphan");
        let id = n.id;
        // Insert into the arena but DON'T add to root's children → inconsistent.
        let mut n = n;
        n.parent = Some(doc.root);
        doc.nodes_for_test_insert(id, n);
        assert!(doc.check_invariants().is_err());
    }

    #[test]
    fn world_transform_composes_ancestors() {
        let mut doc = Document::new();
        // Group translated by (10, 0); leaf translated by (0, 5) within it.
        let mut g = Node {
            kind: NodeKind::Group,
            ..shape_node("g")
        };
        g.transform = Affine::translate((10.0, 0.0));
        let gid = g.id;
        doc.insert_child(doc.root, g, 0);
        let mut leaf = shape_node("leaf");
        leaf.transform = Affine::translate((0.0, 5.0));
        let lid = leaf.id;
        doc.insert_child(gid, leaf, 0);

        let w = doc.world_transform(lid);
        let p = w * kurbo::Point::new(0.0, 0.0);
        assert!((p.x - 10.0).abs() < 1e-9);
        assert!((p.y - 5.0).abs() < 1e-9);
    }

    #[test]
    fn effective_flags_combine_down_the_tree() {
        let mut doc = Document::new();
        let mut g = Node {
            kind: NodeKind::Group,
            ..shape_node("g")
        };
        g.visible = false; // hidden group
        g.opacity = 0.5;
        let gid = g.id;
        doc.insert_child(doc.root, g, 0);
        let mut leaf = shape_node("leaf");
        leaf.locked = true;
        leaf.opacity = 0.5;
        let lid = leaf.id;
        doc.insert_child(gid, leaf, 0);

        let (visible, locked, opacity) = doc.effective_flags(lid);
        assert!(!visible, "hidden ancestor hides the leaf");
        assert!(locked, "leaf's own lock applies");
        assert!((opacity - 0.25).abs() < 1e-6, "opacities multiply");
    }

    #[test]
    fn insert_and_reorder_clamp_past_bounds() {
        let mut doc = Document::new();
        let n = shape_node("far");
        let id = n.id;
        // Insert with a huge index — should be clamped to the end (index 0 of an empty list).
        doc.insert_child(doc.root, n, 999);
        assert_eq!(doc.get(doc.root).unwrap().children.last(), Some(&id));
        // Reorder to a huge index — still last, no panic.
        doc.reorder_within_parent(id, 999);
        assert_eq!(doc.get(doc.root).unwrap().children.last(), Some(&id));
    }

    #[test]
    fn missing_id_operations_are_noops() {
        let doc = Document::new();
        let random_id = uuid::Uuid::new_v4();
        assert!(doc.leaf(random_id).is_none());

        let mut doc = Document::new();
        assert!(doc.leaf_mut(random_id).is_none());
        let removed = doc.remove_subtree(random_id);
        assert!(removed.is_empty());
        doc.reorder_within_parent(random_id, 0); // must not panic
    }

    #[test]
    fn layer_not_under_root_violates_invariants() {
        let mut doc = Document::new();
        // Insert a Group under root, then a Layer under that group.
        let g = Node {
            kind: NodeKind::Group,
            ..shape_node("g")
        };
        let gid = g.id;
        doc.insert_child(doc.root, g, 0);
        let layer = Node {
            kind: NodeKind::Layer,
            ..shape_node("misplaced layer")
        };
        doc.insert_child(gid, layer, 0);
        assert!(doc.check_invariants().is_err());
    }

    #[test]
    fn create_layer_appends_layer_as_last_root_child() {
        let mut doc = Document::from_flat(vec![]); // root -> "Layer 1"
        let id = doc.create_layer("Layer 2");
        assert!(doc.check_invariants().is_ok());
        assert_eq!(doc.get(id).unwrap().name, "Layer 2");
        assert!(matches!(doc.get(id).unwrap().kind, NodeKind::Layer));
        assert_eq!(doc.get(doc.root).unwrap().children.last(), Some(&id));
    }

    #[test]
    fn layers_lists_layer_children_in_order() {
        let mut doc = Document::from_flat(vec![]);
        let l2 = doc.create_layer("Layer 2");
        let layers = doc.layers();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1], l2);
    }

    #[test]
    fn group_leaves_wraps_them_under_common_parent_preserving_order() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let a = doc.insert_child(layer, shape_node("a"), usize::MAX);
        let b = doc.insert_child(layer, shape_node("b"), usize::MAX);
        let c = doc.insert_child(layer, shape_node("c"), usize::MAX);
        // Group a and c (non-contiguous). New group sits at the min index (0),
        // children ordered as given.
        let g = doc.group(&[a, c], "Group 1").expect("group created");
        assert!(doc.check_invariants().is_ok());
        assert_eq!(doc.get(g).unwrap().parent, Some(layer));
        assert_eq!(doc.get(g).unwrap().children, vec![a, c]);
        // b remains a direct child of the layer; the group took a's old slot.
        let layer_children = &doc.get(layer).unwrap().children;
        assert!(layer_children.contains(&g));
        assert!(layer_children.contains(&b));
        assert!(!layer_children.contains(&a));
    }

    #[test]
    fn group_rejects_ids_in_different_parents() {
        let mut doc = Document::from_flat(vec![]);
        let l1 = doc.default_layer();
        let l2 = doc.create_layer("Layer 2");
        let a = doc.insert_child(l1, shape_node("a"), usize::MAX);
        let b = doc.insert_child(l2, shape_node("b"), usize::MAX);
        assert!(doc.group(&[a, b], "G").is_none());
    }

    #[test]
    fn descendant_leaf_ids_collects_in_paint_order() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let a = doc.insert_child(layer, shape_node("a"), usize::MAX);
        let b = doc.insert_child(layer, shape_node("b"), usize::MAX);
        let g = doc.group(&[a, b], "G").unwrap();
        assert_eq!(doc.descendant_leaf_ids(g), vec![a, b]);
        assert_eq!(doc.descendant_leaf_ids(a), vec![a]); // a leaf is its own sole leaf
    }

    #[test]
    fn insert_subtree_restores_removed_nodes_by_id() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let a = doc.insert_child(layer, shape_node("a"), usize::MAX);
        let g = doc.group(&[a], "G").unwrap();
        let g_index = doc.child_index(g).unwrap();
        let removed = doc.remove_subtree(g);
        assert!(doc.get(g).is_none());
        doc.insert_subtree(layer, g_index, removed);
        assert!(doc.check_invariants().is_ok());
        assert!(doc.get(g).is_some());
        assert_eq!(doc.get(g).unwrap().children, vec![a]);
    }

    #[test]
    fn cannot_reparent_into_own_descendant() {
        // root -> layer -> outer(Group) -> inner(Group)
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let outer = doc.create_group(layer, "outer", usize::MAX);
        let inner = doc.create_group(outer, "inner", usize::MAX);
        assert!(doc.check_invariants().is_ok());

        // Moving `outer` into its own descendant `inner` would create a cycle.
        assert!(!doc.can_reparent(outer, inner));
        // `reparent` must be defensive — leave the tree untouched.
        doc.reparent(outer, inner, 0);
        assert_eq!(doc.get(outer).unwrap().parent, Some(layer));
        assert_eq!(doc.get(inner).unwrap().parent, Some(outer));
        assert!(doc.check_invariants().is_ok());
        // Reparenting a node into itself is also rejected.
        assert!(!doc.can_reparent(outer, outer));
    }

    #[test]
    fn cannot_reparent_layer_under_non_root() {
        // root -> layer -> group ; layers may only sit directly under root.
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let group = doc.create_group(layer, "g", usize::MAX);
        assert!(doc.check_invariants().is_ok());

        assert!(!doc.can_reparent(layer, group));
        doc.reparent(layer, group, 0);
        assert_eq!(doc.get(layer).unwrap().parent, Some(doc.root));
        assert!(doc.check_invariants().is_ok());
    }

    #[test]
    fn cannot_reparent_root() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        assert!(!doc.can_reparent(doc.root, layer));
        doc.reparent(doc.root, layer, 0);
        assert_eq!(doc.get(doc.root).unwrap().parent, None);
        assert!(doc.check_invariants().is_ok());
        // Missing new_parent is also rejected.
        assert!(!doc.can_reparent(layer, uuid::Uuid::new_v4()));
    }

    #[test]
    fn can_reparent_leaf_between_layers() {
        let mut doc = Document::from_flat(vec![]);
        let l1 = doc.default_layer();
        let l2 = doc.create_layer("Layer 2");
        let leaf = doc.insert_child(l1, shape_node("leaf"), usize::MAX);
        assert!(doc.can_reparent(leaf, l2));
        doc.reparent(leaf, l2, 0);
        assert_eq!(doc.get(leaf).unwrap().parent, Some(l2));
        assert!(doc.check_invariants().is_ok());
    }

    #[test]
    fn is_descendant_walks_ancestor_chain() {
        let mut doc = Document::from_flat(vec![]);
        let layer = doc.default_layer();
        let outer = doc.create_group(layer, "outer", usize::MAX);
        let inner = doc.create_group(outer, "inner", usize::MAX);
        // `outer` is an ancestor of `inner` (and `inner` is itself).
        assert!(doc.is_descendant(inner, outer));
        assert!(doc.is_descendant(inner, inner));
        assert!(doc.is_descendant(inner, doc.root));
        // `inner` is NOT an ancestor of `outer`.
        assert!(!doc.is_descendant(outer, inner));
    }

    #[test]
    fn paint_order_is_depth_first_in_child_order() {
        let mut doc = Document::new();
        // root → [layerA → [s1, s2], s3]   (s3 is a direct child of root)
        let layer = Node {
            kind: NodeKind::Layer,
            ..shape_node("Layer 1")
        };
        let layer_id = layer.id;
        doc.insert_child(doc.root, layer, 0);
        let s1 = shape_node("s1");
        let s2 = shape_node("s2");
        let s3 = shape_node("s3");
        let (id1, id2, id3) = (s1.id, s2.id, s3.id);
        doc.insert_child(layer_id, s1, 0);
        doc.insert_child(layer_id, s2, 1);
        doc.insert_child(doc.root, s3, 1);

        let order: Vec<_> = doc.leaves_in_paint_order().iter().map(|lv| lv.id).collect();
        assert_eq!(order, vec![id1, id2, id3]);
        // z is the paint index.
        assert_eq!(doc.leaves_in_paint_order()[2].z, 2);
        // Only leaves appear (the Layer/Group nodes are skipped).
        assert_eq!(doc.leaves_in_paint_order().len(), 3);
    }
}
