//! Result grid shared by the collection data editor and console result tabs:
//! Table / Tree / Text views, inline editing with pending changes, floating pager, extractors.

use crate::editor::{self, Lang};
use crate::icons as ic;
use crate::mongo::{Edit, ResultSet, type_name};
use crate::pg;
use crate::theme::*;
use crate::widgets::*;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Align2, Color32, CornerRadius, Id, Key, Rect, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use mongodb::bson::{Bson, Document};
use std::collections::{HashMap, HashSet};

const ROW_H: f32 = 27.0;
const HEADER_H: f32 = 28.0;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum View {
    #[default]
    Table,
    Tree,
    Text,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Extractor {
    SqlInserts,
    SqlUpdates,
    WhereClause,
    Csv,
    Tsv,
    Pipe,
    Semicolon,
    #[default]
    Json,
    Markdown,
    OneRow,
    Pretty,
    SqlInsertMultirow,
    SqlInsertStatements,
}

impl Extractor {
    /// DataGrip's Data Extractors list: (extractor, label, group).
    pub const ALL: [(Extractor, &'static str, &'static str); 13] = [
        (Extractor::SqlInserts, "SQL Inserts", "Built-in"),
        (Extractor::SqlUpdates, "SQL Updates", "Built-in"),
        (Extractor::WhereClause, "Where Clause", "Built-in"),
        (Extractor::Csv, "CSV", "CSV"),
        (Extractor::Tsv, "TSV", "CSV"),
        (Extractor::Pipe, "Pipe-separated", "CSV"),
        (Extractor::Semicolon, "Semicolon-separated", "CSV"),
        (Extractor::Json, "JSON", "Scripted"),
        (Extractor::Markdown, "Markdown", "Scripted"),
        (Extractor::OneRow, "One-row", "Scripted"),
        (Extractor::Pretty, "Pretty", "Scripted"),
        (Extractor::SqlInsertMultirow, "SQL-Insert-Multirow", "Scripted"),
        (Extractor::SqlInsertStatements, "SQL-Insert-Statements", "Scripted"),
    ];
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(e, ..)| *e == self).map_or("JSON", |(_, n, _)| match *n {
            "Pipe-separated" => "Pipe",
            "Semicolon-separated" => "Semicolon",
            n => n,
        })
    }
    pub fn ext(self) -> &'static str {
        match self {
            Extractor::Csv | Extractor::Semicolon | Extractor::Pipe => "csv",
            Extractor::Tsv => "tsv",
            Extractor::Json => "json",
            Extractor::Markdown => "md",
            e if e.sql_only() => "sql",
            _ => "txt",
        }
    }
    /// Offered for SQL data sources only.
    pub fn sql_only(self) -> bool {
        matches!(
            self,
            Extractor::SqlInserts
                | Extractor::SqlUpdates
                | Extractor::WhereClause
                | Extractor::SqlInsertMultirow
                | Extractor::SqlInsertStatements
        )
    }
}

/// The Data Extractors list (toolbar dropdown, the cell menu's Change Data Extractor ›).
fn extractor_items(ui: &mut Ui, current: Extractor, sql: bool) -> Option<Extractor> {
    let mut pick = None;
    let mut group = "";
    for (e, n, g) in Extractor::ALL {
        if e.sql_only() && !sql {
            continue;
        }
        if g != group {
            if !group.is_empty() {
                ui.separator();
            }
            menu_section(ui, g);
            group = g;
        }
        if menu_check(ui, n, current == e) {
            pick = Some(e);
        }
    }
    pick
}

pub enum GridAction {
    Refresh,
    Page { offset: u64, limit: u64 },
    Submit(Vec<Edit>),
    Sort(String, i32),
    FilterBy(String),
    Stop,
    Export(String, &'static str),
}

#[derive(Default)]
pub struct Grid {
    pub rs: Option<ResultSet>,
    pub loading: bool,
    pub error: Option<String>,
    pub view: View,
    pub transpose: bool,
    pub extractor: Extractor,
    /// Selected (row, column index into `columns()`); column `usize::MAX` = whole row.
    pub sel: Option<(usize, usize)>,
    /// Other corner of a range selection (shift-click / shift-arrows); `None` = just `sel`.
    anchor: Option<(usize, usize)>,
    editing: Option<(usize, usize, String, bool)>,
    pub edits: HashMap<(usize, String), String>,
    pub added: Vec<HashMap<String, String>>,
    /// Per added row: the values it was cloned from (Clone Row), for display and value types.
    added_src: Vec<Document>,
    pub deleted: HashSet<usize>,
    /// Client-side sort for read-only results.
    local_sort: Option<(String, bool)>,
    /// Current server sort shown in the header for the data editor.
    pub server_sort: Option<(String, i32)>,
    expanded: HashSet<String>,
    search: Option<String>,
    cols: Vec<String>,
    order: Vec<usize>,
    /// The grid owns keyboard input (set on click, cleared by clicks elsewhere / text fields).
    grid_focus: bool,
    scroll_to_sel: bool,
    h_scroll: Option<f32>,
    spin_until: f64,
    /// Axis chosen by the current scroll gesture: (horizontal, last event time).
    scroll_lock: ScrollLock,
    /// (initial width, type name) per column, computed when rows arrive.
    col_meta: Vec<(f32, String)>,
    /// Value editor side panel (Record / Value tabs).
    pub value_panel: bool,
    /// Data editor hides the `find {}` / `sort` bar when set.
    pub hide_filter: bool,
    panel_record: bool,
    /// Record tab inline edit: (column index, text, focus requested).
    rec_edit: Option<(usize, String, bool)>,
    panel_item: Option<usize>,
    panel_item_edit: Option<(usize, String)>,
    /// Working copy of the array shown in the value panel: ((row, col), items).
    panel_arr: Option<((usize, usize), Vec<Bson>)>,
    /// Text of the value panel editor for non-array values: ((row, col), text).
    panel_text: Option<((usize, usize), String)>,
}

impl Grid {
    pub fn pending() -> Self {
        Grid { loading: true, ..Default::default() }
    }

    pub fn set(&mut self, rs: ResultSet) {
        if rs.sql() && self.rs.is_none() {
            self.extractor = Extractor::Csv; // DataGrip's default for SQL data sources
        }
        self.cols = if rs.sql() { rs.cols.iter().map(|c| c.0.clone()).collect() } else { columns(&rs.docs) };
        self.col_meta = col_meta(&self.cols, &rs);
        self.rs = Some(rs);
        self.loading = false;
        self.error = None;
        self.edits.clear();
        self.added.clear();
        self.added_src.clear();
        self.deleted.clear();
        self.editing = None;
        self.sel = self.sel.filter(|&(r, _)| r < self.rows_len());
        self.anchor = None;
        self.reorder();
    }

    fn rows_len(&self) -> usize {
        self.rs.as_ref().map_or(0, |r| r.docs.len()) + self.added.len()
    }

    pub fn editable(&self) -> bool {
        self.rs.as_ref().and_then(|r| r.table()).is_some()
    }

    pub fn dirty(&self) -> bool {
        !self.edits.is_empty() || !self.added.is_empty() || !self.deleted.is_empty()
    }

    fn reorder(&mut self) {
        let Some(rs) = &self.rs else { return };
        let mut order: Vec<usize> = (0..rs.docs.len()).collect();
        if let Some(q) = self.search.as_ref().filter(|q| !q.is_empty()) {
            let q = q.to_lowercase();
            order.retain(|&i| rs.docs[i].values().any(|v| plain(v).to_lowercase().contains(&q)));
        }
        if let Some((col, asc)) = &self.local_sort {
            order.sort_by(|&a, &b| {
                let o = cmp_bson(rs.docs[a].get(col), rs.docs[b].get(col));
                if *asc { o } else { o.reverse() }
            });
        }
        self.order = order;
    }

    pub fn pending_edits(&self) -> Vec<Edit> {
        let Some(rs) = &self.rs else { return vec![] };
        // row identity: `_id`, or the primary key columns of a SQL table
        let key = rs.key();
        let id_of = |doc: &Document| match rs.sql() {
            true => Bson::Document(key.iter().map(|k| (k.clone(), doc.get(k).cloned().unwrap_or(Bson::Null))).collect()),
            false => doc.get("_id").cloned().unwrap_or(Bson::Null),
        };
        let mut out = Vec::new();
        for ((row, field), text) in &self.edits {
            if self.deleted.contains(row) {
                continue;
            }
            if let Some(doc) = rs.docs.get(*row) {
                let id = id_of(doc);
                let as_string = matches!(doc.get(field), Some(Bson::String(_)));
                out.push(Edit::Set { id, field: field.clone(), text: text.clone(), as_string });
            }
        }
        for (row, src) in self.added.iter().zip(&self.added_src) {
            // column order; a cloned string stays a string (typed values are literals)
            let fields = self
                .cols
                .iter()
                .filter_map(|k| {
                    Some((k.clone(), row.get(k).filter(|v| !v.is_empty())?.clone(), matches!(src.get(k), Some(Bson::String(_)))))
                })
                .collect();
            out.push(Edit::Insert { fields });
        }
        for &row in &self.deleted {
            if let Some(doc) = rs.docs.get(row) {
                out.push(Edit::Delete { id: id_of(doc) });
            }
        }
        out
    }

    pub fn revert(&mut self) {
        self.edits.clear();
        self.added.clear();
        self.added_src.clear();
        self.deleted.clear();
        self.editing = None;
    }

    fn add_row(&mut self) {
        self.added.push(HashMap::new());
        self.added_src.push(Document::new());
        let r = self.rows_len() - 1;
        let key = self.rs.as_ref().map(ResultSet::key).unwrap_or_default();
        let c = self.cols.iter().position(|c| !key.contains(c)).unwrap_or(0);
        if self.cols.is_empty() {
            self.cols.push("_id".into());
        }
        self.sel = Some((r, c));
        self.anchor = None;
        self.editing = Some((r, c, String::new(), true));
    }

    /// Marks the selected rows deleted (or un-deletes them when all already are); added rows go away.
    fn delete_selected(&mut self) {
        let ndocs = self.rs.as_ref().map_or(0, |r| r.docs.len());
        let Some((rows, _)) = self.sel_rect() else { return };
        let (mut added, docs): (Vec<usize>, Vec<usize>) = rows.into_iter().partition(|&r| r >= ndocs);
        let undo = docs.iter().all(|r| self.deleted.contains(r));
        for r in docs {
            if undo {
                self.deleted.remove(&r);
            } else {
                self.deleted.insert(r);
            }
        }
        added.sort_unstable();
        for &r in added.iter().rev() {
            self.added.remove(r - ndocs);
            self.added_src.remove(r - ndocs);
        }
        if !added.is_empty() {
            self.sel = None;
            self.anchor = None;
        }
    }

    /// Display position of a data row (documents in `order`, then the added rows).
    fn pos(&self, r: usize) -> usize {
        let ndocs = self.rs.as_ref().map_or(0, |rs| rs.docs.len());
        match r.checked_sub(ndocs) {
            Some(i) => self.order.len() + i,
            None => self.order.iter().position(|&i| i == r).unwrap_or(r),
        }
    }

    /// The selected range: data rows in display order and column indices; a row number (column
    /// `usize::MAX`) selects whole rows. `None` = nothing selected.
    fn sel_rect(&self) -> Option<(Vec<usize>, Vec<usize>)> {
        let (r1, c1) = self.sel?;
        let (r0, c0) = self.anchor.unwrap_or((r1, c1));
        let ndocs = self.rs.as_ref().map_or(0, |rs| rs.docs.len());
        let (p0, p1) = (self.pos(r0), self.pos(r1));
        let rows = (p0.min(p1)..=p0.max(p1)).map(|k| rows_at(&self.order, ndocs, k)).filter(|&r| r < self.rows_len()).collect();
        let cols = if c0 == usize::MAX || c1 == usize::MAX { (0..self.cols.len()).collect() } else { (c0.min(c1)..=c0.max(c1)).collect() };
        Some((rows, cols))
    }

