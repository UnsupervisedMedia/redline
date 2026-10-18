use crate::{
    state::CanvasState,
    types::{
        Annotation, BezierNode, BoolOp, Command, EventKind, ImageSnapshot, NodeProp, ParticipantId,
        PropChange, ShapeData,
    },
};
use egui::Vec2;
use uuid::Uuid;

pub struct CommandExecutor;

impl CommandExecutor {
    /// Apply `cmd` on behalf of `actor`. Pushes inverse to actor's undo stack,
    /// clears actor's redo stack, records a `CanvasEvent` in the room's log.
    /// Returns `true` if the command was applied, `false` if it was a no-op
    /// (shape not found or shape is locked).
    pub fn execute(canvas: &mut CanvasState, actor: ParticipantId, cmd: Command) -> bool {
        let Some(inverse) = Self::apply(canvas, cmd.clone()) else {
            return false;
        };
        Self::record_event_for(canvas, actor, &cmd, &inverse);
        #[cfg(feature = "agent-server")]
        broadcast_latest_event(canvas);

        canvas.undo_stacks.entry(actor).or_default().push(inverse);
        canvas.redo_stacks.entry(actor).or_default().clear();
        true
    }

    /// Pop from `actor`'s undo stack, apply the inverse, push the result to
    /// `actor`'s redo stack. Also records the inverse as an event.
    /// Returns `true` if the undo was applied, `false` if the stack was empty
    /// or the inverse was a no-op.
    pub fn undo(canvas: &mut CanvasState, actor: ParticipantId) -> bool {
        let Some(inverse) = canvas.undo_stacks.entry(actor).or_default().pop() else {
            return false;
        };
        let Some(redo_cmd) = Self::apply(canvas, inverse.clone()) else {
            // apply no-op'd — push it back so the user can retry once the
            // blocking condition (e.g., shape locked) is resolved.
            canvas.undo_stacks.entry(actor).or_default().push(inverse);
            return false;
        };
        Self::record_event_for(canvas, actor, &inverse, &redo_cmd);
        #[cfg(feature = "agent-server")]
        broadcast_latest_event(canvas);
        canvas.redo_stacks.entry(actor).or_default().push(redo_cmd);
        true
    }

    /// Pop from `actor`'s redo stack, apply it, push its inverse to `actor`'s
    /// undo stack. Records the redone command as an event.
    /// Returns `true` if the redo was applied, `false` if the stack was empty
    /// or the command was a no-op.
    pub fn redo(canvas: &mut CanvasState, actor: ParticipantId) -> bool {
        let Some(cmd) = canvas.redo_stacks.entry(actor).or_default().pop() else {
            return false;
        };
        let Some(inverse) = Self::apply(canvas, cmd.clone()) else {
            // apply no-op'd — push it back so the user can retry once the
            // blocking condition (e.g., shape locked) is resolved.
            canvas.redo_stacks.entry(actor).or_default().push(cmd);
            return false;
        };
        Self::record_event_for(canvas, actor, &cmd, &inverse);
        #[cfg(feature = "agent-server")]
        broadcast_latest_event(canvas);
        canvas.undo_stacks.entry(actor).or_default().push(inverse);
        true
    }

