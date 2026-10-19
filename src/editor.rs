//! Code editor (console) and read-only code view: syntax highlight, line-number gutter,
//! caret-row highlight and right margin, styled like the Islands Dark editor scheme.

use crate::theme::*;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Align, Color32, Id, Key, Modifiers, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;

pub const LINE_H: f32 = 22.0;
const MARGIN_COLS: f32 = 120.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Lang {
    Js,
    Json,
    Sql,
}

pub struct EditorOut {
    /// Char-index range of the selection (empty = caret only).
    pub cursor: Option<std::ops::Range<usize>>,
    pub line_col: (usize, usize),
    pub has_focus: bool,
}

/// Gutter mark for a line (0-based): e.g. ✓ after a successful run.
#[derive(Clone, Copy, PartialEq)]
pub enum Mark {
    Ok,
    Err,
}

fn fmt(color: Color32) -> TextFormat {
    TextFormat { font_id: mono(13.0), color, line_height: Some(LINE_H), valign: Align::Center, ..Default::default() }
}

/// Tokenizes JS / JSON / SQL into a colored layout job.
pub fn highlight(text: &str, lang: Lang) -> LayoutJob {
    if lang == Lang::Sql {
        return highlight_sql(text);
    }
    let mut job = LayoutJob::default();
    let b = text.as_bytes();
    let mut i = 0;
    let mut first_ident_of_stmt = true;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let color;
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            color = COMMENT;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            color = COMMENT;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c && !(c != b'`' && b[i] == b'\n') {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            // JSON / quoted keys: string followed by ':'
            let key = text[i..].trim_start_matches([' ', '\t']).starts_with(':');
            color = if key { FIELD } else { STRING };
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit) && lang == Lang::Json) {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            color = NUMBER;
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let w = &text[start..i];
            let prev = text[..start].trim_end().chars().last();
            let is_key = text[i..].trim_start_matches([' ', '\t']).starts_with(':') && matches!(prev, Some('{') | Some(','));
            color = match w {
                "var" | "let" | "const" | "function" | "return" | "if" | "else" | "for" | "while" | "do" | "new" | "true" | "false"
                | "null" | "undefined" | "this" | "typeof" | "instanceof" | "in" | "of" | "class" | "break" | "continue" | "try"
                | "catch" | "finally" | "throw" | "switch" | "case" | "default" | "async" | "await" => KEYWORD,
                _ if is_key && !w.starts_with('$') => FIELD,
                "db" if prev != Some('.') && first_ident_of_stmt => FUNC,
                _ => EDITOR_TEXT,
            };
            first_ident_of_stmt = false;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            if c == b'\n' || c == b';' {
                first_ident_of_stmt = true;
            }
            color = EDITOR_TEXT;
        }
        job.append(&text[start..i], 0.0, fmt(color));
    }
    if text.is_empty() {
        job.append("", 0.0, fmt(EDITOR_TEXT));
    }
    job
}

/// Key words colored (and completed) in SQL consoles.
const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "in",
    "is",
    "null",
    "like",
    "ilike",
    "between",
    "as",
    "on",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "natural",
    "using",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "distinct",
    "union",
    "all",
    "intersect",
    "except",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "returning",
    "create",
    "table",
    "view",
    "index",
    "unique",
    "primary",
    "key",
    "foreign",
    "references",
    "check",
    "constraint",
    "default",
    "alter",
    "drop",
    "add",
    "column",
    "rename",
    "to",
    "if",
    "exists",
    "cascade",
    "truncate",
    "begin",
    "commit",
    "rollback",
    "transaction",
    "with",
    "recursive",
    "case",
    "when",
    "then",
    "else",
    "end",
    "asc",
    "desc",
    "nulls",
    "first",
    "last",
    "true",
    "false",
    "explain",
    "analyze",
    "show",
    "grant",
    "revoke",
    "schema",
    "database",
    "sequence",
    "function",
    "returns",
    "language",
    "do",
    "declare",
    "return",
    "over",
    "partition",
    "window",
    "filter",
    "lateral",
    "materialized",
    "vacuum",
    "copy",
    "cast",
    "interval",
    "any",
    "some",
    "conflict",
    "nothing",
    "only",
    "for",
    "fetch",
    "next",
    "rows",
];

