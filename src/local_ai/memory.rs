//! Canvas-scoped memory for the local AI: per-agent log, shared cross-agent
//! notes, and invisible spatial bookmarks. Persisted as part of CanvasState.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

use crate::local_ai::AgentId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub ts: SystemTime,
    pub kind: MemoryKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryKind {
    Decision,
    Summary,
    Pinned,
    UserNote,
    ToolWish,
    Lesson,
    Private,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedNote {
    pub id: Uuid,
    pub ts: SystemTime,
    pub author: AgentId,
    pub content: String,
    #[serde(default)]
    pub anchor: Option<[f32; 2]>,
    #[serde(default)]
    pub reply_to: Option<Uuid>,
}

/// Axis-aligned bounding rectangle in image-pixel coordinates.
/// Used as the `anchor` of an `AgentBookmark` so x/y are unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AnchorRect {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBookmark {
    pub id: Uuid,
    pub ts: SystemTime,
    pub anchor: AnchorRect,
    pub label: String,
    pub confidence: f32,
}

/// Total content size of all entries for a given agent — used to decide
/// when compression should fire.
pub fn entries_content_bytes(entries: &[MemoryEntry]) -> usize {
    entries.iter().map(|e| e.content.len()).sum()
}

const COMPRESSION_THRESHOLD_BYTES: usize = 4096;

/// Returns true if the entries for one agent should be compressed.
pub fn should_compress(entries: &[MemoryEntry]) -> bool {
    entries_content_bytes(entries) > COMPRESSION_THRESHOLD_BYTES
}

/// Upgrade legacy entries written before Decision memory had a dedicated op.
/// Old builds stored `decision: ...` as Lesson entries, which polluted the
/// prompt on later loads. Convert them back to Decision entries.
pub fn upgrade_legacy_entries(entries: Vec<MemoryEntry>) -> (Vec<MemoryEntry>, bool) {
    let mut upgraded = false;
    let out = entries
        .into_iter()
        .map(|mut entry| {
            if entry.kind == MemoryKind::Lesson {
                let trimmed = entry.content.trim();
                if let Some(rest) = trimmed.strip_prefix("decision:") {
                    entry.kind = MemoryKind::Decision;
                    entry.content = rest.trim().to_string();
                    upgraded = true;
                }
            }
            entry
        })
        .collect();
    (out, upgraded)
}

/// Replace Decision entries with a single Summary entry. Pinned, UserNote,
/// ToolWish, Lesson, and Private entries are preserved. Returns the
/// updated Vec.
///
/// `summary_text` should be the model's summarization of the dropped
/// Decisions — produced by a separate call to the model with a
/// summarization prompt.
pub fn replace_decisions_with_summary(
    entries: Vec<MemoryEntry>,
    summary_text: String,
) -> Vec<MemoryEntry> {
    let mut kept: Vec<MemoryEntry> = entries
        .into_iter()
        .filter(|e| !matches!(e.kind, MemoryKind::Decision))
        .collect();
    kept.push(MemoryEntry {
        ts: std::time::SystemTime::now(),
        kind: MemoryKind::Summary,
        content: summary_text,
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_drops_decisions_keeps_pinned() {
        let entries = vec![
            MemoryEntry {
                ts: std::time::SystemTime::now(),
                kind: MemoryKind::Decision,
                content: "drew arrow".into(),
            },
            MemoryEntry {
                ts: std::time::SystemTime::now(),
                kind: MemoryKind::Pinned,
                content: "user prefers compact charts".into(),
            },
            MemoryEntry {
                ts: std::time::SystemTime::now(),
                kind: MemoryKind::Decision,
                content: "drew label".into(),
            },
        ];
        let out = replace_decisions_with_summary(entries, "summary of recent work".into());
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|e| matches!(e.kind, MemoryKind::Pinned)));
        assert!(out.iter().any(|e| matches!(e.kind, MemoryKind::Summary)));
        assert!(!out.iter().any(|e| matches!(e.kind, MemoryKind::Decision)));
    }

    #[test]
    fn entries_content_bytes_sums_correctly() {
        let entries = vec![
            MemoryEntry {
                ts: SystemTime::now(),
                kind: MemoryKind::Decision,
                content: "abc".into(),
            },
            MemoryEntry {
                ts: SystemTime::now(),
                kind: MemoryKind::Lesson,
                content: "1234".into(),
            },
        ];
        assert_eq!(entries_content_bytes(&entries), 7);
    }

    #[test]
    fn memory_kind_roundtrips_json() {
        for k in [
            MemoryKind::Decision,
            MemoryKind::Summary,
            MemoryKind::Pinned,
            MemoryKind::UserNote,
            MemoryKind::ToolWish,
            MemoryKind::Lesson,
            MemoryKind::Private,
        ] {
            let s = serde_json::to_string(&k).unwrap();
            let back: MemoryKind = serde_json::from_str(&s).unwrap();
            assert_eq!(k, back);
        }
    }

    #[test]
    fn upgrade_legacy_decision_lessons() {
        let (out, upgraded) = upgrade_legacy_entries(vec![MemoryEntry {
            ts: SystemTime::now(),
            kind: MemoryKind::Lesson,
            content: "decision: drew the roof line".into(),
        }]);
        assert!(upgraded);
        assert!(matches!(out[0].kind, MemoryKind::Decision));
        assert_eq!(out[0].content, "drew the roof line");
    }
}
