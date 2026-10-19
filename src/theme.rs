//! Islands Dark tokens (from DataGrip's ManyIslandsDark.theme.json / IslandSchemeDark.xml).

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub const MAIN_BG: Color32 = rgb(0x26282C);
pub const ISLAND: Color32 = rgb(0x191A1C);
pub const ISLAND_INLINE: Color32 = rgb(0x212326);
pub const BORDER: Color32 = rgb(0x26282C);
pub const POPUP_BG: Color32 = rgb(0x26282C);
pub const POPUP_BORDER: Color32 = rgb(0x33353B);
pub const TEXT: Color32 = rgb(0xD1D3D9);
pub const TEXT_MUTED: Color32 = rgb(0x9FA2A8);
pub const TEXT_SECONDARY: Color32 = rgb(0x73767C);
pub const TEXT_DISABLED: Color32 = rgb(0x4C4F56);
/// Inline labels / placeholders (`find`, `sort` in the data editor's filter bar).
pub const TEXT_INFO: Color32 = rgb(0x868A91);
pub const LINK: Color32 = rgb(0x71A1FE);
pub const ERROR: Color32 = rgb(0xF57E84);
pub const ERROR_OUT: Color32 = rgb(0xF75464);
pub const SUCCESS: Color32 = rgb(0x6DB083);
pub const SEL_ACTIVE: Color32 = rgb(0x2A4371);
pub const SEL_INACTIVE: Color32 = rgb(0x33353B);
pub const HOVER: Color32 = Color32::from_rgba_premultiplied(0x10, 0x10, 0x10, 0x10);
pub const TOOLBAR_HOVER: Color32 = Color32::from_rgba_premultiplied(0x17, 0x17, 0x17, 0x17);
pub const TOOLBAR_PRESSED: Color32 = Color32::from_rgba_premultiplied(0x29, 0x29, 0x29, 0x29);
pub const TOOLBAR_BORDER: Color32 = Color32::from_rgba_premultiplied(0x21, 0x21, 0x21, 0x21);
pub const TAB_SEL_BG: Color32 = rgb(0x233558);
pub const TAB_SEL_BORDER: Color32 = rgb(0x2E4D89);
pub const BRAND: Color32 = rgb(0x3871E1);
pub const CONTROL_BORDER: Color32 = rgb(0x40434A);
pub const CONTROL_BG: Color32 = rgb(0x191A1C);
pub const ICON: Color32 = rgb(0xCED0D6);

// editor / grid
pub const EDITOR_TEXT: Color32 = rgb(0xBCBEC4);
pub const GRID_LINE: Color32 = rgb(0x323438);
pub const GRID_HEADER: Color32 = rgb(0x292A2C);
pub const LINE_NO: Color32 = rgb(0x4B5059);
pub const LINE_NO_CARET: Color32 = rgb(0xA1A3AB);
pub const CARET_ROW: Color32 = rgb(0x1F2024);
pub const CARET: Color32 = rgb(0xCED0D6);
pub const SELECTION: Color32 = rgb(0x214283);
pub const KEYWORD: Color32 = rgb(0xCF8E6D);
pub const STRING: Color32 = rgb(0x6AAB73);
pub const NUMBER: Color32 = rgb(0x2AACB8);
pub const COMMENT: Color32 = rgb(0x7A7E85);
pub const FIELD: Color32 = rgb(0xC77DBB);
pub const FUNC: Color32 = rgb(0x56A8F5);
pub const ADDED_ROW: Color32 = rgb(0x203B2A);
pub const DELETED_ROW: Color32 = rgb(0x56272B);

pub const ISLAND_RADIUS: u8 = 10;
pub const GAP: f32 = 6.0;
pub const ROW_H: f32 = 24.0;

pub fn ui_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, name: &str, data: Vec<u8>| {
        fonts.font_data.insert(name.into(), egui::FontData::from_owned(data).into());
    };
    // macOS UI font like DataGrip; Inter (bundled, OFL) elsewhere.
    match std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        Ok(sf) => add(&mut fonts, "ui", sf),
        Err(_) => add(&mut fonts, "ui", include_bytes!("../assets/fonts/Inter-Regular.otf").to_vec()),
    }
    add(&mut fonts, "bold", include_bytes!("../assets/fonts/Inter-SemiBold.otf").to_vec());
    add(&mut fonts, "mono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").to_vec());
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let p = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    p.insert(0, "ui".into());
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "mono".into());
    let mut bold = vec!["bold".to_string()];
    bold.extend(fallback);
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.text_styles = [
            (egui::TextStyle::Small, ui_font(11.0)),
            (egui::TextStyle::Body, ui_font(13.0)),
            (egui::TextStyle::Button, ui_font(13.0)),
            (egui::TextStyle::Heading, bold_font(13.0)),
            (egui::TextStyle::Monospace, mono(13.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.interaction.selectable_labels = false;
        let mut v = Visuals::dark();
        v.panel_fill = ISLAND;
        v.window_fill = POPUP_BG;
        v.window_stroke = Stroke::new(1.0, POPUP_BORDER);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(8);
        v.extreme_bg_color = CONTROL_BG;
        v.faint_bg_color = ISLAND_INLINE;
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = LINK;
        v.selection.bg_fill = SELECTION;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.text_cursor.stroke = Stroke::new(1.5, CARET);
        v.window_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
        v.popup_shadow = v.window_shadow;
        let w = &mut v.widgets;
        for st in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            st.corner_radius = CornerRadius::same(4);
            st.fg_stroke = Stroke::new(1.0, TEXT);
        }
        w.noninteractive.bg_stroke = Stroke::new(1.0, POPUP_BORDER);
        w.noninteractive.bg_fill = ISLAND;
        w.inactive.bg_fill = Color32::TRANSPARENT;
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_stroke = Stroke::new(1.0, CONTROL_BORDER);
        w.hovered.bg_fill = TOOLBAR_HOVER;
        w.hovered.weak_bg_fill = TOOLBAR_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, CONTROL_BORDER);
        w.active.bg_fill = TOOLBAR_PRESSED;
        w.active.weak_bg_fill = TOOLBAR_PRESSED;
        w.active.bg_stroke = Stroke::new(1.0, BRAND);
        w.open.weak_bg_fill = TOOLBAR_HOVER;
        s.visuals = v;
    });
}
