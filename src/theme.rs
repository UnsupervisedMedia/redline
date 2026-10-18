//! Application color themes. Add a new `ThemeId` variant and a matching arm
//! in `get_theme()` to introduce a new skin.

use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemeId {
    #[default]
    Graphite,
    Midnight,
    Ember,
}

impl ThemeId {
    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Graphite => "Graphite",
            ThemeId::Midnight => "Midnight",
            ThemeId::Ember => "Ember",
        }
    }

    pub fn all() -> &'static [ThemeId] {
        &[ThemeId::Graphite, ThemeId::Midnight, ThemeId::Ember]
    }
}

/// Semantic color tokens for the entire app. Every hardcoded color in toolbar/canvas
/// should resolve through one of these fields so swapping a theme skin-changes the whole UI.
#[derive(Clone)]
pub struct Theme {
    // ── egui Visuals ──────────────────────────────────────────────────────────
    pub panel_bg: Color32,
    pub surface_bg: Color32,
    pub hover_bg: Color32,
    pub active_bg: Color32,
    pub active_text: Color32,
    pub selection_fill: Color32,
    pub selection_stroke: Color32,
    // ── Chrome ───────────────────────────────────────────────────────────────
    /// Top workspace-switcher strip — the darkest chrome band.
    pub chrome_bg: Color32,
    /// Toolbars/status bar — one step above `panel_bg`.
    pub toolbar_bg: Color32,
    /// Hairline separators between toolbar groups and panel sections.
    pub divider: Color32,
    /// The void behind the image on the central canvas.
    pub canvas_void: Color32,
    // ── Accent ───────────────────────────────────────────────────────────────
    pub accent: Color32,
    pub accent_dim: Color32,
    pub accent_subtle: Color32,
    pub accent_secondary: Color32,
    /// Text/icon color that reads on top of a solid `accent` fill.
    pub on_accent: Color32,
    /// Destructive-action tint (delete, clear, redact).
    pub danger: Color32,
    // ── Tool buttons ─────────────────────────────────────────────────────────
    pub tool_active_fill: Color32,
    pub tool_hover_fill: Color32,
    pub tool_icon: Color32,
    /// Icon color for the *active* tool button (sits on `tool_active_fill`).
    pub tool_icon_active: Color32,
    pub border_active: Color32,
    pub border_idle: Color32,
    // ── Text ─────────────────────────────────────────────────────────────────
    pub text_heading: Color32,
    pub text_label: Color32,
    pub text_muted: Color32,
    pub text_dim: Color32,
    pub text_ok: Color32,
    // ── Tab bar ──────────────────────────────────────────────────────────────
    pub tab_canvas_bg: Color32,
    pub tab_active_bg: Color32,
    pub tab_inactive_bg: Color32,
    pub tab_active_fg: Color32,
    pub tab_inactive_fg: Color32,
    // ── Dialog / overlay ─────────────────────────────────────────────────────
    pub dialog_key_bg: Color32,
    pub dialog_key_border: Color32,
    pub dialog_dim_text: Color32,
    pub dialog_recording_bg: Color32,
    pub dialog_row_border: Color32,
    // ── Layers panel ─────────────────────────────────────────────────────────
    pub layer_selected_bg: Color32,
    pub layer_selected_text: Color32,
}

impl Theme {
    pub fn build_visuals(&self) -> egui::Visuals {
        let mut v = egui::Visuals::dark();
        v.window_rounding = egui::Rounding::same(10.0);
        v.menu_rounding = egui::Rounding::same(8.0);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.rounding = egui::Rounding::same(6.0);
            w.expansion = 0.0;
        }
        v.panel_fill = self.panel_bg;
        v.window_fill = self.panel_bg;
        v.window_stroke = egui::Stroke::new(1.0, self.divider);
        v.extreme_bg_color = self.surface_bg;
        v.faint_bg_color = self.toolbar_bg;
        v.code_bg_color = self.surface_bg;

        v.widgets.noninteractive.bg_fill = self.panel_bg;
        v.widgets.noninteractive.weak_bg_fill = self.panel_bg;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, self.divider);
        v.widgets.noninteractive.fg_stroke.color = self.text_muted;

        v.widgets.inactive.bg_fill = self.surface_bg;
        v.widgets.inactive.weak_bg_fill = self.surface_bg;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, self.border_idle);
        v.widgets.inactive.fg_stroke.color = self.text_muted;

        v.widgets.hovered.bg_fill = self.hover_bg;
        v.widgets.hovered.weak_bg_fill = self.hover_bg;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, self.accent_dim);
        v.widgets.hovered.fg_stroke.color = self.text_heading;

        v.widgets.active.bg_fill = self.active_bg;
        v.widgets.active.weak_bg_fill = self.active_bg;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, self.accent);
        v.widgets.active.fg_stroke.color = self.active_text;

        v.widgets.open.bg_fill = self.hover_bg;
        v.widgets.open.weak_bg_fill = self.hover_bg;
        v.widgets.open.bg_stroke = egui::Stroke::new(1.0, self.border_idle);
        v.widgets.open.fg_stroke.color = self.text_heading;

        v.selection.bg_fill = self.selection_fill;
        v.selection.stroke.color = self.selection_stroke;

        v.hyperlink_color = self.accent_secondary;
        v.warn_fg_color = self.accent;
        v.error_fg_color = self.danger;
        v.popup_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 6.0),
            blur: 18.0,
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        };
        v.window_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 10.0),
            blur: 28.0,
            spread: 0.0,
            color: Color32::from_black_alpha(140),
        };
        v
    }
}