/// SQL: key words, '…' / $$…$$ strings, numbers, `--` / `/* */` comments.
fn highlight_sql(text: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let color;
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            color = COMMENT;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = text[i + 2..].find("*/").map_or(b.len(), |k| i + 2 + k + 2);
            color = COMMENT;
        } else if c == b'\'' || c == b'"' {
            i += 1;
            while i < b.len() && !(b[i] == c && b.get(i + 1) != Some(&c)) {
                i += if b[i] == c { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            color = if c == b'"' { EDITOR_TEXT } else { STRING };
        } else if c == b'$'
            && text[i + 1..].find('$').is_some_and(|k| text[i + 1..i + 1 + k].chars().all(|x| x.is_alphanumeric() || x == '_'))
        {
            let tag_len = text[i + 1..].find('$').unwrap() + 2;
            let tag = &text[i..i + tag_len];
            if tag.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
                i += tag_len - 1; // `$1` parameter
                color = EDITOR_TEXT;
            } else {
                i = text[i + tag_len..].find(tag).map_or(b.len(), |k| i + tag_len + k + tag_len);
                color = STRING;
            }
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            color = NUMBER;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            color = if SQL_KEYWORDS.contains(&text[start..i].to_lowercase().as_str()) { KEYWORD } else { EDITOR_TEXT };
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            color = EDITOR_TEXT;
        }
        job.append(&text[start..i], 0.0, fmt(color));
    }
    if text.is_empty() {
        job.append("", 0.0, fmt(EDITOR_TEXT));
    }
    job
}

fn gutter_width(lines: usize) -> f32 {
    let digits = lines.max(1).to_string().len().max(2) as f32;
    digits * 7.8 + 34.0
}

fn char_w(ui: &Ui) -> f32 {
    ui.fonts_mut(|f| f.glyph_width(&mono(13.0), 'm'))
}

/// Paints line numbers + marks for a galley placed at `galley_pos`.
fn paint_gutter(ui: &Ui, gutter: Rect, galley: &egui::Galley, galley_pos: egui::Pos2, caret_line: Option<usize>, marks: &[(usize, Mark)]) {
    let p = ui.painter();
    let mut line = 0usize;
    let mut new_line = true;
    for row in &galley.rows {
        let y = galley_pos.y + row.rect().center().y;
        if new_line && y > gutter.top() - LINE_H && y < gutter.bottom() + LINE_H {
            let col = if Some(line) == caret_line { LINE_NO_CARET } else { LINE_NO };
            p.text(
                pos2(gutter.left() + 4.0 + (gutter.width() - 34.0), y),
                egui::Align2::RIGHT_CENTER,
                (line + 1).to_string(),
                mono(12.5),
                col,
            );
            if let Some((_, m)) = marks.iter().find(|(l, _)| *l == line) {
                let c = pos2(gutter.right() - 18.0, y);
                match m {
                    Mark::Ok => {
                        p.line(vec![c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0), c + vec2(4.0, -3.0)], Stroke::new(1.5, SUCCESS));
                    }
                    Mark::Err => {
                        p.circle_filled(c, 3.5, ERROR_OUT);
                    }
                }
            }
        }
        new_line = row.ends_with_newline;
        if new_line {
            line += 1;
        }
    }
}

fn line_col(text: &str, char_idx: usize) -> (usize, usize) {
    let mut l = 0;
    let mut c = 0;
    for ch in text.chars().take(char_idx) {
        if ch == '\n' {
            l += 1;
            c = 0;
        } else {
            c += 1;
        }
    }
    (l, c)
}

#[derive(Clone)]
pub struct Completion {
    pub label: String,
    pub insert: String,
    pub kind: &'static str,
    /// Caret moves back this many chars after insert (inside `()`).
    pub caret_back: usize,
}

#[derive(Clone, Default)]
struct CompState {
    open: bool,
    items: Vec<Completion>,
    sel: usize,
    prefix: usize,
}

/// Given the text before the caret returns (prefix chars to replace, candidates).
pub type Completer<'a> = &'a dyn Fn(&str) -> (usize, Vec<Completion>);

