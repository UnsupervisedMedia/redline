use crate::state::Settings;
use std::path::PathBuf;

fn settings_path() -> Option<PathBuf> {
    let mut p = dirs_next::config_dir()?;
    p.push("redline");
    std::fs::create_dir_all(&p).ok()?;
    p.push("settings.json");
    Some(p)
}

pub fn save_settings(settings: &Settings) {
    let Some(path) = settings_path() else {
        return;
    };
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

/// Load persisted settings. Shortcuts are always reloaded from their own file
/// regardless of what (if anything) is in settings.json.
pub fn load_settings() -> Settings {
    let loaded: Option<Settings> = settings_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());

    let mut settings = loaded.unwrap_or_default();
    // Shortcuts are persisted separately; the serde(skip) field is empty after
    // deserialization, so we always reload them explicitly.
    settings.shortcuts = crate::shortcuts::load_shortcuts();
    settings
}
