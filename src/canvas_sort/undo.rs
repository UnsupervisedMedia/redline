use egui::Rect;
use uuid::Uuid;

use super::types::{ActionZone, CanvasSortSession, ImageStack, SortImage};

#[derive(Debug, Clone)]
pub enum CanvasCommand {
    MoveImages {
        /// (id, from, to)
        moves: Vec<(Uuid, [f32; 2], [f32; 2])>,
    },
    RemoveImages {
        images: Vec<SortImage>,
    },
    AddImages {
        images: Vec<SortImage>,
    },
    CreateStack {
        stack: ImageStack,
        /// (image_id, old_stack_id) for images pulled into the new stack.
        moved: Vec<(Uuid, Option<Uuid>)>,
    },
    DeleteStack {
        stack: ImageStack,
    },
    MergeStacks {
        source: ImageStack,
        target_id: Uuid,
        target_image_ids_before: Vec<Uuid>,
    },
    UnmergeStacks {
        source: ImageStack,
        target_id: Uuid,
        target_image_ids_before: Vec<Uuid>,
    },
    CreateZone {
        zone: ActionZone,
    },
    DeleteZone {
        zone: ActionZone,
    },
    MoveZone {
        id: Uuid,
        from: Rect,
        to: Rect,
    },
    ResizeZone {
        id: Uuid,
        from: Rect,
        to: Rect,
    },
    SortImages {
        /// (id, from, to)
        moves: Vec<(Uuid, [f32; 2], [f32; 2])>,
    },
    EnterZone {
        image_ids: Vec<Uuid>,
        zone_id: Uuid,
    },
    LeaveZone {
        image_ids: Vec<Uuid>,
        zone_id: Uuid,
    },
    UpdateZone {
        zone: ActionZone,
    },
}

impl CanvasCommand {
    pub fn label(&self) -> &'static str {
        match self {
            CanvasCommand::MoveImages { .. } => "Move images",
            CanvasCommand::RemoveImages { .. } => "Remove images",
            CanvasCommand::AddImages { .. } => "Add images",
            CanvasCommand::CreateStack { .. } => "Create stack",
            CanvasCommand::DeleteStack { .. } => "Delete stack",
            CanvasCommand::MergeStacks { .. } => "Merge stacks",
            CanvasCommand::UnmergeStacks { .. } => "Unmerge stacks",
            CanvasCommand::CreateZone { .. } => "Create zone",
            CanvasCommand::DeleteZone { .. } => "Delete zone",
            CanvasCommand::MoveZone { .. } => "Move zone",
            CanvasCommand::ResizeZone { .. } => "Resize zone",
            CanvasCommand::SortImages { .. } => "Sort images",
            CanvasCommand::EnterZone { .. } => "Enter zone",
            CanvasCommand::LeaveZone { .. } => "Leave zone",
            CanvasCommand::UpdateZone { .. } => "Update zone",
        }
    }
}

/// Apply `cmd` to `session`, push its inverse onto `undo_stack`, clear `redo_stack`.
pub fn apply_command(
    session: &mut Option<CanvasSortSession>,
    cmd: CanvasCommand,
    undo_stack: &mut Vec<CanvasCommand>,
    redo_stack: &mut Vec<CanvasCommand>,
) {
    let Some(s) = session else { return };
    let inverse = apply(s, cmd);
    if undo_stack.len() >= 50 {
        undo_stack.remove(0);
    }
    undo_stack.push(inverse);
    redo_stack.clear();
}

pub fn undo_command(
    session: &mut Option<CanvasSortSession>,
    undo_stack: &mut Vec<CanvasCommand>,
    redo_stack: &mut Vec<CanvasCommand>,
) {
    let Some(s) = session else { return };
    if let Some(cmd) = undo_stack.pop() {
        let inverse = apply(s, cmd);
        redo_stack.push(inverse);
    }
}