fn byte_at(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

/// Editable code area filling `ui`'s available rect.
pub fn code_editor(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    lang: Lang,
    marks: &[(usize, Mark)],
    set_cursor: Option<usize>,
    completer: Option<Completer>,
) -> EditorOut {
    let cid = id.with("completion");
    let mut cs: CompState = ui.data(|d| d.get_temp(cid)).unwrap_or_default();
    let keys = comp_keys(ui, id, &mut cs);
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, ISLAND);
    let lines = text.lines().count() + 1;
    let gw = gutter_width(lines);
    let cw = char_w(ui);
    let mut out = EditorOut { cursor: None, line_col: (0, 0), has_focus: false };
    egui::ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
        let caret_bg = ui.painter().add(Shape::Noop);
        let margin_line = ui.painter().add(Shape::Noop);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let (gutter, _) = ui.allocate_exact_size(vec2(gw, (lines as f32 * LINE_H).max(rect.height())), Sense::hover());
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| ui.fonts_mut(|f| f.layout_job(highlight(buf.as_str(), lang)));
            let mut te = egui::TextEdit::multiline(text)
                .id(id)
                .font(mono(13.0))
                .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 6, right: 0, top: 4, bottom: 0 }))
                .code_editor()
                .lock_focus(true)
                .desired_width(f32::INFINITY)
                .min_size(vec2(rect.width() - gw - 4.0, rect.height()))
                .layouter(&mut layouter);
            let te_out;
            {
                te = te.background_color(Color32::TRANSPARENT);
                te_out = te.show(ui);
            }
            if set_cursor.is_some() {
                ui.memory_mut(|m| m.request_focus(id));
            }
            let has_focus = te_out.response.has_focus();
            if let Some(ci) = set_cursor {
                set_caret(ui, id, ci);
            }
            if let Some(comp) = completer {
                comp_update(ui, id, &mut cs, text, &te_out, keys, comp);
            }
            ui.data_mut(|d| d.insert_temp(cid, cs.clone()));
            let range = te_out.cursor_range.map(|r| {
                let s = r.as_sorted_char_range();
                s.start.0..s.end.0
            });
            let caret = range.as_ref().map(|r| line_col(text, r.end));
            let caret_line = caret.map(|c| c.0);
            if let Some(cl) = caret_line {
                let y = te_out.galley_pos.y
                    + te_out
                        .galley
                        .rows
                        .iter()
                        .scan((0usize, true), |st, r| {
                            let this = st.0;
                            if r.ends_with_newline {
                                st.0 += 1;
                            }
                            Some((this, r.rect().top()))
                        })
                        .find(|(l, _)| *l == cl)
                        .map_or(0.0, |(_, t)| t);
                let bg = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width().max(4000.0), LINE_H));
                ui.painter().set(caret_bg, Shape::rect_filled(bg, 0.0, CARET_ROW));
            }
            let mx = te_out.galley_pos.x + MARGIN_COLS * cw;
            ui.painter().set(margin_line, Shape::vline(mx, ui.clip_rect().y_range(), Stroke::new(1.0, GRID_LINE)));
            paint_gutter(ui, gutter, &te_out.galley, te_out.galley_pos, caret_line, marks);
            out = EditorOut { cursor: range, line_col: caret.unwrap_or((0, 0)), has_focus };
        });
    });
    out
}

/// Single-line mono field with the editor's completion popup (the data editor's `find {}` bar).
pub fn completing_line(ui: &mut Ui, id: Id, text: &mut String, width: f32, completer: Completer) -> egui::Response {
    let cid = id.with("completion");
    let mut cs: CompState = ui.data(|d| d.get_temp(cid)).unwrap_or_default();
    let keys = comp_keys(ui, id, &mut cs);
    let out = egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .font(mono(13.0))
        .text_color(EDITOR_TEXT)
        .desired_width(width)
        .show(ui);
    comp_update(ui, id, &mut cs, text, &out, keys, completer);
    ui.data_mut(|d| d.insert_temp(cid, cs));
    out.response.response
}

pub fn set_caret(ui: &Ui, id: Id, ci: usize) {
    let mut st = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
    st.store(ui.ctx(), id);
}

