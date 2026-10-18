//! Generic action policy helpers for confidence-driven local AI behavior.

use crate::local_ai::analysis::CanvasAnalysis;
use crate::local_ai::decision::DecisionAction;
use crate::local_ai::roles::AgentRole;

pub fn generic_clarification_only(actions: &[DecisionAction]) -> bool {
    actions.len() == 1
        && actions[0].tool == "draw_ask"
        && actions[0]
            .args
            .get("content")
            .and_then(|value| value.as_str())
            .map(is_generic_clarification)
            .unwrap_or(false)
}

pub fn should_drop_generic_clarification(
    analysis: &CanvasAnalysis,
    role: AgentRole,
    actions: &[DecisionAction],
    user_message_present: bool,
) -> bool {
    !user_message_present
        && analysis.has_strong_hypothesis()
        && !matches!(role, AgentRole::Observer)
        && generic_clarification_only(actions)
}

fn is_generic_clarification(content: &str) -> bool {
    let content = content.to_ascii_lowercase();
    [
        "what are you drawing",
        "what are you sketching",
        "what are you building",
        "what is this",
        "what are you making",
    ]
    .iter()
    .any(|pattern| content.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_ai::analysis::{AnalysisHypothesis, AnalysisStrength, CanvasAnalysis};

    #[test]
    fn drops_generic_question_when_strong_hypothesis_exists() {
        let analysis = CanvasAnalysis {
            features: vec![],
            hypotheses: vec![AnalysisHypothesis {
                summary: "A structured multi-cell board or table is present".into(),
                strength: AnalysisStrength::Strong,
                evidence: vec![],
            }],
        };
        let actions = vec![DecisionAction {
            tool: "draw_ask".into(),
            args: serde_json::json!({"content":"What are you sketching?"}),
        }];
        assert!(should_drop_generic_clarification(
            &analysis,
            AgentRole::GamePlayer,
            &actions,
            false,
        ));
    }
}
