use egui::Vec2;
use uuid::Uuid;

use super::types::{CanvasSortSession, SortMode};

// ── Sort algorithms ───────────────────────────────────────────────────────────

/// Sort loose images (no stack) by `mode` and re-grid them.
/// Returns a list of (id, old_pos, new_pos) for undo.
pub fn sort_images(
    session: &mut CanvasSortSession,
    mode: SortMode,
    columns: usize,
    spacing: f32,
    origin: Vec2,
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let mut loose: Vec<Uuid> = session.loose_image_ids();
    match mode {
        SortMode::NameAsc => loose.sort_by(|a, b| {
            let pa = session.image_by_id(*a).map(|i| i.path.as_path());
            let pb = session.image_by_id(*b).map(|i| i.path.as_path());
            pa.cmp(&pb)
        }),
        SortMode::NameDesc => loose.sort_by(|a, b| {
            let pa = session.image_by_id(*a).map(|i| i.path.as_path());
            let pb = session.image_by_id(*b).map(|i| i.path.as_path());
            pb.cmp(&pa)
        }),
        SortMode::DateNewest => loose.sort_by(|a, b| {
            let ta = mtime(session, *a);
            let tb = mtime(session, *b);
            tb.cmp(&ta)
        }),
        SortMode::DateOldest => loose.sort_by(|a, b| {
            let ta = mtime(session, *a);
            let tb = mtime(session, *b);
            ta.cmp(&tb)
        }),
        SortMode::SizeLargest => loose.sort_by(|a, b| {
            let sa = file_size(session, *a);
            let sb = file_size(session, *b);
            sb.cmp(&sa)
        }),
        SortMode::SizeSmallest => loose.sort_by(|a, b| {
            let sa = file_size(session, *a);
            let sb = file_size(session, *b);
            sa.cmp(&sb)
        }),
        SortMode::TypeAsc => loose.sort_by(|a, b| {
            let ea = ext(session, *a);
            let eb = ext(session, *b);
            ea.cmp(&eb)
        }),
        SortMode::Random => {
            // Deterministic shuffle using the current time nanoseconds as seed.
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64;
            knuth_shuffle(&mut loose, seed);
        }
    }

    grid_positions(session, &loose, columns, spacing, origin)
}

/// Arrange a specific set of images in a grid.
/// Returns (id, old_pos, new_pos).
pub fn arrange_in_grid(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
    fixed_columns: bool,
    count: usize,
    spacing: f32,
    origin: Vec2,
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let n = ids.len();
    if n == 0 {
        return Vec::new();
    }
    let cols = if fixed_columns {
        count.max(1)
    } else {
        // Fixed rows: compute cols from rows
        (n as f32 / count.max(1) as f32).ceil() as usize
    };
    let cols = cols.max(1);
    grid_positions_with_cols(session, ids, cols, spacing, origin)
}

/// Smart grid: square-ish layout.
pub fn smart_grid(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
    spacing: f32,
    origin: Vec2,
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let cols = ((ids.len() as f32).sqrt().ceil() as usize).max(1);
    grid_positions_with_cols(session, ids, cols, spacing, origin)
}

// ── Alignment helpers ─────────────────────────────────────────────────────────

pub fn align_left(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let min_x = ids
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.pos[0])
        .fold(f32::MAX, f32::min);
    ids.iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[0] = min_x;
            Some((*id, old, img.pos))
        })
        .collect()
}

pub fn align_right(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let max_x = ids
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.pos[0] + i.size[0])
        .fold(f32::MIN, f32::max);
    ids.iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[0] = max_x - img.size[0];
            Some((*id, old, img.pos))
        })
        .collect()
}

pub fn align_top(session: &mut CanvasSortSession, ids: &[Uuid]) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let min_y = ids
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.pos[1])
        .fold(f32::MAX, f32::min);
    ids.iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[1] = min_y;
            Some((*id, old, img.pos))
        })
        .collect()
}

pub fn align_bottom(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    let max_y = ids
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.pos[1] + i.size[1])
        .fold(f32::MIN, f32::max);
    ids.iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[1] = max_y - img.size[1];
            Some((*id, old, img.pos))
        })
        .collect()
}