    /// Apply `cmd` and return its inverse, or `None` if the command was a
    /// no-op (shape not found or shape is locked). Does NOT touch stacks or
    /// event_log.
    fn apply(canvas: &mut CanvasState, cmd: Command) -> Option<Command> {
        let inverse = match cmd {
            Command::Add(ann) => {
                let id = ann.id;
                let layer = if canvas.document.get(canvas.active_layer).is_some() {
                    canvas.active_layer
                } else {
                    canvas.document.default_layer()
                };
                canvas.document.add_annotation(&ann, layer);
                Some(Command::Delete(id))
            }
            Command::Delete(id) => {
                // Leaves invert via Add; groups (and any non-leaf, non-layer node)
                // invert via RestoreSubtree so the whole subtree comes back on undo.
                match canvas.document.get(id).map(|n| &n.kind) {
                    Some(crate::scene::NodeKind::Shape(_)) => {
                        let ann = canvas.document.leaf_to_annotation(id)?;
                        canvas.document.remove_subtree(id);
                        Some(Command::Add(ann))
                    }
                    Some(crate::scene::NodeKind::Group) => {
                        let parent = canvas.document.get(id)?.parent?;
                        let index = canvas.document.child_index(id)?;
                        let nodes = canvas.document.remove_subtree(id);
                        if nodes.is_empty() {
                            return None;
                        }
                        Some(Command::RestoreSubtree {
                            parent,
                            index,
                            nodes,
                        })
                    }
                    // Layers must go through DeleteLayer; deleting root/missing is a no-op.
                    _ => return None,
                }
            }
            Command::Move { id, delta } => {
                if canvas.document.get(id).map(|n| n.locked).unwrap_or(true) {
                    return None;
                }
                let leaf = canvas.document.leaf_mut(id)?;
                leaf.shape.translate(delta);
                Some(Command::Move {
                    id,
                    delta: Vec2::new(-delta.x, -delta.y),
                })
            }
            Command::Resize { id, new_rect } => {
                if canvas.document.get(id).map(|n| n.locked).unwrap_or(true) {
                    return None;
                }
                let leaf = canvas.document.leaf_mut(id)?;
                let old_rect = leaf.shape.resize_rect();
                leaf.shape.set_rect(new_rect);
                Some(Command::Resize {
                    id,
                    new_rect: old_rect,
                })
            }
            Command::SetProp { id, prop } => {
                // visible/locked live on the node; everything else on the leaf.
                let node = canvas.document.get(id)?;
                let old_prop = match &prop {
                    PropChange::Locked(_) => PropChange::Locked(node.locked),
                    PropChange::Visible(_) => PropChange::Visible(node.visible),
                    PropChange::Color(_) => PropChange::Color(canvas.document.leaf(id)?.color),
                    PropChange::Thickness(_) => {
                        PropChange::Thickness(canvas.document.leaf(id)?.thickness)
                    }
                    PropChange::Label(_) => {
                        PropChange::Label(canvas.document.leaf(id)?.label.clone())
                    }
                    PropChange::StepNumber(_) => {
                        let shape = &canvas.document.leaf(id)?.shape;
                        let n = if let ShapeData::StepMarker { number, .. } = shape {
                            *number
                        } else {
                            0
                        };
                        PropChange::StepNumber(n)
                    }
                    PropChange::TextSize(_) => {
                        let shape = &canvas.document.leaf(id)?.shape;
                        let s = match shape {
                            ShapeData::Text { size, .. } | ShapeData::Question { size, .. } => {
                                *size
                            }
                            _ => 18.0,
                        };
                        PropChange::TextSize(s)
                    }
                };
                match prop {
                    PropChange::Locked(v) => {
                        canvas.document.get_mut(id)?.locked = v;
                    }
                    PropChange::Visible(v) => {
                        canvas.document.get_mut(id)?.visible = v;
                    }
                    PropChange::Color(c) => {
                        canvas.document.leaf_mut(id)?.color = c;
                    }
                    PropChange::Thickness(t) => {
                        canvas.document.leaf_mut(id)?.thickness = t;
                    }
                    PropChange::Label(l) => {
                        canvas.document.leaf_mut(id)?.label = l;
                    }
                    PropChange::StepNumber(n) => {
                        let shape = &mut canvas.document.leaf_mut(id)?.shape;
                        if let ShapeData::StepMarker { number, .. } = shape {
                            *number = n;
                        }
                    }
                    PropChange::TextSize(s) => match &mut canvas.document.leaf_mut(id)?.shape {
                        ShapeData::Text { size, .. } | ShapeData::Question { size, .. } => {
                            *size = s;
                        }
                        _ => {}
                    },
                }
                Some(Command::SetProp { id, prop: old_prop })
            }
            Command::Batch(cmds) => {
                let mut inverses: Vec<Command> = Vec::with_capacity(cmds.len());
                let mut any_applied = false;
                for c in cmds {
                    if let Some(inv) = Self::apply(canvas, c) {
                        inverses.push(inv);
                        any_applied = true;
                    }
                }
                if !any_applied {
                    return None;
                }
                inverses.reverse();
                Some(Command::Batch(inverses))
            }
            Command::ReplaceImage(snap) => {
                let old = canvas.color_image.replace(snap.0).unwrap_or_default();
                canvas.texture = None;
                Some(Command::ReplaceImage(ImageSnapshot(old)))
            }
            Command::BringToFront(id) => {
                let pos = canvas.document.child_index(id)?;
                let parent = canvas.document.get(id)?.parent?;
                let last = canvas
                    .document
                    .get(parent)?
                    .children
                    .len()
                    .saturating_sub(1);
                canvas.document.reorder_within_parent(id, last);
                Some(Command::ReorderTo { id, index: pos })
            }
            Command::SendToBack(id) => {
                let pos = canvas.document.child_index(id)?;
                canvas.document.reorder_within_parent(id, 0);
                Some(Command::ReorderTo { id, index: pos })
            }
            Command::ReorderTo { id, index } => {
                let pos = canvas.document.child_index(id)?;
                canvas.document.reorder_within_parent(id, index);
                Some(Command::ReorderTo { id, index: pos })
            }
            Command::RenumberSteps => {
                let step_info: Vec<(uuid::Uuid, u32)> = canvas
                    .annotations
                    .iter()
                    .filter_map(|a| {
                        if let ShapeData::StepMarker { number, .. } = &a.shape {
                            Some((a.id, *number))
                        } else {
                            None
                        }
                    })
                    .collect();
                let undo_cmds: Vec<Command> = step_info
                    .iter()
                    .map(|&(id, old_num)| Command::SetProp {
                        id,
                        prop: PropChange::StepNumber(old_num),
                    })
                    .collect();
                let mut counter = 1u32;
                for (id, _) in &step_info {
                    if let Some(leaf) = canvas.document.leaf_mut(*id) {
                        if let ShapeData::StepMarker { number, .. } = &mut leaf.shape {
                            *number = counter;
                            counter += 1;
                        }
                    }
                }
                Some(Command::Batch(undo_cmds))
            }
            Command::SetTextContent { id, content } => {
                let leaf = canvas.document.leaf_mut(id)?;
                if let ShapeData::Text {
                    content: ref mut existing,
                    ..
                } = &mut leaf.shape
                {
                    let old = std::mem::replace(existing, content);
                    Some(Command::SetTextContent { id, content: old })
                } else {
                    None
                }
            }
            Command::ReplacePenPoints {
                id,
                raw_points,
                smoothed_points,
            } => {
                let leaf = canvas.document.leaf_mut(id)?;
                if let ShapeData::Pen {
                    raw_points: ref mut old_raw,
                    smoothed_points: ref mut old_smooth,
                } = &mut leaf.shape
                {
                    let prev_raw = std::mem::replace(old_raw, raw_points);
                    let prev_smooth = std::mem::replace(old_smooth, smoothed_points);
                    Some(Command::ReplacePenPoints {
                        id,
                        raw_points: prev_raw,
                        smoothed_points: prev_smooth,
                    })
                } else {
                    None
                }
            }
            Command::CreateLayer { id, name } => {
                // Insert a layer with the *caller-provided* id so the inverse can
                // address it. We can't reuse create_layer() (it mints a new uuid),
                // so insert directly via a constructed node.
                let node = crate::scene::Node {
                    id,
                    parent: None,
                    children: Vec::new(),
                    name,
                    visible: true,
                    locked: false,
                    opacity: 1.0,
                    transform: kurbo::Affine::IDENTITY,
                    kind: crate::scene::NodeKind::Layer,
                };
                let root = canvas.document.root;
                canvas.document.insert_child(root, node, usize::MAX);
                Some(Command::DeleteLayer { id })
            }
            Command::DeleteLayer { id } => {
                let parent = canvas.document.get(id)?.parent?;
                let index = canvas.document.child_index(id)?;
                let nodes = canvas.document.remove_subtree(id);
                if nodes.is_empty() {
                    return None;
                }
                Some(Command::RestoreSubtree {
                    parent,
                    index,
                    nodes,
                })
            }
            Command::RestoreSubtree {
                parent,
                index,
                nodes,
            } => {
                // Inverse is "delete the restored subtree root". RestoreSubtree is
                // produced by DeleteLayer (root = a Layer) and by Delete on a Group
                // (root = a Group); DeleteLayer's removal is generic, so the inverse
                // works for either kind.
                let root_id = nodes.first()?.id;
                canvas.document.insert_subtree(parent, index, nodes);
                Some(Command::DeleteLayer { id: root_id })
            }
            Command::MoveNodeToLayer {
                id,
                layer_id,
                index,
            } => {
                // Reject invalid reparents (cycle, missing parent, root, or a
                // layer under a non-root) as a no-op so nothing is recorded or
                // synced and the document can never be corrupted.
                if !canvas.document.can_reparent(id, layer_id) {
                    return None;
                }
                let old_parent = canvas.document.get(id)?.parent?;
                let old_index = canvas.document.child_index(id)?;
                canvas.document.reparent(id, layer_id, index);
                Some(Command::MoveNodeToLayer {
                    id,
                    layer_id: old_parent,
                    index: old_index,
                })
            }
            Command::ReorderNode { id, index } => {
                let pos = canvas.document.child_index(id)?;
                canvas.document.reorder_within_parent(id, index);
                Some(Command::ReorderNode { id, index: pos })
            }
            Command::Group { ids, name } => {
                let group_id = canvas.document.group(&ids, &name)?;
                Some(Command::Ungroup { group_id })
            }
            Command::Ungroup { group_id } => {
                let parent = canvas.document.get(group_id)?.parent?;
                let index = canvas.document.child_index(group_id)?;
                let name = canvas.document.get(group_id)?.name.clone();
                let children: Vec<uuid::Uuid> = canvas.document.get(group_id)?.children.clone();
                // Reparent each child to the group's parent, in order, at the group's slot.
                for (i, child) in children.iter().enumerate() {
                    canvas.document.reparent(*child, parent, index + i);
                }
                canvas.document.remove_subtree(group_id);
                // Inverse re-groups the same children (mints a fresh group id;
                // acceptable in Phase 2 — groups are panel-managed, not addressed
                // by stable id across undo).
                Some(Command::Group {
                    ids: children,
                    name,
                })
            }
            Command::SetNodeProp { id, prop } => {
                let node = canvas.document.get(id)?;
                let old = match &prop {
                    NodeProp::Name(_) => NodeProp::Name(node.name.clone()),
                    NodeProp::Visible(_) => NodeProp::Visible(node.visible),
                    NodeProp::Locked(_) => NodeProp::Locked(node.locked),
                    NodeProp::Opacity(_) => NodeProp::Opacity(node.opacity),
                    NodeProp::Transform(_) => NodeProp::Transform(node.transform),
                };
                let n = canvas.document.get_mut(id)?;
                match prop {
                    NodeProp::Name(v) => n.name = v,
                    NodeProp::Visible(v) => n.visible = v,
                    NodeProp::Locked(v) => n.locked = v,
                    NodeProp::Opacity(v) => n.opacity = v.clamp(0.0, 1.0),
                    NodeProp::Transform(t) => n.transform = t,
                }
                Some(Command::SetNodeProp { id, prop: old })
            }
            Command::ReplaceLeaves { remove, add } => replace_leaves(canvas, &remove, &add),
            Command::WeldPaths { a, b } => {
                if canvas.document.get(a).map(|n| n.locked).unwrap_or(true)
                    || canvas.document.get(b).map(|n| n.locked).unwrap_or(true)
                {
                    return None;
                }
                let a_ann = canvas.document.leaf_to_annotation(a)?;
                let b_ann = canvas.document.leaf_to_annotation(b)?;
                let (a_nodes, a_closed) = match &a_ann.shape {
                    ShapeData::BezierPath { nodes, closed, .. } => (nodes.clone(), *closed),
                    _ => return None,
                };
                let (b_nodes, b_closed) = match &b_ann.shape {
                    ShapeData::BezierPath { nodes, closed, .. } => (nodes.clone(), *closed),
                    _ => return None,
                };
                let merged =
                    crate::tools::path_edit::weld_paths(&a_nodes, a_closed, &b_nodes, b_closed)?;
                let a_parent = canvas.document.get(a)?.parent?;
                let a_index = canvas.document.child_index(a)?;
                // Frontmost-wins: result inherits a's style and lands at a's slot.
                let mut result = a_ann.clone();
                result.id = uuid::Uuid::new_v4();
                result.shape = ShapeData::BezierPath {
                    nodes: merged,
                    closed: false,
                    fill: None,
                };
                replace_leaves(canvas, &[a, b], &[(result, a_parent, a_index)])
            }
            Command::BooleanOp { op, ids } => apply_boolean(canvas, op, &ids),
            Command::FlattenLayer { layer_id } => {
                // Gather closed BezierPath leaves under the layer; union them.
                let leaf_ids: Vec<uuid::Uuid> = canvas
                    .document
                    .descendant_leaf_ids(layer_id)
                    .into_iter()
                    .collect();
                apply_boolean(canvas, crate::types::BoolOp::Unite, &leaf_ids)
            }
            Command::FlattenToImage => {
                let canvas_size = canvas.canvas_size;
                let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, canvas_size);
                // Bake only leaf-visible annotations (matches what is shown, minus
                // whole-layer hidden state — see plan Global Constraints).
                let visible: Vec<crate::types::Annotation> = canvas
                    .annotations
                    .iter()
                    .filter(|a| a.visible)
                    .cloned()
                    .collect();
                let rgba = crate::flatten::flatten(
                    &visible,
                    canvas.color_image.as_ref(),
                    canvas_size,
                    screen_rect,
                )?; // None → nothing to render → no-op
                    // Capture every top-level node under every layer, in (parent,index).
                let mut to_remove: Vec<(uuid::Uuid, usize, uuid::Uuid)> = Vec::new();
                for lid in canvas.document.layers() {
                    if let Some(layer) = canvas.document.get(lid) {
                        for (idx, child) in layer.children.iter().enumerate() {
                            to_remove.push((lid, idx, *child));
                        }
                    }
                }
                if to_remove.is_empty() {
                    return None; // nothing to merge; flattening would be a no-op
                }
                let old_image = canvas.color_image.clone().unwrap_or_default();
                // Remove subtrees by id (order-independent) and capture for restore.
                let mut captured: Vec<(uuid::Uuid, usize, Vec<crate::scene::Node>)> = Vec::new();
                for (parent, index, child) in &to_remove {
                    let nodes = canvas.document.remove_subtree(*child);
                    if !nodes.is_empty() {
                        captured.push((*parent, *index, nodes));
                    }
                }
                // Swap in the flattened image (texture refresh handled by ReplaceImage
                // semantics elsewhere; set here too so the forward op repaints).
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                let new_ci = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
                canvas.color_image = Some(new_ci);
                canvas.texture = None;
                // Inverse: restore old image, then re-insert subtrees ascending by index.
                // Stable sort by index. Children from the same parent stay in their
                // enumerated (ascending) order, which is what insert_subtree needs;
                // interleaving across parents is harmless since each parent inserts
                // independently.
                captured.sort_by_key(|(_p, idx, _n)| *idx);
                let mut inv: Vec<Command> = Vec::with_capacity(captured.len() + 1);
                inv.push(Command::ReplaceImage(ImageSnapshot(old_image)));
                for (parent, index, nodes) in captured {
                    inv.push(Command::RestoreSubtree {
                        parent,
                        index,
                        nodes,
                    });
                }
                Some(Command::Batch(inv))
            }
        };
        if inverse.is_some() {
            canvas.sync_annotations();
        }
        inverse
    }

    /// Translate a Command into an EventKind and append to the event log.
    fn record_event_for(
        canvas: &mut CanvasState,
        actor: ParticipantId,
        cmd: &Command,
        inverse: &Command,
    ) {
        let kind = match cmd {
            Command::Add(ann) => {
                // After apply, the annotation is at the end of the vec.
                let stored = canvas.annotations.iter().find(|a| a.id == ann.id).cloned();
                EventKind::ShapeAdded(stored.unwrap_or_else(|| ann.clone()))
            }
            Command::Delete(id) => {
                let deleted_author = match inverse {
                    Command::Add(ann) => ann.author,
                    _ => ParticipantId::HUMAN_LOCAL,
                };
                EventKind::ShapeDeleted {
                    id: *id,
                    deleted_author,
                }
            }
            Command::Move { id, delta } => EventKind::ShapeMoved {
                id: *id,
                delta: *delta,
            },
            Command::Resize { id, new_rect } => EventKind::ShapeResized {
                id: *id,
                new_rect: *new_rect,
            },
            Command::SetProp { id, prop } => EventKind::ShapePropChanged {
                id: *id,
                prop: prop.clone(),
            },
            Command::BringToFront(id) => EventKind::ShapeReordered {
                id: *id,
                new_index: canvas.annotations.len().saturating_sub(1),
            },
            Command::SendToBack(id) => EventKind::ShapeReordered {
                id: *id,
                new_index: 0,
            },
            Command::ReorderTo { id, index } => EventKind::ShapeReordered {
                id: *id,
                new_index: *index,
            },
            Command::ReplaceImage(_) => EventKind::ImageReplaced,
            Command::Batch(cmds) => {
                for sub in cmds {
                    Self::record_event_for(canvas, actor, sub, inverse);
                }
                return;
            }
            Command::RenumberSteps => return,
            Command::SetTextContent { id, content } => EventKind::ShapeTextChanged {
                id: *id,
                content: content.clone(),
            },
            Command::ReplacePenPoints {
                id,
                raw_points,
                smoothed_points,
            } => EventKind::ShapePenPointsChanged {
                id: *id,
                raw_points: raw_points.clone(),
                smoothed_points: smoothed_points.clone(),
            },
            Command::CreateLayer { id, name } => EventKind::LayerCreated {
                id: *id,
                name: name.clone(),
            },
            Command::DeleteLayer { id } => EventKind::LayerDeleted { id: *id },
            Command::RestoreSubtree { .. } => return, // internal undo primitive; no event
            Command::MoveNodeToLayer {
                id,
                layer_id,
                index,
            } => EventKind::NodeReparented {
                id: *id,
                new_parent: *layer_id,
                new_index: *index,
            },
            Command::ReorderNode { id, index } => EventKind::NodeReordered {
                id: *id,
                new_index: *index,
            },
            Command::Group { ids, .. } => {
                // group_id was just minted; look it up via the first child's parent
                // (record_event_for runs post-apply).
                let group_id = ids
                    .first()
                    .and_then(|c| canvas.document.get(*c))
                    .and_then(|n| n.parent)
                    .unwrap_or_else(uuid::Uuid::nil);
                EventKind::Grouped {
                    group_id,
                    children: ids.clone(),
                }
            }
            Command::Ungroup { group_id } => EventKind::Ungrouped {
                group_id: *group_id,
            },
            Command::SetNodeProp { id, prop } => EventKind::NodePropChanged {
                id: *id,
                prop: prop.clone(),
            },
            // Path ops are structural leaf swaps. Like RestoreSubtree they record
            // no dedicated event; the local mirror is kept current by
            // sync_annotations and a collab resync covers the replay path. (Phase 4
            // has no path-op-specific EventKind; revisit when collab replays them.)
            Command::WeldPaths { .. }
            | Command::BooleanOp { .. }
            | Command::FlattenLayer { .. }
            | Command::ReplaceLeaves { .. } => return,
            Command::FlattenToImage => EventKind::ImageReplaced,
        };
        canvas.record_event(actor, kind);
    }
}

