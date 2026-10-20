//! Small New UI building blocks: icon buttons, islands, separators, chevrons.

use crate::theme::*;
use egui::{Align2, Color32, CornerRadius, ImageSource, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

pub fn paint_icon(ui: &Ui, rect: Rect, icon: ImageSource<'static>) {
    egui::Image::new(icon).paint_at(ui, Rect::from_center_size(rect.center(), vec2(16.0, 16.0)));
}

pub fn paint_icon_tinted(ui: &Ui, rect: Rect, icon: ImageSource<'static>, tint: Color32) {
    egui::Image::new(icon).tint(tint).paint_at(ui, Rect::from_center_size(rect.center(), vec2(16.0, 16.0)));
}

/// 26×26 toolbar button: transparent, hover/pressed fill, optional dropdown corner mark.
pub fn icon_btn(ui: &mut Ui, icon: ImageSource<'static>, tip: &str) -> Response {
    icon_btn_ex(ui, icon, tip, true, false, false)
}

pub fn icon_btn_ex(ui: &mut Ui, icon: ImageSource<'static>, tip: &str, enabled: bool, selected: bool, dropdown: bool) -> Response {
    icon_btn_sized(ui, icon, tip, enabled, selected, dropdown, 26.0, 16.0)
}

/// Main-toolbar tool window toggle: 20px icon in a 32px button (IntelliJ `@20x20` toolbar icons).
pub fn toolbar_toggle(ui: &mut Ui, icon: ImageSource<'static>, tip: &str, selected: bool) -> Response {
    icon_btn_sized(ui, icon, tip, true, selected, false, 32.0, 20.0)
}

#[allow(clippy::too_many_arguments)]
fn icon_btn_sized(
    ui: &mut Ui,
    icon: ImageSource<'static>,
    tip: &str,
    enabled: bool,
    selected: bool,
    dropdown: bool,
    size: f32,
    icon_size: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::CLICK } else { Sense::hover() });
    let fill = if selected || (enabled && resp.is_pointer_button_down_on()) {
        TOOLBAR_PRESSED
    } else if enabled && resp.hovered() {
        TOOLBAR_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(if size > 28.0 { 6 } else { 4 }), fill);
    let icon = if !enabled && icon.uri() == crate::icons::STOP.uri() { crate::icons::STOP_DISABLED } else { icon };
    let img = egui::Image::new(icon);
    let img = if enabled { img } else { img.tint(Color32::from_white_alpha(90)) };
    img.paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon_size, icon_size)));
    if dropdown {
        let c = rect.right_bottom() + vec2(-6.0, -6.0);
        ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(3.0, 0.0), c + vec2(3.0, 3.0), c + vec2(0.0, 3.0)], ICON, Stroke::NONE));
    }
    let label = tip.split("  ").next().unwrap_or_default().to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// IntelliJ "AsyncProcessIcon": 8 rounded spokes whose brightness rotates.
pub fn ij_spinner(ui: &Ui, center: egui::Pos2, radius: f32) {
    let t = ui.input(|i| i.time);
    let head = ((t * 10.0) as usize) % 8;
    let w = radius * 0.3;
    for i in 0..8usize {
        let a = i as f32 * std::f32::consts::TAU / 8.0 - std::f32::consts::FRAC_PI_2;
        let dir = vec2(a.cos(), a.sin());
        let age = (head + 8 - i) % 8; // 0 = brightest
        let alpha = (255.0 * (1.0 - age as f32 * 0.11)) as u8;
        let col = Color32::from_rgba_unmultiplied(0x9F, 0xA2, 0xA8, alpha);
        let (p0, p1) = (center + dir * radius * 0.42, center + dir * (radius - w / 2.0));
        ui.painter().line_segment([p0, p1], Stroke::new(w, col));
        ui.painter().circle_filled(p0, w / 2.0, col);
        ui.painter().circle_filled(p1, w / 2.0, col);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
}

/// Toolbar text button like `DDL`.
pub fn text_btn(ui: &mut Ui, text: &str, tip: &str, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.into(), ui_font(12.0), if enabled { TEXT } else { TEXT_DISABLED });
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 10.0, 26.0), if enabled { Sense::CLICK } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), TOOLBAR_HOVER);
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, TEXT);
    resp.on_hover_text(tip)
}

