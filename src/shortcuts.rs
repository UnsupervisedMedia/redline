use std::collections::HashMap;
use std::path::PathBuf;

// ── KeyCombo ──────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: egui::Key,
}

impl KeyCombo {
    pub fn plain(key: egui::Key) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }
    pub fn ctrl(key: egui::Key) -> Self {
        Self {
            ctrl: true,
            shift: false,
            alt: false,
            key,
        }
    }
    pub fn ctrl_shift(key: egui::Key) -> Self {
        Self {
            ctrl: true,
            shift: true,
            alt: false,
            key,
        }
    }

    pub fn matches(&self, i: &egui::InputState) -> bool {
        i.modifiers.ctrl == self.ctrl
            && i.modifiers.shift == self.shift
            && i.modifiers.alt == self.alt
            && i.key_pressed(self.key)
    }

    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(key_name(self.key));
        parts.join("+")
    }

    pub fn to_str(&self) -> String {
        self.display()
    }

    pub fn from_str(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('+').collect();
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut key_str = "";
        for part in &parts {
            match *part {
                "Ctrl" => ctrl = true,
                "Shift" => shift = true,
                "Alt" => alt = true,
                other => key_str = other,
            }
        }
        let key = key_from_name(key_str)?;
        Some(Self {
            ctrl,
            shift,
            alt,
            key,
        })
    }
}

// ── Action ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    // Capture
    CaptureRegion,
    CaptureFullScreen,
    // File ops
    ExportSidecar,
    PasteImage,
    // Edit
    Undo,
    Redo,
    // View
    ResetZoom,
    ToggleMagnifier,
    // Tools
    ToolSelect,
    ToolPen,
    ToolRect,
    ToolOval,
    ToolStar,
    ToolArrow,
    ToolLine,
    ToolText,
    ToolBlur,
    ToolCrop,
    ToolEyedropper,
    ToolStep,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::CaptureRegion => "Capture Region",
            Action::CaptureFullScreen => "Capture Full Screen",
            Action::ExportSidecar => "Export Sidecar",
            Action::PasteImage => "Paste Image",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::ResetZoom => "Reset Zoom",
            Action::ToggleMagnifier => "Toggle Magnifier",
            Action::ToolSelect => "Tool: Select",
            Action::ToolPen => "Tool: Pen",
            Action::ToolRect => "Tool: Rectangle",
            Action::ToolOval => "Tool: Oval",
            Action::ToolStar => "Tool: Star",
            Action::ToolArrow => "Tool: Arrow",
            Action::ToolLine => "Tool: Line",
            Action::ToolText => "Tool: Text",
            Action::ToolBlur => "Tool: Blur",
            Action::ToolCrop => "Tool: Crop",
            Action::ToolEyedropper => "Tool: Eyedropper",
            Action::ToolStep => "Tool: Step Marker",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Action::CaptureRegion => "CaptureRegion",
            Action::CaptureFullScreen => "CaptureFullScreen",
            Action::ExportSidecar => "ExportSidecar",
            Action::PasteImage => "PasteImage",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::ResetZoom => "ResetZoom",
            Action::ToggleMagnifier => "ToggleMagnifier",
            Action::ToolSelect => "ToolSelect",
            Action::ToolPen => "ToolPen",
            Action::ToolRect => "ToolRect",
            Action::ToolOval => "ToolOval",
            Action::ToolStar => "ToolStar",
            Action::ToolArrow => "ToolArrow",
            Action::ToolLine => "ToolLine",
            Action::ToolText => "ToolText",
            Action::ToolBlur => "ToolBlur",
            Action::ToolCrop => "ToolCrop",
            Action::ToolEyedropper => "ToolEyedropper",
            Action::ToolStep => "ToolStep",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "CaptureRegion" => Some(Action::CaptureRegion),
            "CaptureFullScreen" => Some(Action::CaptureFullScreen),
            "ExportSidecar" => Some(Action::ExportSidecar),
            "PasteImage" => Some(Action::PasteImage),
            "Undo" => Some(Action::Undo),
            "Redo" => Some(Action::Redo),
            "ResetZoom" => Some(Action::ResetZoom),
            "ToggleMagnifier" => Some(Action::ToggleMagnifier),
            "ToolSelect" => Some(Action::ToolSelect),
            "ToolPen" => Some(Action::ToolPen),
            "ToolRect" => Some(Action::ToolRect),
            "ToolOval" => Some(Action::ToolOval),
            "ToolStar" => Some(Action::ToolStar),
            "ToolArrow" => Some(Action::ToolArrow),
            "ToolLine" => Some(Action::ToolLine),
            "ToolText" => Some(Action::ToolText),
            "ToolBlur" => Some(Action::ToolBlur),
            "ToolCrop" => Some(Action::ToolCrop),
            "ToolEyedropper" => Some(Action::ToolEyedropper),
            "ToolStep" => Some(Action::ToolStep),
            _ => None,
        }
    }
}

