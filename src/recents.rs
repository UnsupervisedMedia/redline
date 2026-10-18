//! Auto-save helpers: write each capture to a user-configurable location
//! (default ~/Pictures/RedLine/YYYY-MM-DD/capture-HHMMSS.png) and track it in
//! the recent-captures list.

use std::path::PathBuf;

use chrono::{DateTime, Local};

use crate::state::{AppState, RecentEntry};

pub const DEFAULT_DIR_TEMPLATE: &str = "~/Pictures/RedLine/%Y-%m-%d";
pub const DEFAULT_NAME_TEMPLATE: &str = "capture-%H%M%S";

/// Expand a strftime template, falling back to `fallback` if the template has
/// invalid specifiers (chrono panics when formatting those, and a bad template
/// typed into a settings box must never take the app down).
pub fn expand_time_template(template: &str, now: DateTime<Local>, fallback: &str) -> String {
    use chrono::format::{Item, StrftimeItems};
    let valid = |t: &str| !StrftimeItems::new(t).any(|i| matches!(i, Item::Error));
    let source = if template.trim().is_empty() || !valid(template) {
        fallback
    } else {
        template
    };
    now.format(source).to_string()
}

/// Expand `~` to the user's home directory. Left as-is when there is no home.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs_next::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

/// Full auto-save directory for `now` from the user's template.
pub fn auto_save_dir_for(template: &str, now: DateTime<Local>) -> PathBuf {
    expand_tilde(&expand_time_template(template, now, DEFAULT_DIR_TEMPLATE))
}

/// The file stem for `now` from the user's template, sanitized so a stray `/`
/// can't escape the target directory.
pub fn auto_save_stem_for(template: &str, now: DateTime<Local>) -> String {
    let raw = expand_time_template(template, now, DEFAULT_NAME_TEMPLATE);
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        expand_time_template(DEFAULT_NAME_TEMPLATE, now, DEFAULT_NAME_TEMPLATE)
    } else {
        trimmed
    }
}

/// The path the *next* capture would be written to — shown in the UI so the
/// templates are self-explanatory without saving anything first.
pub fn preview_auto_save_path(state: &AppState) -> PathBuf {
    let now = Local::now();
    let dir = auto_save_dir_for(&state.settings.auto_save_dir_template, now);
    let stem = auto_save_stem_for(&state.settings.auto_save_name_template, now);
    dir.join(format!("{stem}.png"))
}

/// Write the current canvas image to disk under the configured auto-save
/// location and return a RecentEntry. No-op if nothing is loaded or the target
/// dir cannot be created.
pub fn auto_save_capture(state: &AppState) -> Option<RecentEntry> {
    let ci = state.canvas.color_image.as_ref()?;
    let now = Local::now();
    let dir = auto_save_dir_for(&state.settings.auto_save_dir_template, now);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("auto-save: {} — {e}", dir.display());
        return None;
    }
    let stem = auto_save_stem_for(&state.settings.auto_save_name_template, now);
    let mut img_path = dir.join(format!("{stem}.png"));
    // Avoid clobber if two captures land in the same second.
    let mut counter = 1;
    while img_path.exists() {
        img_path = dir.join(format!("{stem}-{counter}.png"));
        counter += 1;
    }

    let (w, h) = (ci.size[0] as u32, ci.size[1] as u32);
    let raw: Vec<u8> = ci
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    let buf = image::RgbaImage::from_raw(w, h, raw)?;
    if let Err(e) = buf.save(&img_path) {
        eprintln!("auto-save: {e}");
        return None;
    }

    // Also dump a sidecar beside the image if there are any annotations.
    let sidecar_path = if !state.canvas.annotations.is_empty() {
        let mut p = img_path.clone();
        p.set_extension("redline.json");
        let sc = crate::export::build_sidecar(&state.canvas);
        if let Ok(bytes) = serde_json::to_vec_pretty(&sc) {
            if std::fs::write(&p, bytes).is_ok() {
                Some(p)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Some(RecentEntry {
        image_path: img_path,
        sidecar_path,
        captured_at: now.to_rfc3339(),
        width: w,
        height: h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn expands_strftime_tokens() {
        let now = at(2026, 9, 1, 14, 5, 9);
        assert_eq!(
            expand_time_template("shot-%Y%m%d-%H%M%S", now, DEFAULT_NAME_TEMPLATE),
            "shot-20260901-140509"
        );
    }

    #[test]
    fn invalid_or_empty_template_falls_back_instead_of_panicking() {
        let now = at(2026, 9, 1, 14, 5, 9);
        assert_eq!(
            expand_time_template("%Q%!", now, DEFAULT_NAME_TEMPLATE),
            "capture-140509"
        );
        assert_eq!(
            expand_time_template("   ", now, DEFAULT_NAME_TEMPLATE),
            "capture-140509"
        );
    }

    #[test]
    fn name_template_cannot_escape_the_directory() {
        let now = at(2026, 9, 1, 14, 5, 9);
        let stem = auto_save_stem_for("../../etc/passwd", now);
        assert!(!stem.contains('/'), "{stem}");
        assert!(!stem.starts_with('.'), "{stem}");
    }

    #[test]
    fn dir_template_expands_tilde_and_date() {
        let now = at(2026, 9, 1, 14, 5, 9);
        let dir = auto_save_dir_for("~/Pictures/RedLine/%Y-%m-%d", now);
        assert!(dir.ends_with("Pictures/RedLine/2026-09-01"), "{dir:?}");
        assert!(dir.is_absolute() || dirs_next::home_dir().is_none());
    }

    #[test]
    fn absolute_dir_templates_pass_through() {
        let now = at(2026, 9, 1, 14, 5, 9);
        let dir = auto_save_dir_for("/tmp/shots/%Y", now);
        assert_eq!(dir, PathBuf::from("/tmp/shots/2026"));
    }
}