/// Flat dropdown label like `Playground ⌄` / `JSON ⌄` / `shop ⌄`.
pub fn dropdown(ui: &mut Ui, icon: Option<ImageSource<'static>>, text: &str, tip: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(text.into(), ui_font(12.5), TEXT);
    let w = galley.size().x + 26.0 + if icon.is_some() { 20.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::CLICK);
    if resp.hovered() || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), TOOLBAR_HOVER);
    }
    let mut x = rect.left() + 6.0;
    if let Some(i) = icon {
        paint_icon(ui, Rect::from_min_size(pos2(x - 2.0, rect.top() + 5.0), vec2(16.0, 16.0)), i);
        x += 18.0;
    }
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    chevron_down(ui, pos2(rect.right() - 10.0, rect.center().y), TEXT_MUTED);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, if tip.is_empty() { text } else { tip }));
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

pub fn chevron_down(ui: &Ui, c: egui::Pos2, color: Color32) {
    ui.painter().line(vec![c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)], Stroke::new(1.2, color));
}

pub fn vsep(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 26.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range().shrink(5.0), Stroke::new(1.0, TOOLBAR_BORDER));
}

/// Rounded island background; returns the inner rect.
pub fn island(ui: &Ui, rect: Rect) {
    ui.painter().rect_filled(rect, CornerRadius::same(ISLAND_RADIUS), ISLAND);
}

/// Child ui confined to `rect`, laid out left-to-right and vertically centered. Clipped, so a
/// toolbar in a narrowed panel never paints over its neighbour.
pub fn row_ui<R>(ui: &mut Ui, rect: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing.x = 2.0;
        f(ui)
    })
    .inner
}

pub fn rect_ui<R>(ui: &mut Ui, rect: Rect, f: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        f(ui)
    })
    .inner
}

/// Tab strip clipped to `rect` that scrolls horizontally (wheel or trackpad) when its tabs, `width`
/// wide, overflow, with a thin muted bar in the gap under them. `f` paints the tabs in the given row.
pub fn tab_strip<R>(ui: &mut Ui, rect: Rect, id: &str, width: f32, f: impl FnOnce(&mut Ui, Rect) -> R) -> R {
    rect_ui(ui, rect, |ui| {
        let s = &mut ui.spacing_mut().scroll;
        (s.bar_width, s.bar_outer_margin, s.foreground_color) = (4.0, 1.0, false);
        egui::ScrollArea::horizontal()
            .id_salt(id)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (row, _) = ui.allocate_exact_size(vec2(width, rect.height()), Sense::hover());
                f(ui, row)
            })
            .inner
    })
}

/// Scrolls the active tab of strip `id` into view when it becomes active (`key` changes), then
/// leaves the scroll position to the user.
pub fn reveal_active_tab(ui: &Ui, resp: &Response, id: &str, key: u64) {
    let id = egui::Id::new((id, "shown"));
    if ui.data(|d| d.get_temp::<u64>(id)) != Some(key) {
        ui.data_mut(|d| d.insert_temp(id, key));
        resp.scroll_to_me(None);
    }
}

/// Draggable splitter between two areas. Returns drag delta along the axis.
pub fn splitter(ui: &mut Ui, id: &str, rect: Rect, vertical: bool) -> f32 {
    let resp = ui.interact(rect, ui.id().with(id), Sense::DRAG);
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
    }
    let d = resp.drag_delta();
    if vertical { d.x } else { d.y }
}

pub fn text_at(ui: &Ui, pos: egui::Pos2, align: Align2, text: &str, font: egui::FontId, color: Color32) -> Rect {
    ui.painter().text(pos, align, text, font, color)
}

/// Header title row used by tool windows ("Database Explorer", "Services").
pub fn tool_header(ui: &mut Ui, rect: Rect, title: &str) {
    text_at(ui, pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, title, bold_font(13.0), TEXT);
}

/// Context menu whose top-left corner sits at the pointer, like IntelliJ: never flipped to the
/// pointer's left; near the window edge it is only shifted as much as needed to fit.
pub fn context_menu(resp: &Response, add: impl FnOnce(&mut Ui)) {
    egui::Popup::context_menu(resp).align(egui::RectAlign::BOTTOM_START).align_alternatives(&[]).gap(0.0).show(add);
}

/// Menu item with right-aligned shortcut, IntelliJ style.
/// Menu rows fill the menu, but report their natural width during egui's sizing pass so a menu is
/// as wide as its content (otherwise every popup grows to `menu_width` and gets pushed left).
pub fn menu_row_width(ui: &Ui, natural: f32) -> f32 {
    if ui.is_sizing_pass() { natural } else { ui.available_width().max(natural) }
}

pub fn text_w(ui: &Ui, t: &str, size: f32) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), ui_font(size), TEXT).size().x)
}

pub fn menu_item(ui: &mut Ui, icon: Option<ImageSource<'static>>, text: &str, shortcut: &str, enabled: bool) -> bool {
    menu_item_ex(ui, icon, text, shortcut, enabled, None)
}