/// Remove `remove_ids` (capturing each as `(Annotation, parent, index)`) and add
/// `additions` at their `(parent, index)` slots. Returns the symmetric inverse
/// (`ReplaceLeaves`) that re-adds what we removed and removes what we added.
///
/// Used as the single undo/redo mechanism for `WeldPaths`,
/// `BooleanOp`, `FlattenLayer`, and `ReplaceLeaves` itself. Returns `None` (a
/// no-op) if any removal target is missing/parentless so nothing is recorded.
fn replace_leaves(
    canvas: &mut CanvasState,
    remove_ids: &[Uuid],
    additions: &[(Annotation, Uuid, usize)],
) -> Option<Command> {
    // Capture before mutating so we can roll back faithfully on undo.
    let mut captured: Vec<(Annotation, Uuid, usize)> = Vec::with_capacity(remove_ids.len());
    for id in remove_ids {
        let parent = canvas.document.get(*id)?.parent?;
        let index = canvas.document.child_index(*id)?;
        let ann = canvas.document.leaf_to_annotation(*id)?;
        captured.push((ann, parent, index));
    }
    for id in remove_ids {
        canvas.document.remove_subtree(*id);
    }
    let mut added_ids = Vec::with_capacity(additions.len());
    for (ann, parent, index) in additions {
        canvas.document.add_annotation(ann, *parent);
        canvas.document.reorder_within_parent(ann.id, *index);
        added_ids.push(ann.id);
    }
    // Inverse: remove what we added, re-add what we removed.
    Some(Command::ReplaceLeaves {
        remove: added_ids,
        add: captured,
    })
}

