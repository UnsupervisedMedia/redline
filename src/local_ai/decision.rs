//! Decision pass: assemble prompt, POST to Ollama with strict-JSON format,
//! parse the response, enforce the structural justification rule, return a
//! list of (action_name, args) tuples for the agent loop to dispatch.

use serde::Deserialize;

use crate::local_ai::ollama::{
    schema_for_decision, ChatMessage, ChatRequest, OllamaClient, OllamaError,
};
use crate::local_ai::roles::AgentRole;

#[derive(Debug, Clone, Deserialize)]
pub struct Decision {
    pub thinking: String,
    pub scratchpad_update: String,
    pub actions: Vec<DecisionAction>,
    pub private_notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecisionAction {
    pub tool: String,
    pub args: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("ollama: {0}")]
    Ollama(#[from] OllamaError),
    #[error("schema: {0}")]
    Schema(String),
}

pub struct DecisionInput<'a> {
    pub model: &'a str,
    pub system_prompt: &'a str,
    pub behavioral_summary: &'a str,
    pub scratchpad: &'a str,
    pub recent_decisions_summary: &'a str,
    pub recent_events_summary: &'a str,
    pub canvas_state_json: &'a str,
    pub image_base64: Option<String>,
    pub user_instruction: &'a str,
}

pub async fn run(
    client: &OllamaClient,
    input: DecisionInput<'_>,
) -> Result<Decision, DecisionError> {
    let schema = schema_for_decision();
    let user_content = format!(
        "{}\n\n{}\n\nScratchpad:\n{}\n\nRecent decisions:\n{}\n\nRecent events:\n{}\n\nCurrent canvas:\n{}\n",
        input.user_instruction,
        input.behavioral_summary,
        input.scratchpad,
        input.recent_decisions_summary,
        input.recent_events_summary,
        input.canvas_state_json,
    );
    let has_image = input.image_base64.is_some();
    let mut req = ChatRequest::new(input.model)
        .system(input.system_prompt)
        .user(user_content)
        .with_format(schema.clone())
        .with_max_tokens(4096);
    if let Some(b64) = input.image_base64 {
        req = req.with_image_base64(b64);
    }
    let primary_err = match client.chat(&req).await {
        Ok(resp) => match parse_decision_content(&resp.message.content) {
            Ok(parsed) => return Ok(parsed),
            Err(err) => format!("primary invalid: {err}"),
        },
        Err(err) => format!("primary request failed: {err}"),
    };

    let retry = retry_request_without_schema(&req, &schema, has_image);
    let retry_resp = client.chat(&retry).await.map_err(|retry_err| {
        DecisionError::Schema(format!("{primary_err}; retry request failed: {retry_err}"))
    })?;
    parse_decision_content(&retry_resp.message.content).map_err(|retry_err| {
        DecisionError::Schema(format!("{primary_err}; retry invalid: {retry_err}"))
    })
}

fn retry_request_without_schema(
    req: &ChatRequest,
    schema: &serde_json::Value,
    drop_images: bool,
) -> ChatRequest {
    let mut retry = req.clone();
    retry.format = None;
    if drop_images {
        for message in &mut retry.messages {
            message.images.clear();
        }
    }
    retry.messages.push(ChatMessage {
        role: "user".into(),
        content: format!(
            "Your previous reply failed.\n\
Return exactly one JSON object matching this schema and nothing else:\n{}\n\
Do not use markdown fences, commentary, or extra text. {}",
            schema,
            if drop_images {
                "Use the canvas JSON and recent context only; no image is attached on this retry."
            } else {
                "Use the provided context only."
            }
        ),
        images: Vec::new(),
    });
    retry
}

fn parse_decision_content(content: &str) -> Result<Decision, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("parse: empty content".into());
    }

    if let Ok(parsed) = serde_json::from_str::<Decision>(trimmed) {
        return Ok(parsed);
    }

    if let Some(unfenced) = strip_code_fences(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<Decision>(&unfenced) {
            return Ok(parsed);
        }
    }

    if let Some(extracted) = extract_first_json_object(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<Decision>(&extracted) {
            return Ok(parsed);
        }
    }

    match serde_json::from_str::<Decision>(trimmed) {
        Ok(parsed) => Ok(parsed),
        Err(e) => Err(format!(
            "parse: {e}; content: {}",
            summarize_for_error(trimmed)
        )),
    }
}

