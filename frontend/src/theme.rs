use anyhow::{bail, Context, Result};
use eframe::egui::{
    self, Color32, CornerRadius, Margin, Stroke, Theme as EguiTheme, ThemePreference,
};
use serde::Deserialize;

const EMBEDDED_THEMES: &str = include_str!("../assets/themes.toml");

/// Minimum contrast ratio for enabled UI text against its background.
pub const MIN_CONTRAST: f32 = 2.5;

/// Extra headroom so integer color rounding cannot drop a repaired color below
/// [`MIN_CONTRAST`].
const CONTRAST_MARGIN: f32 = 0.02;

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub foreground: Color32,
    pub background: Color32,
    pub normal: [Color32; 8],
    pub bright: [Color32; 8],
}

#[derive(Clone, Copy, Debug)]
pub struct UiTheme {
    pub background: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_on_hover: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub focus_fill: Color32,
    pub hover: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub info: Color32,
    pub dark: bool,
}

#[derive(Deserialize)]
struct ThemeFileEntry {
    foreground: String,
    background: String,
    normal: [String; 8],
    bright: [String; 8],
}

impl Theme {
    pub fn fallback() -> Self {
        Self {
            name: "JAPS1ME".to_string(),
            foreground: Color32::from_rgb(0xf5, 0xf5, 0xf4),
            background: Color32::BLACK,
            normal: [
                Color32::from_rgb(0x0a, 0x0a, 0x0b),
                Color32::from_rgb(0xdf, 0x00, 0x24),
                Color32::from_rgb(0x00, 0xab, 0x9f),
                Color32::from_rgb(0xf3, 0xc3, 0x00),
                Color32::from_rgb(0x2e, 0x6d, 0xb4),
                Color32::from_rgb(0xf6, 0x9d, 0xc8),
                Color32::from_rgb(0x6b, 0xe4, 0xdf),
                Color32::from_rgb(0xf5, 0xf5, 0xf4),
            ],
            bright: [
                Color32::from_rgb(0x1f, 0x1f, 0x23),
                Color32::from_rgb(0xf7, 0x5f, 0x66),
                Color32::from_rgb(0x00, 0xab, 0x9f),
                Color32::from_rgb(0xf3, 0xc3, 0x00),
                Color32::from_rgb(0x76, 0x7e, 0xe1),
                Color32::from_rgb(0xf6, 0x9d, 0xc8),
                Color32::from_rgb(0x6b, 0xe4, 0xdf),
                Color32::WHITE,
            ],
        }
    }

    pub fn ui(&self) -> UiTheme {
        let dark = luminance(self.background) < 0.5;
        let surface = mix(self.background, self.foreground, 0.07);
        let surface2 = mix(self.background, self.foreground, 0.13);
        let border = mix(self.background, self.foreground, 0.28);
        let accent = self.normal[5];
        let hover = mix(surface, accent, 0.20);

        // `surface2` is the text-edit background and the surface closest to the
        // theme's text color, so it is the hardest text background to read.
        // Repairing against it also keeps text readable on `surface` and
        // `background`.
        let text = ensure_contrast(self.foreground, surface2);
        let muted = ensure_contrast(mix(self.background, self.foreground, 0.62), surface2);
        let text_on_hover = ensure_contrast(text, hover);

        UiTheme {
            background: self.background,
            surface,
            surface2,
            border,
            text,
            text_on_hover,
            muted,
            accent,
            on_accent: higher_contrast_text(accent),
            focus_fill: mix(self.background, accent, 0.38),
            hover,
            warning: ensure_contrast(self.normal[3], surface),
            error: ensure_contrast(self.normal[1], surface),
            info: ensure_contrast(self.normal[4], surface),
            dark,
        }
    }

    /// Color used to draw this theme's name in the theme picker. The picker is
    /// part of Settings, so it must honor the same contrast floor as the rest
    /// of the UI while the ANSI swatches still show the exact palette.
    pub fn preview_text_color(&self) -> Color32 {
        ensure_contrast(self.foreground, self.background)
    }