/// Before the TextEdit: while the popup is open ↑↓ move, ↩/⇥ accept, ⎋ closes; ⌃Space forces it.
/// Returns (accepted candidate, forced).
fn comp_keys(ui: &Ui, id: Id, cs: &mut CompState) -> (Option<Completion>, bool) {
    let mut accept = None;
    let focused = ui.memory(|m| m.has_focus(id));
    if cs.open && focused && !cs.items.is_empty() {
        let n = cs.items.len();
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                cs.sel = (cs.sel + 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                cs.sel = (cs.sel + n - 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                accept = cs.items.get(cs.sel).cloned();
                cs.open = false;
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                cs.open = false;
            }
        });
    }
    (accept, focused && ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Space)))
}

/// After the TextEdit: refreshes candidates while typing, shows the popup under the word being
/// completed and inserts the accepted / clicked candidate.
fn comp_update(
    ui: &Ui,
    id: Id,
    cs: &mut CompState,
    text: &mut String,
    te: &egui::text_edit::TextEditOutput,
    (accept, force): (Option<Completion>, bool),
    completer: Completer,
) {
    let Some(ci) = te.cursor_range.map(|r| r.primary.index.0) else { return };
    let mut pick = accept;
    if pick.is_none() {
        let typed = te.response.changed()
            && ui.input(|i| {
                i.events.iter().any(
                    |e| matches!(e, egui::Event::Text(t) if t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.')),
                )
            });
        if typed || force || (cs.open && te.response.changed()) {
            let before = &text[..byte_at(text, ci)];
            let (prefix, items) = completer(before);
            let open = !items.is_empty() && (prefix > 0 || force || before.ends_with('.'));
            *cs = CompState { open, items, sel: 0, prefix };
        } else if te.response.clicked() || !te.response.has_focus() {
            cs.open = false;
        }
        if cs.open {
            let r = te.galley.pos_from_cursor(CCursor::new(ci.saturating_sub(cs.prefix))).translate(te.galley_pos.to_vec2());
            pick = completion_popup(ui, id, cs, r.left_bottom() + vec2(-4.0, 2.0)).map(|i| cs.items[i].clone());
        }
    }
    if let Some(c) = pick {
        let start = ci.saturating_sub(cs.prefix);
        text.replace_range(byte_at(text, start)..byte_at(text, ci), &c.insert);
        set_caret(ui, id, start + c.insert.chars().count() - c.caret_back);
        ui.memory_mut(|m| m.request_focus(id));
        cs.open = false;
        ui.ctx().request_repaint();
    }
}

fn completion_popup(ui: &Ui, id: Id, cs: &mut CompState, pos: egui::Pos2) -> Option<usize> {
    let mut picked = None;
    egui::Area::new(id.with("completion-popup")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::new()
            .fill(Color32::from_rgb(0x2B, 0x2D, 0x30))
            .stroke(Stroke::new(1.0, POPUP_BORDER))
            .corner_radius(8)
            .inner_margin(4)
            .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(100) })
            .show(ui, |ui| {
                ui.set_width(380.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let h = 22.0 * cs.items.len().min(12) as f32;
                egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).show(ui, |ui| {
                    for (i, c) in cs.items.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                        if i == cs.sel {
                            ui.painter().rect_filled(r, 4.0, Color32::from_rgb(0x2E, 0x43, 0x6E));
                            resp.scroll_to_me(None);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, HOVER);
                        }
                        let mut job = LayoutJob::default();
                        let n = cs.prefix.min(c.label.chars().count());
                        let cut = byte_at(&c.label, n);
                        let tf = |col| TextFormat { font_id: mono(13.0), color: col, ..Default::default() };
                        job.append(&c.label[..cut], 0.0, tf(Color32::from_rgb(0x54, 0x8A, 0xF7)));
                        job.append(&c.label[cut..], 0.0, tf(EDITOR_TEXT));
                        let g = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().galley(pos2(r.left() + 8.0, r.center().y - g.size().y / 2.0), g, EDITOR_TEXT);
                        ui.painter().text(
                            pos2(r.right() - 8.0, r.center().y),
                            egui::Align2::RIGHT_CENTER,
                            c.kind,
                            ui_font(12.0),
                            TEXT_SECONDARY,
                        );
                        if resp.clicked() {
                            picked = Some(i);
                        }
                    }
                });
            });
    });
    picked
}