fn strip_code_fences(content: &str) -> Option<String> {
    let stripped = content.strip_prefix("```")?;
    let stripped = stripped.trim_start();
    let after_lang = match stripped.find('\n') {
        Some(idx) => &stripped[idx + 1..],
        None => return None,
    };
    let inner = after_lang.strip_suffix("```")?.trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

fn extract_first_json_object(content: &str) -> Option<String> {
    let start = content.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in content[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(content[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn summarize_for_error(content: &str) -> String {
    let compact = content.replace('\n', "\\n");
    if compact.chars().count() <= 240 {
        compact
    } else {
        let prefix: String = compact.chars().take(240).collect();
        format!("{prefix}…")
    }
}

/// Filter decision actions: enforce role allow-list and require the model
/// to have written *something* in `thinking` (non-empty, ≥20 chars). The
/// strict-JSON schema enforcement at the Ollama layer already prevents
/// malformed/hallucinated actions, so we only need the lightest possible
/// CoT guarantee here — not a substring check on the literal tool name,
/// which over-rejects natural prose like "I'll propose the center move".
pub fn enforce_justification(
    decision: &Decision,
    role: AgentRole,
) -> (Vec<DecisionAction>, Vec<String>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let thinking_ok = decision.thinking.trim().chars().count() >= 8;
    for action in &decision.actions {
        if !role.allows(&action.tool) {
            dropped.push(format!("dropped (role disallows): {}", action.tool));
            continue;
        }
        if !thinking_ok {
            dropped.push(format!("dropped (empty thinking): {}", action.tool));
            continue;
        }
        kept.push(action.clone());
    }
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(thinking: &str, actions: Vec<(&str, serde_json::Value)>) -> Decision {
        Decision {
            thinking: thinking.into(),
            scratchpad_update: String::new(),
            actions: actions
                .into_iter()
                .map(|(t, a)| DecisionAction {
                    tool: t.into(),
                    args: a,
                })
                .collect(),
            private_notes: Vec::new(),
        }
    }

    #[test]
    fn empty_thinking_drops_actions() {
        let dec = d(
            "ok", // too short — counts as no real reasoning
            vec![("draw_arrow", serde_json::json!({}))],
        );
        let (kept, dropped) = enforce_justification(&dec, AgentRole::CollaboratorLight);
        assert!(kept.is_empty());
        assert!(dropped[0].contains("empty thinking"));
    }

    #[test]
    fn natural_prose_passes() {
        // The model writes prose without repeating the tool name verbatim;
        // we used to over-reject these. Now they should pass.
        let dec = d(
            "The canvas shows a tic-tac-toe grid. I'll propose the center move.",
            vec![("draw_propose", serde_json::json!({}))],
        );
        let (kept, dropped) = enforce_justification(&dec, AgentRole::CollaboratorLight);
        assert_eq!(kept.len(), 1);
        assert!(dropped.is_empty());
    }

    #[test]
    fn observer_role_disallows_draw_arrow() {
        let dec = d(
            "draw_arrow would help here",
            vec![("draw_arrow", serde_json::json!({}))],
        );
        let (kept, dropped) = enforce_justification(&dec, AgentRole::Observer);
        assert!(kept.is_empty());
        assert!(dropped[0].contains("role disallows"));
    }

    #[test]
    fn parses_fenced_json_content() {
        let parsed = parse_decision_content(
            "```json\n{\"thinking\":\"I can help.\",\"scratchpad_update\":\"\",\"actions\":[],\"private_notes\":[]}\n```",
        )
        .expect("should parse fenced JSON");
        assert_eq!(parsed.thinking, "I can help.");
    }

    #[test]
    fn parses_json_embedded_in_prose() {
        let parsed = parse_decision_content(
            "Here is the result:\n{\"thinking\":\"I can help.\",\"scratchpad_update\":\"\",\"actions\":[],\"private_notes\":[]}\nThanks!",
        )
        .expect("should extract JSON object");
        assert_eq!(parsed.thinking, "I can help.");
    }

    #[test]
    fn empty_content_reports_parse_error() {
        let err = parse_decision_content("   ").expect_err("empty content should fail");
        assert!(err.contains("empty content"));
    }
}