    /// Apply this theme's colors and the large TV layout.
    pub fn apply(&self, ctx: &egui::Context) -> UiTheme {
        let ui = self.ui();
        let mut style = (*ctx.style_of(EguiTheme::Dark)).clone();

        style.text_styles = [
            (egui::TextStyle::Heading, egui::FontId::proportional(32.0)),
            (egui::TextStyle::Body, egui::FontId::proportional(20.0)),
            (egui::TextStyle::Button, egui::FontId::proportional(20.0)),
            (egui::TextStyle::Small, egui::FontId::proportional(16.0)),
            (egui::TextStyle::Monospace, egui::FontId::monospace(17.0)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(14.0, 10.0);
        style.spacing.button_padding = egui::vec2(16.0, 10.0);
        style.spacing.interact_size = egui::vec2(52.0, 42.0);
        style.spacing.slider_width = 260.0;
        style.spacing.combo_width = 300.0;
        style.spacing.text_edit_width = 320.0;
        style.spacing.menu_margin = Margin::same(10);
        style.spacing.window_margin = Margin::same(14);

        let mut visuals = if ui.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.dark_mode = ui.dark;
        // Widget text colors are set per state below. A global override would
        // also apply to hovered/active widgets, which have their own fills.
        visuals.override_text_color = None;
        visuals.weak_text_color = Some(ui.muted);
        visuals.weak_text_alpha = 1.0;
        visuals.text_cursor.stroke = Stroke::new(2.0, ui.text);
        visuals.panel_fill = ui.background;
        visuals.window_fill = ui.surface;
        visuals.faint_bg_color = ui.surface;
        visuals.extreme_bg_color = ui.surface2;
        visuals.text_edit_bg_color = Some(ui.surface2);
        visuals.code_bg_color = ui.surface2;
        visuals.hyperlink_color = ui.info;
        visuals.warn_fg_color = ui.warning;
        visuals.error_fg_color = ui.error;
        visuals.window_stroke = Stroke::new(1.0, ui.border);
        visuals.window_corner_radius = CornerRadius::same(10);
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.selection = egui::style::Selection {
            bg_fill: ui.accent,
            stroke: Stroke::new(1.0, ui.on_accent),
        };

        let inactive = egui::style::WidgetVisuals {
            bg_fill: ui.surface,
            weak_bg_fill: ui.surface,
            bg_stroke: Stroke::new(1.0, ui.border),
            corner_radius: CornerRadius::same(6),
            fg_stroke: Stroke::new(1.0, ui.text),
            expansion: 0.0,
        };
        visuals.widgets.noninteractive = egui::style::WidgetVisuals {
            bg_fill: ui.background,
            weak_bg_fill: ui.background,
            bg_stroke: Stroke::new(1.0, ui.border),
            corner_radius: CornerRadius::same(6),
            // Labels and headings are non-interactive; they use the normal
            // theme text color, not the muted color.
            fg_stroke: Stroke::new(1.0, ui.text),
            expansion: 0.0,
        };
        visuals.widgets.inactive = inactive;
        visuals.widgets.hovered = egui::style::WidgetVisuals {
            bg_fill: ui.hover,
            weak_bg_fill: ui.hover,
            bg_stroke: Stroke::new(1.0, ui.accent),
            corner_radius: CornerRadius::same(6),
            fg_stroke: Stroke::new(1.5, ui.text_on_hover),
            expansion: 0.0,
        };
        visuals.widgets.active = egui::style::WidgetVisuals {
            bg_fill: ui.accent,
            weak_bg_fill: ui.accent,
            bg_stroke: Stroke::new(1.0, ui.on_accent),
            corner_radius: CornerRadius::same(6),
            fg_stroke: Stroke::new(1.5, ui.on_accent),
            expansion: 0.0,
        };
        visuals.widgets.open = visuals.widgets.active;

        style.visuals = visuals;
        ctx.set_theme(ThemePreference::Dark);
        ctx.set_style_of(EguiTheme::Dark, style.clone());
        ctx.set_style_of(EguiTheme::Light, style);

        ui
    }
}

impl UiTheme {
    /// Pick the higher-contrast of black and white for a filled
    /// accent/focus/toast background.
    pub fn on_color(&self, color: Color32) -> Color32 {
        higher_contrast_text(color)
    }
}

pub fn external_path() -> std::path::PathBuf {
    crate::config::config_dir()
        .join("tsunderestation")
        .join("themes.toml")
}

/// Load embedded themes and append any themes from the optional external file.
/// External entries with an existing name replace that entry in place.
pub fn load_catalog() -> (Vec<Theme>, Option<String>) {
    let mut themes = match parse_themes(EMBEDDED_THEMES) {
        Ok(themes) => themes,
        Err(e) => {
            log::error!("Could not parse embedded themes: {e:#}");
            return (
                vec![Theme::fallback()],
                Some(format!("Embedded themes failed to parse: {e:#}")),
            );
        }
    };

    let path = external_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse_themes(&text) {
                Ok(external) => {
                    for theme in external {
                        if let Some(existing) = themes.iter_mut().find(|t| t.name == theme.name) {
                            *existing = theme;
                        } else {
                            themes.push(theme);
                        }
                    }
                }
                Err(e) => {
                    let message = format!("Could not parse {}: {e:#}", path.display());
                    log::warn!("{message}");
                    return (themes, Some(message));
                }
            },
            Err(e) => {
                let message = format!("Could not read {}: {e}", path.display());
                log::warn!("{message}");
                return (themes, Some(message));
            }
        }
    }