const DB_METHODS: &[&str] = &[
    "getCollection",
    "getCollectionNames",
    "getCollectionInfos",
    "getSiblingDB",
    "getName",
    "createCollection",
    "createView",
    "runCommand",
    "adminCommand",
    "dropDatabase",
    "stats",
    "version",
    "serverStatus",
    "aggregate",
    "getMongo",
];
const COLL_METHODS: &[&str] = &[
    "find",
    "findOne",
    "aggregate",
    "insertOne",
    "insertMany",
    "updateOne",
    "updateMany",
    "replaceOne",
    "deleteOne",
    "deleteMany",
    "findOneAndUpdate",
    "findOneAndReplace",
    "findOneAndDelete",
    "countDocuments",
    "estimatedDocumentCount",
    "distinct",
    "createIndex",
    "createIndexes",
    "getIndexes",
    "dropIndex",
    "dropIndexes",
    "drop",
    "renameCollection",
    "stats",
    "bulkWrite",
];
const CURSOR_METHODS: &[&str] = &[
    "sort",
    "limit",
    "skip",
    "projection",
    "toArray",
    "forEach",
    "map",
    "count",
    "explain",
    "hasNext",
    "next",
    "pretty",
    "hint",
    "collation",
    "batchSize",
    "itcount",
];
const QUERY_OPS: &[&str] = &[
    "$eq",
    "$ne",
    "$gt",
    "$gte",
    "$lt",
    "$lte",
    "$in",
    "$nin",
    "$and",
    "$or",
    "$nor",
    "$not",
    "$exists",
    "$type",
    "$regex",
    "$expr",
    "$elemMatch",
    "$size",
    "$all",
    "$text",
    "$mod",
];
const UPDATE_OPS: &[&str] =
    &["$set", "$unset", "$inc", "$push", "$pull", "$addToSet", "$pop", "$rename", "$min", "$max", "$mul", "$currentDate", "$setOnInsert"];
const STAGES: &[&str] = &[
    "$match",
    "$group",
    "$project",
    "$sort",
    "$limit",
    "$skip",
    "$lookup",
    "$unwind",
    "$addFields",
    "$set",
    "$count",
    "$facet",
    "$bucket",
    "$sample",
    "$replaceRoot",
    "$out",
    "$merge",
    "$unionWith",
    "$sum",
    "$avg",
    "$first",
    "$last",
    "$push",
];
const GLOBALS: &[(&str, bool)] = &[
    ("db", false),
    ("ObjectId", true),
    ("ISODate", true),
    ("NumberLong", true),
    ("NumberInt", true),
    ("NumberDecimal", true),
    ("UUID", true),
    ("Timestamp", true),
    ("print", true),
    ("printjson", true),
    ("use", false),
    ("show", false),
];

fn methods(names: &[&str], kind: &'static str) -> Vec<Completion> {
    names.iter().map(|m| Completion { label: format!("{m}()"), insert: format!("{m}()"), kind, caret_back: 1 }).collect()
}

