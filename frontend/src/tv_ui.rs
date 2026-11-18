use crate::theme::UiTheme;
use eframe::egui;

pub const TV_ROW_HEIGHT: f32 = 52.0;

/// A large, full-width menu row used by the launcher, file browser and pause
/// menu. The focused row uses the active theme's accent.
pub fn tv_row(
    ui: &mut egui::Ui,
    focused: bool,
    label: &str,
    theme: &UiTheme,
    key: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let width = ui.available_width();
    let (_, rect) = ui.allocate_space(egui::vec2(width, TV_ROW_HEIGHT));
    let response = ui.interact(rect, ui.id().with(key), egui::Sense::click());

    let fill = if focused {
        theme.focus_fill
    } else if response.hovered() {
        theme.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), fill);
    }
    if focused {
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(8),
            egui::Stroke::new(3.0, theme.accent),
            egui::StrokeKind::Inside,
        );
    }

    let color = if focused {
        // The focus fill is a blend of the panel and accent, so choose the
        // text color from that fill rather than from the pure accent.
        theme.on_color(theme.focus_fill)
    } else if response.hovered() {
        theme.text_on_hover
    } else {
        theme.text
    };
    ui.painter().text(
        rect.left_center() + egui::vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(20.0),
        color,
    );

    response
}

pub fn controller_hint(ui: &mut egui::Ui, text: &str, theme: &UiTheme) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).size(16.0).color(theme.muted));
}