    (themes, None)
}

fn parse_themes(text: &str) -> Result<Vec<Theme>> {
    let value: toml::Value = toml::from_str(text).context("parsing themes TOML")?;
    let table = value.as_table().context("themes TOML must be a table")?;
    let mut themes = Vec::with_capacity(table.len());
    for (name, value) in table {
        let entry: ThemeFileEntry = value
            .clone()
            .try_into()
            .with_context(|| format!("parsing theme {name:?}"))?;
        themes.push(Theme {
            name: name.clone(),
            foreground: parse_hex(&entry.foreground)
                .with_context(|| format!("{name}: foreground"))?,
            background: parse_hex(&entry.background)
                .with_context(|| format!("{name}: background"))?,
            normal: parse_colors(&entry.normal).with_context(|| format!("{name}: normal"))?,
            bright: parse_colors(&entry.bright).with_context(|| format!("{name}: bright"))?,
        });
    }
    if themes.is_empty() {
        bail!("no themes were found");
    }
    Ok(themes)
}

fn parse_colors(values: &[String; 8]) -> Result<[Color32; 8]> {
    let mut out = [Color32::BLACK; 8];
    for (index, value) in values.iter().enumerate() {
        out[index] = parse_hex(value)?;
    }
    Ok(out)
}

fn parse_hex(value: &str) -> Result<Color32> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 {
        bail!("expected #rrggbb, got {value:?}");
    }
    let number =
        u32::from_str_radix(value, 16).with_context(|| format!("invalid color {value:?}"))?;
    Ok(Color32::from_rgb(
        ((number >> 16) & 0xff) as u8,
        ((number >> 8) & 0xff) as u8,
        (number & 0xff) as u8,
    ))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let blend = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(
        blend(a.r(), b.r()),
        blend(a.g(), b.g()),
        blend(a.b(), b.b()),
    )
}

