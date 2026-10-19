//! Files tool window: attached directories plus "Scratches and Consoles"
//! (Database Consoles per data source, Scratches), like DataGrip's Files view.

use crate::app::{NodeExtra, node};
use crate::config;
use crate::icons as ic;
use crate::theme::*;
use crate::widgets::*;
use egui::{ImageSource, Rect, Ui, pos2, vec2};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub enum FileAction {
    OpenConsole(u64, PathBuf),
    NewConsole(u64),
    OpenFile(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    Attach,
    Detach(String),
    NewScratch(&'static str),
    Hide,
}

#[derive(Default)]
pub struct Files {
    /// Toggled nodes: default-open nodes are closed when present, directories are open when present.
    pub toggled: HashSet<String>,
    pub selected: Option<String>,
}

impl Files {
    /// Expands a connection under Database Consoles (after creating a console in it).
    pub fn reveal_connection(&mut self, ds: u64) {
        self.toggled.insert(format!("fv:ds:{ds}"));
    }
}

pub fn file_icon(p: &Path) -> ImageSource<'static> {
    match p.extension().and_then(|e| e.to_str()) {
        Some("js" | "mjs" | "ts") => ic::JAVA_SCRIPT,
        Some("json") => ic::JSON,
        Some("sql") => ic::SQL,
        _ => ic::TEXT,
    }
}

fn tilde(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if p.starts_with(&h) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    }
}

impl Files {
    fn open(&self, key: &str, default_open: bool) -> bool {
        self.toggled.contains(key) != default_open
    }