    /// Moves the cursor; `extend` (shift) keeps the other corner of the range.
    fn select(&mut self, r: usize, c: usize, extend: bool) {
        if !extend {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = self.sel;
        }
        self.sel = Some((r, c));
    }

    /// Clone Row (⌘D, DataGrip's `CloneRowAction`): a new pending row at the end with the selected
    /// row's values, its pending edits included. Key columns are left to the database like DataGrip
    /// does for primary keys (`<default>`); for MongoDB that is `_id` too (DataGrip copies it, so
    /// the insert fails on the duplicate key), which gets a new ObjectId instead.
    fn clone_row(&mut self) {
        let (Some(rs), Some((r, c))) = (&self.rs, self.sel) else { return };
        if r >= self.rows_len() || self.deleted.contains(&r) {
            return;
        }
        let key = rs.key();
        let ndocs = rs.docs.len();
        let mut values = HashMap::new();
        let mut src = Document::new();
        for col in self.cols.iter().filter(|c| !key.contains(c)) {
            let (v, pending) = self.cell_text(r, col);
            let v = v.or_else(|| r.checked_sub(ndocs).and_then(|i| self.added_src[i].get(col)));
            if let Some(v) = v {
                src.insert(col.clone(), v.clone());
            }
            if let Some(text) = pending.or_else(|| v.map(edit_text)) {
                values.insert(col.clone(), text);
            }
        }
        self.added.push(values);
        self.added_src.push(src);
        self.select(self.rows_len() - 1, c, false);
        self.scroll_to_sel = true;
    }

    pub fn extract_all(&self) -> String {
        let Some(rs) = &self.rs else {
            return String::new();
        };
        let docs: Vec<&Document> = self.order.iter().map(|&i| &rs.docs[i]).collect();
        extract(self.extractor, &self.cols, &docs, sql_info(rs).as_ref())
    }

    /// ⌘C / Copy Using Data Extractor, like DataGrip: a single cell copies its value, any other
    /// selection (cell block, rows) goes through the current extractor; nothing selected = all rows.
    pub fn selection_text(&self) -> String {
        let Some(rs) = &self.rs else { return String::new() };
        let Some((rows, cols)) = self.sel_rect() else { return self.extract_all() };
        if let ([r], [c], Some((_, sc))) = (rows.as_slice(), cols.as_slice(), self.sel)
            && sc != usize::MAX
        {
            let col = &self.cols[*c];
            let (v, pending) = self.cell_text(*r, col);
            return pending.unwrap_or_else(|| v.map(edit_text).unwrap_or_default());
        }
        let names: Vec<String> = cols.iter().map(|&c| self.cols[c].clone()).collect();
        let docs: Vec<&Document> = rows.iter().filter_map(|&r| rs.docs.get(r)).collect();
        extract(self.extractor, &names, &docs, sql_info(rs).as_ref())
    }

    fn copy_selection(&self, ui: &Ui) {
        ui.ctx().copy_text(self.selection_text());
    }

    // ---------------------------------------------------------------- toolbar

