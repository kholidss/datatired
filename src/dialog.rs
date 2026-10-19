//! "Data Sources and Drivers" dialog (MongoDB / PostgreSQL drivers, General tab).

use crate::config::{Auth, ConnType, DataSource, Driver};
use crate::icons as ic;
use crate::theme::*;
use crate::widgets::*;
use egui::{Align2, CornerRadius, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;

pub enum TestState {
    Idle,
    Running,
    Ok(String),
    Err(String),
}

pub struct DsDialog {
    pub items: Vec<DataSource>,
    pub passwords: HashMap<u64, String>,
    pub sel: usize,
    tab: usize,
    pub test: TestState,
    show_comment: bool,
}

pub enum DialogOut {
    None,
    Apply,
    Ok,
    Cancel,
    Test(DataSource, String),
}

const READ_PREFS: [(&str, &str); 6] = [
    ("", "Default"),
    ("primary", "primary"),
    ("primaryPreferred", "primaryPreferred"),
    ("secondary", "secondary"),
    ("secondaryPreferred", "secondaryPreferred"),
    ("nearest", "nearest"),
];

impl DsDialog {
    pub fn new(items: Vec<DataSource>, passwords: HashMap<u64, String>, select: Option<u64>, create: Option<Driver>) -> Self {
        let mut d = DsDialog { sel: 0, items, passwords, tab: 0, test: TestState::Idle, show_comment: false };
        // like DataGrip: the URL as typed (with its password) and the fields filled from it
        for ds in &mut d.items {
            ds.url = ds.url_with_password(d.passwords.get(&ds.id).map(String::as_str));
            ds.sync_from_url();
        }
        if let Some(driver) = create {
            d.add(driver);
        } else if let Some(id) = select {
            d.sel = d.items.iter().position(|x| x.id == id).unwrap_or(0);
        }
        d
    }

    fn add(&mut self, driver: Driver) {
        let mut ds = DataSource { id: crate::config::new_ds_id(), ..DataSource::new(driver) };
        ds.name = format!("@{}", ds.host);
        self.items.push(ds);
        self.sel = self.items.len() - 1;
        self.test = TestState::Idle;
    }

    pub fn show(&mut self, ctx: &egui::Context) -> DialogOut {
        let mut out = DialogOut::None;
        let modal = egui::Modal::new(egui::Id::new("ds-dialog")).backdrop_color(egui::Color32::from_black_alpha(40)).frame(
            egui::Frame::new().fill(ISLAND).stroke(Stroke::new(1.0, POPUP_BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(0),
        );
        let resp = modal.show(ctx, |ui| {
            let size = vec2(1000.0, 690.0).min(ctx.content_rect().size() - vec2(40.0, 40.0));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            // title
            text_at(
                ui,
                pos2(rect.left() + 82.0, rect.top() + 16.0),
                Align2::LEFT_CENTER,
                "Data Sources and Drivers",
                bold_font(13.0),
                TEXT,
            );
            let body = Rect::from_min_max(rect.left_top() + vec2(0.0, 32.0), rect.right_bottom() - vec2(0.0, 52.0));
            // left strip with categories
            let strip = Rect::from_min_size(body.min, vec2(44.0, body.height()));
            for (i, icon) in [ic::MONGO_DB, ic::DS_PROPERTIES, ic::PROJECT_STRUCTURE].into_iter().enumerate() {
                let r = Rect::from_min_size(strip.min + vec2(8.0, 6.0 + i as f32 * 30.0), vec2(28.0, 28.0));
                if i == 0 {
                    ui.painter().rect_filled(r, CornerRadius::same(4), TOOLBAR_PRESSED);
                }
                paint_icon(ui, r, icon);
            }
            ui.painter().vline(strip.right(), body.y_range(), Stroke::new(1.0, BORDER));
            // list
            let list = Rect::from_min_max(pos2(strip.right(), body.top()), pos2(strip.right() + 200.0, body.bottom()));
            ui.painter().vline(list.right(), body.y_range(), Stroke::new(1.0, BORDER));
            row_ui(ui, Rect::from_min_size(list.min + vec2(6.0, 4.0), vec2(list.width() - 12.0, 28.0)), |ui| {
                let add = icon_btn_ex(ui, ic::ADD, "Add", true, false, true);
                egui::Popup::menu(&add).show(|ui| {
                    ui.set_min_width(160.0);
                    for d in [Driver::Mongo, Driver::Postgres] {
                        if menu_item(ui, Some(driver_icon(d)), d.name(), "", true) {
                            self.add(d);
                        }
                    }
                });
                if icon_btn_ex(ui, ic::REMOVE, "Remove", !self.items.is_empty(), false, false).clicked() && !self.items.is_empty() {
                    self.items.remove(self.sel);
                    self.sel = self.sel.saturating_sub(1);
                }
                if icon_btn_ex(ui, ic::COPY, "Duplicate", !self.items.is_empty(), false, false).clicked() && !self.items.is_empty() {
                    let mut c = self.items[self.sel].clone();
                    c.id = crate::config::new_ds_id();
                    c.name = format!("{} [2]", c.name);
                    self.items.push(c);
                    self.sel = self.items.len() - 1;
                }
            });
            text_at(ui, list.min + vec2(12.0, 50.0), Align2::LEFT_CENTER, "Project Data Sources", ui_font(12.0), TEXT_SECONDARY);
            for (i, ds) in self.items.iter().enumerate() {
                let r = Rect::from_min_size(list.min + vec2(6.0, 62.0 + i as f32 * 26.0), vec2(list.width() - 12.0, 24.0));
                let resp = ui.interact(r, ui.id().with(("dsi", i)), Sense::click());
                if i == self.sel {
                    ui.painter().rect_filled(r, CornerRadius::same(4), SEL_ACTIVE);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), HOVER);
                }
                paint_icon(ui, Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(16.0, 16.0)), driver_icon(ds.driver));
                ui.painter().with_clip_rect(r).text(r.min + vec2(26.0, 12.0), Align2::LEFT_CENTER, &ds.name, ui_font(13.0), TEXT);
                if resp.clicked() {
                    self.sel = i;
                    self.test = TestState::Idle;
                }
            }
            let py = list.min.y + 74.0 + self.items.len() as f32 * 26.0;
            text_at(ui, pos2(list.left() + 38.0, py + 12.0), Align2::LEFT_CENTER, "Problems", ui_font(13.0), TEXT);

            // form
            let form = Rect::from_min_max(pos2(list.right() + 12.0, body.top() + 8.0), body.right_bottom() - vec2(14.0, 0.0));
            if self.items.is_empty() {
                text_at(ui, form.center(), Align2::CENTER_CENTER, "Nothing to show", ui_font(13.0), TEXT_SECONDARY);
            } else {
                rect_ui(ui, form, |ui| self.form(ui, &mut out));
            }

            // footer
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 52.0), rect.max);
            ui.painter().hline(foot.x_range(), foot.top(), Stroke::new(1.0, BORDER));
            let help = Rect::from_center_size(pos2(foot.left() + 26.0, foot.center().y), vec2(24.0, 24.0));
            ui.painter().circle_stroke(help.center(), 10.0, Stroke::new(1.0, CONTROL_BORDER));
            text_at(ui, help.center(), Align2::CENTER_CENTER, "?", ui_font(13.0), TEXT);
            row_ui(ui, Rect::from_min_max(pos2(foot.right() - 300.0, foot.top() + 10.0), foot.right_bottom() - vec2(14.0, 10.0)), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if primary_btn(ui, "OK").clicked() {
                        out = DialogOut::Ok;
                    }
                    if ui.add(egui::Button::new("Apply").min_size(vec2(72.0, 28.0))).clicked() {
                        out = DialogOut::Apply;
                    }
                    if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 28.0))).clicked() {
                        out = DialogOut::Cancel;
                    }
                });
            });
        });
        if resp.should_close() && matches!(out, DialogOut::None) {
            out = DialogOut::Cancel;
        }
        out
    }

    fn form(&mut self, ui: &mut Ui, out: &mut DialogOut) {
        let label_w = 116.0;
        let sel = self.sel;
        let id = self.items[sel].id;
        let mut pw = self.passwords.get(&id).cloned().unwrap_or_default();
        let pw_before = pw.clone();
        let ds = &mut self.items[sel];
        let before = ds.clone();
        ui.spacing_mut().item_spacing.y = 10.0;
        let field_w = ui.available_width() - label_w;
        let label = |ui: &mut Ui, t: &str| {
            let (r, _) = ui.allocate_exact_size(vec2(label_w, 28.0), Sense::hover());
            text_at(ui, pos2(r.left(), r.center().y), Align2::LEFT_CENTER, t, ui_font(13.0), TEXT);
        };
        ui.horizontal(|ui| {
            label(ui, "Name:");
            ui.add(egui::TextEdit::singleline(&mut ds.name).desired_width(field_w - 210.0).margin(vec2(8.0, 5.0)));
            ui.label("Color:");
            egui::ComboBox::from_id_salt("ds-color").icon(combo_chevron).width(110.0).selected_text(color_name(ds.color)).show_ui(
                ui,
                |ui| {
                    for (c, n) in COLORS {
                        ui.selectable_value(&mut ds.color, c, n);
                    }
                },
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(label_w + 6.0);
            if self.show_comment || !ds.comment.is_empty() {
                ui.add(egui::TextEdit::singleline(&mut ds.comment).hint_text("Comment").desired_width(field_w - 10.0));
            } else if ui.link("Add comment").clicked() {
                self.show_comment = true;
            }
        });
        // tabs
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, t) in ["General", "Options", "SSH/SSL", "Schemas", "Advanced"].iter().enumerate() {
                let g = ui.painter().layout_no_wrap(t.to_string(), ui_font(13.0), TEXT);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 30.0), Sense::click());
                if i == self.tab {
                    ui.painter().rect(r, CornerRadius::same(4), TAB_SEL_BG, Stroke::new(1.0, TAB_SEL_BORDER), egui::StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(4), TOOLBAR_HOVER);
                }
                ui.painter().galley(r.center() - g.size() / 2.0, g, TEXT);
                if resp.clicked() {
                    self.tab = i;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled(false, egui::Link::new("Save as Template"));
            });
        });
        ui.add_space(4.0);
        if self.tab != 0 {
            ui.label(
                RichText::new(format!("These settings are not available for {} in DataTired yet.", ds.driver.name())).color(TEXT_SECONDARY),
            );
            if self.tab == 3 {
                ui.label(
                    RichText::new("Use the database counter next to the data source in the Database Explorer to choose databases.")
                        .color(TEXT_SECONDARY),
                );
            }
            return;
        }
        ui.horizontal(|ui| {
            label(ui, "Driver:");
            egui::ComboBox::from_id_salt("ds-driver").icon(combo_chevron).width(field_w - 8.0).selected_text(ds.driver.name()).show_ui(
                ui,
                |ui| {
                    for d in [Driver::Mongo, Driver::Postgres] {
                        ui.selectable_value(&mut ds.driver, d, d.name());
                    }
                },
            );
        });
        if ds.driver != before.driver {
            // another driver: its default port and URL, no MongoDB-only connection type
            if ds.port == before.driver.default_port() {
                ds.port = ds.driver.default_port().into();
            }
            if ds.conn_type == ConnType::Srv {
                ds.conn_type = ConnType::Default;
            }
            ds.url = ds.generated_url();
        }
        let pg = ds.driver == Driver::Postgres;
        ui.horizontal(|ui| {
            label(ui, "Connection type:");
            ui.spacing_mut().item_spacing.x = 0.0;
            for (t, n) in [(ConnType::Default, "default"), (ConnType::Srv, "MongoDB Atlas (SRV protocol)"), (ConnType::UrlOnly, "URL only")]
            {
                if pg && t == ConnType::Srv {
                    continue;
                }
                let g = ui.painter().layout_no_wrap(n.into(), ui_font(13.0), TEXT);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 28.0), Sense::click());
                ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, CONTROL_BORDER), egui::StrokeKind::Inside);
                if ds.conn_type == t {
                    ui.painter().rect(
                        r.shrink(2.0),
                        CornerRadius::same(3),
                        TOOLBAR_PRESSED,
                        Stroke::new(1.0, CONTROL_BORDER),
                        egui::StrokeKind::Inside,
                    );
                }
                ui.painter().galley(r.center() - g.size() / 2.0, g, TEXT);
                if resp.clicked() {
                    ds.conn_type = t;
                }
            }
        });
        let half = (field_w - 120.0) / 2.0;
        if ds.conn_type != ConnType::UrlOnly {
            ui.horizontal(|ui| {
                label(ui, "Host:");
                ui.add(egui::TextEdit::singleline(&mut ds.host).desired_width(half).margin(vec2(8.0, 5.0)));
                if ds.conn_type == ConnType::Default {
                    ui.add_sized(vec2(110.0, 28.0), egui::Label::new("Port:").halign(egui::Align::Center));
                    ui.add(egui::TextEdit::singleline(&mut ds.port).desired_width(half - 20.0).margin(vec2(8.0, 5.0)));
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            label(ui, "Authentication:");
            egui::ComboBox::from_id_salt("ds-auth")
                .icon(combo_chevron)
                .width(field_w - 8.0)
                .selected_text(if ds.auth == Auth::NoAuth { "No auth" } else { "User & Password" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut ds.auth, Auth::UserPassword, "User & Password");
                    ui.selectable_value(&mut ds.auth, Auth::NoAuth, "No auth");
                });
        });
        if ds.auth == Auth::UserPassword {
            ui.horizontal(|ui| {
                label(ui, "User:");
                ui.add(egui::TextEdit::singleline(&mut ds.user).desired_width(half).margin(vec2(8.0, 5.0)));
            });
            ui.horizontal(|ui| {
                label(ui, "Password:");
                ui.add(egui::TextEdit::singleline(&mut pw).password(true).hint_text("<hidden>").desired_width(half).margin(vec2(8.0, 5.0)));
                ui.add_sized(vec2(110.0, 28.0), egui::Label::new("Save:").halign(egui::Align::Center));
                egui::ComboBox::from_id_salt("ds-save").icon(combo_chevron).width(half - 28.0).selected_text("Forever").show_ui(ui, |ui| {
                    let _ = ui.selectable_label(true, "Forever");
                });
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            label(ui, "Database:");
            let w = if pg { field_w - 8.0 } else { half };
            ui.add(
                egui::TextEdit::singleline(&mut ds.database)
                    .desired_width(w)
                    .hint_text(if pg { "postgres" } else { "" })
                    .margin(vec2(8.0, 5.0)),
            );
            if pg {
                return;
            }
            ui.add_sized(vec2(110.0, 28.0), egui::Label::new("Read preference:").halign(egui::Align::Center));
            let cur = READ_PREFS.iter().find(|(k, _)| *k == ds.read_pref).map_or("Default", |(_, n)| n);
            egui::ComboBox::from_id_salt("ds-rp").icon(combo_chevron).width(half - 28.0).selected_text(cur).show_ui(ui, |ui| {
                for (k, n) in READ_PREFS {
                    ui.selectable_value(&mut ds.read_pref, k.to_string(), n);
                }
            });
        });
        if !pg {
            ui.horizontal(|ui| {
                label(ui, "Replica set:");
                ui.add(egui::TextEdit::singleline(&mut ds.replica_set).desired_width(field_w - 8.0).margin(vec2(8.0, 5.0)));
            });
        }
        ui.add_space(if pg { 64.0 } else { 26.0 });
        let fields_changed = before.host != ds.host
            || before.port != ds.port
            || before.user != ds.user
            || before.auth != ds.auth
            || before.database != ds.database
            || before.read_pref != ds.read_pref
            || before.replica_set != ds.replica_set
            || before.conn_type != ds.conn_type;
        if fields_changed && ds.conn_type != ConnType::UrlOnly {
            ds.url = ds.regenerated_url(&before.url);
        }
        if pw != pw_before {
            ds.set_url_password(&pw);
        }
        let url_before = ds.url.clone();
        ui.horizontal(|ui| {
            label(ui, "URL:");
            ui.add(egui::TextEdit::singleline(&mut ds.url).desired_width(field_w - 8.0).font(ui_font(13.0)).margin(vec2(8.0, 5.0)));
        });
        if ds.url != url_before {
            ds.sync_from_url(); // the URL overrides the settings above
        }
        if (fields_changed || ds.url != url_before) && before.name == format!("@{}", before.host) {
            ds.name = format!("@{}", ds.host);
        }
        ui.horizontal(|ui| {
            ui.add_space(label_w + 6.0);
            ui.label(RichText::new("Overrides settings above").size(11.5).color(TEXT_SECONDARY));
        });
        // test connection row pinned to the form bottom
        let bottom = ui.max_rect().bottom();
        let r = Rect::from_min_max(pos2(ui.max_rect().left() - 12.0, bottom - 40.0), pos2(ui.max_rect().right() + 14.0, bottom));
        ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, BORDER));
        let ds_clone = ds.clone();
        row_ui(ui, r.shrink2(vec2(10.0, 4.0)), |ui| {
            if ui.link("Test Connection").clicked() {
                *out = DialogOut::Test(ds_clone.clone(), pw.clone());
                self.test = TestState::Running;
            }
            ui.add_space(20.0);
            match &self.test {
                TestState::Idle => {}
                TestState::Running => {
                    ui.spinner();
                    ui.label(RichText::new("Connecting…").color(TEXT_SECONDARY));
                }
                TestState::Ok(v) => {
                    ui.label(RichText::new("✓").color(SUCCESS));
                    ui.label(RichText::new(format!("{} {v}", ds_clone.driver.name())).color(TEXT_SECONDARY));
                }
                TestState::Err(e) => {
                    ui.label(RichText::new(e.lines().next().unwrap_or_default()).color(ERROR));
                }
            }
        });
        self.passwords.insert(id, pw);
    }
}

