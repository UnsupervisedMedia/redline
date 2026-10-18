//! Scene-graph document model — the layered vector tree.
//!
//! A `Document` is an arena of `Node`s keyed by `Uuid`, with one root `Group`.
//! The tree is the source of truth; `Document::leaves_in_paint_order` produces
//! the derived flat view that rendering, hit-testing, export, and the MCP layer
//! consume in later phases. See
//! `docs/superpowers/specs/2026-05-30-layered-vector-editor-design.md`.

mod document;
mod migrate;
mod node;

// public API surface for later phases
#[allow(unused_imports)]
pub use document::{Document, LeafView};
#[allow(unused_imports)]
pub use node::{Node, NodeId, NodeKind, ShapeLeaf};