    /// Toolbar row. `results` adds the Table/Chart switch used in Services result tabs.
    pub fn toolbar(&mut self, ui: &mut Ui, rect: Rect, results: bool) -> Vec<GridAction> {
        let mut acts = Vec::new();
        let editable = self.editable();
        let dirty = self.dirty();
        // narrow toolbars drop optional (mostly disabled) actions, like the IDE's overflow
        let wide = rect.width() > 820.0;
        let widest = rect.width() > 980.0;
        row_ui(ui, rect.shrink2(vec2(8.0, 0.0)), |ui| {
            if results {
                icon_btn_ex(ui, ic::TABLE, "Table", true, true, false);
                icon_btn_ex(ui, ic::CHART, "Chart (not available)", false, false, false);
                vsep(ui);
            }
            if icon_btn(ui, ic::REFRESH, "Reload Page  ⌘R").clicked() {
                acts.push(GridAction::Refresh);
            }
            if wide {
                icon_btn_ex(ui, ic::HISTORY, "Query history", false, false, true);
            }
            if icon_btn_ex(ui, ic::STOP, "Cancel Running Statements", self.loading, false, false).clicked() {
                acts.push(GridAction::Stop);
            }
            vsep(ui);
            if icon_btn_ex(ui, ic::ADD, "Add Row  ⌘N", editable, false, false).clicked() {
                self.add_row();
            }
            if icon_btn_ex(ui, ic::REMOVE, "Delete Row  ⌘⌫", editable && self.sel.is_some(), false, false).clicked() {
                self.delete_selected();
            }
            if icon_btn_ex(ui, ic::UNDO, "Revert Selected", dirty, false, false).clicked() {
                self.revert();
            }
            let preview = icon_btn_ex(ui, ic::PREVIEW, "Preview Pending Changes", dirty, false, false);
            let script = if dirty { self.preview_script() } else { String::new() };
            egui::Popup::menu(&preview).show(|ui| {
                ui.set_min_width(420.0);
                menu_section(ui, "Pending changes");
                ui.add(
                    egui::Label::new(egui::RichText::new(&script).font(mono(12.5)).color(EDITOR_TEXT))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
            // green arrow once there is something to commit, like the IDE
            let submit = if dirty { ic::SUBMIT_READY } else { ic::SUBMIT };
            if icon_btn_ex(ui, submit, "Submit  ⌘↩", dirty, false, false).clicked() {
                acts.push(GridAction::Submit(self.pending_edits()));
            }
            if wide && self.rs.as_ref().is_some_and(ResultSet::sql) {
                text_btn(ui, "Tx: Auto", "Transaction mode: auto-commit (Submit runs the changes in one transaction)", false);
            }
            vsep(ui);
            if wide {
                text_btn(ui, "DDL", "No source text available for MongoDB collections", false);
            }
            if icon_btn_ex(ui, ic::SEARCH, "Search  ⌘F", true, self.search.is_some(), false).clicked() {
                self.search = if self.search.is_some() { None } else { Some(String::new()) };
                self.reorder();
            }
            if let Some(q) = &mut self.search {
                let r = ui.add(egui::TextEdit::singleline(q).desired_width(160.0).hint_text("Search…").font(ui_font(12.5)));
                if r.changed() {
                    self.reorder();
                }
            }
            if !results && icon_btn_ex(ui, ic::FILTER_ROWS, "Show Filter and Sort Bar", true, false, true).clicked() {
                self.hide_filter = !self.hide_filter;
            }
            if icon_btn_ex(ui, ic::VALUE_PANEL, "Value Editor  ⇧⏎", true, self.value_panel, false).clicked() {
                self.value_panel = !self.value_panel;
            }
            if widest {
                icon_btn_ex(ui, ic::COLORIZE, "Colorize (not available)", false, false, true);
                icon_btn_ex(ui, ic::GLOBE, "Geo Viewer (not available)", false, false, false);
                if !results {
                    icon_btn_ex(ui, ic::CHART, "Chart (not available)", false, false, false);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if wide {
                    icon_btn_ex(ui, ic::SETTINGS, "Data Editor Settings", false, false, false);
                }
                let eye = icon_btn_ex(ui, ic::SHOW, "View as", true, false, true);
                egui::Popup::menu(&eye).show(|ui| {
                    ui.set_min_width(140.0);
                    if menu_check(ui, "Transpose", self.transpose) {
                        self.transpose = !self.transpose;
                    }
                    ui.separator();
                    for (v, n) in [(View::Table, "Table"), (View::Tree, "Tree"), (View::Text, "Text")] {
                        if menu_check(ui, n, self.view == v) {
                            self.view = v;
                        }
                    }
                });
                vsep(ui);
                if wide {
                    icon_btn_ex(ui, ic::JUMP, "Compare (not available)", false, false, false);
                    icon_btn_ex(ui, ic::UPLOAD, "Import (use mongoimport-like insertMany in console)", false, false, false);
                }
                if icon_btn(ui, ic::DOWNLOAD, "Export Data…").clicked() {
                    acts.push(GridAction::Export(self.extract_all(), self.extractor.ext()));
                }
                vsep(ui);
                let dd = dropdown(ui, None, self.extractor.name(), "Data Extractor (copy format)");
                let sql = self.rs.as_ref().is_some_and(ResultSet::sql);
                egui::Popup::menu(&dd).show(|ui| {
                    ui.set_min_width(200.0);
                    if let Some(e) = extractor_items(ui, self.extractor, sql) {
                        self.extractor = e;
                    }
                });
            });
        });
        acts
    }

    // ---------------------------------------------------------------- body

    /// Clicking outside the whole editor (toolbar, filter, grid, panel) gives keyboard input back.
    pub fn blur_outside(&mut self, ui: &Ui, editor: Rect) {
        // plain geometry: a press on a popup/modal above the editor must not blur it
        let pressed_outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !editor.contains(p)));
        if pressed_outside {
            self.grid_focus = false;
        }
    }

    /// Loaded columns with their BSON type, for the filter bar's completion.
    pub fn fields(&self) -> Vec<(String, String)> {
        let types = self.col_meta.iter().map(|m| m.1.as_str()).chain(std::iter::repeat(""));
        self.cols.iter().zip(types).map(|(c, t)| (c.clone(), t.to_string())).collect()
    }

    /// Width taken by the value editor panel (0 when hidden); the data editor shortens its filter bar by it.
    pub fn panel_width(&self, w: f32) -> f32 {
        if self.value_panel && self.rs.is_some() { (w * 0.32).clamp(220.0, 380.0) } else { 0.0 }
    }

    /// `panel_top`: the value panel starts there (the filter bar row in the data editor, like the IDE).
    pub fn body(&mut self, ui: &mut Ui, rect: Rect, id: Id, panel_top: f32) -> Vec<GridAction> {
        let mut acts = Vec::new();
        ui.painter().rect_filled(rect, 0.0, ISLAND);
        if let Some(e) = &self.error {
            text_at(ui, rect.left_top() + vec2(14.0, 16.0), Align2::LEFT_CENTER, e, ui_font(13.0), ERROR);
            return acts;
        }
        // keep the "Querying..." overlay visible briefly even when the reload is instant
        let now = ui.input(|i| i.time);
        if self.loading {
            self.spin_until = self.spin_until.max(now + 0.45);
        }
        let querying = self.loading || now < self.spin_until;
        if self.rs.is_none() {
            if querying {
                querying_overlay(ui, rect);
            }
            return acts;
        }
        let area = Rect::from_min_max(pos2(rect.left(), panel_top.min(rect.top())), rect.max);
        let rect = if self.value_panel {
            let w = self.panel_width(rect.width());
            let panel = Rect::from_min_max(pos2(rect.right() - w, panel_top.min(rect.top())), rect.max);
            ui.painter().rect_filled(panel, 0.0, ISLAND);
            ui.painter().vline(panel.left(), panel.y_range(), Stroke::new(1.0, GRID_LINE));
            rect_ui(ui, panel, |ui| self.value_panel_ui(ui, panel, id));
            Rect::from_min_max(rect.min, pos2(panel.left() - 1.0, rect.bottom()))
        } else {
            rect
        };
        if ui.input(|i| i.modifiers.shift && i.key_pressed(Key::Enter)) && self.grid_focus {
            self.value_panel = !self.value_panel;
        }
        // grid + value panel form one keyboard context (⌘↩ submits from either)
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(area) {
            self.grid_focus = true;
        }
        if ui.rect_contains_pointer(rect) {
            lock_scroll_axis(ui, &mut self.scroll_lock);
        }
        // AccessKit node for the grid (keyboard focus itself is `grid_focus`)
        let focus = ui.interact(rect, id.with("grid"), Sense::focusable_noninteractive());
        focus.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Data grid"));
        rect_ui(ui, rect, |ui| match self.view {
            View::Table if self.transpose => self.transposed(ui, id),
            View::Table => self.table(ui, id, &mut acts),
            View::Tree => self.tree(ui, id),
            View::Text => {
                let rs = self.rs.as_ref().unwrap();
                let docs: Vec<&Document> = self.order.iter().map(|&i| &rs.docs[i]).collect();
                editor::code_view(ui, id.with("text"), &text_json(&docs, &self.cols), Lang::Json);
            }
        });
        self.pager(ui, rect, &mut acts);
        if querying {
            querying_overlay(ui, rect);
        }
        acts
    }

    fn table(&mut self, ui: &mut Ui, id: Id, acts: &mut Vec<GridAction>) {
        let rs = self.rs.as_ref().unwrap();
        let ndocs = rs.docs.len();
        let cols = self.cols.clone();
        let editable = rs.table().is_some();
        if self.col_meta.len() != cols.len() {
            self.col_meta = col_meta(&cols, rs);
        }
        let types: Vec<String> = self.col_meta.iter().map(|m| m.1.clone()).collect();
        // SQL: numbers right-aligned by column type (values are server text), key columns marked
        let sql = rs.sql();
        let numeric: Vec<bool> = types.iter().map(|t| sql && is_numeric(t)).collect();
        let key = if sql { rs.key() } else { vec![] };
        let widths: Vec<f32> = self.col_meta.iter().map(|m| m.0).collect();
        let rows: Vec<Option<usize>> = self.order.iter().map(|&i| Some(i)).chain((0..self.added.len()).map(|_| None)).collect();
        let row_doc = |k: usize| rows[k];
        let num_w = ((rows.len().max(1).to_string().len() as f32) * 8.0 + 18.0).max(34.0);
        let focused = self.grid_focus;

        let mut clicked: Option<(usize, usize, bool)> = None;
        let mut header_sort: Option<(String, i32)> = None;
        let mut commit: Option<(usize, String, String)> = None;
        let mut cancel_edit = false;
        let mut menu: Option<CellMenu> = None;
        let sort_state =
            if editable { self.server_sort.clone() } else { self.local_sort.as_ref().map(|(c, a)| (c.clone(), if *a { 1 } else { -1 })) };
        // range selection: rows + column span (all columns for row-number selections)
        let (sel_rows, sel_cols): (HashSet<usize>, Vec<usize>) =
            self.sel_rect().map_or_else(Default::default, |(r, c)| (r.into_iter().collect(), c));
        let editing = &mut self.editing;
        let edits = &self.edits;
        let deleted = &self.deleted;
        let added = &self.added;
        let added_src = &self.added_src;
        let sel = self.sel;
        let in_sel = |r: usize, j: usize| sel_rows.contains(&r) && sel_cols.contains(&j);
        let extractor = self.extractor;
        let sql = rs.sql();
        let scroll_to = std::mem::take(&mut self.scroll_to_sel);

        let mut sel_rect: Option<Rect> = None;
        let mut pinned: Vec<(egui::Rangef, String, bool)> = vec![];
        let mut h_area = egui::ScrollArea::horizontal().id_salt(id.with("h")).auto_shrink([false, false]);
        if let Some(x) = self.h_scroll.take() {
            h_area = h_area.horizontal_scroll_offset(x);
        }
        let h_out = h_area.show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            // TableBuilder paints full-height column resize lines with these strokes; the grid draws
            // its own separators per cell so they stop at the last row, like the IDE
            let w = &mut ui.visuals_mut().widgets;
            for st in [&mut w.noninteractive, &mut w.hovered, &mut w.active] {
                st.bg_stroke = Stroke::NONE;
            }
            let mut tb = TableBuilder::new(ui)
                .id_salt(id.with("t"))
                .striped(false)
                .resizable(true)
                .sense(Sense::CLICK)
                .auto_shrink([false, false])
                .cell_layout(egui::Layout::left_to_right(Align::Center))
                .column(Column::exact(num_w));
            for j in 0..cols.len() {
                tb = tb.column(Column::initial(widths[j]).at_least(36.0).clip(true).resizable(true));
            }
            tb = tb.column(Column::remainder().at_least(0.0));
            if scroll_to {
                if let Some(k) = sel.and_then(|(r, _)| {
                    rows.iter().position(|x| x.unwrap_or(usize::MAX) == r).or(r.checked_sub(ndocs).map(|i| rows.len() - added.len() + i))
                }) {
                    tb = tb.scroll_to_row(k, None);
                }
            }
            tb.header(HEADER_H, |mut h| {
                h.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().rect_filled(r, 0.0, GRID_HEADER);
                    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                });
                for (j, c) in cols.iter().enumerate() {
                    h.col(|ui| {
                        let r = ui.max_rect();
                        ui.painter().rect_filled(r, 0.0, GRID_HEADER);
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                        ui.painter().vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, GRID_LINE));
                        let icon = match (sql, key.contains(c)) {
                            (true, true) => ic::COLUMN_KEY,
                            (true, false) => ic::COLUMN,
                            _ if c == "_id" => ic::FIELD_KEY,
                            _ => ic::FIELD,
                        };
                        paint_icon(ui, Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 8.0), vec2(16.0, 16.0)), icon);
                        let t = text_at(ui, pos2(r.left() + 25.0, r.center().y), Align2::LEFT_CENTER, c, mono(13.0), EDITOR_TEXT);
                        paint_icon_tinted(
                            ui,
                            Rect::from_min_size(pos2(t.right() + 4.0, r.center().y - 8.0), vec2(16.0, 16.0)),
                            ic::FILTER,
                            TEXT_MUTED,
                        );
                        let sr = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(16.0, 20.0));
                        let dir = sort_state.as_ref().filter(|(sc, _)| sc == c).map(|(_, d)| *d);
                        match dir {
                            Some(d) => {
                                let col = LINK;
                                let cpt = sr.center();
                                let s = if d > 0 { -1.0 } else { 1.0 };
                                ui.painter().line(
                                    vec![cpt + vec2(-3.5, -1.5 * s), cpt + vec2(0.0, 2.0 * s), cpt + vec2(3.5, -1.5 * s)],
                                    Stroke::new(1.3, col),
                                );
                            }
                            None => paint_icon(ui, sr, ic::SORT_ARROWS),
                        }
                        let resp = ui.interact(r, id.with(("hdr", j)), Sense::CLICK);
                        if resp.clicked() {
                            let next = match dir {
                                None => 1,
                                Some(1) => -1,
                                _ => 0,
                            };
                            header_sort = Some((c.clone(), next));
                        }
                        resp.on_hover_text(format!("{c}  {}\nClick to sort", types[j]));
                    });
                }
                h.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().rect_filled(r, 0.0, GRID_HEADER);
                    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                });
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let k = row.index();
                    let doc_idx = row_doc(k);
                    let data_row = doc_idx.unwrap_or(ndocs + k - (rows.len() - added.len()));
                    let doc = doc_idx.map(|i| &rs.docs[i]);
                    let is_del = deleted.contains(&data_row);
                    let is_new = doc_idx.is_none();
                    let row_sel = sel_rows.contains(&data_row);
                    let (_, nr) = row.col(|ui| {
                        let r = ui.max_rect();
                        if row_sel {
                            ui.painter().rect_filled(r, 0.0, SEL_INACTIVE);
                        }
                        ui.painter().vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, GRID_LINE));
                        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                        let label = if is_new { "+".to_string() } else { (k + 1 + rs.offset as usize).to_string() };
                        text_at(ui, pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, &label, mono(12.0), LINE_NO);
                        pinned.push((r.y_range(), label, row_sel));
                    });
                    if nr.clicked() {
                        clicked = Some((data_row, usize::MAX, false));
                    }
                    for (j, c) in cols.iter().enumerate() {
                        let (_, resp) = row.col(|ui| {
                            let r = ui.max_rect();
                            let cell_sel = in_sel(data_row, j);
                            if scroll_to && sel == Some((data_row, j)) {
                                sel_rect = Some(r);
                            }
                            // selected cells show even on an added / deleted row (the clone gets the caret)
                            let bg = if cell_sel {
                                if focused { SEL_ACTIVE } else { SEL_INACTIVE }
                            } else if is_del {
                                DELETED_ROW
                            } else if is_new {
                                ADDED_ROW
                            } else if edits.contains_key(&(data_row, c.clone())) {
                                Color32::from_rgb(0x25, 0x32, 0x4D)
                            } else if row_sel {
                                Color32::from_white_alpha(6)
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(r, 0.0, bg);
                            ui.painter().vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, GRID_LINE));
                            ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                            if let Some((er, ec, buf, first)) = editing.as_mut().filter(|(er, ec, _, _)| *er == data_row && *ec == j) {
                                let te = egui::TextEdit::singleline(buf)
                                    .id(id.with(("cell-edit", *er, *ec)))
                                    .font(mono(13.0))
                                    .frame(egui::Frame::NONE)
                                    .desired_width(r.width() - 8.0)
                                    .margin(egui::Margin::symmetric(5, 0));
                                let out = ui.put(r.shrink2(vec2(1.0, 2.0)), te);
                                if *first {
                                    out.request_focus();
                                    *first = false;
                                }
                                if ui.input(|i| i.key_pressed(Key::Escape)) {
                                    cancel_edit = true;
                                } else if out.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Tab)) {
                                    commit = Some((*er, cols[*ec].clone(), buf.clone()));
                                }
                                return;
                            }
                            let pending = edits.get(&(data_row, c.clone()));
                            let new_val = if is_new { added[data_row - ndocs].get(c) } else { None };
                            let src = if is_new { added_src[data_row - ndocs].get(c) } else { None };
                            let (job, right) = match (pending.or(new_val), doc.and_then(|d| d.get(c)).or(src)) {
                                // a cloned value shows like the row it came from
                                (Some(t), Some(v)) if is_new && pending.is_none() && edit_text(v) == *t => {
                                    let (job, right) = cell_job(Some(v));
                                    (job, right || numeric[j])
                                }
                                (Some(t), _) if t == "null" => (plain_job("<null>", TEXT_SECONDARY), false),
                                (Some(t), _) => (plain_job(t, EDITOR_TEXT), numeric[j]),
                                (None, _) if is_new && sql => (plain_job("<default>", TEXT_SECONDARY), false),
                                (None, v) => {
                                    let (job, right) = cell_job(v);
                                    (job, right || (numeric[j] && !matches!(v, Some(Bson::Null))))
                                }
                            };
                            let galley = ui.fonts_mut(|f| f.layout_job(job));
                            let x = if right { r.right() - 8.0 - galley.size().x } else { r.left() + 7.0 };
                            ui.painter().with_clip_rect(r.shrink2(vec2(3.0, 0.0))).galley(
                                pos2(x, r.center().y - galley.size().y / 2.0),
                                galley,
                                EDITOR_TEXT,
                            );
                        });
                        // a right click inside the selection keeps it (Copy Using Data Extractor copies it all)
                        if resp.clicked() || (resp.secondary_clicked() && !in_sel(data_row, j)) {
                            clicked = Some((data_row, j, false));
                        }
                        if resp.double_clicked() {
                            clicked = Some((data_row, j, true));
                        }
                        context_menu(&resp, |ui| {
                            ui.set_min_width(180.0);
                            if menu_item(ui, Some(ic::COPY), &format!("Copy Using Data Extractor ({})", extractor.name()), "⌘C", true) {
                                menu = Some(CellMenu::Copy);
                            }
                            submenu(ui, None, &format!("Change Data Extractor ({})", extractor.name()), true, |ui| {
                                ui.set_min_width(200.0);
                                if let Some(e) = extractor_items(ui, extractor, sql) {
                                    menu = Some(CellMenu::Extractor(e));
                                }
                            });
                            ui.separator();
                            if menu_item(ui, Some(ic::EDIT), "Edit", "↩", editable && !is_del) {
                                menu = Some(CellMenu::Edit(data_row, j));
                            }
                            if menu_item(ui, None, "Set NULL", "⌥⌫", editable && !is_del && c != "_id") {
                                menu = Some(CellMenu::SetNull(data_row, j));
                            }
                            if menu_item(ui, Some(ic::ADD), "Add Row", "⌘N", editable) {
                                menu = Some(CellMenu::AddRow);
                            }
                            if menu_item(ui, Some(ic::REMOVE), "Delete Row", "⌘⌫", editable) {
                                menu = Some(CellMenu::Delete(data_row));
                            }
                            if menu_item(ui, Some(ic::COPY), "Clone Row", "⌘D", editable && !is_del) {
                                menu = Some(CellMenu::Clone(data_row, j));
                            }
                            ui.separator();
                            if menu_item(ui, Some(ic::FILTER), &format!("Filter by {c}"), "", editable && doc.is_some()) {
                                menu = Some(CellMenu::FilterBy(data_row, j));
                            }
                            if menu_item(ui, Some(ic::REFRESH), "Reload Page", "⌘R", true) {
                                menu = Some(CellMenu::Refresh);
                            }
                        });
                    }
                    // empty space after the last column: no row lines, like the IDE
                    row.col(|_| {});
                });
            });
        });
        if h_out.state.offset.x > 0.5 {
            // row numbers stay pinned while scrolling horizontally
            let view = h_out.inner_rect;
            // rows are clipped below the header so a half-scrolled row can't paint over it
            let p = ui.painter().with_clip_rect(Rect::from_min_max(pos2(view.left(), view.top() + HEADER_H), view.max));
            for (y, label, row_sel) in &pinned {
                let r = Rect::from_x_y_ranges(view.left()..=view.left() + num_w, *y);
                p.rect_filled(r, 0.0, if *row_sel { SEL_INACTIVE } else { ISLAND });
                p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                p.text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, label, mono(12.0), LINE_NO);
            }
            let p = ui.painter().with_clip_rect(view);
            p.rect_filled(Rect::from_min_size(view.min, vec2(num_w, HEADER_H)), 0.0, GRID_HEADER);
            p.hline(view.left()..=view.left() + num_w, view.top() + HEADER_H - 0.5, Stroke::new(1.0, GRID_LINE));
            // separator stops at the last row, like the per-cell lines when not scrolled
            let bottom = pinned.last().map_or(view.top() + HEADER_H, |(y, ..)| y.max).min(view.bottom());
            p.vline(view.left() + num_w - 0.5, view.top()..=bottom, Stroke::new(1.0, GRID_LINE));
        }
        if let Some(r) = sel_rect {
            // keep the keyboard-selected cell inside the horizontal viewport (row numbers stay pinned left)
            let view = h_out.inner_rect;
            let mut x = h_out.state.offset.x;
            if r.right() > view.right() {
                x += r.right() - view.right() + 8.0;
            } else if r.left() < view.left() + num_w {
                x -= view.left() + num_w - r.left() + 8.0;
            }
            if (x - h_out.state.offset.x).abs() > 0.5 {
                self.h_scroll = Some(x.max(0.0));
                ui.ctx().request_repaint();
            }
        }

        // keyboard + focus
        let total_rows = rows.len();
        if let Some((r, c, double)) = clicked {
            let shift = ui.input(|i| i.modifiers.shift);
            self.select(r, c, shift && !double);
            self.grid_focus = true;
            if double && editable && c != usize::MAX && !self.deleted.contains(&r) {
                self.begin_edit(r, c);
            }
        }
        let commit_done = commit.is_some();
        if cancel_edit {
            self.editing = None;
            self.grid_focus = true;
        } else if let Some((r, col, text)) = commit {
            self.editing = None;
            self.commit_cell(r, &col, text);
            self.grid_focus = true;
        }
        if let Some((c, dir)) = header_sort {
            if editable {
                self.server_sort = (dir != 0).then(|| (c.clone(), dir));
                acts.push(GridAction::Sort(c, dir));
            } else {
                self.local_sort = (dir != 0).then_some((c, dir > 0));
                self.reorder();
            }
        }
        if let Some(m) = menu {
            self.cell_menu(ui, m, acts);
        }
        // keyboard focus is tracked by the grid itself: egui focus traversal would steal the first
        // arrow key after a click (its lock filter only applies from the second focused frame)
        let focused = self.grid_focus && ui.memory(|m| m.focused().is_none());
        let just_edited = cancel_edit || commit_done;
        if focused && self.editing.is_none() && !just_edited {
            let (mut r, mut c) = self.sel.unwrap_or((0, 0));
            let ncols = cols.len();
            let k = self.pos(r);
            let mut moved = false;
            let extend = ui.input(|i| i.modifiers.shift);
            ui.input(|i| {
                if i.key_pressed(Key::ArrowDown) && k + 1 < total_rows {
                    r = rows_at(&self.order, ndocs, k + 1);
                    moved = true;
                }
                if i.key_pressed(Key::ArrowUp) && k > 0 {
                    r = rows_at(&self.order, ndocs, k - 1);
                    moved = true;
                }
                if (i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::Tab)) && ncols > 0 {
                    c = if c == usize::MAX { 0 } else { (c + 1).min(ncols - 1) };
                    moved = true;
                }
                if i.key_pressed(Key::ArrowLeft) && c != usize::MAX && c > 0 {
                    c -= 1;
                    moved = true;
                }
            });
            if moved {
                self.select(r, c, extend);
                self.scroll_to_sel = true;
            }
            let clone = ui.input(|i| i.modifiers.command && i.key_pressed(Key::D));
            if clone && editable && self.sel.is_some() {
                self.clone_row();
            }
            let (enter, f2, del, copy, add, submit, refresh) = ui.input(|i| {
                (
                    i.key_pressed(Key::Enter) && !i.modifiers.command,
                    i.key_pressed(Key::F2),
                    i.modifiers.command && i.key_pressed(Key::Backspace),
                    i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                    i.modifiers.command && i.key_pressed(Key::N),
                    i.modifiers.command && i.key_pressed(Key::Enter),
                    i.modifiers.command && i.key_pressed(Key::R),
                )
            });
            if (enter || f2) && editable && c != usize::MAX && self.sel.is_some() {
                self.begin_edit(r, c);
            }
            if del && editable {
                self.delete_selected();
            }
            if copy {
                self.copy_selection(ui);
            }
            if add && editable {
                self.add_row();
            }
            if submit && self.dirty() {
                acts.push(GridAction::Submit(self.pending_edits()));
            }
            if refresh {
                acts.push(GridAction::Refresh);
            }
        }
    }

    fn begin_edit(&mut self, r: usize, c: usize) {
        let Some(rs) = &self.rs else { return };
        let Some(col) = self.cols.get(c) else { return };
        let text = if r >= rs.docs.len() {
            self.added[r - rs.docs.len()].get(col).cloned().unwrap_or_default()
        } else {
            self.edits.get(&(r, col.clone())).cloned().unwrap_or_else(|| rs.docs[r].get(col).map(edit_text).unwrap_or_default())
        };
        self.editing = Some((r, c, text, true));
    }

    fn commit_cell(&mut self, r: usize, col: &str, text: String) {
        let Some(rs) = &self.rs else { return };
        if r >= rs.docs.len() {
            if let Some(row) = self.added.get_mut(r - rs.docs.len()) {
                row.insert(col.into(), text);
            }
            return;
        }
        let orig = rs.docs[r].get(col).map(edit_text).unwrap_or_default();
        if text == orig {
            self.edits.remove(&(r, col.to_string()));
        } else {
            self.edits.insert((r, col.to_string()), text);
        }
    }

    fn cell_menu(&mut self, ui: &Ui, m: CellMenu, acts: &mut Vec<GridAction>) {
        match m {
            CellMenu::Copy => self.copy_selection(ui),
            CellMenu::Extractor(e) => self.extractor = e,
            CellMenu::Edit(r, c) => self.begin_edit(r, c),
            CellMenu::SetNull(r, c) => {
                let col = self.cols[c].clone();
                self.commit_cell(r, &col, "null".into());
            }
            CellMenu::AddRow => self.add_row(),
            CellMenu::Clone(r, c) => {
                self.select(r, c, false);
                self.clone_row();
            }
            CellMenu::Delete(r) => {
                // the whole selection when the row is part of it
                if !self.sel_rect().is_some_and(|(rows, _)| rows.contains(&r)) {
                    self.select(r, usize::MAX, false);
                }
                self.delete_selected();
            }
            CellMenu::FilterBy(r, c) => {
                let sql = self.rs.as_ref().is_some_and(ResultSet::sql);
                if let Some(v) = self.rs.as_ref().and_then(|rs| rs.docs.get(r)).and_then(|d| d.get(&self.cols[c])) {
                    let col = &self.cols[c];
                    acts.push(GridAction::FilterBy(match (sql, v) {
                        (true, Bson::Null) => format!("{} IS NULL", pg::ident(col)),
                        (true, Bson::Boolean(b)) => format!("{} = {b}", pg::ident(col)),
                        (true, v) => format!("{} = {}", pg::ident(col), pg::lit(&edit_text(v))),
                        (false, Bson::String(s)) => format!("{{{}: {s:?}}}", key_lit(col)),
                        (false, v) => format!("{{{}: {}}}", key_lit(col), edit_text(v)),
                    }));
                }
            }
            CellMenu::Refresh => acts.push(GridAction::Refresh),
        }
    }

    /// Pending changes rendered as the shell statements Submit will effectively run.
    fn preview_script(&self) -> String {
        let Some((_, coll)) = self.rs.as_ref().and_then(|r| r.table()) else {
            return String::new();
        };
        if self.rs.as_ref().is_some_and(ResultSet::sql) {
            return self.pending_edits().iter().map(|e| pg::edit_sql(&coll, e) + ";").collect::<Vec<_>>().join("\n");
        }
        let target = format!("db.getCollection({coll:?})");
        let mut out = Vec::new();
        for e in self.pending_edits() {
            out.push(match e {
                Edit::Set { id, field, text, as_string } => {
                    let v = if as_string { format!("{text:?}") } else { text };
                    format!("{target}.updateOne({{_id: {}}}, {{$set: {{{}: {v}}}}})", edit_text(&id), key_lit(&field))
                }
                Edit::Insert { fields } => format!(
                    "{target}.insertOne({{{}}})",
                    fields
                        .iter()
                        .map(|(k, v, as_string)| if *as_string { format!("{}: {v:?}", key_lit(k)) } else { format!("{}: {v}", key_lit(k)) })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Edit::Delete { id } => format!("{target}.deleteOne({{_id: {}}})", edit_text(&id)),
            });
        }
        out.join("\n")
    }

    /// Current value of a cell including a pending edit, as editor text.
    fn cell_text(&self, r: usize, col: &str) -> (Option<&Bson>, Option<String>) {
        let rs = self.rs.as_ref().unwrap();
        let v = rs.docs.get(r).and_then(|d| d.get(col));
        let pending = self
            .edits
            .get(&(r, col.to_string()))
            .cloned()
            .or_else(|| r.checked_sub(rs.docs.len()).and_then(|i| self.added.get(i)).and_then(|m| m.get(col).cloned()));
        (v, pending)
    }

    fn value_panel_ui(&mut self, ui: &mut Ui, rect: Rect, id: Id) {
        ui.painter().rect_filled(rect, 0.0, ISLAND);
        let editable = self.editable();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), 30.0));
        row_ui(ui, head.shrink2(vec2(8.0, 0.0)), |ui| {
            for (rec, label) in [(true, "Record"), (false, "Value")] {
                let g = ui.painter().layout_no_wrap(label.into(), ui_font(13.0), TEXT);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 24.0), Sense::CLICK);
                if self.panel_record == rec {
                    ui.painter().rect(r, CornerRadius::same(6), TOOLBAR_HOVER, Stroke::new(1.0, CONTROL_BORDER), egui::StrokeKind::Inside);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(6), HOVER);
                }
                ui.painter().galley(r.center() - g.size() / 2.0, g, TEXT);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, self.panel_record == rec, label));
                if resp.clicked() {
                    self.panel_record = rec;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if icon_btn(ui, ic::HIDE, "Hide Value Editor  ⇧⏎").clicked() {
                    self.value_panel = false;
                }
            });
        });
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
        let Some((r, c)) = self.sel else {
            text_at(ui, body.center(), Align2::CENTER_CENTER, "Select a cell", ui_font(13.0), TEXT_SECONDARY);
            return;
        };
        if self.panel_record || c == usize::MAX {
            // every field of the selected row
            let rows: Vec<(String, LayoutJob)> = self
                .cols
                .iter()
                .map(|col| {
                    let (v, pending) = self.cell_text(r, col);
                    (
                        col.clone(),
                        match pending {
                            Some(t) => plain_job(&t, EDITOR_TEXT),
                            None => cell_job(v).0,
                        },
                    )
                })
                .collect();
            let mut pick = None;
            let mut commit: Option<(String, String)> = None;
            let mut cancel = false;
            let edit_ok = editable && !self.deleted.contains(&r) && self.editing.is_none();
            let rec_edit = &mut self.rec_edit;
            rect_ui(ui, body, |ui| {
                egui::ScrollArea::vertical().id_salt(id.with("record")).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    for (j, (col, job)) in rows.into_iter().enumerate() {
                        let (rr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::CLICK);
                        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, j == c, &col));
                        if j == c {
                            ui.painter().rect_filled(rr, 0.0, SEL_INACTIVE);
                        }
                        let kw = (rr.width() * 0.38).min(140.0);
                        if let Some((ec, buf, first)) = rec_edit.as_mut().filter(|(ec, _, _)| *ec == j) {
                            ui.painter().hline(rr.x_range(), rr.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                            ui.painter().with_clip_rect(Rect::from_min_size(rr.min, vec2(kw, ROW_H))).text(
                                pos2(rr.left() + 10.0, rr.center().y),
                                Align2::LEFT_CENTER,
                                &col,
                                mono(12.5),
                                TEXT_MUTED,
                            );
                            let vr = Rect::from_min_max(pos2(rr.left() + kw + 1.0, rr.top() + 2.0), rr.max - vec2(2.0, 2.0));
                            ui.painter().rect_stroke(vr, CornerRadius::same(3), Stroke::new(1.0, BRAND), egui::StrokeKind::Inside);
                            let te = egui::TextEdit::singleline(buf)
                                .id(id.with(("record-edit", *ec)))
                                .font(mono(13.0))
                                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(5, 3)));
                            let out = ui.put(vr, te);
                            if *first {
                                out.request_focus();
                                *first = false;
                            }
                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                cancel = true;
                            } else if out.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Tab)) {
                                commit = Some((col.clone(), buf.clone()));
                            }
                            continue;
                        }
                        ui.painter().hline(rr.x_range(), rr.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                        ui.painter().with_clip_rect(Rect::from_min_size(rr.min, vec2(kw, ROW_H))).text(
                            pos2(rr.left() + 10.0, rr.center().y),
                            Align2::LEFT_CENTER,
                            col,
                            mono(12.5),
                            TEXT_MUTED,
                        );
                        let g = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().with_clip_rect(Rect::from_min_max(pos2(rr.left() + kw, rr.top()), rr.max)).galley(
                            pos2(rr.left() + kw + 6.0, rr.center().y - g.size().y / 2.0),
                            g,
                            EDITOR_TEXT,
                        );
                        if resp.clicked() {
                            pick = Some(j);
                        }
                    }
                });
            });
            if cancel || commit.is_some() {
                // the inline editor disappears; release its keyboard focus so ⌘↩ / arrows reach the grid,
                // and eat the Enter/Tab so the grid (drawn after the panel) doesn't start a cell edit too
                if let Some((ec, _, _)) = self.rec_edit.take() {
                    ui.memory_mut(|m| m.surrender_focus(id.with(("record-edit", ec))));
                }
                consume_commit_keys(ui);
            }
            if let Some((col, text)) = commit {
                self.commit_cell(r, &col, text);
            }
            if let Some(j) = pick {
                self.select(r, j, false);
                if edit_ok && self.cols[j] != "_id" {
                    let col = self.cols[j].clone();
                    let (v, pending) = self.cell_text(r, &col);
                    let text = pending.unwrap_or_else(|| match v {
                        Some(Bson::String(s)) => s.clone(),
                        Some(x) => edit_text(x),
                        None => String::new(),
                    });
                    self.rec_edit = Some((j, text, true));
                }
            }
            return;
        }
        let Some(col) = self.cols.get(c).cloned() else {
            return;
        };
        let (v, pending) = {
            let (v, p) = self.cell_text(r, &col);
            (v.cloned(), p)
        };
        let v = v.as_ref();
        let key = (r, c);
        // arrays: item list with + − ↶ like the IDE's value editor (unless edited as text elsewhere)
        let arr_mode = pending.is_none() || self.panel_arr.as_ref().is_some_and(|(k, _)| *k == key);
        if let (Some(Bson::Array(items)), true) = (v, arr_mode) {
            if self.panel_arr.as_ref().is_none_or(|(k, _)| *k != key) {
                self.panel_arr = Some((key, items.clone()));
                self.panel_item = None;
                self.panel_item_edit = None;
            }
            let strings = items.iter().all(|x| matches!(x, Bson::String(_)));
            let mut changed = false;
            let tb = Rect::from_min_size(body.min, vec2(body.width(), 32.0));
            row_ui(ui, tb.shrink2(vec2(8.0, 0.0)), |ui| {
                if icon_btn_ex(ui, ic::ADD, "Add Element", editable, false, false).clicked() {
                    let arr = &mut self.panel_arr.as_mut().unwrap().1;
                    arr.push(if strings { Bson::String(String::new()) } else { Bson::Null });
                    let i = arr.len() - 1;
                    self.panel_item = Some(i);
                    self.panel_item_edit = Some((i, String::new()));
                    changed = true;
                }
                if icon_btn_ex(ui, ic::REMOVE, "Remove Element", editable && self.panel_item.is_some(), false, false).clicked() {
                    if let (Some(i), Some((_, arr))) = (self.panel_item, self.panel_arr.as_mut()) {
                        if i < arr.len() {
                            arr.remove(i);
                            changed = true;
                        }
                    }
                    self.panel_item = None;
                }
                if icon_btn_ex(ui, ic::UNDO, "Revert", pending.is_some(), false, false).clicked() {
                    self.edits.remove(&(r, col.clone()));
                    self.panel_arr = Some((key, items.clone()));
                }
            });
            let list = Rect::from_min_max(pos2(body.left(), tb.bottom()), body.max);
            let mut commit_item: Option<(usize, String)> = None;
            rect_ui(ui, list, |ui| {
                egui::ScrollArea::vertical().id_salt(id.with("arr")).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    let arr = self.panel_arr.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
                    for (i, x) in arr.iter().enumerate() {
                        let (rr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::CLICK);
                        if self.panel_item == Some(i) {
                            ui.painter().rect_filled(rr, 0.0, SEL_ACTIVE);
                        }
                        ui.painter().hline(rr.x_range(), rr.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                        if let Some((ei, buf)) = self.panel_item_edit.as_mut().filter(|(ei, _)| *ei == i) {
                            let out = ui
                                .put(rr.shrink2(vec2(6.0, 3.0)), egui::TextEdit::singleline(buf).font(mono(13.0)).frame(egui::Frame::NONE));
                            if !out.has_focus() && !out.lost_focus() {
                                out.request_focus();
                            }
                            if out.lost_focus() || ui.input(|inp| inp.key_pressed(Key::Enter)) {
                                commit_item = Some((*ei, buf.clone()));
                            }
                            continue;
                        }
                        let text = match x {
                            Bson::String(s) => s.clone(),
                            v => plain(v),
                        };
                        ui.painter().with_clip_rect(rr.shrink(2.0)).text(
                            pos2(rr.left() + 12.0, rr.center().y),
                            Align2::LEFT_CENTER,
                            text,
                            mono(13.0),
                            EDITOR_TEXT,
                        );
                        if resp.clicked() {
                            self.panel_item = Some(i);
                        }
                        if resp.double_clicked() && editable {
                            self.panel_item_edit = Some((
                                i,
                                match x {
                                    Bson::String(s) => s.clone(),
                                    v => edit_text(v),
                                },
                            ));
                        }
                    }
                });
            });
            if let Some((i, t)) = commit_item {
                self.panel_item_edit = None;
                consume_commit_keys(ui);
                if let Some((_, arr)) = self.panel_arr.as_mut() {
                    if let Some(slot) = arr.get_mut(i) {
                        // string arrays take raw text; otherwise keep it as a shell literal string
                        *slot = if strings || matches!(slot, Bson::String(_)) { Bson::String(t) } else { Bson::JavaScriptCode(t) };
                        changed = true;
                    }
                }
            }
            if changed {
                let arr = self.panel_arr.as_ref().map(|(_, a)| a.clone()).unwrap_or_default();
                let lit = format!(
                    "[{}]",
                    arr.iter()
                        .map(|x| match x {
                            Bson::JavaScriptCode(code) => code.clone(),
                            Bson::String(s) => format!("{s:?}"),
                            v => edit_text(v),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.commit_cell(r, &col, lit);
            }
            return;
        }
        // other values: editable text (strings raw, everything else a shell literal)
        let text_id = id.with("value-text");
        let panel_focused = ui.memory(|m| m.has_focus(text_id));
        if !panel_focused || self.panel_text.as_ref().is_none_or(|(k, _)| *k != key) {
            let text = pending.clone().unwrap_or_else(|| match v {
                Some(Bson::Document(_)) => serde_json::to_string_pretty(&v.unwrap().clone().into_relaxed_extjson()).unwrap_or_default(),
                Some(x) => edit_text(x),
                None => String::new(),
            });
            self.panel_text = Some((key, text));
        }
        let er = body.shrink2(vec2(0.0, 2.0));
        let mut new_text = None;
        rect_ui(ui, er, |ui| {
            let (_, text) = self.panel_text.as_mut().unwrap();
            let te = egui::TextEdit::multiline(text)
                .font(mono(13.0))
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)))
                .desired_width(f32::INFINITY)
                .id(text_id)
                .interactive(editable && col != "_id" && !self.deleted.contains(&r) && self.editing.is_none())
                .min_size(er.size());
            let out = egui::ScrollArea::both().id_salt(id.with("val")).auto_shrink([false, false]).show(ui, |ui| ui.add(te)).inner;
            if out.changed() {
                new_text = Some(text.clone());
            }
        });
        if let Some(t) = new_text {
            self.commit_cell(r, &col, t);
        }
    }

    fn transposed(&mut self, ui: &mut Ui, id: Id) {
        let rs = self.rs.as_ref().unwrap();
        let docs: Vec<&Document> = self.order.iter().map(|&i| &rs.docs[i]).collect();
        egui::ScrollArea::both().id_salt(id.with("tr")).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let name_w = 160.0;
            let col_w = 220.0;
            let width = name_w + col_w * docs.len() as f32;
            for (j, c) in self.cols.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(width.max(ui.available_width()), ROW_H), Sense::hover());
                let nr = Rect::from_min_size(r.min, vec2(name_w, ROW_H));
                ui.painter().rect_filled(nr, 0.0, GRID_HEADER);
                text_at(ui, pos2(nr.left() + 8.0, nr.center().y), Align2::LEFT_CENTER, c, mono(13.0), EDITOR_TEXT);
                ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, GRID_LINE));
                for (k, d) in docs.iter().enumerate() {
                    let cr = Rect::from_min_size(pos2(r.left() + name_w + k as f32 * col_w, r.top()), vec2(col_w, ROW_H));
                    ui.painter().vline(cr.left(), cr.y_range(), Stroke::new(1.0, GRID_LINE));
                    let (job, _) = cell_job(d.get(c));
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    ui.painter().with_clip_rect(cr.shrink(2.0)).galley(
                        pos2(cr.left() + 7.0, cr.center().y - g.size().y / 2.0),
                        g,
                        EDITOR_TEXT,
                    );
                }
                let _ = j;
            }
        });
    }

    fn tree(&mut self, ui: &mut Ui, id: Id) {
        let rs = self.rs.as_ref().unwrap();
        let docs: Vec<(usize, &Document)> = self.order.iter().map(|&i| (i, &rs.docs[i])).collect();
        let key_w = (ui.available_width() * 0.3).clamp(160.0, 420.0);
        let expanded = &mut self.expanded;
        let cols = &self.cols;
        egui::ScrollArea::both().id_salt(id.with("tree")).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for (k, (_, d)) in docs.iter().enumerate() {
                let path = k.to_string();
                let sorted = Bson::Document(sorted_doc(d, cols));
                tree_row(ui, expanded, &path, &(k + 1 + rs.offset as usize).to_string(), &sorted, 0, key_w);
            }
        });
    }

    fn pager(&mut self, ui: &mut Ui, rect: Rect, acts: &mut Vec<GridAction>) {
        let Some(rs) = &self.rs else { return };
        let n = rs.docs.len() as u64;
        let paged = rs.offset > 0 || rs.more;
        let label =
            if paged { format!("{}-{}", rs.offset + 1, rs.offset + n) } else { format!("{n} row{}", if n == 1 { "" } else { "s" }) };
        let font = ui_font(12.0);
        let lw = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), TEXT).size().x) + 18.0;
        let of = if paged {
            format!("of {}", if rs.more { format!("{}+", rs.offset + n + 1) } else { (rs.offset + n).to_string() })
        } else {
            String::new()
        };
        let ow = if paged { ui.fonts_mut(|f| f.layout_no_wrap(of.clone(), font.clone(), TEXT).size().x) + 8.0 } else { 0.0 };
        let nav = if paged { 26.0 * 4.0 } else { 0.0 };
        let w = nav + lw + ow + 30.0 + 12.0;
        let pr = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 24.0), vec2(w, 30.0));
        ui.painter().rect(pr, CornerRadius::same(8), rgb_popup(), Stroke::new(1.0, POPUP_BORDER), egui::StrokeKind::Inside);
        let page = rs.page.max(1);
        let (offset, more, has_spec) = (rs.offset, rs.more, rs.spec.is_some());
        row_ui(ui, pr.shrink2(vec2(6.0, 2.0)), |ui| {
            if paged {
                if icon_btn_ex(ui, ic::PLAY_FIRST, "First Page", offset > 0, false, false).clicked() {
                    acts.push(GridAction::Page { offset: 0, limit: page });
                }
                if icon_btn_ex(ui, ic::CHEVRON_LEFT, "Previous Page", offset > 0, false, false).clicked() {
                    acts.push(GridAction::Page { offset: offset.saturating_sub(page), limit: page });
                }
            }
            let dd = dropdown(ui, None, &label, "Page size");
            egui::Popup::menu(&dd).show(|ui| {
                ui.set_min_width(150.0);
                menu_section(ui, "Page Size");
                for s in [10u64, 100, 500, 1000] {
                    if menu_check(ui, &s.to_string(), page == s) && has_spec {
                        acts.push(GridAction::Page { offset: 0, limit: s });
                    }
                }
            });
            if paged {
                ui.label(egui::RichText::new(&of).font(font.clone()).color(TEXT_MUTED));
                if icon_btn_ex(ui, ic::CHEVRON_RIGHT, "Next Page", more, false, false).clicked() {
                    acts.push(GridAction::Page { offset: offset + page, limit: page });
                }
                if icon_btn_ex(ui, ic::PLAY_LAST, "Last Page", more, false, false).clicked() {
                    acts.push(GridAction::Page { offset: offset + page * 10, limit: page });
                }
            }
            vsep(ui);
            let more_btn = icon_btn(ui, ic::MORE_VERTICAL, "");
            egui::Popup::menu(&more_btn).show(|ui| {
                ui.set_min_width(180.0);
                if menu_item(ui, Some(ic::REFRESH), "Reload Page", "⌘R", true) {
                    acts.push(GridAction::Refresh);
                }
            });
        });
    }
}