/// Global metrics: spacing, paddings and font sizes shared by every panel.
/// Called once per frame right after `set_visuals` so a theme swap can never
/// leave stale geometry behind.
pub fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(7.0, 6.0);
    s.button_padding = egui::vec2(9.0, 5.0);
    s.menu_margin = egui::Margin::symmetric(6.0, 6.0);
    s.indent = 14.0;
    s.interact_size.y = 24.0;
    s.slider_width = 110.0;
    s.combo_width = 110.0;
    s.icon_width = 16.0;
    s.icon_width_inner = 10.0;
    s.scroll.bar_width = 9.0;
    s.scroll.floating = false;

    use egui::{FontFamily::Proportional, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.0, Proportional)),
        (TextStyle::Body, FontId::new(13.5, Proportional)),
        (TextStyle::Button, FontId::new(13.5, Proportional)),
        (TextStyle::Heading, FontId::new(17.0, Proportional)),
        (
            TextStyle::Monospace,
            FontId::new(12.5, egui::FontFamily::Monospace),
        ),
    ]
    .into();

    style.visuals.button_frame = true;
    ctx.set_style(style);
}

/// Install Noto Sans as the UI typeface, keeping egui's built-in families as
/// fallbacks so emoji glyphs used across the toolbars still resolve.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSans-Regular.ttf")),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("inter".to_owned());
    ctx.set_fonts(fonts);
}

pub fn get_theme(id: ThemeId) -> Theme {
    match id {
        ThemeId::Graphite => graphite(),
        ThemeId::Midnight => midnight(),
        ThemeId::Ember => ember(),
    }
}

fn graphite() -> Theme {
    Theme {
        panel_bg: Color32::from_rgb(38, 40, 44),
        surface_bg: Color32::from_rgb(50, 53, 58),
        hover_bg: Color32::from_rgb(64, 68, 75),
        active_bg: Color32::from_rgb(227, 179, 65),
        active_text: Color32::from_rgb(26, 20, 6),
        selection_fill: Color32::from_rgb(120, 96, 36),
        selection_stroke: Color32::from_rgb(245, 214, 138),
        chrome_bg: Color32::from_rgb(24, 26, 29),
        toolbar_bg: Color32::from_rgb(33, 35, 39),
        divider: Color32::from_rgb(56, 59, 65),
        canvas_void: Color32::from_rgb(22, 23, 26),
        accent: Color32::from_rgb(233, 186, 74),
        accent_dim: Color32::from_rgb(160, 128, 52),
        accent_subtle: Color32::from_rgb(58, 47, 22),
        accent_secondary: Color32::from_rgb(86, 156, 245),
        on_accent: Color32::from_rgb(26, 20, 6),
        danger: Color32::from_rgb(232, 93, 87),
        tool_active_fill: Color32::from_rgb(233, 186, 74),
        tool_hover_fill: Color32::from_rgb(60, 64, 70),
        tool_icon: Color32::from_rgb(219, 222, 228),
        tool_icon_active: Color32::from_rgb(26, 20, 6),
        border_active: Color32::from_rgb(245, 208, 128),
        border_idle: Color32::from_rgb(62, 66, 73),
        text_heading: Color32::from_rgb(240, 242, 246),
        text_label: Color32::from_rgb(146, 152, 162),
        text_muted: Color32::from_rgb(199, 204, 213),
        text_dim: Color32::from_rgb(104, 110, 120),
        text_ok: Color32::from_rgb(88, 197, 118),
        tab_canvas_bg: Color32::from_rgb(24, 26, 29),
        tab_active_bg: Color32::from_rgb(46, 49, 55),
        tab_inactive_bg: Color32::from_rgb(30, 32, 36),
        tab_active_fg: Color32::from_rgb(238, 240, 245),
        tab_inactive_fg: Color32::from_rgb(140, 146, 156),
        dialog_key_bg: Color32::from_rgb(46, 49, 55),
        dialog_key_border: Color32::from_rgb(76, 81, 90),
        dialog_dim_text: Color32::from_rgb(132, 138, 148),
        dialog_recording_bg: Color32::from_rgb(58, 47, 22),
        dialog_row_border: Color32::from_rgb(52, 56, 62),
        layer_selected_bg: Color32::from_rgb(58, 50, 30),
        layer_selected_text: Color32::from_rgb(248, 226, 176),
    }
}

