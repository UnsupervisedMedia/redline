use crate::command::CommandExecutor;
use crate::state::CanvasState;
use crate::types::ParticipantId;

use super::mask::{MaskBuffer, MaskPatch};

/// One reversible action performed in the Native Inpaint panel.
pub enum InpaintAction {
    /// A mask change (one brush stroke / wand click / clear), stored as a patch.
    MaskEdit(MaskPatch),
    /// A committed Remove/Generate result, owned by the canvas command stack.
    AppliedResult,
}

/// Unlimited undo + full redo for the Native Inpaint panel. Mask edits are
/// applied to the panel's [`MaskBuffer`]; applied results are delegated to the
/// canvas [`CommandExecutor`] (which already stores the layer images).
#[derive(Default)]
pub struct InpaintHistory {
    undo: Vec<InpaintAction>,
    redo: Vec<InpaintAction>,
}

impl InpaintHistory {
    /// Record a new action, discarding any pending redo branch.
    pub fn push(&mut self, action: InpaintAction) {
        self.undo.push(action);
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Drop all history (e.g. when the active document changes).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Reverse the most recent action.
    pub fn undo(&mut self, mask: Option<&mut MaskBuffer>, canvas: &mut CanvasState) {
        let Some(action) = self.undo.pop() else {
            return;
        };
        match &action {
            InpaintAction::MaskEdit(p) => {
                if let Some(m) = mask {
                    m.apply_region(p.x, p.y, p.w, p.h, &p.before);
                }
            }
            InpaintAction::AppliedResult => {
                CommandExecutor::undo(canvas, ParticipantId::HUMAN_LOCAL);
            }
        }
        self.redo.push(action);
    }

    /// Replay the most recently undone action.
    pub fn redo(&mut self, mask: Option<&mut MaskBuffer>, canvas: &mut CanvasState) {
        let Some(action) = self.redo.pop() else {
            return;
        };
        match &action {
            InpaintAction::MaskEdit(p) => {
                if let Some(m) = mask {
                    m.apply_region(p.x, p.y, p.w, p.h, &p.after);
                }
            }
            InpaintAction::AppliedResult => {
                CommandExecutor::redo(canvas, ParticipantId::HUMAN_LOCAL);
            }
        }
        self.undo.push(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandExecutor;
    use crate::native_inpaint::mask::{MaskBuffer, MaskPatch};
    use crate::state::CanvasState;
    use crate::types::{Annotation, Command, ParticipantId, ShapeData};
    use eframe::egui::{Color32, Pos2, Rect, Vec2};

    fn mask_edit_patch() -> (MaskBuffer, MaskPatch) {
        let mut m = MaskBuffer::new(8, 8);
        let before = m.image().clone();
        m.stamp(4.0, 4.0, 2.0, true);
        let patch = MaskPatch::between(&before, m.image()).expect("a change");
        (m, patch)
    }

    #[test]
    fn push_clears_redo() {
        let mut h = InpaintHistory::default();
        let mut canvas = CanvasState::default();
        let (_, p) = mask_edit_patch();
        h.push(InpaintAction::MaskEdit(p.clone()));
        h.undo(None, &mut canvas); // moves the entry to the redo stack
        assert!(h.can_redo());
        h.push(InpaintAction::MaskEdit(p)); // a new action discards redo
        assert!(!h.can_redo(), "a new push clears the redo branch");
    }

    #[test]
    fn mask_edit_undo_redo_roundtrips_the_mask() {
        let (mut mask, patch) = mask_edit_patch();
        assert!(mask.any_set());
        let mut h = InpaintHistory::default();
        let mut canvas = CanvasState::default();
        h.push(InpaintAction::MaskEdit(patch));
        h.undo(Some(&mut mask), &mut canvas);
        assert!(!mask.any_set(), "undo clears the painted disc");
        h.redo(Some(&mut mask), &mut canvas);
        assert!(mask.any_set(), "redo restores it");
    }

    #[test]
    fn applied_result_delegates_to_command_executor() {
        let mut canvas = CanvasState::default();
        let ann = Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(4.0, 4.0))),
            Color32::RED,
            2.0,
        );
        let id = ann.id;
        CommandExecutor::execute(&mut canvas, ParticipantId::HUMAN_LOCAL, Command::Add(ann));
        assert!(
            canvas.document.leaf(id).is_some(),
            "layer present after commit"
        );

        let mut h = InpaintHistory::default();
        h.push(InpaintAction::AppliedResult);
        h.undo(None, &mut canvas);
        assert!(canvas.document.leaf(id).is_none(), "undo removes the layer");
        h.redo(None, &mut canvas);
        assert!(
            canvas.document.leaf(id).is_some(),
            "redo restores the layer"
        );
    }

    #[test]
    fn empty_undo_and_redo_are_noops() {
        let mut h = InpaintHistory::default();
        let mut canvas = CanvasState::default();
        h.undo(None, &mut canvas); // must not panic
        h.redo(None, &mut canvas);
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn clear_empties_both_stacks() {
        let mut h = InpaintHistory::default();
        h.push(InpaintAction::AppliedResult);
        h.clear();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }
}