pub fn driver_icon(d: Driver) -> egui::ImageSource<'static> {
    match d {
        Driver::Mongo => ic::LEAF,
        Driver::Postgres => ic::POSTGRES,
    }
}

pub fn primary_btn(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(egui::Color32::WHITE)).fill(BRAND).stroke(Stroke::NONE).min_size(vec2(72.0, 28.0)))
}

const COLORS: [(Option<[u8; 3]>, &str); 6] = [
    (None, "No color"),
    (Some([0x45, 0x30, 0x30]), "Red"),
    (Some([0x45, 0x3B, 0x22]), "Orange"),
    (Some([0x27, 0x3D, 0x2B]), "Green"),
    (Some([0x25, 0x32, 0x4D]), "Blue"),
    (Some([0x3A, 0x2D, 0x45]), "Violet"),
];

fn color_name(c: Option<[u8; 3]>) -> &'static str {
    COLORS.iter().find(|(x, _)| *x == c).map_or("Custom", |(_, n)| n)
}

pub(crate) fn combo_chevron(ui: &Ui, rect: Rect, _v: &egui::style::WidgetVisuals, _open: bool) {
    chevron_down(ui, rect.center(), TEXT_MUTED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_url_as_typed() {
        // stored: URL without the password (credentials file has it), fields never filled
        let ds = DataSource {
            id: 9,
            url: "mongodb+srv://backend_dev_3@vernoss.example.net/?appName=V".into(),
            user: "backend_dev_3".into(),
            url_password: true,
            ..Default::default()
        };
        let d = DsDialog::new(vec![ds], HashMap::from([(9, "s3cr3t".to_string())]), Some(9), None);
        let shown = &d.items[0];
        assert_eq!(shown.url, "mongodb+srv://backend_dev_3:s3cr3t@vernoss.example.net/?appName=V");
        assert!(shown.conn_type == ConnType::Srv && shown.host == "vernoss.example.net", "fields follow the URL");
        // a password only typed in the Password field stays out of the URL
        let plain = DataSource { id: 1, user: "u".into(), ..Default::default() };
        let plain = DataSource { url: plain.generated_url(), ..plain };
        let d = DsDialog::new(vec![plain], HashMap::from([(1, "pw".to_string())]), Some(1), None);
        assert_eq!(d.items[0].url, "mongodb://u@localhost:27017");
    }
}