/// mongosh-aware completion: collections after `db.`, methods after a collection / cursor,
/// field names and operators inside query objects, globals otherwise.
pub fn mongo_completions(before: &str, colls: &[String], fields: &HashMap<String, Vec<(String, String)>>) -> (usize, Vec<Completion>) {
    let prefix: String =
        before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect::<Vec<_>>().into_iter().rev().collect();
    let rest = &before[..before.len() - prefix.len()];
    let p = prefix.chars().count();
    let mut items: Vec<Completion> = vec![];
    // statement start = after last top-level ';' or blank line
    let stmt = rest.rsplit_once(';').map_or(rest, |(_, s)| s);
    let coll_of = |s: &str| -> Option<String> {
        let at = s.rfind("db.")?;
        let tail = &s[at + 3..];
        if let Some(t) = tail.strip_prefix("getCollection(") {
            return t.trim_start_matches(['"', '\'']).split(['"', '\'']).next().map(str::to_string);
        }
        let name: String = tail.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        (!name.is_empty()).then_some(name)
    };
    if let Some(chain) = rest.strip_suffix('.') {
        let mut depth = 0i32;
        let mut start = chain.len();
        for (i, ch) in chain.char_indices().rev() {
            match ch {
                ')' | ']' | '}' => depth += 1,
                '(' | '[' | '{' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1
                }
                c if depth == 0 && !(c.is_alphanumeric() || c == '_' || c == '$' || c == '.' || c == '"' || c == '\'') => {
                    break;
                }
                _ => {}
            }
            start = i;
        }
        let expr = chain[start..].trim();
        if expr == "db" {
            items.extend(colls.iter().map(|c| Completion { label: c.clone(), insert: c.clone(), kind: "collection", caret_back: 0 }));
            items.extend(methods(DB_METHODS, "Database"));
        } else if expr.starts_with("db.") && expr.ends_with(')') && (expr.contains(".find(") || expr.contains(".aggregate(")) {
            items.extend(methods(CURSOR_METHODS, "Cursor"));
        } else if expr.starts_with("db.")
            && (!expr.ends_with(')') || expr.starts_with("db.getCollection("))
            && expr.matches('(').count() <= 1
        {
            items.extend(methods(COLL_METHODS, "Collection"));
        }
    } else if stmt.matches('{').count() > stmt.matches('}').count() {
        let after = stmt.trim_end().chars().last();
        let key_pos = matches!(after, Some('{') | Some(',')) || prefix.starts_with('$');
        if key_pos {
            let in_update = stmt.contains("update") || stmt.contains("Update");
            let in_agg = stmt.contains("aggregate");
            let ops = if in_agg {
                STAGES
            } else if in_update && stmt.matches('{').count() > 1 && stmt.contains("}, {") {
                UPDATE_OPS
            } else {
                QUERY_OPS
            };
            items.extend(ops.iter().map(|o| Completion { label: o.to_string(), insert: o.to_string(), kind: "operator", caret_back: 0 }));
            if !prefix.starts_with('$')
                && let Some(fs) = coll_of(stmt).and_then(|c| fields.get(&c))
            {
                let mut fi: Vec<Completion> =
                    fs.iter().map(|(f, t)| Completion { label: f.clone(), insert: f.clone(), kind: leak_kind(t), caret_back: 0 }).collect();
                fi.extend(items);
                items = fi;
            }
        }
    } else if p > 0 {
        items.extend(GLOBALS.iter().map(|(g, call)| Completion {
            label: if *call { format!("{g}()") } else { g.to_string() },
            insert: if *call { format!("{g}()") } else { g.to_string() },
            kind: "global",
            caret_back: *call as usize,
        }));
    }
    let lp = prefix.to_lowercase();
    let mut out: Vec<Completion> = items.iter().filter(|c| c.label.to_lowercase().starts_with(&lp)).cloned().collect();
    out.extend(
        items.into_iter().filter(|c| !c.label.to_lowercase().starts_with(&lp) && lp.len() > 1 && c.label.to_lowercase().contains(&lp)),
    );
    out.truncate(60);
    (p, out)
}