enum CellMenu {
    Copy,
    Extractor(Extractor),
    Edit(usize, usize),
    SetNull(usize, usize),
    AddRow,
    Clone(usize, usize),
    Delete(usize),
    FilterBy(usize, usize),
    Refresh,
}

/// Trackpad gestures scroll either horizontally or vertically, never diagonally (like the IDE's
/// tables): the dominant axis at the start of a gesture wins until it pauses for 250 ms.
fn lock_scroll_axis(ui: &Ui, lock: &mut ScrollLock) {
    ui.ctx().input_mut(|i| {
        let d = i.smooth_scroll_delta;
        if d != egui::Vec2::ZERO {
            i.smooth_scroll_delta = lock.filter(d, i.time);
        }
    });
}

/// Centered spinner + "Querying..." over the grid while a page loads, as in the IDE.
fn querying_overlay(ui: &Ui, rect: Rect) {
    let c = rect.center() - vec2(0.0, 14.0);
    let box_r = Rect::from_center_size(c + vec2(0.0, 14.0), vec2(120.0, 76.0));
    ui.painter().rect_filled(box_r, CornerRadius::same(8), ISLAND.gamma_multiply(0.92));
    ij_spinner(ui, c, 14.0);
    text_at(ui, c + vec2(0.0, 30.0), Align2::CENTER_CENTER, "Querying...", ui_font(14.0), TEXT_SECONDARY);
}

