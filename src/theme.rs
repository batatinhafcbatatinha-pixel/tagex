//! Fonts and visual style.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(0x7c, 0x6c, 0xff);
pub const BG: Color32 = Color32::from_rgb(0x13, 0x15, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x18, 0x1b, 0x21);
pub const CARD: Color32 = Color32::from_rgb(0x1f, 0x23, 0x2b);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x27, 0x2c, 0x37);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xee);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8d, 0x94, 0xa3);
pub const DANGER: Color32 = Color32::from_rgb(0xf0, 0x6c, 0x7a);

pub fn selected_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(0x7c, 0x6c, 0xff, 0x38)
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = Color32::from_rgb(0x1c, 0x1f, 0x27);
    v.extreme_bg_color = Color32::from_rgb(0x0f, 0x11, 0x15);
    v.faint_bg_color = Color32::from_rgb(0x1a, 0x1d, 0x24);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.override_text_color = None;

    let r = CornerRadius::same(6);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = r;
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x24, 0x28, 0x31);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x24, 0x28, 0x31);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2e, 0x33, 0x3f);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x2e, 0x33, 0x3f);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x40, 0x4e));
    v.widgets.active.corner_radius = r;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.open.corner_radius = r;

    // Always dark, regardless of the Windows light/dark setting.
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let load = |file: &str| std::fs::read(format!("{windir}\\Fonts\\{file}")).ok();

    let mut front = Vec::new();
    if let Some(bytes) = load("segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), FontData::from_owned(bytes).into());
        front.push("segoe".to_string());
    }
    // Symbols used for icons (↻, ⊕, ▦, ☰ …) fall back to Segoe UI Symbol.
    let mut back = Vec::new();
    if let Some(bytes) = load("seguisym.ttf") {
        fonts.font_data.insert("segoe-sym".into(), FontData::from_owned(bytes).into());
        back.push("segoe-sym".to_string());
    }
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    for (i, f) in front.into_iter().enumerate() {
        prop.insert(i, f);
    }
    prop.extend(back);

    if let Some(bytes) = load("CascadiaMono.ttf").or_else(|| load("consola.ttf")) {
        fonts.font_data.insert("mono".into(), FontData::from_owned(bytes).into());
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    }
    ctx.set_fonts(fonts);
}