pub fn distribute_h(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    if ids.len() < 2 {
        return Vec::new();
    }
    let mut sorted = ids.to_vec();
    sorted.sort_by(|a, b| {
        let xa = session.image_by_id(*a).map(|i| i.pos[0]).unwrap_or(0.0);
        let xb = session.image_by_id(*b).map(|i| i.pos[0]).unwrap_or(0.0);
        xa.partial_cmp(&xb).unwrap()
    });
    let first_x = session
        .image_by_id(sorted[0])
        .map(|i| i.pos[0])
        .unwrap_or(0.0);
    let last = session.image_by_id(*sorted.last().unwrap());
    let last_x = last.map(|i| i.pos[0] + i.size[0]).unwrap_or(0.0);
    let total_w: f32 = sorted
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.size[0])
        .sum();
    let gap = (last_x - first_x - total_w) / (sorted.len() - 1) as f32;
    let mut cursor = first_x;
    sorted
        .iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[0] = cursor;
            cursor += img.size[0] + gap;
            Some((*id, old, img.pos))
        })
        .collect()
}

pub fn distribute_v(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    if ids.len() < 2 {
        return Vec::new();
    }
    let mut sorted = ids.to_vec();
    sorted.sort_by(|a, b| {
        let ya = session.image_by_id(*a).map(|i| i.pos[1]).unwrap_or(0.0);
        let yb = session.image_by_id(*b).map(|i| i.pos[1]).unwrap_or(0.0);
        ya.partial_cmp(&yb).unwrap()
    });
    let first_y = session
        .image_by_id(sorted[0])
        .map(|i| i.pos[1])
        .unwrap_or(0.0);
    let last_y = session
        .image_by_id(*sorted.last().unwrap())
        .map(|i| i.pos[1] + i.size[1])
        .unwrap_or(0.0);
    let total_h: f32 = sorted
        .iter()
        .filter_map(|id| session.image_by_id(*id))
        .map(|i| i.size[1])
        .sum();
    let gap = (last_y - first_y - total_h) / (sorted.len() - 1) as f32;
    let mut cursor = first_y;
    sorted
        .iter()
        .filter_map(|id| {
            let img = session.image_by_id_mut(*id)?;
            let old = img.pos;
            img.pos[1] = cursor;
            cursor += img.size[1] + gap;
            Some((*id, old, img.pos))
        })
        .collect()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn grid_positions(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
    columns: usize,
    spacing: f32,
    origin: Vec2,
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    grid_positions_with_cols(session, ids, columns.max(1), spacing, origin)
}

fn grid_positions_with_cols(
    session: &mut CanvasSortSession,
    ids: &[Uuid],
    cols: usize,
    spacing: f32,
    origin: Vec2,
) -> Vec<(Uuid, [f32; 2], [f32; 2])> {
    ids.iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let img = session.image_by_id_mut(*id)?;
            let col = (i % cols) as f32;
            let row = (i / cols) as f32;
            let old = img.pos;
            let new_pos = [
                origin.x + col * (img.size[0] + spacing),
                origin.y + row * (img.size[1] + spacing),
            ];
            img.pos = new_pos;
            Some((*id, old, new_pos))
        })
        .collect()
}

fn mtime(session: &CanvasSortSession, id: Uuid) -> std::time::SystemTime {
    session
        .image_by_id(id)
        .and_then(|i| std::fs::metadata(&i.path).ok())
        .and_then(|m| m.modified().ok())
        .unwrap_or(std::time::UNIX_EPOCH)
}

fn file_size(session: &CanvasSortSession, id: Uuid) -> u64 {
    session
        .image_by_id(id)
        .and_then(|i| std::fs::metadata(&i.path).ok())
        .map(|m| m.len())
        .unwrap_or(0)
}

fn ext(session: &CanvasSortSession, id: Uuid) -> String {
    session
        .image_by_id(id)
        .and_then(|i| i.path.extension())
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn knuth_shuffle(v: &mut Vec<Uuid>, seed: u64) {
    let mut rng = seed;
    for i in (1..v.len()).rev() {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (rng >> 33) as usize % (i + 1);
        v.swap(i, j);
    }
}