pub fn redo_command(
    session: &mut Option<CanvasSortSession>,
    undo_stack: &mut Vec<CanvasCommand>,
    redo_stack: &mut Vec<CanvasCommand>,
) {
    let Some(s) = session else { return };
    if let Some(cmd) = redo_stack.pop() {
        let inverse = apply(s, cmd);
        undo_stack.push(inverse);
    }
}

fn apply(s: &mut CanvasSortSession, cmd: CanvasCommand) -> CanvasCommand {
    match cmd {
        CanvasCommand::MoveImages { moves } => {
            for (id, _from, to) in &moves {
                if let Some(img) = s.image_by_id_mut(*id) {
                    img.pos = *to;
                }
            }
            CanvasCommand::MoveImages {
                moves: moves
                    .iter()
                    .map(|(id, from, to)| (*id, *to, *from))
                    .collect(),
            }
        }

        CanvasCommand::RemoveImages { images } => {
            let ids: std::collections::HashSet<Uuid> = images.iter().map(|i| i.id).collect();
            s.images.retain(|i| !ids.contains(&i.id));
            CanvasCommand::AddImages { images }
        }

        CanvasCommand::AddImages { images } => {
            s.images.extend(images.clone());
            CanvasCommand::RemoveImages { images }
        }

        CanvasCommand::CreateStack { stack, moved } => {
            for (img_id, _old_stack) in &moved {
                if let Some(img) = s.image_by_id_mut(*img_id) {
                    img.stack_id = Some(stack.id);
                }
            }
            s.stacks.push(stack.clone());
            CanvasCommand::DeleteStack { stack }
        }

        CanvasCommand::DeleteStack { stack } => {
            s.stacks.retain(|st| st.id != stack.id);
            for img_id in &stack.image_ids {
                if let Some(img) = s.image_by_id_mut(*img_id) {
                    img.stack_id = None;
                }
            }
            CanvasCommand::CreateStack {
                moved: stack
                    .image_ids
                    .iter()
                    .map(|id| (*id, Some(stack.id)))
                    .collect(),
                stack,
            }
        }

        CanvasCommand::MergeStacks {
            source,
            target_id,
            target_image_ids_before,
        } => {
            // Absorb source images into target.
            if let Some(target) = s.stack_by_id_mut(target_id) {
                target.image_ids.extend_from_slice(&source.image_ids);
            }
            for img_id in &source.image_ids {
                if let Some(img) = s.image_by_id_mut(*img_id) {
                    img.stack_id = Some(target_id);
                }
            }
            s.stacks.retain(|st| st.id != source.id);
            CanvasCommand::UnmergeStacks {
                source,
                target_id,
                target_image_ids_before,
            }
        }

        CanvasCommand::UnmergeStacks {
            source,
            target_id,
            target_image_ids_before,
        } => {
            // Restore the target's pre-merge image list.
            if let Some(target) = s.stack_by_id_mut(target_id) {
                target.image_ids = target_image_ids_before.clone();
            }
            // Move the source's images back under the source stack.
            for img_id in &source.image_ids {
                if let Some(img) = s.image_by_id_mut(*img_id) {
                    img.stack_id = Some(source.id);
                }
            }
            // Re-insert the source stack.
            s.stacks.push(source.clone());
            CanvasCommand::MergeStacks {
                source,
                target_id,
                target_image_ids_before,
            }
        }

        CanvasCommand::CreateZone { zone } => {
            s.zones.push(zone.clone());
            CanvasCommand::DeleteZone { zone }
        }

        CanvasCommand::DeleteZone { zone } => {
            s.zones.retain(|z| z.id != zone.id);
            CanvasCommand::CreateZone { zone }
        }

        CanvasCommand::MoveZone { id, from, to } => {
            if let Some(z) = s.zone_by_id_mut(id) {
                z.set_rect(to);
            }
            CanvasCommand::MoveZone {
                id,
                from: to,
                to: from,
            }
        }

        CanvasCommand::ResizeZone { id, from, to } => {
            if let Some(z) = s.zone_by_id_mut(id) {
                z.set_rect(to);
            }
            CanvasCommand::ResizeZone {
                id,
                from: to,
                to: from,
            }
        }

        CanvasCommand::SortImages { moves } => {
            for (id, _from, to) in &moves {
                if let Some(img) = s.image_by_id_mut(*id) {
                    img.pos = *to;
                }
            }
            CanvasCommand::SortImages {
                moves: moves
                    .iter()
                    .map(|(id, from, to)| (*id, *to, *from))
                    .collect(),
            }
        }

        CanvasCommand::EnterZone { image_ids, zone_id } => {
            if let Some(z) = s.zone_by_id_mut(zone_id) {
                for id in &image_ids {
                    if !z.contained_image_ids.contains(id) {
                        z.contained_image_ids.push(*id);
                    }
                }
            }
            CanvasCommand::LeaveZone { image_ids, zone_id }
        }

        CanvasCommand::LeaveZone { image_ids, zone_id } => {
            if let Some(z) = s.zone_by_id_mut(zone_id) {
                z.contained_image_ids.retain(|id| !image_ids.contains(id));
            }
            CanvasCommand::EnterZone { image_ids, zone_id }
        }

        CanvasCommand::UpdateZone { zone } => {
            let old = s.zone_by_id(zone.id).cloned();
            if let Some(z) = s.zone_by_id_mut(zone.id) {
                *z = zone.clone();
            }
            CanvasCommand::UpdateZone {
                zone: old.unwrap_or(zone),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_sort::types::{CanvasSortSession, ImageStack, SortImage};
    use egui::Pos2;

    fn push_img(s: &mut CanvasSortSession, stack_id: Option<uuid::Uuid>) -> uuid::Uuid {
        let mut i = SortImage::new(std::path::PathBuf::from("x.png"), [0u8; 32]);
        i.stack_id = stack_id;
        let id = i.id;
        s.images.push(i);
        id
    }

    #[test]
    fn merge_then_undo_restores_both_stacks_exactly() {
        let mut s = CanvasSortSession::new("t");
        // Target stack B with one image.
        let mut b = ImageStack::new(Pos2::new(0.0, 0.0));
        let b_img = push_img(&mut s, Some(b.id));
        b.image_ids = vec![b_img];
        let b_id = b.id;
        s.stacks.push(b);
        // Source stack A with two images.
        let mut a = ImageStack::new(Pos2::new(10.0, 10.0));
        let a1 = push_img(&mut s, Some(a.id));
        let a2 = push_img(&mut s, Some(a.id));
        a.image_ids = vec![a1, a2];
        let a_id = a.id;
        let a_clone = a.clone();
        s.stacks.push(a);

        // Forward: merge A (source) into B (target).
        let inverse = apply(
            &mut s,
            CanvasCommand::MergeStacks {
                source: a_clone,
                target_id: b_id,
                target_image_ids_before: vec![b_img],
            },
        );

        // After merge: A gone, B holds 3, all three images point at B.
        assert!(s.stacks.iter().find(|st| st.id == a_id).is_none());
        let b_after = s.stacks.iter().find(|st| st.id == b_id).unwrap();
        assert_eq!(b_after.image_ids, vec![b_img, a1, a2]);
        assert_eq!(s.image_by_id(a1).unwrap().stack_id, Some(b_id));

        // Undo: apply the inverse → A restored, B back to one, memberships restored.
        apply(&mut s, inverse);
        let b_back = s.stacks.iter().find(|st| st.id == b_id).unwrap();
        assert_eq!(b_back.image_ids, vec![b_img]);
        let a_back = s
            .stacks
            .iter()
            .find(|st| st.id == a_id)
            .expect("source stack restored");
        assert_eq!(a_back.image_ids, vec![a1, a2]);
        assert_eq!(s.image_by_id(a1).unwrap().stack_id, Some(a_id));
        assert_eq!(s.image_by_id(a2).unwrap().stack_id, Some(a_id));
    }
}