fn consume_commit_keys(ui: &Ui) {
    ui.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, Key::Enter);
        i.consume_key(egui::Modifiers::NONE, Key::Tab);
        i.consume_key(egui::Modifiers::NONE, Key::Escape);
    });
}

/// Trackpad gestures scroll along one axis, like the IDE's tables. The axis is decided from the
/// movement accumulated at the start of a gesture (a single noisy event can't lock the wrong one);
/// until then only the dominant component of each event passes. A short pause starts a new gesture,
/// and a clearly perpendicular swipe switches axis instead of feeling stuck.
#[derive(Default, Clone, Copy)]
struct ScrollLock {
    axis: Option<bool>,
    acc: egui::Vec2,
    last: f64,
}

impl ScrollLock {
    fn filter(&mut self, d: egui::Vec2, now: f64) -> egui::Vec2 {
        if now - self.last > 0.12 {
            *self = ScrollLock::default();
        }
        self.last = now;
        self.acc += d.abs();
        let (ax, ay) = (self.acc.x, self.acc.y);
        match self.axis {
            None if ax > 6.0 && ax > ay * 1.5 => self.axis = Some(true),
            None if ay > 6.0 && ay > ax * 1.5 => self.axis = Some(false),
            Some(true) if d.y.abs() > 4.0 && d.y.abs() > d.x.abs() * 3.0 => self.axis = Some(false),
            Some(false) if d.x.abs() > 4.0 && d.x.abs() > d.y.abs() * 3.0 => self.axis = Some(true),
            _ => {}
        }
        let horizontal = self.axis.unwrap_or(d.x.abs() > d.y.abs());
        if horizontal { egui::vec2(d.x, 0.0) } else { egui::vec2(0.0, d.y) }
    }
}