/// Filter `ids` to the closed BezierPath leaves and return them sorted by paint
/// order (back-to-front, so the frontmost qualifying leaf is last).
fn closed_bezier_ids_in_paint_order(canvas: &CanvasState, ids: &[Uuid]) -> Vec<Uuid> {
    let order: Vec<Uuid> = canvas
        .document
        .leaves_in_paint_order()
        .into_iter()
        .map(|lv| lv.id)
        .collect();
    let mut kept: Vec<Uuid> = ids
        .iter()
        .copied()
        .filter(|id| {
            matches!(
                canvas.document.leaf(*id).map(|l| &l.shape),
                Some(ShapeData::BezierPath { closed: true, .. })
            )
        })
        .collect();
    kept.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    kept.dedup();
    kept
}

/// Shared boolean-op core for `BooleanOp` and `FlattenLayer`. Collects the closed
/// BezierPath leaves among `ids` in paint order, runs the op, and replaces the
/// inputs with the result leaves placed at the frontmost input's slot.
///
/// Guards degenerate output: a 0-contour result is treated as a no-op (`None`),
/// never emitting empty/garbage geometry. Multi-contour results become sibling
/// `BezierPath` leaves in the frontmost input's layer (even-odd holes deferred).
fn apply_boolean(canvas: &mut CanvasState, op: BoolOp, ids: &[Uuid]) -> Option<Command> {
    let qualifying = closed_bezier_ids_in_paint_order(canvas, ids);
    if qualifying.len() < 2 {
        return None;
    }
    // Reject if any qualifying input is locked.
    if qualifying
        .iter()
        .any(|id| canvas.document.get(*id).map(|n| n.locked).unwrap_or(true))
    {
        return None;
    }
    // Operand order = paint order (frontmost last). For Subtract/Exclude the
    // first-painted (bottom-most) operand is the base; later operands subtract.
    let inputs: Vec<(Vec<BezierNode>, bool)> = qualifying
        .iter()
        .filter_map(|id| match &canvas.document.leaf(*id)?.shape {
            ShapeData::BezierPath { nodes, closed, .. } => Some((nodes.clone(), *closed)),
            _ => None,
        })
        .collect();
    if inputs.len() < 2 {
        return None;
    }
    let result = match op {
        BoolOp::Unite => crate::pathops::unite(&inputs),
        BoolOp::Subtract => crate::pathops::subtract(&inputs),
        BoolOp::Intersect => crate::pathops::intersect(&inputs),
        BoolOp::Exclude => crate::pathops::exclude(&inputs),
    };
    // Guard: 0 contours (e.g. disjoint intersection) or degenerate → no-op.
    if result.is_empty() {
        return None;
    }
    // Placement: frontmost input's parent + index (visually-top operand's slot).
    let front = *qualifying.last().unwrap();
    let parent = canvas.document.get(front)?.parent?;
    let index = canvas.document.child_index(front)?;
    // Style template from the frontmost input.
    let template = canvas.document.leaf_to_annotation(front)?;
    let mut additions: Vec<(Annotation, Uuid, usize)> = Vec::with_capacity(result.len());
    for (i, (nodes, closed)) in result.into_iter().enumerate() {
        let mut ann = template.clone();
        ann.id = uuid::Uuid::new_v4();
        ann.shape = ShapeData::BezierPath {
            nodes,
            closed,
            fill: None,
        };
        additions.push((ann, parent, index + i));
    }
    replace_leaves(canvas, &qualifying, &additions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CanvasState;
    use crate::types::{Annotation, BezierNode, LineStyle, NodeProp, NodeType};
    use egui::{Color32, Pos2, Rect};

    fn human() -> ParticipantId {
        ParticipantId::HUMAN_LOCAL
    }

    fn push_shape(canvas: &mut CanvasState, shape: ShapeData) -> uuid::Uuid {
        let ann = Annotation::new(shape, Color32::RED, 2.0);
        canvas.add_annotation(ann)
    }

    fn shape_of(canvas: &CanvasState, id: uuid::Uuid) -> ShapeData {
        canvas
            .document
            .leaf(id)
            .expect("leaf present")
            .shape
            .clone()
    }

    #[test]
    fn document_and_mirror_stay_consistent_through_add_move_delete_undo() {
        use egui::Vec2;

        let mut c = CanvasState::default();
        let a = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))),
            Color32::RED,
            2.0,
        );
        let id = a.id;
        CommandExecutor::execute(&mut c, human(), Command::Add(a));
        assert_eq!(c.annotations.len(), 1);
        assert!(c.document.leaf(id).is_some());

        CommandExecutor::execute(
            &mut c,
            human(),
            Command::Move {
                id,
                delta: Vec2::new(5.0, 0.0),
            },
        );
        // mirror reflects moved geometry
        assert_eq!(c.annotations[0].shape.bounding_rect().min.x, 5.0);

        CommandExecutor::undo(&mut c, human()); // undo move
        assert_eq!(c.annotations[0].shape.bounding_rect().min.x, 0.0);

        CommandExecutor::execute(&mut c, human(), Command::Delete(id));
        assert!(c.annotations.is_empty());
        assert!(c.document.leaf(id).is_none());

        CommandExecutor::undo(&mut c, human()); // undo delete -> back
        assert_eq!(c.annotations.len(), 1);
        assert!(c.document.leaf(id).is_some());
    }

    /// FIX 1: Resizing a Callout and undoing must restore BOTH the original
    /// bubble rect AND the original tail exactly — not the synthetic bounding
    /// rect (which unions rect with the tail and previously corrupted undo).
    #[test]
    fn callout_resize_undo_restores_rect_and_tail() {
        let mut canvas = CanvasState::default();
        let orig_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(110.0, 70.0));
        let orig_tail = Pos2::new(5.0, 200.0); // far outside the bubble rect
        let id = push_shape(
            &mut canvas,
            ShapeData::Callout {
                rect: orig_rect,
                tail: orig_tail,
            },
        );

        let new_rect = Rect::from_min_max(Pos2::new(40.0, 50.0), Pos2::new(240.0, 150.0));
        assert!(CommandExecutor::execute(
            &mut canvas,
            human(),
            Command::Resize { id, new_rect }
        ));

        // After resize: bubble rect is new_rect, tail shifted by the min-delta.
        match shape_of(&canvas, id) {
            ShapeData::Callout { rect, tail } => {
                assert_eq!(rect, new_rect);
                let delta = new_rect.min - orig_rect.min;
                assert_eq!(tail, orig_tail + delta);
            }
            other => panic!("expected Callout, got {other:?}"),
        }

        // Undo must restore the ORIGINAL geometry exactly.
        assert!(CommandExecutor::undo(&mut canvas, human()));
        match shape_of(&canvas, id) {
            ShapeData::Callout { rect, tail } => {
                assert_eq!(rect, orig_rect, "bubble rect not restored");
                assert_eq!(tail, orig_tail, "tail not restored");
            }
            other => panic!("expected Callout, got {other:?}"),
        }

        // Redo must return to the resized geometry exactly.
        assert!(CommandExecutor::redo(&mut canvas, human()));
        match shape_of(&canvas, id) {
            ShapeData::Callout { rect, tail } => {
                assert_eq!(rect, new_rect);
                let delta = new_rect.min - orig_rect.min;
                assert_eq!(tail, orig_tail + delta);
            }
            other => panic!("expected Callout, got {other:?}"),
        }
    }

    #[test]
    fn create_then_delete_layer_round_trips() {
        let mut c = CanvasState::default();
        let layer_id = uuid::Uuid::new_v4();
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::CreateLayer {
                id: layer_id,
                name: "Layer 2".into()
            }
        ));
        assert_eq!(c.document.layers().len(), 2);
        // Put a shape into the new layer so delete must capture & restore it.
        let a = Annotation::new(ShapeData::plain_rect(Rect::ZERO), Color32::RED, 2.0);
        let aid = a.id;
        c.active_layer = layer_id;
        c.add_annotation(a);
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::DeleteLayer { id: layer_id }
        ));
        assert_eq!(c.document.layers().len(), 1);
        assert!(c.document.leaf(aid).is_none());
        // Undo restores the layer AND its shape, by id.
        assert!(CommandExecutor::undo(&mut c, human()));
        assert_eq!(c.document.layers().len(), 2);
        assert!(c.document.leaf(aid).is_some());
    }

    #[test]
    fn move_node_to_layer_round_trips() {
        let mut c = CanvasState::default();
        let l1 = c.document.default_layer();
        let l2_id = uuid::Uuid::new_v4();
        CommandExecutor::execute(
            &mut c,
            human(),
            Command::CreateLayer {
                id: l2_id,
                name: "L2".into(),
            },
        );
        let id = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO)); // in l1
        assert_eq!(c.document.get(id).unwrap().parent, Some(l1));
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::MoveNodeToLayer {
                id,
                layer_id: l2_id,
                index: 0
            }
        ));
        assert_eq!(c.document.get(id).unwrap().parent, Some(l2_id));
        assert!(CommandExecutor::undo(&mut c, human()));
        assert_eq!(c.document.get(id).unwrap().parent, Some(l1));
    }

    #[test]
    fn move_node_to_layer_into_descendant_is_noop() {
        // layer -> outer(Group) -> inner(Group) -> leaf
        let mut c = CanvasState::default();
        let layer = c.document.default_layer();
        let outer = c.document.create_group(layer, "outer", usize::MAX);
        let inner = c.document.create_group(outer, "inner", usize::MAX);
        let leaf = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO));
        // Move the leaf into `inner` first (valid) so a shape lives in the subtree.
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::MoveNodeToLayer {
                id: leaf,
                layer_id: inner,
                index: 0
            }
        ));
        assert!(c.document.check_invariants().is_ok());
        let leaves_before = c.document.descendant_leaf_ids(c.document.root);

        // Attempt the corrupting move: `outer` into its own descendant `inner`.
        assert!(!CommandExecutor::execute(
            &mut c,
            human(),
            Command::MoveNodeToLayer {
                id: outer,
                layer_id: inner,
                index: 0
            }
        ));
        // Tree unchanged: no cycle, no lost shapes, invariants hold.
        assert!(c.document.check_invariants().is_ok());
        assert_eq!(c.document.get(outer).unwrap().parent, Some(layer));
        assert_eq!(
            c.document.descendant_leaf_ids(c.document.root),
            leaves_before
        );
    }

    #[test]
    fn group_then_ungroup_round_trips() {
        let mut c = CanvasState::default();
        let layer = c.document.default_layer();
        let a = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO));
        let b = push_shape(&mut c, ShapeData::plain_oval(Rect::ZERO));
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::Group {
                ids: vec![a, b],
                name: "G".into()
            }
        ));
        // a and b now share a Group parent that is a child of the layer.
        let pa = c.document.get(a).unwrap().parent.unwrap();
        assert_eq!(c.document.get(b).unwrap().parent, Some(pa));
        assert_ne!(pa, layer);
        assert_eq!(c.document.get(pa).unwrap().parent, Some(layer));
        // Undo (Ungroup) puts them back under the layer.
        assert!(CommandExecutor::undo(&mut c, human()));
        assert_eq!(c.document.get(a).unwrap().parent, Some(layer));
        assert_eq!(c.document.get(b).unwrap().parent, Some(layer));
    }

    #[test]
    fn set_node_prop_round_trips() {
        let mut c = CanvasState::default();
        let layer = c.document.default_layer();
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::SetNodeProp {
                id: layer,
                prop: NodeProp::Opacity(0.5)
            }
        ));
        assert!((c.document.get(layer).unwrap().opacity - 0.5).abs() < 1e-6);
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::SetNodeProp {
                id: layer,
                prop: NodeProp::Name("Renamed".into())
            }
        ));
        assert_eq!(c.document.get(layer).unwrap().name, "Renamed");
        // Undo name, then opacity.
        assert!(CommandExecutor::undo(&mut c, human()));
        assert_eq!(c.document.get(layer).unwrap().name, "Layer 1");
        assert!(CommandExecutor::undo(&mut c, human()));
        assert!((c.document.get(layer).unwrap().opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn set_node_transform_applies_and_inverts() {
        use kurbo::Affine;
        let mut canvas = CanvasState::default();
        let id = canvas.document.default_layer();
        let before = canvas.document.get(id).unwrap().transform;
        let target = Affine::rotate(0.5) * Affine::scale(2.0);

        assert!(CommandExecutor::execute(
            &mut canvas,
            human(),
            Command::SetNodeProp {
                id,
                prop: NodeProp::Transform(target)
            },
        ));
        assert_eq!(canvas.document.get(id).unwrap().transform, target);

        assert!(CommandExecutor::undo(&mut canvas, human()));
        assert_eq!(canvas.document.get(id).unwrap().transform, before);
    }

    #[test]
    fn reorder_node_round_trips() {
        let mut c = CanvasState::default();
        let a = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO));
        let b = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO));
        let order =
            |c: &CanvasState| -> Vec<uuid::Uuid> { c.annotations.iter().map(|x| x.id).collect() };
        assert_eq!(order(&c), vec![a, b]);
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::ReorderNode { id: a, index: 1 }
        ));
        assert_eq!(order(&c), vec![b, a]);
        assert!(CommandExecutor::undo(&mut c, human()));
        assert_eq!(order(&c), vec![a, b]);
    }

    /// A plain Rect must also round-trip (sanity for the general getter).
    #[test]
    fn rect_resize_undo_redo_roundtrip() {
        let mut canvas = CanvasState::default();
        let orig = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(50.0, 50.0));
        let id = push_shape(&mut canvas, ShapeData::plain_rect(orig));
        let new_rect = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(90.0, 90.0));

        CommandExecutor::execute(&mut canvas, human(), Command::Resize { id, new_rect });
        CommandExecutor::undo(&mut canvas, human());
        match shape_of(&canvas, id) {
            ShapeData::Rect { rect, .. } => assert_eq!(rect, orig),
            other => panic!("expected Rect, got {other:?}"),
        }
        CommandExecutor::redo(&mut canvas, human());
        match shape_of(&canvas, id) {
            ShapeData::Rect { rect, .. } => assert_eq!(rect, new_rect),
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    /// FIX 2: ReorderTo forward+inverse must restore the EXACT original vec
    /// ordering for arbitrary (from, to) index pairs, and forward+undo+redo
    /// must return to the forward ordering.
    #[test]
    fn reorder_to_arbitrary_permutation_roundtrip() {
        // Build a canvas with distinct shapes and capture their ids in order.
        fn fresh() -> (CanvasState, Vec<uuid::Uuid>) {
            let mut canvas = CanvasState::default();
            let mut ids = Vec::new();
            for i in 0..5 {
                let r =
                    Rect::from_min_max(Pos2::new(i as f32, 0.0), Pos2::new(i as f32 + 1.0, 1.0));
                ids.push(push_shape(&mut canvas, ShapeData::plain_rect(r)));
            }
            (canvas, ids)
        }

        let order =
            |c: &CanvasState| -> Vec<uuid::Uuid> { c.annotations.iter().map(|a| a.id).collect() };

        // Several (from, to) permutations, including clamped out-of-range.
        let pairs = [
            (0, 2),
            (3, 1),
            (4, 0),
            (1, 4),
            (2, 2),
            (0, 99),
            (4, 0),
            (1, 3),
        ];

        for (from, to) in pairs {
            let (mut canvas, ids) = fresh();
            let original = order(&canvas);
            let moved_id = ids[from];

            // Forward.
            assert!(CommandExecutor::execute(
                &mut canvas,
                human(),
                Command::ReorderTo {
                    id: moved_id,
                    index: to,
                }
            ));
            let forward = order(&canvas);

            // Undo (applies the recorded inverse) must restore the original.
            assert!(CommandExecutor::undo(&mut canvas, human()));
            assert_eq!(
                order(&canvas),
                original,
                "forward+undo failed to restore original for ({from},{to})"
            );

            // Redo must return to the forward ordering.
            assert!(CommandExecutor::redo(&mut canvas, human()));
            assert_eq!(
                order(&canvas),
                forward,
                "forward+undo+redo did not return to forward order for ({from},{to})"
            );
        }
    }

    #[test]
    fn delete_group_removes_subtree_and_undo_restores() {
        let mut c = CanvasState::default();
        // Two leaves under the default layer, grouped together.
        let a = push_shape(&mut c, ShapeData::plain_rect(Rect::ZERO));
        let b = push_shape(&mut c, ShapeData::plain_oval(Rect::ZERO));
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::Group {
                ids: vec![a, b],
                name: "G".into()
            }
        ));
        // The group is the parent of a and b.
        let gid = c.document.get(a).unwrap().parent.unwrap();
        assert!(matches!(
            c.document.get(gid).unwrap().kind,
            crate::scene::NodeKind::Group
        ));

        // Delete the group → both leaves gone.
        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::Delete(gid)
        ));
        assert!(c.document.get(gid).is_none());
        assert!(c.document.get(a).is_none());
        assert!(c.document.get(b).is_none());

        // Undo → group and leaves restored.
        CommandExecutor::undo(&mut c, human());
        assert!(c.document.get(gid).is_some());
        assert!(c.document.get(a).is_some());
        assert!(c.document.get(b).is_some());

        // Redo re-applies the group deletion (group redo routes through the
        // generic DeleteLayer path, which is a no-op-safe subtree removal).
        CommandExecutor::redo(&mut c, human());
        assert!(
            c.document.get(gid).is_none(),
            "redo should remove the group again"
        );
        assert!(c.document.get(a).is_none());
        assert!(c.document.get(b).is_none());
    }

    #[test]
    fn flatten_to_image_merges_then_undo_restores() {
        let mut c = CanvasState {
            color_image: Some(egui::ColorImage::new([8, 8], egui::Color32::WHITE)),
            canvas_size: egui::vec2(8.0, 8.0),
            ..Default::default()
        };
        c.sync_annotations();
        let a = push_shape(
            &mut c,
            ShapeData::plain_rect(Rect::from_min_size(
                Pos2::new(1.0, 1.0),
                egui::Vec2::splat(3.0),
            )),
        );
        let b = push_shape(
            &mut c,
            ShapeData::plain_oval(Rect::from_min_size(
                Pos2::new(4.0, 4.0),
                egui::Vec2::splat(3.0),
            )),
        );
        let before_leaves = c.document.leaves_in_paint_order().len();
        assert!(before_leaves >= 2);

        assert!(CommandExecutor::execute(
            &mut c,
            human(),
            Command::FlattenToImage
        ));
        // After flatten: no shape leaves remain, base image still present.
        assert_eq!(c.document.leaves_in_paint_order().len(), 0);
        assert!(c.color_image.is_some());

        // Undo restores the leaves.
        CommandExecutor::undo(&mut c, human());
        assert_eq!(c.document.leaves_in_paint_order().len(), before_leaves);
        assert!(c.document.get(a).is_some());
        assert!(c.document.get(b).is_some());

        // Redo re-applies the flatten: leaves removed again, image is the flattened one.
        CommandExecutor::redo(&mut c, human());
        assert_eq!(
            c.document.leaves_in_paint_order().len(),
            0,
            "redo should re-merge leaves"
        );
        assert!(c.color_image.is_some());
    }
}

#[cfg(feature = "agent-server")]
fn broadcast_latest_event(canvas: &mut crate::state::CanvasState) {
    let Some(sender) = canvas.event_broadcast.as_ref() else {
        return;
    };
    let Some(latest) = canvas.event_log.back() else {
        return;
    };

    // Phase 3: CanvasState doesn't carry a stable room_id yet, so use Uuid::nil()
    // as a placeholder. The per-session subscriber knows which room it serves and
    // can ignore this field. Phase 4 generalizes to multiple rooms.
    let room_id = uuid::Uuid::nil();

    // Need to look up the post-mutation annotation for ShapeAdded events.
    // Build the find_shape closure with what we have access to.
    let annotations = &canvas.annotations;
    let event = crate::agent::events::ServerEvent::from_canvas_event(room_id, latest, |id| {
        annotations
            .iter()
            .find(|a| a.id == id)
            .map(crate::agent::handlers::annotation_to_descriptor)
    });

    // Best-effort send. Returns Err only when there are no subscribers; that's fine.
    let _ = sender.send(event);
}