fn midnight() -> Theme {
    Theme {
        panel_bg: Color32::from_rgb(24, 30, 42),
        surface_bg: Color32::from_rgb(32, 40, 56),
        hover_bg: Color32::from_rgb(43, 55, 79),
        active_bg: Color32::from_rgb(56, 120, 220),
        active_text: Color32::WHITE,
        selection_fill: Color32::from_rgb(46, 100, 200),
        selection_stroke: Color32::from_rgb(180, 210, 255),
        chrome_bg: Color32::from_rgb(12, 16, 26),
        toolbar_bg: Color32::from_rgb(20, 26, 38),
        divider: Color32::from_rgb(40, 52, 74),
        canvas_void: Color32::from_rgb(9, 12, 20),
        accent: Color32::from_rgb(80, 170, 255),
        accent_dim: Color32::from_rgb(60, 130, 200),
        accent_subtle: Color32::from_rgb(16, 36, 72),
        accent_secondary: Color32::from_rgb(100, 190, 255),
        on_accent: Color32::from_rgb(6, 14, 28),
        danger: Color32::from_rgb(255, 105, 97),
        tool_active_fill: Color32::from_rgb(56, 120, 220),
        tool_hover_fill: Color32::from_rgb(40, 52, 76),
        tool_icon: Color32::from_rgb(206, 219, 238),
        tool_icon_active: Color32::WHITE,
        border_active: Color32::from_rgb(120, 190, 255),
        border_idle: Color32::from_rgb(44, 58, 84),
        text_heading: Color32::from_rgb(216, 233, 255),
        text_label: Color32::from_rgb(116, 145, 190),
        text_muted: Color32::from_rgb(168, 192, 225),
        text_dim: Color32::from_rgb(78, 100, 132),
        text_ok: Color32::from_rgb(60, 200, 110),
        tab_canvas_bg: Color32::from_rgb(8, 10, 18),
        tab_active_bg: Color32::from_rgb(30, 40, 62),
        tab_inactive_bg: Color32::from_rgb(14, 18, 30),
        tab_active_fg: Color32::from_rgb(210, 230, 255),
        tab_inactive_fg: Color32::from_rgb(100, 125, 170),
        dialog_key_bg: Color32::from_rgb(24, 36, 60),
        dialog_key_border: Color32::from_rgb(44, 68, 110),
        dialog_dim_text: Color32::from_rgb(100, 125, 170),
        dialog_recording_bg: Color32::from_rgb(16, 36, 72),
        dialog_row_border: Color32::from_rgb(28, 40, 64),
        layer_selected_bg: Color32::from_rgb(26, 50, 100),
        layer_selected_text: Color32::from_rgb(180, 215, 255),
    }
}

fn ember() -> Theme {
    Theme {
        panel_bg: Color32::from_rgb(44, 36, 28),
        surface_bg: Color32::from_rgb(56, 46, 36),
        hover_bg: Color32::from_rgb(72, 60, 46),
        active_bg: Color32::from_rgb(200, 130, 60),
        active_text: Color32::from_rgb(20, 12, 4),
        selection_fill: Color32::from_rgb(170, 110, 40),
        selection_stroke: Color32::from_rgb(255, 220, 150),
        chrome_bg: Color32::from_rgb(22, 16, 10),
        toolbar_bg: Color32::from_rgb(36, 29, 22),
        divider: Color32::from_rgb(66, 53, 39),
        canvas_void: Color32::from_rgb(18, 13, 8),
        accent: Color32::from_rgb(240, 130, 40),
        accent_dim: Color32::from_rgb(190, 100, 30),
        accent_subtle: Color32::from_rgb(64, 32, 8),
        accent_secondary: Color32::from_rgb(240, 80, 40),
        on_accent: Color32::from_rgb(28, 14, 2),
        danger: Color32::from_rgb(240, 80, 40),
        tool_active_fill: Color32::from_rgb(220, 140, 60),
        tool_hover_fill: Color32::from_rgb(72, 60, 46),
        tool_icon: Color32::from_rgb(240, 220, 195),
        tool_icon_active: Color32::from_rgb(28, 14, 2),
        border_active: Color32::from_rgb(255, 186, 116),
        border_idle: Color32::from_rgb(72, 58, 43),
        text_heading: Color32::from_rgb(255, 240, 220),
        text_label: Color32::from_rgb(168, 133, 96),
        text_muted: Color32::from_rgb(215, 195, 165),
        text_dim: Color32::from_rgb(110, 85, 62),
        text_ok: Color32::from_rgb(90, 210, 90),
        tab_canvas_bg: Color32::from_rgb(12, 8, 4),
        tab_active_bg: Color32::from_rgb(52, 40, 28),
        tab_inactive_bg: Color32::from_rgb(24, 18, 12),
        tab_active_fg: Color32::from_rgb(245, 225, 200),
        tab_inactive_fg: Color32::from_rgb(145, 115, 85),
        dialog_key_bg: Color32::from_rgb(52, 40, 28),
        dialog_key_border: Color32::from_rgb(95, 72, 50),
        dialog_dim_text: Color32::from_rgb(145, 115, 85),
        dialog_recording_bg: Color32::from_rgb(64, 32, 8),
        dialog_row_border: Color32::from_rgb(46, 36, 26),
        layer_selected_bg: Color32::from_rgb(70, 50, 28),
        layer_selected_text: Color32::from_rgb(255, 230, 195),
    }
}