/// SQL completion: columns after `table.` / `alias.`; otherwise key words, tables and the columns of
/// the tables the statement mentions (`columns` is keyed by table name).
pub fn sql_completions(before: &str, tables: &[String], columns: &HashMap<String, Vec<(String, String)>>) -> (usize, Vec<Completion>) {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let prefix: String = before.chars().rev().take_while(|c| word(*c)).collect::<Vec<_>>().into_iter().rev().collect();
    let rest = &before[..before.len() - prefix.len()];
    let stmt = rest.rsplit_once(';').map_or(rest, |(_, s)| s);
    let lower = stmt.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !(word(c) || c == '.')).filter(|w| !w.is_empty()).collect();
    // tables after FROM / JOIN, with their aliases
    let mut used: Vec<(String, Option<String>)> = vec![];
    for (i, w) in words.iter().enumerate() {
        if (*w == "from" || *w == "join" || *w == "update" || *w == "into")
            && let Some(t) = words.get(i + 1)
        {
            let t = t.rsplit('.').next().unwrap_or(t).to_string();
            let mut alias = words.get(i + 2).copied();
            if alias == Some("as") {
                alias = words.get(i + 3).copied();
            }
            used.push((t, alias.filter(|a| !SQL_KEYWORDS.contains(a)).map(str::to_string)));
        }
    }
    let cols_of = |t: &str| {
        columns
            .iter()
            .find(|(k, _)| k.to_lowercase() == t)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|(c, _)| Completion { label: c.clone(), insert: c, kind: "column", caret_back: 0 })
    };
    let mut items: Vec<Completion> = vec![];
    if let Some(q) = rest.strip_suffix('.') {
        let q: String = q.chars().rev().take_while(|c| word(*c)).collect::<Vec<_>>().into_iter().rev().collect::<String>().to_lowercase();
        let table = used.iter().find(|(t, a)| a.as_deref() == Some(q.as_str()) || *t == q).map_or(q.clone(), |(t, _)| t.clone());
        items.extend(cols_of(&table));
    } else if !prefix.is_empty() {
        for (t, _) in &used {
            items.extend(cols_of(t));
        }
        items.extend(tables.iter().map(|t| Completion { label: t.clone(), insert: t.clone(), kind: "table", caret_back: 0 }));
        items.extend(SQL_KEYWORDS.iter().map(|k| Completion {
            label: k.to_string(),
            insert: k.to_string(),
            kind: "keyword",
            caret_back: 0,
        }));
    }
    let lp = prefix.to_lowercase();
    let mut out: Vec<Completion> = items.iter().filter(|c| c.label.to_lowercase().starts_with(&lp)).cloned().collect();
    out.extend(
        items.into_iter().filter(|c| !c.label.to_lowercase().starts_with(&lp) && lp.len() > 1 && c.label.to_lowercase().contains(&lp)),
    );
    out.dedup_by(|a, b| a.label == b.label);
    out.truncate(60);
    (prefix.chars().count(), out)
}

/// Completion in the SQL data editor's `WHERE` / `ORDER BY` fields: the table's columns, key words.
pub fn where_completions(before: &str, fields: &[(String, String)]) -> (usize, Vec<Completion>) {
    sql_completions(&format!("select * from t where {before}"), &[], &HashMap::from([("t".to_string(), fields.to_vec())]))
}

/// Completion inside a bare query object (the data editor's `find {}` bar): field names, `$` operators.
pub fn filter_completions(before: &str, fields: &[(String, String)]) -> (usize, Vec<Completion>) {
    mongo_completions(&format!("db.c.find({before}"), &[], &HashMap::from([("c".to_string(), fields.to_vec())]))
}

fn leak_kind(t: &str) -> &'static str {
    match t {
        "ObjectId" => "ObjectId",
        "Boolean" => "Boolean",
        "ISODate" => "ISODate",
        "String" => "String",
        "Int64" => "Int64",
        "Int32" => "Int32",
        "Double" => "Double",
        "Decimal128" => "Decimal128",
        "Object" => "Object",
        "list" => "list",
        _ => "field",
    }
}

