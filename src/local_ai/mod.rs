//! Local AI collaborator — turnless event-driven agent that runs inside redline,
//! talks to Ollama, and participates in canvases as a first-class actor.

pub mod action_policy;
pub mod agent;
pub mod analysis;
pub mod behavioral;
pub mod budget;
pub mod decision;
pub mod event_filter;
pub mod memory;
pub mod ollama;
pub mod prompt;
pub mod provisional;
pub mod roles;
pub mod safety;
pub mod ui;

/// Stable identity string for an agent (e.g. "local-ai", "claude").
/// Used as the key for canvas-scoped agent memory so memory survives
/// session toggle-off/on cycles.
pub type AgentId = String;

pub const LOCAL_AI_ID: &str = "local-ai";