// ── Defaults ──────────────────────────────────────────────────────────────────

pub fn default_shortcuts() -> HashMap<Action, KeyCombo> {
    use egui::Key;
    let mut m = HashMap::new();
    m.insert(Action::CaptureRegion, KeyCombo::ctrl_shift(Key::S));
    m.insert(Action::CaptureFullScreen, KeyCombo::ctrl_shift(Key::F));
    m.insert(Action::ExportSidecar, KeyCombo::ctrl_shift(Key::C));
    m.insert(Action::PasteImage, KeyCombo::ctrl(Key::V));
    m.insert(Action::Undo, KeyCombo::ctrl(Key::Z));
    m.insert(Action::Redo, KeyCombo::ctrl(Key::Y));
    m.insert(Action::ResetZoom, KeyCombo::plain(Key::Num0));
    m.insert(Action::ToggleMagnifier, KeyCombo::plain(Key::M));
    m.insert(Action::ToolSelect, KeyCombo::plain(Key::V));
    m.insert(Action::ToolPen, KeyCombo::plain(Key::P));
    m.insert(Action::ToolRect, KeyCombo::plain(Key::R));
    m.insert(Action::ToolOval, KeyCombo::plain(Key::O));
    m.insert(Action::ToolStar, KeyCombo::plain(Key::S));
    m.insert(Action::ToolArrow, KeyCombo::plain(Key::A));
    m.insert(Action::ToolLine, KeyCombo::plain(Key::L));
    m.insert(Action::ToolText, KeyCombo::plain(Key::T));
    m.insert(Action::ToolBlur, KeyCombo::plain(Key::B));
    m.insert(Action::ToolCrop, KeyCombo::plain(Key::K));
    m.insert(Action::ToolEyedropper, KeyCombo::plain(Key::I));
    m.insert(Action::ToolStep, KeyCombo::plain(Key::N));
    m
}

// ── Persistence ───────────────────────────────────────────────────────────────

fn shortcuts_path() -> Option<PathBuf> {
    let mut p = dirs_next::config_dir()?;
    p.push("redline");
    std::fs::create_dir_all(&p).ok()?;
    p.push("shortcuts.json");
    Some(p)
}

pub fn save_shortcuts(shortcuts: &HashMap<Action, KeyCombo>) {
    let Some(path) = shortcuts_path() else {
        return;
    };
    let map: HashMap<&str, String> = shortcuts
        .iter()
        .map(|(a, k)| (a.as_str(), k.to_str()))
        .collect();
    if let Ok(json) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(path, json);
    }
}

pub fn load_shortcuts() -> HashMap<Action, KeyCombo> {
    let mut defaults = default_shortcuts();
    let Some(path) = shortcuts_path() else {
        return defaults;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return defaults;
    };
    let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(&bytes) else {
        return defaults;
    };
    for (action_str, combo_str) in &map {
        if let (Some(action), Some(combo)) =
            (Action::from_str(action_str), KeyCombo::from_str(combo_str))
        {
            defaults.insert(action, combo);
        }
    }
    defaults
}

// ── Key name helpers ──────────────────────────────────────────────────────────

pub fn key_name(key: egui::Key) -> &'static str {
    use egui::Key::*;
    match key {
        A => "A",
        B => "B",
        C => "C",
        D => "D",
        E => "E",
        F => "F",
        G => "G",
        H => "H",
        I => "I",
        J => "J",
        K => "K",
        L => "L",
        M => "M",
        N => "N",
        O => "O",
        P => "P",
        Q => "Q",
        R => "R",
        S => "S",
        T => "T",
        U => "U",
        V => "V",
        W => "W",
        X => "X",
        Y => "Y",
        Z => "Z",
        Num0 => "0",
        Num1 => "1",
        Num2 => "2",
        Num3 => "3",
        Num4 => "4",
        Num5 => "5",
        Num6 => "6",
        Num7 => "7",
        Num8 => "8",
        Num9 => "9",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        Space => "Space",
        Enter => "Enter",
        Escape => "Esc",
        Delete => "Del",
        Backspace => "Backspace",
        Home => "Home",
        End => "End",
        PageUp => "PgUp",
        PageDown => "PgDn",
        Insert => "Ins",
        Tab => "Tab",
        _ => "?",
    }
}