/// Read-only highlighted text with gutter (grid Text view).
pub fn code_view(ui: &mut Ui, id: Id, text: &str, lang: Lang) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, ISLAND);
    let lines = text.lines().count();
    let gw = gutter_width(lines);
    let cw = char_w(ui);
    let galley = ui.fonts_mut(|f| f.layout_job(highlight(text, lang)));
    egui::ScrollArea::both().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
        let size = vec2(gw + galley.size().x + 40.0, galley.size().y + 8.0).max(rect.size());
        let (r, _) = ui.allocate_exact_size(size, Sense::click());
        let pos = pos2(r.left() + gw + 6.0, r.top() + 4.0);
        ui.painter().vline(pos.x + MARGIN_COLS * cw, ui.clip_rect().y_range(), Stroke::new(1.0, GRID_LINE));
        // indent guides like the IDE (every 2 spaces of leading indentation)
        paint_gutter(ui, Rect::from_min_size(r.min, vec2(gw, r.height())), &galley, pos, None, &[]);
        ui.painter().galley(pos, galley.clone(), EDITOR_TEXT);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_like_the_shell() {
        let colls = vec!["customers".to_string(), "orders".to_string()];
        let mut fields = HashMap::new();
        fields.insert("customers".to_string(), vec![("age".to_string(), "Int32".to_string()), ("tier".to_string(), "String".to_string())]);
        let labels = |b: &str| mongo_completions(b, &colls, &fields).1.into_iter().map(|c| c.label).collect::<Vec<_>>();
        assert_eq!(labels("db.cu"), ["customers"]);
        assert!(labels("db.").contains(&"orders".to_string()));
        assert_eq!(labels("db.customers.fin"), ["find()", "findOne()", "findOneAndUpdate()", "findOneAndReplace()", "findOneAndDelete()"]);
        assert!(labels("db.customers.find({a: 1}).").contains(&"sort()".to_string()));
        assert_eq!(labels("db.customers.find({ag"), ["age"]);
        assert_eq!(labels("db.customers.find({age: {$g"), ["$gt", "$gte"]);
        assert_eq!(labels("Obj"), ["ObjectId()"]);
        assert_eq!(mongo_completions("db.customers.fin", &colls, &fields).0, 3);
    }

    #[test]
    fn completes_filter_bar_fields() {
        let fields = [("lastName", "String"), ("email", "String"), ("age", "Int32")].map(|(f, t)| (f.to_string(), t.to_string()));
        let labels = |b: &str| filter_completions(b, &fields).1.into_iter().map(|c| c.label).collect::<Vec<_>>();
        assert_eq!(labels("{l"), ["lastName"]);
        assert_eq!(labels("{age: 1, em")[0], "email");
        assert_eq!(labels("{age: {$lt"), ["$lt", "$lte"]);
        assert_eq!(filter_completions("{la", &fields).0, 2);
    }

    #[test]
    fn completes_sql() {
        let tables = vec!["customers".to_string(), "orders".to_string()];
        let mut cols = HashMap::new();
        cols.insert("customers".to_string(), vec![("name".to_string(), "text".to_string()), ("age".to_string(), "int4".to_string())]);
        let labels = |b: &str| sql_completions(b, &tables, &cols).1.into_iter().map(|c| c.label).collect::<Vec<_>>();
        assert_eq!(labels("select * from cu")[0], "customers", "prefix matches first, then substrings");
        assert_eq!(labels("select * from customers c where c."), ["name", "age"]);
        assert_eq!(labels("select * from customers as c where c.a"), ["age"]);
        assert_eq!(labels("select * from customers where na")[0], "name", "columns of the tables in the statement");
        assert_eq!(labels("sel")[0], "select");
        assert_eq!(where_completions("ag", &[("age".into(), "int4".into())]).1[0].label, "age");
    }

    #[test]
    fn highlights_sql() {
        let job = highlight("select 'a''b', \"user\" from t -- x\nwhere $$q$$ = 1", Lang::Sql);
        let color_of = |s: &str| {
            let at = job.text.find(s).unwrap();
            job.sections.iter().find(|x| x.byte_range.contains(&egui::text::ByteIndex(at))).unwrap().format.color
        };
        assert_eq!(color_of("select"), KEYWORD);
        assert_eq!(color_of("'a''b'"), STRING);
        assert_eq!(color_of("\"user\""), EDITOR_TEXT);
        assert_eq!(color_of("-- x"), COMMENT);
        assert_eq!(color_of("where"), KEYWORD);
        assert_eq!(color_of("$$q$$"), STRING);
        assert_eq!(color_of("1"), NUMBER);
    }

    #[test]
    fn highlights_keys_strings_numbers() {
        let job = highlight("db.c.find({tier: \"gold\", $gt: 30})", Lang::Js);
        let color_of = |s: &str| {
            let at = job.text.find(s).unwrap();
            job.sections.iter().find(|x| x.byte_range.contains(&egui::text::ByteIndex(at))).unwrap().format.color
        };
        assert_eq!(color_of("db"), FUNC);
        assert_eq!(color_of("tier"), FIELD);
        assert_eq!(color_of("\"gold\""), STRING);
        assert_eq!(color_of("30"), NUMBER);
        assert_eq!(color_of("$gt"), EDITOR_TEXT);
        assert_eq!(color_of("find"), EDITOR_TEXT);
    }
}
