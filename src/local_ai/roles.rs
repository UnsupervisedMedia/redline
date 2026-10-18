//! Agent role presets. Each role bundles a budget config, a prompt prefix,
//! and an allowed-action subset. Selected from the UI dropdown; persisted
//! in canvas-scoped agent_memory under a known Lesson entry.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::local_ai::budget::BudgetConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentRole {
    Observer,
    #[default]
    CollaboratorLight,
    CollaboratorHeavy,
    Curator,
    GamePlayer,
}

impl AgentRole {
    pub fn label(&self) -> &'static str {
        match self {
            AgentRole::Observer => "Observer",
            AgentRole::CollaboratorLight => "Collaborator-Light",
            AgentRole::CollaboratorHeavy => "Collaborator-Heavy",
            AgentRole::Curator => "Curator",
            AgentRole::GamePlayer => "Game-Player",
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self {
            AgentRole::Observer => "Observe",
            AgentRole::CollaboratorLight => "Interactive",
            AgentRole::CollaboratorHeavy => "Freeform",
            AgentRole::Curator => "Curate",
            AgentRole::GamePlayer => "Game",
        }
    }

    pub fn mode_description(&self) -> &'static str {
        match self {
            AgentRole::Observer => "Comment and ask, but do not draw.",
            AgentRole::CollaboratorLight => "React conversationally and prefer restrained assists.",
            AgentRole::CollaboratorHeavy => "Draw and modify proactively when intent is clear.",
            AgentRole::Curator => "Organize and refine what is already on the canvas.",
            AgentRole::GamePlayer => "Wait for completed moves, then play clearly.",
        }
    }

    pub fn budget(&self) -> BudgetConfig {
        match self {
            AgentRole::Observer => BudgetConfig {
                act_max: 0,
                act_refill_secs: 600,
                ask_max: 10,
                ask_refill_secs: 30,
                bookmark_max: 30,
                bookmark_refill_secs: 10,
            },
            AgentRole::CollaboratorLight => BudgetConfig {
                act_max: 10,
                act_refill_secs: 30,
                ask_max: 10,
                ask_refill_secs: 30,
                bookmark_max: 30,
                bookmark_refill_secs: 10,
            },
            AgentRole::CollaboratorHeavy => BudgetConfig {
                act_max: 30,
                act_refill_secs: 10,
                ask_max: 15,
                ask_refill_secs: 20,
                bookmark_max: 30,
                bookmark_refill_secs: 10,
            },
            AgentRole::Curator => BudgetConfig {
                act_max: 15,
                act_refill_secs: 20,
                ask_max: 10,
                ask_refill_secs: 30,
                bookmark_max: 30,
                bookmark_refill_secs: 10,
            },
            AgentRole::GamePlayer => BudgetConfig {
                act_max: 20,
                act_refill_secs: 15,
                ask_max: 10,
                ask_refill_secs: 30,
                bookmark_max: 30,
                bookmark_refill_secs: 10,
            },
        }
    }

    pub fn prompt_prefix(&self) -> &'static str {
        match self {
            AgentRole::Observer => "You are a thoughtful consultant. You do not draw or modify shapes. You ask clarifying questions, leave brief text notes, and bookmark areas of interest. Restraint is your default; speak only when you can add real value.",
            AgentRole::CollaboratorLight => "You are a quiet collaborator. Suggest more than you commit. When unsure, ask via draw_ask or propose via draw_propose instead of acting outright, but if a common structure clearly implies intent then make the best assumption instead of asking a generic question. Honor restraint — abstaining is a normal, valued output.",
            AgentRole::CollaboratorHeavy => "You are an active collaborator. Take initiative on visible gaps in the user's work, but justify every action in your `thinking` field before dispatch. Watch for undos and adjust.",
            AgentRole::Curator => "You organize, label, and group what already exists on the canvas. You do not create new shapes. Your tools are modify and label, not draw.",
            AgentRole::GamePlayer => "You are participating in an inferred turn-based task on the canvas. React only to deliberate user moves (not mid-stroke pen events). Use the structured analysis to infer turns, slots, or cells without hardcoded game rules. Optimize for clarity over speed.",
        }
    }

    pub fn reaction_quiet_period(&self) -> Duration {
        match self {
            AgentRole::Observer => Duration::from_millis(1600),
            AgentRole::CollaboratorLight => Duration::from_millis(1400),
            AgentRole::CollaboratorHeavy => Duration::from_millis(900),
            AgentRole::Curator => Duration::from_millis(1400),
            AgentRole::GamePlayer => Duration::from_millis(2200),
        }
    }

    pub fn idle_followup_period(&self) -> Duration {
        match self {
            AgentRole::Observer => Duration::from_secs(8),
            AgentRole::CollaboratorLight => Duration::from_secs(6),
            AgentRole::CollaboratorHeavy => Duration::from_secs(5),
            AgentRole::Curator => Duration::from_secs(6),
            AgentRole::GamePlayer => Duration::from_secs(7),
        }
    }

    /// Returns true if this role is allowed to dispatch the named action.
    /// Unknown action names default to allowed (forward-compatible).
    pub fn allows(&self, action: &str) -> bool {
        match self {
            AgentRole::Observer => matches!(
                action,
                "draw_ask"
                    | "draw_text"
                    | "add_bookmark"
                    | "remove_bookmark"
                    | "record_tool_wish"
                    | "record_lesson"
                    | "record_private_note"
                    | "append_shared_note"
                    | "delete_shared_note"
            ),
            AgentRole::CollaboratorLight | AgentRole::CollaboratorHeavy | AgentRole::GamePlayer => {
                true
            }
            AgentRole::Curator => matches!(
                action,
                // Modify existing shapes — the Curator's primary surface.
                "move_shape"
                    | "resize_shape"
                    | "set_prop"
                    | "set_text_content"
                    | "replace_pen_points"
                    | "delete_shape"
                    | "bring_to_front"
                    | "send_to_back"
                    | "reorder_to"
                    | "renumber_steps"
                    | "undo"
                    | "redo"
                    // Ask/propose — the Curator may question but not create outright.
                    | "draw_ask"
                    | "draw_propose"
                    // Memory and bookmarking are always available.
                    | "add_bookmark"
                    | "remove_bookmark"
                    | "record_tool_wish"
                    | "record_lesson"
                    | "record_private_note"
                    | "append_shared_note"
                    | "delete_shared_note"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_disallows_drawing() {
        let r = AgentRole::Observer;
        assert!(!r.allows("draw_arrow"));
        assert!(!r.allows("draw_rect"));
        assert!(r.allows("draw_ask"));
        assert!(r.allows("add_bookmark"));
        assert_eq!(r.budget().act_max, 0);
    }

    #[test]
    fn curator_disallows_new_draws_but_allows_modify() {
        let r = AgentRole::Curator;
        assert!(!r.allows("draw_arrow"));
        assert!(!r.allows("draw_rect"));
        assert!(!r.allows("draw_pen"));
        assert!(!r.allows("draw_text"));
        assert!(!r.allows("draw_oval"));
        assert!(!r.allows("draw_line"));
        assert!(!r.allows("draw_star"));
        assert!(!r.allows("draw_callout"));
        assert!(!r.allows("draw_rounded_rect"));
        assert!(!r.allows("draw_highlighter"));
        // Modify surface must be allowed.
        assert!(r.allows("move_shape"));
        assert!(r.allows("resize_shape"));
        assert!(r.allows("set_text_content"));
        assert!(r.allows("set_prop"));
        assert!(r.allows("delete_shape"));
        assert!(r.allows("bring_to_front"));
        assert!(r.allows("send_to_back"));
        assert!(r.allows("reorder_to"));
        assert!(r.allows("renumber_steps"));
        assert!(r.allows("undo"));
        assert!(r.allows("redo"));
        // Ask/propose allowed; outright draw blocked.
        assert!(r.allows("draw_ask"));
        assert!(r.allows("draw_propose"));
        // Unknown future tools are NOT allowed by the explicit allowlist.
        assert!(!r.allows("some_future_tool"));
    }

    #[test]
    fn collaborator_heavy_allows_everything() {
        let r = AgentRole::CollaboratorHeavy;
        for action in [
            "draw_arrow",
            "draw_propose",
            "draw_ask",
            "move_shape",
            "delete_shape",
        ] {
            assert!(r.allows(action), "{action}");
        }
    }
}