/// Pick whichever of black or white has the higher contrast ratio against
/// `background`.
fn higher_contrast_text(background: Color32) -> Color32 {
    if contrast_ratio(background, Color32::BLACK) >= contrast_ratio(background, Color32::WHITE) {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Return a variant of `color` that reaches [`MIN_CONTRAST`] against
/// `background`. Colors that are already readable are returned unchanged.
/// Otherwise the color is moved toward black or white by the smallest amount
/// that reaches the floor, preserving as much of the original hue as possible.
fn ensure_contrast(color: Color32, background: Color32) -> Color32 {
    if contrast_ratio(color, background) >= MIN_CONTRAST {
        return color;
    }

    let target = MIN_CONTRAST + CONTRAST_MARGIN;
    let toward = |destination: Color32| -> Option<(f32, Color32)> {
        for step in 1..=100 {
            let amount = step as f32 / 100.0;
            let candidate = mix(color, destination, amount);
            if contrast_ratio(candidate, background) >= target {
                return Some((amount, candidate));
            }
        }
        None
    };

    match (toward(Color32::BLACK), toward(Color32::WHITE)) {
        (Some((black_amount, black)), Some((white_amount, white))) => {
            if black_amount <= white_amount {
                black
            } else {
                white
            }
        }
        (Some((_, black)), None) => black,
        (None, Some((_, white))) => white,
        (None, None) => higher_contrast_text(background),
    }
}

/// WCAG contrast ratio between two colors.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

fn luminance(color: Color32) -> f32 {
    let channel = |value: u8| {
        let value = value as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// Draw one theme picker row with a live foreground/background sample and all
/// 16 ANSI colors. The selection border uses the active UI accent.
pub fn theme_picker_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    selected: bool,
    active_accent: Color32,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 56.0));
    let response = ui.interact(
        rect,
        ui.id().with(("theme-picker", &theme.name)),
        egui::Sense::click(),
    );
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(8), theme.background);
    let border = if selected {
        Stroke::new(3.0, active_accent)
    } else if response.hovered() {
        Stroke::new(2.0, mix(theme.background, active_accent, 0.7))
    } else {
        Stroke::new(1.0, mix(theme.background, theme.foreground, 0.25))
    };
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        border,
        egui::StrokeKind::Inside,
    );

    painter.text(
        rect.left_center() + egui::vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        &theme.name,
        egui::FontId::proportional(20.0),
        theme.preview_text_color(),
    );

    let swatch = 14.0;
    let gap = 3.0;
    let total = 16.0 * swatch + 15.0 * gap;
    let start_x = (rect.right() - total - 16.0).max(rect.left() + 260.0);
    for index in 0..16 {
        let color = if index < 8 {
            theme.normal[index]
        } else {
            theme.bright[index - 8]
        };
        let x = start_x + index as f32 * (swatch + gap);
        let swatch_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - swatch * 0.5),
            egui::vec2(swatch, swatch),
        );
        painter.rect_filled(swatch_rect, CornerRadius::same(2), color);
        painter.rect_stroke(
            swatch_rect,
            CornerRadius::same(2),
            Stroke::new(1.0, mix(theme.background, theme.foreground, 0.35)),
            egui::StrokeKind::Inside,
        );
    }

    response
}

/// Window frame for the custom-headed windows. The header and body provide
/// their own padding so the accent header reaches the window edges.
pub fn window_frame(style: &egui::Style, theme: &UiTheme) -> egui::Frame {
    egui::Frame::window(style)
        .inner_margin(Margin::ZERO)
        .fill(theme.surface)
}

/// Padded body area below a [`window_header`].
pub fn window_body<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin::same(14))
        .show(ui, add_contents)
        .inner
}

/// Draw an accent-filled window header. The title and close button use
/// `on_accent`, so they stay readable even when the accent is a light color
/// such as JAPS1ME's pink. Returns `true` when the close button is clicked.
pub fn window_header(ui: &mut egui::Ui, title: &str, theme: &UiTheme) -> bool {
    let radius = ui.style().visuals.window_corner_radius;
    let mut close = false;
    egui::Frame::new()
        .fill(theme.accent)
        .inner_margin(Margin {
            left: 16,
            right: 10,
            top: 8,
            bottom: 8,
        })
        .corner_radius(CornerRadius {
            nw: radius.nw,
            ne: radius.ne,
            sw: 0,
            se: 0,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(28.0)
                        .strong()
                        .color(theme.on_accent),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = window_close_button(ui, theme);
                });
            });
        });
    close
}

pub fn page_header(ui: &mut egui::Ui, title: &str, theme: &UiTheme) -> bool {
    let mut back = false;
    egui::Frame::new()
        .fill(theme.accent)
        .inner_margin(Margin {
            left: 18,
            right: 14,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(28.0)
                        .strong()
                        .color(theme.on_accent),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let response = ui.add(
                        egui::Button::new(
                            egui::RichText::new("Back")
                                .size(18.0)
                                .color(theme.on_accent),
                        )
                        .fill(theme.accent)
                        .stroke(egui::Stroke::new(1.0, theme.on_accent)),
                    );
                    back = response.clicked();
                });
            });
        });
    back
}