fn rows_at(order: &[usize], ndocs: usize, k: usize) -> usize {
    order.get(k).copied().unwrap_or(ndocs + k - order.len())
}

fn rgb_popup() -> Color32 {
    Color32::from_rgb(0x2B, 0x2D, 0x30)
}

fn tree_row(ui: &mut Ui, expanded: &mut HashSet<String>, path: &str, key: &str, v: &Bson, depth: usize, key_w: f32) {
    let w = ui.available_width().max(800.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 25.0), Sense::CLICK);
    let container = matches!(v, Bson::Document(d) if !d.is_empty()) || matches!(v, Bson::Array(a) if !a.is_empty());
    let open = expanded.contains(path);
    if open && depth == 0 {
        ui.painter().rect_filled(r, 0.0, SEL_ACTIVE);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, HOVER);
    }
    let x = r.left() + 6.0 + depth as f32 * 18.0;
    if container {
        let icon = if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT };
        paint_icon(ui, Rect::from_min_size(pos2(x - 2.0, r.center().y - 8.0), vec2(16.0, 16.0)), icon);
    }
    text_at(ui, pos2(x + 16.0, r.center().y), Align2::LEFT_CENTER, key, mono(13.0), EDITOR_TEXT);
    ui.painter().vline(r.left() + key_w, r.y_range(), Stroke::new(1.0, GRID_LINE));
    if !(open && depth == 0) {
        let (job, _) = cell_job(Some(v));
        let g = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().with_clip_rect(Rect::from_min_max(pos2(r.left() + key_w + 1.0, r.top()), r.max)).galley(
            pos2(r.left() + key_w + 8.0, r.center().y - g.size().y / 2.0),
            g,
            EDITOR_TEXT,
        );
    }
    if container && resp.clicked() && !expanded.remove(path) {
        expanded.insert(path.to_string());
    }
    if open {
        match v {
            Bson::Document(d) => {
                for (k, c) in d {
                    tree_row(ui, expanded, &format!("{path}/{k}"), k, c, depth + 1, key_w);
                }
            }
            Bson::Array(a) => {
                for (i, c) in a.iter().enumerate() {
                    tree_row(ui, expanded, &format!("{path}/{i}"), &i.to_string(), c, depth + 1, key_w);
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ formatting

/// Columns: `_id` first, then the remaining top-level keys alphabetically (as DataGrip).
pub fn columns(docs: &[Document]) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for d in docs {
        for k in d.keys() {
            if !set.contains(k) {
                set.push(k.clone());
            }
        }
    }
    set.sort_by(|a, b| (a != "_id", a).cmp(&(b != "_id", b)));
    set
}

fn sorted_doc(d: &Document, cols: &[String]) -> Document {
    let mut out = Document::new();
    for c in cols {
        if let Some(v) = d.get(c) {
            out.insert(c.clone(), v.clone());
        }
    }
    out
}

fn col_meta(cols: &[String], rs: &ResultSet) -> Vec<(f32, String)> {
    let docs = &rs.docs;
    cols.iter()
        .map(|c| {
            let ty = match rs.cols.iter().find(|x| x.0 == *c) {
                Some((_, t)) => t.as_str(),
                None => docs.iter().find_map(|d| d.get(c).filter(|v| !matches!(v, Bson::Null))).map_or("", type_name),
            };
            (initial_width(c, ty, docs), ty.to_string())
        })
        .collect()
}

/// PostgreSQL number types (right-aligned like DataGrip).
fn is_numeric(ty: &str) -> bool {
    matches!(ty, "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid" | "money")
}

fn initial_width(col: &str, ty: &str, docs: &[Document]) -> f32 {
    let header = col.chars().count() as f32 * 7.8 + 70.0;
    let sample = docs.iter().take(50).filter_map(|d| d.get(col)).map(|v| plain(v).chars().count()).max().unwrap_or(0) as f32 * 7.8 + 20.0;
    let cap = if ty == "Object" || ty == "list" { 330.0 } else { 260.0 };
    header.max(sample).clamp(60.0, cap)
}

pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

fn trim_num(s: String) -> String {
    if s.contains('.') && !s.contains('E') && !s.contains('e') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn double(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{f}") }
}

/// Plain single-line text of a value (search, width estimation, extractors).
pub fn plain(v: &Bson) -> String {
    match v {
        Bson::String(s) => s.replace('\n', " "),
        Bson::ObjectId(o) => o.to_hex(),
        Bson::Boolean(b) => b.to_string(),
        Bson::Int32(i) => i.to_string(),
        Bson::Int64(i) => i.to_string(),
        Bson::Double(f) => double(*f),
        Bson::Decimal128(d) => trim_num(d.to_string()),
        Bson::DateTime(d) => iso(d.timestamp_millis()),
        Bson::Null => "<null>".into(),
        v => {
            let mut job = LayoutJob::default();
            js_repr(v, &mut job, usize::MAX);
            job.text
        }
    }
}

/// Text placed in the inline editor; parsed back as a JS literal unless the column is a string.
pub fn edit_text(v: &Bson) -> String {
    match v {
        Bson::String(s) => s.clone(),
        Bson::ObjectId(o) => format!("ObjectId(\"{}\")", o.to_hex()),
        Bson::DateTime(d) => format!("ISODate(\"{}\")", iso(d.timestamp_millis())),
        Bson::Int64(i) => format!("NumberLong({i})"),
        Bson::Decimal128(d) => format!("NumberDecimal(\"{d}\")"),
        Bson::Null => "null".into(),
        Bson::Document(_) | Bson::Array(_) => {
            let mut job = LayoutJob::default();
            js_repr_literal(v, &mut job);
            job.text
        }
        v => plain(v),
    }
}

fn key_lit(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') && !k.starts_with(|c: char| c.is_ascii_digit()) {
        k.to_string()
    } else {
        format!("{k:?}")
    }
}

fn tf(color: Color32) -> TextFormat {
    TextFormat { font_id: mono(13.0), color, ..Default::default() }
}

/// A cell shows one clipped line (≈2000px at most); laying out more text than that every frame
/// makes scrolling crawl on big arrays / long strings.
const CELL_CHARS: usize = 300;

fn plain_job(t: &str, color: Color32) -> LayoutJob {
    let t: String = t.chars().take(CELL_CHARS).map(|c| if c == '\n' { ' ' } else { c }).collect();
    let mut j = LayoutJob::single_section(t, tf(color));
    j.wrap.max_rows = 1;
    j
}

/// Colored cell content + whether it is right-aligned (numbers).
pub fn cell_job(v: Option<&Bson>) -> (LayoutJob, bool) {
    let Some(v) = v else {
        return (plain_job("", EDITOR_TEXT), false);
    };
    match v {
        Bson::Null => (plain_job("<null>", TEXT_SECONDARY), false),
        Bson::Boolean(b) => (plain_job(if *b { "• true" } else { "  false" }, EDITOR_TEXT), false),
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => (plain_job(&plain(v), EDITOR_TEXT), true),
        Bson::Document(_) | Bson::Array(_) => {
            let mut job = LayoutJob::default();
            js_repr(v, &mut job, CELL_CHARS);
            job.wrap.max_rows = 1;
            (job, false)
        }
        v => (plain_job(&plain(v), EDITOR_TEXT), false),
    }
}

/// `{"city": "Bandung", "zip": "10001"}` with JSON-ish coloring (cells, tree view); nested
/// values stop once the text reaches `cap` bytes.
fn js_repr(v: &Bson, job: &mut LayoutJob, cap: usize) {
    let p = |job: &mut LayoutJob, s: &str, c: Color32| job.append(s, 0.0, tf(c));
    if job.text.len() >= cap {
        return;
    }
    match v {
        Bson::Document(d) => {
            p(job, "{", EDITOR_TEXT);
            for (i, (k, x)) in d.iter().enumerate() {
                if job.text.len() >= cap {
                    break;
                }
                if i > 0 {
                    p(job, ", ", EDITOR_TEXT);
                }
                p(job, &format!("{k:?}"), STRING);
                p(job, ": ", EDITOR_TEXT);
                js_repr(x, job, cap);
            }
            p(job, "}", EDITOR_TEXT);
        }
        Bson::Array(a) => {
            p(job, "[", EDITOR_TEXT);
            for (i, x) in a.iter().enumerate() {
                if job.text.len() >= cap {
                    break;
                }
                if i > 0 {
                    p(job, ", ", EDITOR_TEXT);
                }
                js_repr(x, job, cap);
            }
            p(job, "]", EDITOR_TEXT);
        }
        Bson::String(s) => p(job, &format!("{s:?}"), STRING),
        Bson::Boolean(b) => p(job, &b.to_string(), KEYWORD),
        Bson::Null => p(job, "null", KEYWORD),
        Bson::ObjectId(o) => {
            p(job, "new ", KEYWORD);
            p(job, "ObjectId(", EDITOR_TEXT);
            p(job, &format!("\"{}\"", o.to_hex()), STRING);
            p(job, ")", EDITOR_TEXT);
        }
        Bson::DateTime(d) => {
            p(job, "ISODate(", EDITOR_TEXT);
            p(job, &format!("\"{}\"", iso(d.timestamp_millis())), STRING);
            p(job, ")", EDITOR_TEXT);
        }
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => p(job, &plain(v), NUMBER),
        other => p(job, &other.clone().into_relaxed_extjson().to_string(), EDITOR_TEXT),
    }
}

/// Like `js_repr` but valid shell input (no `new`, typed wrappers kept).
fn js_repr_literal(v: &Bson, job: &mut LayoutJob) {
    match v {
        Bson::Document(d) => {
            job.append("{", 0.0, tf(EDITOR_TEXT));
            for (i, (k, x)) in d.iter().enumerate() {
                if i > 0 {
                    job.append(", ", 0.0, tf(EDITOR_TEXT));
                }
                job.append(&format!("{k:?}: "), 0.0, tf(EDITOR_TEXT));
                js_repr_literal(x, job);
            }
            job.append("}", 0.0, tf(EDITOR_TEXT));
        }
        Bson::Array(a) => {
            job.append("[", 0.0, tf(EDITOR_TEXT));
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    job.append(", ", 0.0, tf(EDITOR_TEXT));
                }
                js_repr_literal(x, job);
            }
            job.append("]", 0.0, tf(EDITOR_TEXT));
        }
        Bson::String(s) => job.append(&format!("{s:?}"), 0.0, tf(EDITOR_TEXT)),
        v => job.append(&edit_text(v), 0.0, tf(EDITOR_TEXT)),
    }
}

fn cmp_bson(a: Option<&Bson>, b: Option<&Bson>) -> std::cmp::Ordering {
    let num = |v: &Bson| match v {
        Bson::Int32(i) => Some(*i as f64),
        Bson::Int64(i) => Some(*i as f64),
        Bson::Double(f) => Some(*f),
        Bson::Decimal128(d) => d.to_string().parse().ok(),
        Bson::DateTime(d) => Some(d.timestamp_millis() as f64),
        _ => None,
    };
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => match (num(x), num(y)) {
            (Some(p), Some(q)) => p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal),
            _ => plain(x).cmp(&plain(y)),
        },
    }
}