    /// Renders the island; `sources` = (id, name, driver icon) of data sources.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        sources: &[(u64, String, ImageSource<'static>)],
        attached: &[String],
    ) -> Vec<FileAction> {
        let mut acts = Vec::new();
        island(ui, rect);
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        let t = text_at(ui, pos2(head.left() + 12.0, head.center().y), egui::Align2::LEFT_CENTER, "Files", bold_font(13.0), TEXT);
        chevron_down(ui, pos2(t.right() + 9.0, head.center().y), TEXT_MUTED);
        row_ui(ui, Rect::from_min_max(pos2(head.right() - 70.0, head.top() + 7.0), head.right_bottom() - vec2(8.0, 7.0)), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, ic::HIDE, "Hide  ⇧⎋").clicked() {
                    acts.push(FileAction::Hide);
                }
                let more = icon_btn(ui, ic::MORE_VERTICAL, "Options");
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(180.0);
                    if menu_item(ui, Some(ic::ADD_DIRECTORY), "Attach Directory…", "", true) {
                        acts.push(FileAction::Attach);
                    }
                    if menu_item(ui, Some(ic::JAVA_SCRIPT), "New Scratch File › JavaScript", "", true) {
                        acts.push(FileAction::NewScratch("js"));
                    }
                    if menu_item(ui, Some(ic::JSON), "New Scratch File › JSON", "", true) {
                        acts.push(FileAction::NewScratch("json"));
                    }
                    if menu_item(ui, Some(ic::SQL), "New Scratch File › SQL", "", true) {
                        acts.push(FileAction::NewScratch("sql"));
                    }
                });
            });
        });
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max - vec2(0.0, 6.0));
        let mut toggles: Vec<String> = vec![];
        let mut select: Option<String> = None;
        rect_ui(ui, body, |ui| {
            egui::ScrollArea::both().id_salt("files").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let sel = self.selected.clone();
                let mut row = |ui: &mut Ui,
                               depth: usize,
                               key: String,
                               open: Option<bool>,
                               icon: ImageSource<'static>,
                               label: &str,
                               suffix: Option<String>| {
                    let r = node(ui, &sel, depth, &key, open, icon, label, suffix, NodeExtra::default());
                    if r.toggle || (open.is_some() && r.row.double_clicked()) {
                        toggles.push(key.clone());
                    }
                    if r.row.clicked() || r.row.secondary_clicked() {
                        select = Some(key);
                    }
                    r.row
                };
                // attached directories
                for dir in attached {
                    let key = format!("fv:a:{dir}");
                    let open = self.open(&key, false);
                    let name = Path::new(dir).file_name().map_or(dir.clone(), |n| n.to_string_lossy().into_owned());
                    let r = row(ui, 0, key, Some(open), ic::FOLDER_GRAY, &name, Some(tilde(dir)));
                    context_menu(&r, |ui| {
                        ui.set_min_width(180.0);
                        if menu_item(ui, None, "Reveal in Finder", "", true) {
                            acts.push(FileAction::Reveal(PathBuf::from(dir)));
                        }
                        if menu_item(ui, Some(ic::REMOVE), "Remove from Files", "", true) {
                            acts.push(FileAction::Detach(dir.clone()));
                        }
                    });
                    if open {
                        self.dir_rows(ui, Path::new(dir), 1, &mut row, &mut acts);
                    }
                }
                // Scratches and Consoles
                let sc = "fv:sc".to_string();
                let sc_open = self.open(&sc, true);
                row(ui, 0, sc.clone(), Some(sc_open), ic::SCRATCHES, "Scratches and Consoles", None);
                if !sc_open {
                    return;
                }
                let dbc = "fv:dbc".to_string();
                let dbc_open = self.open(&dbc, true);
                row(ui, 1, dbc.clone(), Some(dbc_open), ic::FOLDER_GRAY, "Database Consoles", None);
                if dbc_open {
                    // every connection is listed (like DataGrip); only those with console files expand.
                    // ponytail: lists console dirs every frame; fine for dozens of files, cache if it grows
                    let mut all: Vec<&(u64, String, ImageSource<'static>)> = sources.iter().collect();
                    all.sort_by_key(|(_, n, _)| n.to_lowercase());
                    for (id, name, icon) in all {
                        let files = config::console_files(*id);
                        let key = format!("fv:ds:{id}");
                        let open = !files.is_empty() && self.open(&key, false);
                        let r = row(ui, 2, key, (!files.is_empty()).then_some(open), icon.clone(), name, None);
                        if files.is_empty() && r.double_clicked() {
                            acts.push(FileAction::NewConsole(*id));
                        }
                        context_menu(&r, |ui| {
                            ui.set_min_width(180.0);
                            if menu_item(ui, Some(ic::CONSOLE), "New Query Console", "⌘⇧L", true) {
                                acts.push(FileAction::NewConsole(*id));
                            }
                            if menu_item(ui, None, "Reveal in Finder", "", true) {
                                acts.push(FileAction::Reveal(config::consoles_dir(*id)));
                            }
                        });
                        if !open {
                            continue;
                        }
                        for f in files {
                            let stem = f.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
                            let r = row(ui, 3, format!("fv:c:{}", f.display()), None, icon.clone(), &format!("{stem} [{name}]"), None);
                            if r.double_clicked() {
                                acts.push(FileAction::OpenConsole(*id, f.clone()));
                            }
                            file_menu(&r, &f, &mut acts, Some(*id));
                        }
                    }
                }
                let scratches = config::list_files(&config::scratches_dir(), &["js", "json", "sql", "txt"]);
                if !scratches.is_empty() {
                    let key = "fv:scr".to_string();
                    let open = self.open(&key, true);
                    row(ui, 1, key, Some(open), ic::FOLDER_GRAY, "Scratches", None);
                    if open {
                        for f in scratches {
                            let name = f.file_name().map_or(String::new(), |s| s.to_string_lossy().into_owned());
                            let r = row(ui, 2, format!("fv:f:{}", f.display()), None, file_icon(&f), &name, None);
                            if r.double_clicked() {
                                acts.push(FileAction::OpenFile(f.clone()));
                            }
                            file_menu(&r, &f, &mut acts, None);
                        }
                    }
                }
            });
        });
        for k in toggles {
            if !self.toggled.remove(&k) {
                self.toggled.insert(k);
            }
        }
        if select.is_some() {
            self.selected = select;
        }
        acts
    }

    fn dir_rows(
        &self,
        ui: &mut Ui,
        dir: &Path,
        depth: usize,
        row: &mut impl FnMut(&mut Ui, usize, String, Option<bool>, ImageSource<'static>, &str, Option<String>) -> egui::Response,
        acts: &mut Vec<FileAction>,
    ) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|r| {
                r.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
        for p in entries {
            let name = p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
            if p.is_dir() {
                let key = format!("fv:d:{}", p.display());
                let open = self.open(&key, false);
                row(ui, depth, key, Some(open), ic::FOLDER_GRAY, &name, None);
                if open {
                    self.dir_rows(ui, &p, depth + 1, row, acts);
                }
            } else {
                let r = row(ui, depth, format!("fv:f:{}", p.display()), None, file_icon(&p), &name, None);
                if r.double_clicked() {
                    acts.push(FileAction::OpenFile(p.clone()));
                }
                file_menu(&r, &p, acts, None);
            }
        }
    }
}

fn file_menu(r: &egui::Response, f: &Path, acts: &mut Vec<FileAction>, console_of: Option<u64>) {
    context_menu(&r, |ui| {
        ui.set_min_width(180.0);
        if menu_item(ui, None, "Open", "↩", true) {
            acts.push(match console_of {
                Some(id) => FileAction::OpenConsole(id, f.to_path_buf()),
                None => FileAction::OpenFile(f.to_path_buf()),
            });
        }
        if menu_item(ui, None, "Rename…", "⇧F6", true) {
            acts.push(FileAction::Rename(f.to_path_buf()));
        }
        if menu_item(ui, None, "Reveal in Finder", "", true) {
            acts.push(FileAction::Reveal(f.to_path_buf()));
        }
        ui.separator();
        if menu_item(ui, Some(ic::DELETE), "Delete…", "⌦", true) {
            acts.push(FileAction::Delete(f.to_path_buf()));
        }
    });
}