fn window_close_button(ui: &mut egui::Ui, theme: &UiTheme) -> bool {
    let (_, rect) = ui.allocate_space(egui::vec2(34.0, 34.0));
    let response = ui.interact(
        rect,
        ui.id().with("window-close-button"),
        egui::Sense::click(),
    );
    let (fill, color) = if response.hovered() {
        (theme.on_accent, theme.accent)
    } else {
        (Color32::TRANSPARENT, theme.on_accent)
    };
    let painter = ui.painter();
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, CornerRadius::same(6), fill);
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "×",
        egui::FontId::proportional(24.0),
        color,
    );
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_themes() -> Vec<Theme> {
        let mut themes = parse_themes(EMBEDDED_THEMES).expect("embedded themes should parse");
        themes.push(Theme::fallback());
        themes
    }

    fn assert_contrast_floor(theme: &Theme) {
        let ui = theme.ui();
        let pairs = [
            ("text/background", ui.text, ui.background),
            ("text/surface", ui.text, ui.surface),
            ("text/surface2", ui.text, ui.surface2),
            ("text_on_hover/hover", ui.text_on_hover, ui.hover),
            ("muted/background", ui.muted, ui.background),
            ("muted/surface", ui.muted, ui.surface),
            ("muted/surface2", ui.muted, ui.surface2),
            ("warning/surface", ui.warning, ui.surface),
            ("error/surface", ui.error, ui.surface),
            ("info/surface", ui.info, ui.surface),
            ("on_accent/accent", ui.on_accent, ui.accent),
            (
                "on_focus/focus_fill",
                ui.on_color(ui.focus_fill),
                ui.focus_fill,
            ),
            ("on_info/info", ui.on_color(ui.info), ui.info),
            ("on_warning/warning", ui.on_color(ui.warning), ui.warning),
            ("on_error/error", ui.on_color(ui.error), ui.error),
            (
                "preview_text/background",
                theme.preview_text_color(),
                theme.background,
            ),
        ];

        for (name, foreground, background) in pairs {
            let ratio = contrast_ratio(foreground, background);
            assert!(
                ratio >= MIN_CONTRAST,
                "{}: {} ratio {ratio:.3} is below {MIN_CONTRAST}",
                theme.name,
                name
            );
        }
    }

    #[test]
    fn embedded_themes_meet_settings_contrast_floor() {
        for theme in all_themes() {
            assert_contrast_floor(&theme);
        }
    }

    #[test]
    fn filled_backgrounds_use_higher_contrast_text() {
        for theme in all_themes() {
            let ui = theme.ui();
            for (name, background) in [
                ("accent", ui.accent),
                ("focus_fill", ui.focus_fill),
                ("info", ui.info),
                ("warning", ui.warning),
                ("error", ui.error),
            ] {
                let chosen = ui.on_color(background);
                assert!(
                    chosen == Color32::BLACK || chosen == Color32::WHITE,
                    "{}: {name} chose {chosen:?}, expected black or white",
                    theme.name
                );
                let other = if chosen == Color32::BLACK {
                    Color32::WHITE
                } else {
                    Color32::BLACK
                };
                let chosen_ratio = contrast_ratio(chosen, background);
                let other_ratio = contrast_ratio(other, background);
                assert!(
                    chosen_ratio >= other_ratio,
                    "{}: {name} chose a lower-contrast text color ({chosen_ratio:.3} < {other_ratio:.3})",
                    theme.name
                );
                assert!(
                    chosen_ratio >= MIN_CONTRAST,
                    "{}: {name} text ratio {chosen_ratio:.3} is below {MIN_CONTRAST}",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn japs1me_pink_uses_black_text() {
        let themes = parse_themes(EMBEDDED_THEMES).expect("embedded themes should parse");
        let theme = themes
            .iter()
            .find(|theme| theme.name == "JAPS1ME")
            .expect("JAPS1ME theme should exist");
        let ui = theme.ui();
        assert_eq!(ui.accent, Color32::from_rgb(0xf6, 0x9d, 0xc8));
        assert_eq!(ui.on_accent, Color32::BLACK);
        assert!(contrast_ratio(ui.on_accent, ui.accent) >= MIN_CONTRAST);
    }

    #[test]
    fn pathological_external_theme_is_repaired() {
        let text = r##"
["Pathological"]
foreground = "#900020"
background = "#000000"
normal = ["#000000", "#550000", "#000000", "#101010", "#000010", "#777777", "#000000", "#900020"]
bright = ["#000000", "#550000", "#000000", "#101010", "#000010", "#777777", "#000000", "#900020"]
"##;
        let theme = parse_themes(text)
            .expect("pathological theme should parse")
            .remove(0);
        assert_contrast_floor(&theme);
    }
}
