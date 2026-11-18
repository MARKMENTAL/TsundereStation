use eframe::egui;

pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(230, 30, 120);
pub const TV_ROW_HEIGHT: f32 = 52.0;

/// Apply the large, high-contrast styling used throughout the TV UI.
pub fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();

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
    style.spacing.menu_margin = egui::Margin::same(10);
    style.spacing.window_margin = egui::Margin::same(14);

    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    style.visuals.window_corner_radius = egui::CornerRadius::same(10);

    ctx.set_global_style(style);
}

/// A large, full-width menu row used by the launcher, file browser and pause
/// menu. The focused row gets the main TV accent.
pub fn tv_row(ui: &mut egui::Ui, focused: bool, label: &str) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, TV_ROW_HEIGHT), egui::Sense::click());

    let fill = if focused {
        egui::Color32::from_rgba_unmultiplied(230, 30, 120, 95)
    } else if response.hovered() {
        egui::Color32::from_white_alpha(18)
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
            egui::Stroke::new(3.0, ACCENT),
            egui::StrokeKind::Inside,
        );
    }

    let color = if focused {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_white_alpha(220)
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

pub fn controller_hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(text)
            .size(16.0)
            .color(egui::Color32::from_white_alpha(170)),
    );
}
