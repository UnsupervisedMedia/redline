use std::path::PathBuf;

use uuid::Uuid;

use super::state::CanvasSortState;
use super::types::{CanvasSortSession, SessionMeta, StickyNote, StickyNoteData};

// ── Data directory ────────────────────────────────────────────────────────────

pub fn data_dir() -> PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("redline")
        .join("canvas_sort")
}

pub fn session_path(id: Uuid) -> PathBuf {
    data_dir().join(format!("{id}.bin"))
}

pub fn sessions_index_path() -> PathBuf {
    data_dir().join("sessions.bin")
}

pub fn thumb_dir(session_id: Uuid) -> PathBuf {
    data_dir().join("thumbnails").join(session_id.to_string())
}

pub fn notes_path(session_id: Uuid) -> PathBuf {
    data_dir().join(format!("{session_id}.notes.bin"))
}

// ── Session list ──────────────────────────────────────────────────────────────

pub fn load_session_list() -> Vec<SessionMeta> {
    let path = sessions_index_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    bincode::deserialize::<Vec<SessionMeta>>(&bytes).unwrap_or_default()
}

pub fn save_session_list(list: &[SessionMeta]) -> std::io::Result<()> {
    let path = sessions_index_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let bytes = bincode::serialize(list).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

fn upsert_session_meta(session: &CanvasSortSession) {
    let mut list = load_session_list();
    let meta = SessionMeta {
        id: session.id,
        name: session.name.clone(),
        image_count: session.images.len(),
        last_accessed: chrono::Utc::now(),
    };
    if let Some(entry) = list.iter_mut().find(|m| m.id == session.id) {
        *entry = meta;
    } else {
        list.push(meta);
    }
    // Sort by last_accessed descending
    list.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    let _ = save_session_list(&list);
}

// ── Session I/O ───────────────────────────────────────────────────────────────

pub fn save_session(state: &CanvasSortState) -> std::io::Result<()> {
    let session = match &state.session {
        Some(s) => s,
        None => return Ok(()),
    };
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = session_path(session.id);
    let bytes = bincode::serialize(session).map_err(std::io::Error::other)?;
    std::fs::write(&path, bytes)?;
    upsert_session_meta(session);
    let _ = save_notes(session.id, &state.sticky_notes);
    Ok(())
}

pub fn load_session(id: Uuid) -> Option<CanvasSortSession> {
    let path = session_path(id);
    let bytes = std::fs::read(path).ok()?;
    bincode::deserialize::<CanvasSortSession>(&bytes).ok()
}

pub fn delete_session(id: Uuid) -> std::io::Result<()> {
    let _ = std::fs::remove_file(session_path(id));
    let _ = std::fs::remove_file(notes_path(id));
    // Remove from list
    let mut list = load_session_list();
    list.retain(|m| m.id != id);
    save_session_list(&list)?;
    // Remove thumbnails dir
    let _ = std::fs::remove_dir_all(thumb_dir(id));
    Ok(())
}

// ── Sticky-note sidecar ─────────────────────────────────────────────────────────

pub fn save_notes(session_id: Uuid, notes: &[StickyNote]) -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let data: Vec<StickyNoteData> = notes.iter().map(StickyNoteData::from).collect();
    let bytes = bincode::serialize(&data).map_err(std::io::Error::other)?;
    std::fs::write(notes_path(session_id), bytes)
}

/// Load notes for a session. Missing or unreadable sidecar → empty (notes are
/// non-critical annotations; never fail a session open over them).
pub fn load_notes(session_id: Uuid) -> Vec<StickyNote> {
    let bytes = match std::fs::read(notes_path(session_id)) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    match bincode::deserialize::<Vec<StickyNoteData>>(&bytes) {
        Ok(v) => v.into_iter().map(StickyNoteData::into_note).collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_notes_missing_file_is_empty() {
        // A random session id has no sidecar on disk → empty, never an error.
        let id = Uuid::new_v4();
        assert!(load_notes(id).is_empty());
    }
}