/// Text view content: Extended JSON array, keys in column order, `{"$oid": …}` wrappers inline.
fn text_json(docs: &[&Document], cols: &[String]) -> String {
    let arr: Vec<serde_json::Value> = docs.iter().map(|d| Bson::Document(sorted_doc(d, cols)).into_relaxed_extjson()).collect();
    let mut out = String::new();
    pretty(&serde_json::Value::Array(arr), 0, &mut out);
    out
}

fn pretty(v: &serde_json::Value, ind: usize, out: &mut String) {
    use serde_json::Value as J;
    let pad = |n: usize| "  ".repeat(n);
    match v {
        J::Object(m) if m.keys().all(|k| k.starts_with('$')) && !m.is_empty() => {
            out.push_str(&v.to_string().replace("\":\"", "\": \"").replace(",\"", ", \""))
        }
        J::Object(m) if !m.is_empty() => {
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                out.push_str(&format!("{}{}: ", pad(ind + 1), J::String(k.clone())));
                pretty(x, ind + 1, out);
                out.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(ind));
            out.push('}');
        }
        J::Array(a) if !a.is_empty() => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&pad(ind + 1));
                pretty(x, ind + 1, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(ind));
            out.push(']');
        }
        _ => out.push_str(&v.to_string()),
    }
}

/// What the SQL extractors need besides the values (built from a SQL result).
pub struct SqlInfo {
    /// Target table as SQL (`public.products`); `None` for results not from one table (`MY_TABLE`).
    pub table: Option<String>,
    /// Primary key columns (UPDATE's WHERE).
    pub key: Vec<String>,
    /// Columns whose values are bare numbers.
    pub numeric: HashSet<String>,
}

