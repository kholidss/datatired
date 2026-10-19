//! Multi-project shell: every open project is a `Workspace` (DataGrip: a project window, shown as
//! window tabs in full screen). The project widget creates, opens, switches and closes projects.

use crate::app::{Project, ProjectCmd, ProjectEntry, Workspace};
use crate::config::{self, Registry};
use crate::dialog::primary_btn;
use crate::icons as ic;
use crate::theme::*;
use egui::{Color32, Id, Key, Sense, Stroke, Ui, vec2};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

pub struct App {
    ws: Vec<Workspace>,
    active: usize,
    reg: Registry,
    next_uid: u64,
    /// "New Project" input dialog text.
    new_project: Option<String>,
    picker: Option<Receiver<Option<String>>>,
    title: String,
    error: Option<String>,
}

fn project(path: &Path, reg: &Registry) -> Project {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    Project { name, path: path.to_path_buf(), color: reg.color(&path.display().to_string()) }
}

/// `name` → `~/DataTiredProjects/name`; `~/x` and absolute paths are taken as given.
pub fn resolve_project_path(input: &str) -> Option<PathBuf> {
    let t = input.trim();
    if t.is_empty() || t.contains('\0') {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let p = if t == "~" {
        PathBuf::from(&home)
    } else if let Some(rest) = t.strip_prefix("~/") {
        PathBuf::from(&home).join(rest)
    } else {
        PathBuf::from(t)
    };
    Some(if p.is_absolute() { p } else { config::projects_home().join(p) })
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        #[cfg(target_os = "macos")]
        claim_title_bar_clicks();
        install(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let reg = config::load_registry();
        let mut app = App { ws: vec![], active: 0, reg, next_uid: 1, new_project: None, picker: None, title: String::new(), error: None };
        let open: Vec<String> = app.reg.open.clone();
        for p in open.iter().filter(|p| config::project_file(Path::new(p)).exists()) {
            app.push(Path::new(p));
        }
        if app.ws.is_empty() {
            // every project folder is gone: fall back to the default one
            app.open_project(&config::projects_home().join("DataTired"));
        }
        let active = app.reg.active.clone();
        app.active = app.ws.iter().position(|w| Some(w.project.path.display().to_string()) == active).unwrap_or(0);
        app
    }

    fn push(&mut self, path: &Path) {
        let uid = self.next_uid;
        self.next_uid += 1;
        self.ws.push(Workspace::new(project(path, &self.reg), uid));
    }

    fn save_registry(&mut self) {
        self.reg.open = self.ws.iter().map(|w| w.project.path.display().to_string()).collect();
        self.reg.active = self.ws.get(self.active).map(|w| w.project.path.display().to_string());
        config::save_registry(&self.reg);
    }

    /// Opens (creating it if needed) a project and makes it active; an already open one is focused.
    fn open_project(&mut self, path: &Path) {
        if let Some(i) = self.ws.iter().position(|w| w.project.path == path) {
            self.active = i;
        } else {
            if let Err(e) = config::ensure_project(path) {
                self.error = Some(format!("Cannot open {}: {e}", path.display()));
                return;
            }
            self.reg.touch(&path.display().to_string());
            self.push(path);
            self.active = self.ws.len() - 1;
        }
        self.reg.touch(&path.display().to_string());
        self.save_registry();
    }

    fn close_project(&mut self, path: &Path) {
        if self.ws.len() < 2 {
            return;
        }
        if let Some(i) = self.ws.iter().position(|w| w.project.path == path) {
            self.ws[i].save();
            self.ws.remove(i);
            if self.active >= self.ws.len() || self.active > i {
                self.active = self.active.saturating_sub(1).min(self.ws.len() - 1);
            }
            self.save_registry();
        }
    }

    fn entries(&self) -> Vec<ProjectEntry> {
        let mut v: Vec<ProjectEntry> = self
            .ws
            .iter()
            .enumerate()
            .map(|(i, w)| ProjectEntry {
                project: w.project.clone(),
                open: true,
                active: i == self.active,
                caption: match w.current_title() {
                    Some(t) => format!("{} – {t}", w.project.name),
                    None => w.project.name.clone(),
                },
            })
            .collect();
        for r in &self.reg.recent {
            let p = PathBuf::from(&r.path);
            if !v.iter().any(|e| e.project.path == p) && config::project_file(&p).exists() {
                let project = project(&p, &self.reg);
                v.push(ProjectEntry { caption: project.name.clone(), project, open: false, active: false });
            }
        }
        v
    }

    fn command(&mut self, ctx: &egui::Context, cmd: ProjectCmd) {
        match cmd {
            ProjectCmd::New => self.new_project = Some(String::new()),
            ProjectCmd::Open => {
                let (tx, rx) = channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let out = std::process::Command::new("osascript")
                        .args(["-e", "POSIX path of (choose folder with prompt \"Open Project\")"])
                        .output();
                    let p = out
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_end_matches('/').to_string());
                    let _ = tx.send(p);
                    ctx2.request_repaint();
                });
                self.picker = Some(rx);
            }
            ProjectCmd::Switch(p) => self.open_project(&p),
            ProjectCmd::Close(p) => self.close_project(&p),
            ProjectCmd::RemoveRecent(p) => {
                let p = p.display().to_string();
                self.reg.recent.retain(|r| r.path != p);
                self.save_registry();
            }
        }
    }

    /// DataGrip's `NewProjectAction`: an input dialog; a bare name goes to the projects home.
    fn new_project_dialog(&mut self, ctx: &egui::Context) {
        let Some(text) = &mut self.new_project else { return };
        let target = resolve_project_path(text);
        let mut done: Option<bool> = None;
        let modal = egui::Modal::new(Id::new("new-project")).backdrop_color(Color32::from_black_alpha(40)).frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x2B, 0x2D, 0x30))
                .stroke(Stroke::new(1.0, POPUP_BORDER))
                .corner_radius(10)
                .inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 18 }),
        );
        let r = modal.show(ctx, |ui: &mut Ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new("New Project").font(bold_font(14.0)).color(TEXT));
            ui.add_space(12.0);
            ui.horizontal_top(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                egui::Image::new(ic::QUESTION_DIALOG).paint_at(ui, ir);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Enter new project name or path:").color(TEXT));
                    ui.add_space(6.0);
                    let te = ui.add(egui::TextEdit::singleline(text).desired_width(360.0).margin(vec2(8.0, 5.0)));
                    te.request_focus();
                    if let Some(p) = &target {
                        let home = std::env::var("HOME").unwrap_or_default();
                        let shown = p.display().to_string().replacen(&home, "~", 1);
                        let note = if config::project_file(p).exists() { "opens existing project" } else { "will be created" };
                        ui.label(egui::RichText::new(format!("{shown}  ({note})")).size(11.5).color(TEXT_SECONDARY));
                    }
                });
            });
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let ok = ui.add_enabled_ui(target.is_some(), |ui| primary_btn(ui, "OK")).inner;
                if ok.clicked() || (target.is_some() && ui.input(|i| i.key_pressed(Key::Enter))) {
                    done = Some(true);
                }
                if ui.add(egui::Button::new(egui::RichText::new("Cancel").color(TEXT)).min_size(vec2(72.0, 28.0))).clicked() {
                    done = Some(false);
                }
            });
        });
        if r.should_close() {
            done = done.or(Some(false));
        }
        match done {
            Some(true) => {
                self.new_project = None;
                if let Some(p) = target {
                    self.open_project(&p);
                }
            }
            Some(false) => self.new_project = None,
            None => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // background projects keep receiving query results / connection state
        for w in &mut self.ws {
            w.poll(&ctx);
        }
        // shell dialogs take the keyboard before the project window sees it
        let modal = self.new_project.is_some();
        self.new_project_dialog(&ctx);
        if modal {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_))));
        }
        if let Some(rx) = &self.picker
            && let Ok(res) = rx.try_recv()
        {
            self.picker = None;
            if let Some(p) = res.filter(|p| !p.is_empty()) {
                self.open_project(Path::new(&p));
            }
        }
        let entries = self.entries();
        let active = self.active;
        let w = &mut self.ws[active];
        w.shell = entries;
        if let Some(e) = self.error.take() {
            w.set_status(e);
        }
        ui.push_id(("project", w.project.path.clone()), |ui| w.ui(ui));
        if let Some(cmd) = self.ws[active].cmd.take() {
            self.command(&ctx, cmd);
        }
        // window title like the IDE: `project – active tab`
        let w = &self.ws[self.active];
        let title = match w.current_title() {
            Some(t) => format!("{} – {t}", w.project.name),
            None => w.project.name.clone(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn on_exit(&mut self) {
        for w in &mut self.ws {
            w.save();
        }
        self.save_registry();
    }
}

/// The window keeps a transparent native title bar over the top ~28 px of the header (traffic
/// lights), and AppKit treats clicks there as title bar clicks whenever the view below says it
/// `mouseDownCanMoveWindow` (NSView's default for non-opaque views, like winit's): a fast double click
/// on a toolbar button then zooms the window. Our view claims those clicks; the header itself drags /
/// zooms the window on its empty parts (`Workspace::header`).
#[cfg(target_os = "macos")]
fn claim_title_bar_clicks() {
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn class_addMethod(cls: *mut c_void, name: *const c_void, imp: *const c_void, types: *const c_char) -> bool;
    }
    extern "C" fn no(_view: *mut c_void, _sel: *const c_void) -> bool {
        false
    }
    let types = if cfg!(target_arch = "aarch64") { c"B@:" } else { c"c@:" };
    // SAFETY: plain Objective-C runtime calls; `no` matches `- (BOOL)mouseDownCanMoveWindow`. winit's
    // `WinitView` (created with the window, before the app) doesn't define the method, so this adds
    // an override instead of replacing anything.
    unsafe {
        let cls = objc_getClass(c"WinitView".as_ptr());
        if !cls.is_null() {
            class_addMethod(cls, sel_registerName(c"mouseDownCanMoveWindow".as_ptr()), no as *const c_void, types.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_new_project_input() {
        assert_eq!(resolve_project_path("demo"), Some(config::projects_home().join("demo")));
        assert_eq!(resolve_project_path("/tmp/x"), Some(PathBuf::from("/tmp/x")));
        let home = std::env::var("HOME").unwrap();
        assert_eq!(resolve_project_path("~/a/b"), Some(PathBuf::from(home).join("a/b")));
        assert_eq!(resolve_project_path("   "), None);
    }
}