pub fn key_from_name(s: &str) -> Option<egui::Key> {
    use egui::Key;
    match s {
        "A" => Some(Key::A),
        "B" => Some(Key::B),
        "C" => Some(Key::C),
        "D" => Some(Key::D),
        "E" => Some(Key::E),
        "F" => Some(Key::F),
        "G" => Some(Key::G),
        "H" => Some(Key::H),
        "I" => Some(Key::I),
        "J" => Some(Key::J),
        "K" => Some(Key::K),
        "L" => Some(Key::L),
        "M" => Some(Key::M),
        "N" => Some(Key::N),
        "O" => Some(Key::O),
        "P" => Some(Key::P),
        "Q" => Some(Key::Q),
        "R" => Some(Key::R),
        "S" => Some(Key::S),
        "T" => Some(Key::T),
        "U" => Some(Key::U),
        "V" => Some(Key::V),
        "W" => Some(Key::W),
        "X" => Some(Key::X),
        "Y" => Some(Key::Y),
        "Z" => Some(Key::Z),
        "0" => Some(Key::Num0),
        "1" => Some(Key::Num1),
        "2" => Some(Key::Num2),
        "3" => Some(Key::Num3),
        "4" => Some(Key::Num4),
        "5" => Some(Key::Num5),
        "6" => Some(Key::Num6),
        "7" => Some(Key::Num7),
        "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "Space" => Some(Key::Space),
        "Enter" => Some(Key::Enter),
        "Esc" => Some(Key::Escape),
        "Del" => Some(Key::Delete),
        "Backspace" => Some(Key::Backspace),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PgUp" => Some(Key::PageUp),
        "PgDn" => Some(Key::PageDown),
        "Ins" => Some(Key::Insert),
        "Tab" => Some(Key::Tab),
        _ => None,
    }
}

/// Try to extract a meaningful KeyCombo from the current input state (for recording).
/// Returns `None` if only modifiers are pressed, or Escape was pressed (cancel signal).
pub fn capture_from_input(i: &egui::InputState) -> Option<Result<KeyCombo, ()>> {
    if i.key_pressed(egui::Key::Escape) {
        return Some(Err(())); // cancel
    }
    // All keys that can be recorded (skip modifier-only presses)
    for &key in RECORDABLE_KEYS {
        if i.key_pressed(key) {
            return Some(Ok(KeyCombo {
                ctrl: i.modifiers.ctrl,
                shift: i.modifiers.shift,
                alt: i.modifiers.alt,
                key,
            }));
        }
    }
    None
}

const RECORDABLE_KEYS: &[egui::Key] = &[
    egui::Key::A,
    egui::Key::B,
    egui::Key::C,
    egui::Key::D,
    egui::Key::E,
    egui::Key::F,
    egui::Key::G,
    egui::Key::H,
    egui::Key::I,
    egui::Key::J,
    egui::Key::K,
    egui::Key::L,
    egui::Key::M,
    egui::Key::N,
    egui::Key::O,
    egui::Key::P,
    egui::Key::Q,
    egui::Key::R,
    egui::Key::S,
    egui::Key::T,
    egui::Key::U,
    egui::Key::V,
    egui::Key::W,
    egui::Key::X,
    egui::Key::Y,
    egui::Key::Z,
    egui::Key::Num0,
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
    egui::Key::F1,
    egui::Key::F2,
    egui::Key::F3,
    egui::Key::F4,
    egui::Key::F5,
    egui::Key::F6,
    egui::Key::F7,
    egui::Key::F8,
    egui::Key::F9,
    egui::Key::F10,
    egui::Key::F11,
    egui::Key::F12,
    egui::Key::Space,
    egui::Key::Enter,
    egui::Key::Delete,
    egui::Key::Backspace,
    egui::Key::Home,
    egui::Key::End,
    egui::Key::PageUp,
    egui::Key::PageDown,
    egui::Key::Insert,
    egui::Key::Tab,
];

#[cfg(test)]
mod tests {
    use super::*;
}
