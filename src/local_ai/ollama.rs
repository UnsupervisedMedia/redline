//! HTTP wrapper around Ollama's `/api/chat` and `/api/tags` endpoints with
//! strict JSON schema enforcement via the `format` parameter.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<ChatOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            stream: false,
            format: None,
            options: None,
        }
    }

    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.messages.push(ChatMessage {
            role: "system".into(),
            content: content.into(),
            images: Vec::new(),
        });
        self
    }

    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.messages.push(ChatMessage {
            role: "user".into(),
            content: content.into(),
            images: Vec::new(),
        });
        self
    }

    /// Attach a base64-encoded image to the most-recently-added user message.
    ///
    /// **Caller must add a `.user(...)` call before this**; if no user message
    /// is present this is a silent no-op. The image will not be retroactively
    /// attached to a later-added user message.
    pub fn with_image_base64(mut self, b64: String) -> Self {
        if let Some(last_user) = self.messages.iter_mut().rev().find(|m| m.role == "user") {
            last_user.images.push(b64);
        }
        self
    }

    pub fn with_format(mut self, schema: serde_json::Value) -> Self {
        self.format = Some(schema);
        self
    }

    pub fn with_max_tokens(mut self, n: i32) -> Self {
        let mut opts = self.options.unwrap_or(ChatOptions {
            num_predict: None,
            temperature: None,
        });
        opts.num_predict = Some(n);
        self.options = Some(opts);
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub message: ChatResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponseMessage {
    pub content: String,
}

#[derive(Debug, Error)]
pub enum OllamaError {
    #[error("ollama unreachable: {0}")]
    Unreachable(String),
    #[error("ollama returned non-success: status={status} body={body}")]
    BadStatus { status: u16, body: String },
    #[error("json decode failed: {0}")]
    JsonDecode(String),
}

pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

fn build_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        // Fail fast when Ollama isn't listening rather than waiting out the
        // full request timeout on every connection attempt.
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builder")
}

/// Validate that `url` is a well-formed http/https endpoint. Returns the
/// trimmed URL (without a trailing slash) on success, or an error string.
pub fn validate_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!(
            "ollama url must start with http:// or https:// (got {trimmed:?})"
        ));
    }
    // Require a non-empty host after the scheme.
    let after_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or("");
    if after_scheme.is_empty() {
        return Err("ollama url is missing a host".into());
    }
    Ok(trimmed.to_string())
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_OLLAMA_URL.to_string(),
            http: build_http(),
        }
    }
}

impl OllamaClient {
    /// Build a client, falling back to the default endpoint if `base_url` is not
    /// a valid http/https URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        let raw = base_url.into();
        let base_url = match validate_url(&raw) {
            Ok(valid) => valid,
            Err(e) => {
                eprintln!("[local-ai] {e}; falling back to {DEFAULT_OLLAMA_URL}");
                DEFAULT_OLLAMA_URL.to_string()
            }
        };
        Self {
            base_url,
            http: build_http(),
        }
    }

    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, OllamaError> {
        let url = format!("{}/api/chat", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| OllamaError::Unreachable(format!("{e}")))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(OllamaError::BadStatus { status, body });
        }
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| OllamaError::JsonDecode(format!("{e}")))?;
        Ok(parsed)
    }
}

/// JSON schema for the decision-pass output. Used as the `format` parameter
/// in the Ollama chat request so the model is constrained to produce a
/// conforming JSON object.
pub fn schema_for_decision() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["thinking", "scratchpad_update", "actions", "private_notes"],
        "properties": {
            "thinking": { "type": "string" },
            "scratchpad_update": { "type": "string" },
            "actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["tool", "args"],
                    "properties": {
                        "tool": { "type": "string" },
                        "args": { "type": "object" }
                    }
                }
            },
            "private_notes": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_chat_request_includes_format_schema() {
        let req = ChatRequest::new("qwen3-vl")
            .system("you are a test")
            .user("hello")
            .with_image_base64("AAAA".into())
            .with_format(schema_for_decision());
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "qwen3-vl");
        assert!(json["messages"].as_array().unwrap().len() == 2);
        assert!(
            json["format"].is_object(),
            "format must be the JSON schema object"
        );
        assert_eq!(json["stream"], false);
        let user_msg = &json["messages"][1];
        assert_eq!(user_msg["role"], "user");
        assert_eq!(user_msg["content"], "hello");
        assert_eq!(user_msg["images"][0], "AAAA");
    }

    #[test]
    fn validate_url_accepts_http_and_https() {
        assert_eq!(
            validate_url("http://localhost:11434").unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(
            validate_url("https://example.com/").unwrap(),
            "https://example.com"
        );
        assert_eq!(validate_url("  http://host:1/  ").unwrap(), "http://host:1");
    }

    #[test]
    fn validate_url_rejects_bad_schemes() {
        assert!(validate_url("ftp://host").is_err());
        assert!(validate_url("localhost:11434").is_err());
        assert!(validate_url("http://").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn new_falls_back_on_invalid_url() {
        let c = OllamaClient::new("not-a-url");
        assert_eq!(c.base_url, DEFAULT_OLLAMA_URL);
    }
}