/// `menu_item` with a pill after the text (IntelliJ's `Beta` / `New` badges).
pub fn menu_item_ex(
    ui: &mut Ui,
    icon: Option<ImageSource<'static>>,
    text: &str,
    shortcut: &str,
    enabled: bool,
    badge: Option<&str>,
) -> bool {
    let sc = if shortcut.is_empty() { 0.0 } else { 36.0 + text_w(ui, shortcut, 12.0) };
    let bw = badge.map_or(0.0, |b| text_w(ui, b, 11.5) + 22.0);
    let w = menu_row_width(ui, 28.0 + text_w(ui, text, 13.0) + bw + sc + 12.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), if enabled { Sense::CLICK } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), SEL_ACTIVE);
    }
    if let Some(i) = icon {
        menu_icon(ui, rect, i, enabled);
    }
    let col = if enabled { TEXT } else { TEXT_DISABLED };
    let t = text_at(ui, pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, text, ui_font(13.0), col);
    if let Some(b) = badge {
        let g = ui.painter().layout_no_wrap(b.into(), ui_font(11.5), Color32::from_rgb(0xC9, 0xB3, 0xF5));
        let r = Rect::from_min_size(pos2(t.right() + 8.0, rect.center().y - 9.0), vec2(g.size().x + 12.0, 18.0));
        ui.painter().rect_filled(r, CornerRadius::same(9), Color32::from_rgb(0x43, 0x36, 0x5E));
        ui.painter().galley(r.center() - g.size() / 2.0, g, TEXT);
    }
    if !shortcut.is_empty() {
        text_at(ui, pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, shortcut, ui_font(12.0), TEXT_SECONDARY);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    let clicked = resp.clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Leading icon of a menu row, faded when the item is disabled (like disabled toolbar buttons).
fn menu_icon(ui: &Ui, row: Rect, icon: ImageSource<'static>, enabled: bool) {
    let img = egui::Image::new(icon);
    let img = if enabled { img } else { img.tint(Color32::from_white_alpha(90)) };
    img.paint_at(ui, Rect::from_min_size(row.left_top() + vec2(6.0, 5.0), vec2(16.0, 16.0)));
}

/// Menu row opening a submenu on hover, like IntelliJ's `Data Source ›`. Returns the content's result
/// while the submenu is open.
pub fn submenu<R>(
    ui: &mut Ui,
    icon: Option<ImageSource<'static>>,
    text: &str,
    enabled: bool,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let w = menu_row_width(ui, 28.0 + text_w(ui, text, 13.0) + 40.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), if enabled { Sense::CLICK } else { Sense::hover() });
    let bg = ui.painter().add(egui::Shape::Noop);
    if let Some(i) = icon {
        menu_icon(ui, rect, i, enabled);
    }
    let col = if enabled { TEXT } else { TEXT_DISABLED };
    text_at(ui, pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, text, ui_font(13.0), col);
    let chevron = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(16.0, 16.0));
    paint_icon_tinted(ui, chevron, crate::icons::CHEVRON_RIGHT, if enabled { TEXT_MUTED } else { TEXT_DISABLED });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    if !enabled {
        return None;
    }
    let out = egui::containers::menu::SubMenu::new().show(ui, &resp, content);
    if out.is_some() || resp.hovered() {
        ui.painter().set(bg, egui::Shape::rect_filled(rect, CornerRadius::same(4), SEL_ACTIVE));
    }
    out.map(|r| r.inner)
}

pub fn menu_section(ui: &mut Ui, title: &str) {
    let w = menu_row_width(ui, 12.0 + text_w(ui, title, 12.0));
    let (rect, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    text_at(ui, pos2(rect.left() + 6.0, rect.center().y), Align2::LEFT_CENTER, title, ui_font(12.0), TEXT_SECONDARY);
}

pub fn menu_check(ui: &mut Ui, text: &str, checked: bool) -> bool {
    let w = menu_row_width(ui, 26.0 + text_w(ui, text, 13.0) + 14.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::CLICK);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), SEL_ACTIVE);
    }
    if checked {
        let c = pos2(rect.left() + 13.0, rect.center().y);
        ui.painter().line(vec![c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0), c + vec2(4.0, -3.5)], Stroke::new(1.4, TEXT));
    }
    text_at(ui, pos2(rect.left() + 26.0, rect.center().y), Align2::LEFT_CENTER, text, ui_font(13.0), TEXT);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, checked, text));
    let clicked = resp.clicked();
    if clicked {
        ui.close();
    }
    clicked
}