fn sql_info(rs: &ResultSet) -> Option<SqlInfo> {
    if !rs.sql() {
        return None;
    }
    let table = rs.table().map(|(_, t)| t);
    let key = if table.is_some() { rs.key() } else { vec![] };
    let numeric = rs
        .cols
        .iter()
        .filter(|(_, t)| matches!(t.as_str(), "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "oid"))
        .map(|(c, _)| c.clone())
        .collect();
    Some(SqlInfo { table, key, numeric })
}

/// SQL literal of a cell: numbers / booleans bare, NULL as `null`, everything else (text, dates,
/// json, arrays) a quoted string — as DataGrip's SQL extractors write them.
fn sql_value(v: Option<&Bson>, numeric: bool, null: &str) -> String {
    match v {
        None | Some(Bson::Null) => null.into(),
        Some(Bson::Boolean(b)) => b.to_string(),
        Some(Bson::String(s)) if numeric && s.parse::<f64>().is_ok_and(f64::is_finite) => s.clone(),
        Some(Bson::String(s)) => pg::lit(s),
        Some(v) => pg::lit(&plain(v)),
    }
}

/// SQL Inserts / Updates / Where Clause and the scripted SQL-Insert-* extractors.
fn extract_sql(ex: Extractor, cols: &[String], docs: &[&Document], sql: Option<&SqlInfo>) -> String {
    let empty = SqlInfo { table: None, key: vec![], numeric: HashSet::new() };
    let info = sql.unwrap_or(&empty);
    let table = info.table.clone().unwrap_or_else(|| "MY_TABLE".into());
    // the scripted ones print `schema.table` and column names as they are
    let raw_table = info.table.as_deref().map_or_else(
        || "MY_TABLE".into(),
        |t| {
            let (schema, name) = pg::split_qual(t);
            if schema.is_empty() { name } else { format!("{schema}.{name}") }
        },
    );
    let val = |d: &Document, c: &str, null: &str| sql_value(d.get(c), info.numeric.contains(c), null);
    let list = |names: Vec<String>| names.join(", ");
    let quoted_cols = list(cols.iter().map(|c| pg::ident(c)).collect());
    let raw_cols = list(cols.to_vec());
    let eq = |d: &Document, c: &str| match d.get(c) {
        None | Some(Bson::Null) => format!("{} IS NULL", pg::ident(c)),
        _ => format!("{} = {}", pg::ident(c), val(d, c, "null")),
    };
    match ex {
        Extractor::SqlInserts => docs
            .iter()
            .map(|d| format!("INSERT INTO {table} ({quoted_cols}) VALUES ({});\n", list(cols.iter().map(|c| val(d, c, "null")).collect())))
            .collect(),
        Extractor::SqlInsertStatements => docs
            .iter()
            .map(|d| format!("INSERT INTO {raw_table} ({raw_cols}) VALUES ({});\n", list(cols.iter().map(|c| val(d, c, "NULL")).collect())))
            .collect(),
        Extractor::SqlInsertMultirow => {
            let rows: Vec<String> = docs.iter().map(|d| format!("({})", list(cols.iter().map(|c| val(d, c, "NULL")).collect()))).collect();
            format!("INSERT INTO {raw_table} ({raw_cols})\nVALUES  {};", rows.join(",\n        "))
        }
        Extractor::SqlUpdates => {
            let set: Vec<&String> = cols.iter().filter(|c| !info.key.contains(c)).collect();
            let set = if set.is_empty() { cols.iter().collect() } else { set };
            let key: Vec<&String> = if info.key.is_empty() { cols.iter().collect() } else { info.key.iter().collect() };
            docs.iter()
                .map(|d| {
                    let assign = list(set.iter().map(|c| format!("{} = {}", pg::ident(c), val(d, c, "null"))).collect());
                    let cond = key.iter().map(|c| eq(d, c)).collect::<Vec<_>>().join(" AND ");
                    format!("UPDATE {table} SET {assign} WHERE {cond};\n")
                })
                .collect()
        }
        _ => docs.iter().map(|d| cols.iter().map(|c| eq(d, c)).collect::<Vec<_>>().join(" AND ")).collect::<Vec<_>>().join("\nOR "),
    }
}

pub fn extract(ex: Extractor, cols: &[String], docs: &[&Document], sql: Option<&SqlInfo>) -> String {
    if ex.sql_only() {
        return extract_sql(ex, cols, docs, sql);
    }
    let cell = |d: &Document, c: &str| d.get(c).map(|v| if matches!(v, Bson::Null) { String::new() } else { plain(v) }).unwrap_or_default();
    let sep_join = |sep: char| {
        let q = |s: String| {
            if s.contains(sep) || s.contains('"') || s.contains('\n') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
        };
        let mut out = cols.iter().map(|c| q(c.clone())).collect::<Vec<_>>().join(&sep.to_string());
        for d in docs {
            out.push('\n');
            out.push_str(&cols.iter().map(|c| q(cell(d, c))).collect::<Vec<_>>().join(&sep.to_string()));
        }
        out
    };
    match ex {
        Extractor::Csv => sep_join(','),
        Extractor::Tsv => sep_join('\t'),
        Extractor::Pipe => sep_join('|'),
        Extractor::Semicolon => sep_join(';'),
        Extractor::Json => text_json(docs, cols),
        Extractor::OneRow => docs.iter().flat_map(|d| cols.iter().map(|c| cell(d, c))).collect::<Vec<_>>().join(", "),
        _ => {
            let widths: Vec<usize> =
                cols.iter().map(|c| docs.iter().map(|d| cell(d, c).chars().count()).max().unwrap_or(0).max(c.chars().count())).collect();
            let line = |vals: Vec<String>| {
                let cells: Vec<String> = vals.iter().zip(&widths).map(|(v, w)| format!("{v:<w$}")).collect();
                format!("| {} |", cells.join(" | "))
            };
            let mut out = vec![line(cols.to_vec())];
            let rule = if ex == Extractor::Markdown { "|:" } else { "+-" };
            out.push(
                format!(
                    "{}{}",
                    rule,
                    widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join(if ex == Extractor::Markdown { "-|:" } else { "-+-" })
                ) + if ex == Extractor::Markdown { "-|" } else { "-+" },
            );
            for d in docs {
                out.push(line(cols.iter().map(|c| cell(d, c)).collect()));
            }
            out.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn scroll_gesture_keeps_one_axis() {
        let v = egui::vec2;
        let mut l = ScrollLock::default();
        // noisy start: first event slightly horizontal, then clearly vertical -> vertical, never stuck
        assert_eq!(l.filter(v(1.0, 0.6), 0.0), v(1.0, 0.0));
        assert_eq!(l.filter(v(0.5, 6.0), 0.016), v(0.0, 6.0));
        assert_eq!(l.filter(v(3.0, 8.0), 0.032), v(0.0, 8.0));
        // locked vertical: diagonal drift is ignored
        assert_eq!(l.filter(v(5.0, 6.0), 0.048), v(0.0, 6.0));
        // a clearly horizontal swipe switches
        assert_eq!(l.filter(v(12.0, 1.0), 0.064), v(12.0, 0.0));
        // after a pause a new gesture decides again
        let mut l = ScrollLock { axis: Some(true), acc: v(50.0, 0.0), last: 1.0 };
        assert_eq!(l.filter(v(0.0, 9.0), 1.3), v(0.0, 9.0));
    }

    #[test]
    fn formats_like_datagrip() {
        assert_eq!(iso(1738492200000), "2025-02-02T10:30:00.000Z");
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(plain(&Bson::Double(299.0)), "299");
        assert_eq!(plain(&Bson::Double(12.5)), "12.5");
        assert_eq!(trim_num("1225.00".into()), "1225");
        assert_eq!(plain(&Bson::Document(doc! {"city": "Bandung", "zip": "1"})), r#"{"city": "Bandung", "zip": "1"}"#);
        assert_eq!(columns(&[doc! {"b": 1, "_id": 1, "a": 1}, doc! {"c": 1}]), ["_id", "a", "b", "c"]);
        let d = doc! {"a": 1, "b": "x,y"};
        assert_eq!(extract(Extractor::Csv, &["a".into(), "b".into()], &[&d], None), "a,b\n1,\"x,y\"");
        assert_eq!(edit_text(&Bson::Document(doc! {"k": [1, "s"]})), r#"{"k": [1, "s"]}"#);
    }

    fn inserts(g: &Grid) -> Vec<Vec<(String, String, bool)>> {
        g.pending_edits()
            .into_iter()
            .filter_map(|e| match e {
                Edit::Insert { fields } => Some(fields),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn clone_row_copies_values_but_not_the_key() {
        // SQL: primary key left to the database, pending edits and NULLs copied
        let spec = doc! {"db": "d", "sql": "select", "table": "public.t", "key": ["id"]};
        let cols = [("id", "int4"), ("name", "text"), ("note", "text"), ("ok", "bool")].map(|(c, t)| (c.to_string(), t.to_string()));
        let mut g = Grid::default();
        g.set(ResultSet {
            title: "t".into(),
            docs: vec![doc! {"id": "1", "name": "a", "note": Bson::Null, "ok": true}],
            offset: 0,
            page: 500,
            more: false,
            spec: Some(spec),
            exec_ms: 0,
            fetch_ms: 0,
            cols: cols.to_vec(),
        });
        g.sel = Some((0, 1));
        g.commit_cell(0, "name", "b".into());
        g.clone_row();
        assert_eq!(g.sel, Some((1, 1)), "the clone is the last row, same column");
        let s = |a: &str, b: &str, c| (a.to_string(), b.to_string(), c);
        assert_eq!(inserts(&g), [vec![s("name", "b", true), s("note", "null", false), s("ok", "true", false)]]);
        // a clone of the clone
        g.clone_row();
        assert_eq!(inserts(&g).len(), 2);
        assert_eq!(inserts(&g)[1], inserts(&g)[0]);

        // MongoDB: `_id` gets a new ObjectId, strings stay strings, typed values stay typed
        let mut g = Grid::default();
        let id = mongodb::bson::oid::ObjectId::new();
        let when = mongodb::bson::DateTime::from_millis(0);
        g.set(ResultSet {
            title: "c".into(),
            docs: vec![doc! {"_id": id, "zip": "10002", "at": when, "n": 5_i64}],
            offset: 0,
            page: 100,
            more: false,
            spec: Some(doc! {"db": "shop", "coll": "c", "op": "find"}),
            exec_ms: 0,
            fetch_ms: 0,
            cols: vec![],
        });
        g.sel = Some((0, 0));
        g.clone_row();
        let ins = inserts(&g);
        assert_eq!(
            ins[0],
            [s("at", "ISODate(\"1970-01-01T00:00:00.000Z\")", false), s("n", "NumberLong(5)", false), s("zip", "10002", true)]
        );
    }

    /// `public.products` as the PostgreSQL worker returns it (text values, typed columns, key `sku`).
    fn products() -> Grid {
        let cols =
            [("sku", "varchar"), ("name", "text"), ("price", "numeric"), ("stock", "int4"), ("category", "text"), ("discontinued", "bool")];
        let row = |sku: &str, name: &str, price: &str, stock: &str, cat: &str| {
            doc! {"sku": sku, "name": name, "price": price, "stock": stock, "category": cat, "discontinued": false}
        };
        let mut g = Grid::default();
        g.set(ResultSet {
            title: "datatired.public.products".into(),
            docs: vec![row("KB-01", "Keyboard", "49.90", "120", "peripheral"), row("MS-01", "Mouse", "19.90", "300", "peripheral")],
            offset: 0,
            page: 500,
            more: false,
            spec: Some(doc! {"db": "datatired", "sql": "select", "table": "public.products", "key": ["sku"]}),
            exec_ms: 0,
            fetch_ms: 0,
            cols: cols.map(|(c, t)| (c.to_string(), t.to_string())).to_vec(),
        });
        g
    }

    #[test]
    fn sql_extractors_write_what_datagrip_copies() {
        let mut g = products();
        g.extractor = Extractor::SqlInserts;
        // whole row (row number) -> one statement; observed in DataGrip for `public.products`
        g.select(1, usize::MAX, false);
        assert_eq!(
            g.selection_text(),
            "INSERT INTO public.products (sku, name, price, stock, category, discontinued) VALUES ('MS-01', 'Mouse', 19.90, 300, 'peripheral', false);\n"
        );
        // two rows -> two statements; a single cell -> its value whatever the extractor
        g.select(0, usize::MAX, false);
        g.select(1, usize::MAX, true);
        assert_eq!(g.selection_text().lines().count(), 2);
        g.select(0, 2, false);
        assert_eq!(g.selection_text(), "49.90");
        // a cell block -> only its columns
        g.select(0, 1, false);
        g.select(1, 2, true);
        assert_eq!(
            g.selection_text(),
            "INSERT INTO public.products (name, price) VALUES ('Keyboard', 49.90);\nINSERT INTO public.products (name, price) VALUES ('Mouse', 19.90);\n"
        );
        g.select(0, usize::MAX, false);
        g.extractor = Extractor::SqlUpdates;
        assert_eq!(
            g.selection_text(),
            "UPDATE public.products SET name = 'Keyboard', price = 49.90, stock = 120, category = 'peripheral', discontinued = false WHERE sku = 'KB-01';\n"
        );
        g.extractor = Extractor::WhereClause;
        g.select(0, 0, false);
        g.select(1, 1, true);
        assert_eq!(g.selection_text(), "sku = 'KB-01' AND name = 'Keyboard'\nOR sku = 'MS-01' AND name = 'Mouse'");
        g.select(0, usize::MAX, false);
        g.select(1, usize::MAX, true);
        g.extractor = Extractor::SqlInsertMultirow;
        assert_eq!(
            g.selection_text(),
            "INSERT INTO public.products (sku, name, price, stock, category, discontinued)\nVALUES  ('KB-01', 'Keyboard', 49.90, 120, 'peripheral', false),\n        ('MS-01', 'Mouse', 19.90, 300, 'peripheral', false);"
        );
        // quotes, NULL, values that look like numbers in text columns, no table -> MY_TABLE
        let info = SqlInfo { table: None, key: vec![], numeric: HashSet::from(["n".to_string()]) };
        let d = doc! {"s": "it's", "zip": "10002", "n": "NaN", "x": Bson::Null};
        let cols: Vec<String> = ["s", "zip", "n", "x"].map(String::from).to_vec();
        assert_eq!(
            extract(Extractor::SqlInserts, &cols, &[&d], Some(&info)),
            "INSERT INTO MY_TABLE (s, zip, n, x) VALUES ('it''s', '10002', 'NaN', null);\n"
        );
        assert_eq!(
            extract(Extractor::SqlInsertStatements, &cols, &[&d], Some(&info)),
            "INSERT INTO MY_TABLE (s, zip, n, x) VALUES ('it''s', '10002', 'NaN', NULL);\n"
        );
    }

    #[test]
    fn big_values_keep_table_geometry_small() {
        // one cell laid out a whole 1MB string / 50k-element array per frame: 300MB vertex buffer, wgpu panic
        let docs: Vec<Document> =
            (0..40).map(|i| doc! {"_id": i, "blob": "x".repeat(1_000_000), "list": (0..50_000).collect::<Vec<i32>>()}).collect();
        let mut g = Grid::default();
        g.set(ResultSet { title: "t".into(), docs, offset: 0, page: 40, more: false, spec: None, exec_ms: 0, fetch_ms: 0, cols: vec![] });
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1440.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let r = ui.max_rect();
            g.body(ui, r, Id::new("b"), r.top());
        });
        out.textures_delta.clear();
        let verts: usize = ctx
            .tessellate(std::mem::take(&mut out.shapes), 1.0)
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                _ => 0,
            })
            .sum();
        assert!(verts < 200_000, "{verts} vertices");
    }
}
