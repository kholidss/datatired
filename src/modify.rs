//! Modify Table (PostgreSQL), like DataGrip's dialog: the table's objects in a tree, a form for the
//! selected one and a live preview of the script that applies the changes.

use crate::dialog::{combo_chevron, primary_btn};
use crate::editor::{self, Lang};
use crate::icons as ic;
use crate::pg::{IdxCol, Rel, ident, lit, qualify, split_qual};
use crate::theme::*;
use crate::widgets::*;
use egui::{Align2, Color32, CornerRadius, Id, ImageSource, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub name: String,
    pub comment: String,
    /// `p` permanent, `u` unlogged, `t` temporary.
    pub persistence: char,
    pub owner: String,
    pub tablespace: String,
    pub method: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Col {
    /// Position in the original columns; `None` = new.
    pub orig: Option<usize>,
    pub name: String,
    pub comment: String,
    pub ty: String,
    pub not_null: bool,
    pub default: String,
    /// Expression of a stored generated column.
    pub generated: String,
    pub identity: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Con {
    pub orig: Option<usize>,
    /// `p` primary key, `u` unique, `f` foreign key, `c` check.
    pub kind: char,
    pub name: String,
    /// The name follows DataGrip's naming template until it is edited.
    pub auto: bool,
    pub comment: String,
    pub cols: Vec<String>,
    /// Foreign key target (qualified) and its columns.
    pub target: String,
    pub ref_cols: Vec<String>,
    pub on_delete: char,
    pub on_update: char,
    pub deferrable: bool,
    pub deferred: bool,
    pub predicate: String,
    pub no_inherit: bool,
    pub not_valid: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Idx {
    pub orig: Option<usize>,
    pub name: String,
    pub auto: bool,
    pub comment: String,
    pub unique: bool,
    pub cols: Vec<IdxCol>,
    pub pred: String,
    pub method: String,
}

/// The table as edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub table: Table,
    pub cols: Vec<Col>,
    pub cons: Vec<Con>,
    pub idxs: Vec<Idx>,
}

/// `CHECK ((qty > 0)) NOT VALID` → `qty > 0`.
fn predicate(def: &str) -> String {
    let mut e = def.strip_prefix("CHECK ").unwrap_or(def).trim();
    for suffix in [" NOT VALID", " NO INHERIT"] {
        e = e.strip_suffix(suffix).unwrap_or(e).trim();
    }
    for _ in 0..2 {
        if crate::pg::wrapped(e) {
            e = &e[1..e.len() - 1];
        }
    }
    e.to_string()
}

impl Model {
    pub fn of(r: &Rel) -> Model {
        let s = |o: &Option<String>| o.clone().unwrap_or_default();
        Model {
            table: Table {
                name: r.name.clone(),
                comment: s(&r.comment),
                persistence: r.persistence,
                owner: r.owner.clone(),
                tablespace: r.tablespace.clone(),
                method: r.access_method.clone(),
            },
            cols: r
                .cols
                .iter()
                .enumerate()
                .map(|(i, c)| Col {
                    orig: Some(i),
                    name: c.name.clone(),
                    comment: s(&c.comment),
                    ty: c.ty.clone(),
                    not_null: c.not_null,
                    default: if c.generated { String::new() } else { s(&c.default) },
                    generated: if c.generated { s(&c.default) } else { String::new() },
                    identity: c.identity.is_some(),
                })
                .collect(),
            cons: r
                .cons
                .iter()
                .enumerate()
                .map(|(i, c)| Con {
                    orig: Some(i),
                    kind: c.kind,
                    name: c.name.clone(),
                    auto: false,
                    comment: s(&c.comment),
                    cols: c.cols.clone(),
                    target: if c.kind == 'f' { qualify(&c.ref_schema, &c.ref_table) } else { String::new() },
                    ref_cols: c.ref_cols.clone(),
                    on_delete: c.on_delete,
                    on_update: c.on_update,
                    deferrable: c.deferrable,
                    deferred: c.deferred,
                    predicate: if c.kind == 'c' { predicate(&c.def) } else { String::new() },
                    no_inherit: c.no_inherit,
                    not_valid: !c.validated,
                })
                .collect(),
            idxs: r
                .indexes
                .iter()
                .enumerate()
                .map(|(i, x)| Idx {
                    orig: Some(i),
                    name: x.name.clone(),
                    auto: false,
                    comment: s(&x.comment),
                    unique: x.unique,
                    cols: x.cols.clone(),
                    pred: s(&x.pred),
                    method: x.method.clone(),
                })
                .collect(),
        }
    }
}

const ACTIONS: [(char, &str, &str); 5] =
    [('a', "no_action", ""), ('r', "restrict", "restrict"), ('n', "set_null", "set null"), ('d', "set_default", "set default"), ('c', "cascade", "cascade")];

/// The table name in statements: bare in `public` (the current schema), else qualified.
fn table_ref(schema: &str, name: &str) -> String {
    if schema == "public" { ident(name) } else { qualify(schema, name) }
}

/// A foreign key target as written: bare when it is in the table's schema.
fn target_ref(target: &str, schema: &str) -> String {
    let (s, n) = split_qual(target);
    if s == schema || s.is_empty() { ident(&n) } else { target.to_string() }
}

fn idents(v: &[String]) -> String {
    v.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", ")
}

/// `alter table t\n    add constraint …` of a key, foreign key or check.
fn add_constraint(t: &str, c: &Con, r: &Rel) -> String {
    let mut body = match c.kind {
        'p' => format!("primary key ({})", idents(&c.cols)),
        'u' => format!("unique ({})", idents(&c.cols)),
        'f' => {
            let pk = r.tables.iter().find(|x| x.0 == c.target).map(|x| &x.2);
            let target = if c.target.is_empty() { "???".to_string() } else { target_ref(&c.target, &r.schema) };
            let mut s = format!("foreign key ({}) references {target}", idents(&c.cols));
            if pk != Some(&c.ref_cols) {
                s += &format!(" ({})", idents(&c.ref_cols));
            }
            s
        }
        _ => format!("check ({})", c.predicate),
    };
    if c.deferrable {
        body += " deferrable";
    }
    if c.deferred {
        body += " initially deferred";
    }
    if c.kind == 'c' && c.no_inherit {
        body += " no inherit";
    }
    if matches!(c.kind, 'c' | 'f') && c.not_valid {
        body += " not valid";
    }
    let mut s = format!("alter table {t}\n    add constraint {}\n        {body}", ident(&c.name));
    if c.kind == 'f' {
        for (when, a) in [("update", c.on_update), ("delete", c.on_delete)] {
            if let Some((_, _, act)) = ACTIONS.iter().find(|x| x.0 == a).filter(|x| !x.2.is_empty()) {
                s += &format!("\n            on {when} {act}");
            }
        }
    }
    s + ";"
}

/// Two constraints define the same thing (names and comments aside).
fn same_def(a: &Con, b: &Con) -> bool {
    let key = |c: &Con| {
        (c.kind, c.cols.clone(), c.target.clone(), c.ref_cols.clone(), c.on_delete, c.on_update, c.deferrable, c.deferred)
    };
    key(a) == key(b) && (a.predicate.clone(), a.no_inherit, a.not_valid) == (b.predicate.clone(), b.no_inherit, b.not_valid)
}

fn index_of(i: &Idx) -> crate::pg::RelIdx {
    crate::pg::RelIdx {
        name: i.name.clone(),
        unique: i.unique,
        method: i.method.clone(),
        cols: i.cols.clone(),
        pred: Some(i.pred.clone()).filter(|p| !p.trim().is_empty()),
        ..Default::default()
    }
}

/// The script that turns `o` into `m`, statement order as DataGrip's: columns, the table, new
/// columns, indexes, keys, foreign keys, checks.
pub fn script(o: &Model, m: &Model, r: &Rel) -> String {
    let t0 = table_ref(&r.schema, &o.table.name);
    let t1 = table_ref(&r.schema, &m.table.name);
    let alter = |t: &str, action: String| format!("alter table {t}\n    {action};");
    let comment = |c: &str| if c.is_empty() { "null".to_string() } else { lit(c) };
    let mut out: Vec<String> = vec![];
    // constraints and indexes follow column renames; those on dropped columns go with them
    let ren: HashMap<&str, &str> = m.cols.iter().filter_map(|c| c.orig.map(|i| (o.cols[i].name.as_str(), c.name.as_str()))).collect();
    let mapped = |v: &[String]| v.iter().map(|c| ren.get(c.as_str()).map_or(c.clone(), |n| n.to_string())).collect::<Vec<_>>();
    let dropped: Vec<&str> =
        o.cols.iter().enumerate().filter(|(i, _)| !m.cols.iter().any(|c| c.orig == Some(*i))).map(|(_, c)| c.name.as_str()).collect();
    let gone = |v: &[String]| v.iter().any(|c| dropped.contains(&c.as_str()));

    for (i, oc) in o.cols.iter().enumerate() {
        let Some(c) = m.cols.iter().find(|c| c.orig == Some(i)) else {
            out.push(alter(&t0, format!("drop column {}", ident(&oc.name))));
            continue;
        };
        let n = ident(&c.name);
        if c.name != oc.name {
            out.push(alter(&t0, format!("rename column {} to {n}", ident(&oc.name))));
        }
        if c.comment != oc.comment {
            out.push(format!("comment on column {t0}.{n} is {};", comment(&c.comment)));
        }
        if c.ty != oc.ty {
            out.push(alter(&t0, format!("alter column {n} type {} using {n}::{}", c.ty, c.ty)));
        }
        if c.not_null != oc.not_null {
            out.push(alter(&t0, format!("alter column {n} {} not null", if c.not_null { "set" } else { "drop" })));
        }
        if c.default != oc.default {
            out.push(alter(
                &t0,
                if c.default.trim().is_empty() { format!("alter column {n} drop default") } else { format!("alter column {n} set default {}", c.default) },
            ));
        }
        if c.identity != oc.identity {
            out.push(alter(
                &t0,
                if c.identity { format!("alter column {n} add generated always as identity") } else { format!("alter column {n} drop identity") },
            ));
        }
        if c.generated.is_empty() && !oc.generated.is_empty() {
            out.push(alter(&t0, format!("alter column {n} drop expression")));
        }
    }

    let (ot, mt) = (&o.table, &m.table);
    if mt.name != ot.name {
        out.push(alter(&t0, format!("rename to {}", ident(&mt.name))));
    }
    if mt.comment != ot.comment {
        out.push(format!("comment on table {t1} is {};", comment(&mt.comment)));
    }
    if mt.persistence != ot.persistence && mt.persistence != 't' {
        out.push(alter(&t1, if mt.persistence == 'u' { "set unlogged".into() } else { "set logged".into() }));
    }
    if mt.owner != ot.owner && !mt.owner.is_empty() {
        out.push(alter(&t1, format!("owner to {}", ident(&mt.owner))));
    }
    if mt.tablespace != ot.tablespace {
        let ts = if mt.tablespace.is_empty() { "pg_default".to_string() } else { ident(&mt.tablespace) };
        out.push(alter(&t1, format!("set tablespace {ts}")));
    }
    if mt.method != ot.method && !mt.method.is_empty() {
        out.push(alter(&t1, format!("set access method {}", ident(&mt.method))));
    }

    for c in m.cols.iter().filter(|c| c.orig.is_none()) {
        let mut a = format!("add {} {}", ident(&c.name), c.ty);
        if !c.default.trim().is_empty() {
            a += &format!(" default {}", c.default);
        }
        if !c.generated.trim().is_empty() {
            a += &format!(" generated always as ({}) stored", c.generated);
        }
        if c.identity {
            a += " generated always as identity";
        } else if c.not_null {
            a += " not null";
        }
        out.push(alter(&t1, a));
        if !c.comment.is_empty() {
            out.push(format!("comment on column {t1}.{} is {};", ident(&c.name), lit(&c.comment)));
        }
    }

    let unit = "    ";
    for (i, oi) in o.idxs.iter().enumerate() {
        let exprs: Vec<String> = oi.cols.iter().map(|c| c.expr.clone()).collect();
        let Some(x) = m.idxs.iter().find(|x| x.orig == Some(i)) else {
            if !gone(&exprs) && !r.indexes.get(i).is_some_and(|x| x.constraint) {
                out.push(format!("drop index {};", ident(&oi.name)));
            }
            continue;
        };
        let mut was = oi.clone();
        for (c, e) in was.cols.iter_mut().zip(mapped(&exprs)) {
            c.expr = e;
        }
        let def = |i: &Idx| (i.unique, i.cols.clone(), i.pred.trim().to_string(), i.method.clone());
        if def(&was) != def(x) {
            out.push(format!("drop index {};", ident(&oi.name)));
            out.push(crate::pg::create_index(&index_of(x), &t1, false, unit));
        } else if x.name != oi.name {
            out.push(format!("alter index {} rename to {};", ident(&oi.name), ident(&x.name)));
        }
        if x.comment != oi.comment {
            out.push(format!("comment on index {} is {};", ident(&x.name), comment(&x.comment)));
        }
    }
    for x in m.idxs.iter().filter(|x| x.orig.is_none()) {
        out.push(crate::pg::create_index(&index_of(x), &t1, false, unit));
        if !x.comment.is_empty() {
            out.push(format!("comment on index {} is {};", ident(&x.name), lit(&x.comment)));
        }
    }

    for kinds in [&['p', 'u'][..], &['f'], &['c']] {
        for (i, oc) in o.cons.iter().enumerate().filter(|(_, c)| kinds.contains(&c.kind)) {
            let Some(c) = m.cons.iter().find(|c| c.orig == Some(i)) else {
                if !gone(&oc.cols) {
                    out.push(alter(&t1, format!("drop constraint {}", ident(&oc.name))));
                }
                continue;
            };
            let was = Con { cols: mapped(&oc.cols), ..oc.clone() };
            if !same_def(&was, c) {
                out.push(alter(&t1, format!("drop constraint {}", ident(&oc.name))));
                out.push(add_constraint(&t1, c, r));
            } else if c.name != oc.name {
                out.push(alter(&t1, format!("rename constraint {} to {}", ident(&oc.name), ident(&c.name))));
            }
            if c.comment != oc.comment {
                out.push(format!("comment on constraint {} on {t1} is {};", ident(&c.name), comment(&c.comment)));
            }
        }
        for c in m.cons.iter().filter(|c| c.orig.is_none() && kinds.contains(&c.kind)) {
            out.push(add_constraint(&t1, c, r));
            if !c.comment.is_empty() {
                out.push(format!("comment on constraint {} on {t1} is {};", ident(&c.name), lit(&c.comment)));
            }
        }
    }
    out.join("\n\n")
}

// ------------------------------------------------------------------ dialog

#[derive(Clone, Copy, PartialEq, Debug)]
enum Folder {
    Columns,
    Keys,
    Fks,
    Indexes,
    Checks,
}

impl Folder {
    const ALL: [Folder; 5] = [Folder::Columns, Folder::Keys, Folder::Fks, Folder::Indexes, Folder::Checks];

    fn label(self) -> &'static str {
        match self {
            Folder::Columns => "columns",
            Folder::Keys => "keys",
            Folder::Fks => "foreign keys",
            Folder::Indexes => "indexes",
            Folder::Checks => "checks",
        }
    }

    fn of_con(kind: char) -> Folder {
        match kind {
            'f' => Folder::Fks,
            'c' => Folder::Checks,
            _ => Folder::Keys,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Node {
    Table,
    Folder(Folder),
    Col(usize),
    Con(usize),
    Idx(usize),
}

pub enum ModifyOut {
    None,
    Close,
    /// Run the script (OK).
    Run(String),
    /// Preview › Open in console.
    Console(String),
}

pub struct ModifyDlg {
    pub ds: u64,
    pub db: String,
    pub coll: String,
    /// `datatired.public [datatired-pg@localhost]` beside the table node.
    location: String,
    rel: Option<Rel>,
    orig: Model,
    cur: Model,
    sel: Node,
    back: Vec<Node>,
    fwd: Vec<Node>,
    folded: Vec<Folder>,
    /// Selected entry of a key / index / foreign key column list.
    part: usize,
    preview: bool,
    pub error: Option<String>,
    pub running: bool,
    ask_cancel: bool,
}

/// Common PostgreSQL types for the Data Type drop-down.
const TYPES: [&str; 22] = [
    "bigint",
    "bigserial",
    "boolean",
    "bytea",
    "char",
    "date",
    "double precision",
    "integer",
    "interval",
    "json",
    "jsonb",
    "numeric",
    "real",
    "serial",
    "smallint",
    "text",
    "text[]",
    "time",
    "timestamp",
    "timestamp with time zone",
    "uuid",
    "varchar",
];

const LABEL_W: f32 = 135.0;

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}...{}", s.chars().take(n - 2).collect::<String>(), if s.ends_with(')') { ")" } else { "" }) }
}

/// `(address ->> 'city'::text) asc, age desc`
fn idx_cols_text(cols: &[IdxCol]) -> String {
    let desc = cols.iter().any(|c| c.desc);
    cols.iter().map(|c| format!("{}{}", c.expr, if !desc { "" } else if c.desc { " desc" } else { " asc" })).collect::<Vec<_>>().join(", ")
}

fn name_part(v: &[String]) -> String {
    v.iter().map(|c| c.chars().filter(|ch| ch.is_alphanumeric() || *ch == '_').collect::<String>()).collect::<Vec<_>>().join("_")
}

impl ModifyDlg {
    pub fn new(ds: u64, db: String, coll: String, ds_name: &str) -> Self {
        let (schema, _) = split_qual(&coll);
        ModifyDlg {
            location: format!("{db}.{schema} [{ds_name}]"),
            ds,
            db,
            coll,
            rel: None,
            orig: Model::default(),
            cur: Model::default(),
            sel: Node::Table,
            back: vec![],
            fwd: vec![],
            folded: vec![],
            part: 0,
            preview: true,
            error: None,
            running: false,
            ask_cancel: false,
        }
    }

    pub fn loaded(&mut self, r: Rel) {
        self.orig = Model::of(&r);
        self.cur = self.orig.clone();
        self.rel = Some(r);
    }

    pub fn failed(&mut self, e: String) {
        self.error = Some(e);
        self.running = false;
    }

    pub fn script(&self) -> String {
        self.rel.as_ref().map_or(String::new(), |r| script(&self.orig, &self.cur, r))
    }

    fn go(&mut self, n: Node) {
        if n != self.sel {
            self.back.push(self.sel);
            self.fwd.clear();
            self.sel = n;
            self.part = 0;
        }
    }

    /// DataGrip's naming templates for new keys, foreign keys and indexes.
    fn auto_names(&mut self) {
        let t = self.cur.table.name.clone();
        for c in self.cur.cons.iter_mut().filter(|c| c.auto) {
            c.name = match c.kind {
                'f' => format!("{t}_{}_{}_fk", split_qual(&c.target).1, name_part(&c.ref_cols)),
                'c' => c.name.clone(),
                _ if c.cols.is_empty() || c.kind == 'p' => format!("{t}_pk"),
                _ => format!("{t}_{}_pk", name_part(&c.cols)),
            };
        }
        for x in self.cur.idxs.iter_mut().filter(|x| x.auto) {
            let cols: Vec<String> = x.cols.iter().map(|c| c.expr.clone()).collect();
            x.name = format!("{t}_{}_{}index", name_part(&cols), if x.unique { "u" } else { "" });
        }
    }

    fn add(&mut self, f: Folder) {
        let node = match f {
            Folder::Columns => {
                self.cur.cols.push(Col { name: "column_name".into(), ty: "integer".into(), ..Col::default() });
                Node::Col(self.cur.cols.len() - 1)
            }
            Folder::Indexes => {
                self.cur.idxs.push(Idx { auto: true, method: String::new(), ..Idx::default() });
                Node::Idx(self.cur.idxs.len() - 1)
            }
            _ => {
                let kind = match f {
                    Folder::Fks => 'f',
                    Folder::Checks => 'c',
                    _ => 'u',
                };
                let name = if kind == 'c' { "check_name".to_string() } else { String::new() };
                self.cur.cons.push(Con { kind, name, auto: kind != 'c', on_delete: 'a', on_update: 'a', ..Con::default() });
                Node::Con(self.cur.cons.len() - 1)
            }
        };
        self.folded.retain(|x| *x != f);
        self.go(node);
    }

    fn folder_of(&self, n: Node) -> Folder {
        match n {
            Node::Table => Folder::Columns,
            Node::Folder(f) => f,
            Node::Col(_) => Folder::Columns,
            Node::Con(i) => Folder::of_con(self.cur.cons[i].kind),
            Node::Idx(_) => Folder::Indexes,
        }
    }

    fn remove(&mut self) {
        let next = match self.sel {
            Node::Col(i) => {
                self.cur.cols.remove(i);
                if i < self.cur.cols.len() { Node::Col(i) } else { Node::Folder(Folder::Columns) }
            }
            Node::Con(i) => {
                let f = Folder::of_con(self.cur.cons[i].kind);
                self.cur.cons.remove(i);
                (i..self.cur.cons.len()).find(|j| Folder::of_con(self.cur.cons[*j].kind) == f).map_or(Node::Folder(f), Node::Con)
            }
            Node::Idx(i) => {
                self.cur.idxs.remove(i);
                if i < self.cur.idxs.len() { Node::Idx(i) } else { Node::Folder(Folder::Indexes) }
            }
            n => n,
        };
        self.sel = next;
        self.back.clear();
        self.fwd.clear();
    }

    fn duplicate(&mut self) {
        match self.sel {
            Node::Col(i) => {
                let c = Col { orig: None, name: format!("{}1", self.cur.cols[i].name), ..self.cur.cols[i].clone() };
                self.cur.cols.push(c);
                self.go(Node::Col(self.cur.cols.len() - 1));
            }
            Node::Con(i) => {
                let c = Con { orig: None, name: format!("{}1", self.cur.cons[i].name), auto: false, ..self.cur.cons[i].clone() };
                self.cur.cons.push(c);
                self.go(Node::Con(self.cur.cons.len() - 1));
            }
            Node::Idx(i) => {
                let x = Idx { orig: None, name: format!("{}1", self.cur.idxs[i].name), auto: false, ..self.cur.idxs[i].clone() };
                self.cur.idxs.push(x);
                self.go(Node::Idx(self.cur.idxs.len() - 1));
            }
            _ => {}
        }
    }

    /// New columns can be reordered among the new ones (existing columns keep their place).
    fn movable(&self, up: bool) -> bool {
        let Node::Col(i) = self.sel else { return false };
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        self.cur.cols[i].orig.is_none() && j.and_then(|j| self.cur.cols.get(j)).is_some_and(|c| c.orig.is_none())
    }

    fn shift(&mut self, up: bool) {
        if let Node::Col(i) = self.sel
            && self.movable(up)
        {
            let j = if up { i - 1 } else { i + 1 };
            self.cur.cols.swap(i, j);
            self.sel = Node::Col(j);
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> ModifyOut {
        let mut out = ModifyOut::None;
        let modal = egui::Modal::new(Id::new("modify-table")).backdrop_color(Color32::from_black_alpha(40)).frame(
            egui::Frame::new().fill(ISLAND).stroke(Stroke::new(1.0, POPUP_BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(0),
        );
        let resp = modal.show(ctx, |ui| {
            let size = vec2(820.0, 800.0).min(ctx.content_rect().size() - vec2(40.0, 40.0));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            text_at(ui, pos2(rect.left() + 82.0, rect.top() + 16.0), Align2::LEFT_CENTER, "Modify", bold_font(13.0), TEXT);
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 56.0), rect.max);
            let body = Rect::from_min_max(rect.min + vec2(14.0, 34.0), pos2(rect.right() - 14.0, foot.top()));
            if self.rel.is_none() {
                match &self.error {
                    Some(e) => {
                        text_at(ui, body.center(), Align2::CENTER_CENTER, e, ui_font(13.0), ERROR_OUT);
                    }
                    None => ij_spinner(ui, body.center(), 12.0),
                }
            } else {
                self.auto_names();
                let top_h = if self.preview { (body.height() * 0.56).max(260.0) } else { body.height() - 30.0 };
                let top = Rect::from_min_size(body.min, vec2(body.width(), top_h));
                let tree = Rect::from_min_size(top.min, vec2(355.0_f32.min(top.width() * 0.45), top.height()));
                ui.painter().rect_stroke(tree, CornerRadius::same(4), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
                self.tree(ui, tree);
                let form = Rect::from_min_max(pos2(tree.right() + 10.0, top.top()), top.max);
                self.form(ui, form);
                let ph = Rect::from_min_size(pos2(body.left(), top.bottom() + 6.0), vec2(body.width(), 26.0));
                self.preview_area(ui, ph, body.bottom(), &mut out);
            }
            // footer
            let help = Rect::from_center_size(pos2(foot.left() + 26.0, foot.center().y), vec2(24.0, 24.0));
            ui.painter().circle_stroke(help.center(), 11.0, Stroke::new(1.0, CONTROL_BORDER));
            text_at(ui, help.center(), Align2::CENTER_CENTER, "?", ui_font(13.0), TEXT);
            if let (Some(e), true) = (&self.error, self.rel.is_some()) {
                let r = Rect::from_min_max(pos2(foot.left() + 50.0, foot.top()), pos2(foot.right() - 200.0, foot.bottom()));
                ui.painter().with_clip_rect(r).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, e, ui_font(12.5), ERROR_OUT);
            }
            row_ui(ui, Rect::from_min_max(pos2(foot.right() - 220.0, foot.top() + 12.0), foot.right_bottom() - vec2(14.0, 12.0)), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let ok = ui.add_enabled_ui(!self.running && self.rel.is_some(), |ui| primary_btn(ui, "OK")).inner;
                    if ok.clicked() {
                        let s = self.script();
                        out = if s.is_empty() { ModifyOut::Close } else { ModifyOut::Run(s) };
                    }
                    if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 28.0))).clicked() {
                        self.cancel(&mut out);
                    }
                    if self.running {
                        ij_spinner(ui, pos2(foot.right() - 190.0, foot.center().y), 9.0);
                    }
                });
            });
        });
        if resp.should_close() && matches!(out, ModifyOut::None) && !self.ask_cancel {
            self.cancel(&mut out);
        }
        // shortcuts of the tree
        if self.rel.is_some() && !self.ask_cancel {
            let (dup, back, fwd) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::D),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::OpenBracket),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::CloseBracket),
                )
            });
            if dup {
                self.duplicate();
            }
            if back {
                self.nav(true);
            }
            if fwd {
                self.nav(false);
            }
        }
        if self.ask_cancel {
            self.ask_cancel_dialog(ctx, &mut out);
        }
        out
    }

    fn cancel(&mut self, out: &mut ModifyOut) {
        if self.cur != self.orig {
            self.ask_cancel = true;
        } else {
            *out = ModifyOut::Close;
        }
    }

    fn ask_cancel_dialog(&mut self, ctx: &egui::Context, out: &mut ModifyOut) {
        let modal = egui::Modal::new(Id::new("modify-lose-changes")).frame(
            egui::Frame::new().fill(POPUP_BG).stroke(Stroke::new(1.0, POPUP_BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(16),
        );
        modal.show(ctx, |ui| {
            ui.set_width(330.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                paint_icon(ui, r, ic::WARNING_DIALOG);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Warning").strong());
                    ui.label("You're about to lose your changes");
                });
            });
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if primary_btn(ui, "OK").clicked() {
                    *out = ModifyOut::Close;
                    self.ask_cancel = false;
                }
                if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 28.0))).clicked() {
                    self.ask_cancel = false;
                }
            });
        });
    }

    fn nav(&mut self, back: bool) {
        let (from, to) = if back { (&mut self.back, &mut self.fwd) } else { (&mut self.fwd, &mut self.back) };
        if let Some(n) = from.pop() {
            to.push(self.sel);
            self.sel = n;
            self.part = 0;
        }
    }

    // -------------------------------------------------------------- tree

    fn tree(&mut self, ui: &mut Ui, rect: Rect) {
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 2.0), vec2(rect.width() - 8.0, 28.0));
        let folder = self.folder_of(self.sel);
        let removable = matches!(self.sel, Node::Col(_) | Node::Con(_) | Node::Idx(_));
        let mut act: Option<&str> = None;
        let mut add_kind = None;
        row_ui(ui, bar, |ui| {
            let add = icon_btn_ex(ui, ic::ADD, "Add", true, false, true);
            if add.clicked() {
                act = Some("add");
            }
            context_menu(&add, |ui| {
                ui.set_min_width(150.0);
                for (f, icon, t) in [
                    (Folder::Columns, ic::COLUMN, "Column"),
                    (Folder::Keys, ic::KEY, "Key"),
                    (Folder::Fks, ic::FOREIGN_KEY, "Foreign Key"),
                    (Folder::Indexes, ic::INDEX, "Index"),
                    (Folder::Checks, ic::CHECK, "Check"),
                ] {
                    if menu_item(ui, Some(icon), t, "", true) {
                        add_kind = Some(f);
                    }
                }
            });
            if icon_btn_ex(ui, ic::REMOVE, "Remove", removable, false, false).clicked() {
                act = Some("remove");
            }
            if icon_btn_ex(ui, ic::COPY, "Duplicate  ⌘D", removable, false, false).clicked() {
                act = Some("dup");
            }
            if icon_btn_ex(ui, ic::MOVE_UP, "Move Up", self.movable(true), false, false).clicked() {
                act = Some("up");
            }
            if icon_btn_ex(ui, ic::MOVE_DOWN, "Move Down", self.movable(false), false, false).clicked() {
                act = Some("down");
            }
            icon_btn_ex(ui, ic::EDIT_ON_SELECTION, "Edit On Selection", true, true, false);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn_ex(ui, ic::FORWARD, "Forward  ⌘]", !self.fwd.is_empty(), false, false).clicked() {
                    act = Some("fwd");
                }
                if icon_btn_ex(ui, ic::BACK, "Back  ⌘[", !self.back.is_empty(), false, false).clicked() {
                    act = Some("back");
                }
            });
        });
        match act {
            Some("add") => self.add(folder),
            Some("remove") => self.remove(),
            Some("dup") => self.duplicate(),
            Some("up") => self.shift(true),
            Some("down") => self.shift(false),
            Some("back") => self.nav(true),
            Some("fwd") => self.nav(false),
            _ => {}
        }
        if let Some(f) = add_kind {
            self.add(f);
        }
        let list = Rect::from_min_max(pos2(rect.left() + 1.0, bar.bottom() + 2.0), rect.max - vec2(1.0, 1.0));
        rect_ui(ui, list, |ui| {
            egui::ScrollArea::vertical().id_salt("modify-tree").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let pk: Vec<String> = self.cur.cons.iter().filter(|c| c.kind == 'p').flat_map(|c| c.cols.clone()).collect();
                let changed = self.cur.table != self.orig.table;
                let label = self.cur.table.name.clone();
                let loc = self.location.clone();
                self.row(ui, 0, Node::Table, Some(true), ic::TABLE, &label, &loc, if changed { LINK } else { TEXT });
                for f in Folder::ALL {
                    let items: Vec<Node> = match f {
                        Folder::Columns => (0..self.cur.cols.len()).map(Node::Col).collect(),
                        Folder::Indexes => (0..self.cur.idxs.len()).map(Node::Idx).collect(),
                        _ => (0..self.cur.cons.len()).filter(|i| Folder::of_con(self.cur.cons[*i].kind) == f).map(Node::Con).collect(),
                    };
                    let open = !items.is_empty() && !self.folded.contains(&f);
                    let count = if items.is_empty() { String::new() } else { items.len().to_string() };
                    if self.row(ui, 1, Node::Folder(f), (!items.is_empty()).then_some(open), ic::FOLDER, f.label(), &count, TEXT) {
                        if open {
                            self.folded.push(f);
                        } else {
                            self.folded.retain(|x| *x != f);
                        }
                    }
                    if !open {
                        continue;
                    }
                    for n in items {
                        let (icon, name, detail, orig_eq, new) = match n {
                            Node::Col(i) => {
                                let c = &self.cur.cols[i];
                                let icon = if pk.contains(&c.name) {
                                    ic::COLUMN_KEY
                                } else if c.not_null {
                                    ic::COLUMN_NOT_NULL
                                } else {
                                    ic::COLUMN
                                };
                                let mut d = c.ty.replace(", ", ",");
                                if !c.default.is_empty() {
                                    d += &format!(" = {}", trunc(&c.default, 20));
                                }
                                let eq = c.orig.is_some_and(|o| self.orig.cols[o] == Col { orig: Some(o), ..c.clone() });
                                (icon, c.name.clone(), d, eq, c.orig.is_none())
                            }
                            Node::Con(i) => {
                                let c = &self.cur.cons[i];
                                let icon = match c.kind {
                                    'p' => ic::KEY_PRIMARY,
                                    'u' => ic::KEY,
                                    'f' => ic::FOREIGN_KEY,
                                    _ => ic::CHECK,
                                };
                                let d = match c.kind {
                                    'f' => String::new(),
                                    'c' => format!("({})", c.predicate),
                                    _ => format!("({})", c.cols.join(", ")),
                                };
                                let eq = c.orig.is_some_and(|o| self.orig.cons[o] == *c);
                                (icon, c.name.clone(), d, eq, c.orig.is_none())
                            }
                            Node::Idx(i) => {
                                let x = &self.cur.idxs[i];
                                let icon = if x.unique { ic::INDEX_UNIQUE } else { ic::INDEX };
                                let mut d = format!("({})", idx_cols_text(&x.cols));
                                if x.unique {
                                    d += " UNIQUE";
                                }
                                let eq = x.orig.is_some_and(|o| self.orig.idxs[o] == *x);
                                (icon, x.name.clone(), d, eq, x.orig.is_none())
                            }
                            _ => continue,
                        };
                        let color = if new {
                            SUCCESS
                        } else if !orig_eq {
                            LINK
                        } else {
                            TEXT
                        };
                        self.row(ui, 2, n, None, icon, &name, &detail, color);
                    }
                }
            });
        });
    }

    /// One tree row; returns true when its chevron was clicked.
    #[allow(clippy::too_many_arguments)]
    fn row(&mut self, ui: &mut Ui, depth: usize, n: Node, open: Option<bool>, icon: ImageSource<'static>, label: &str, detail: &str, color: Color32) -> bool {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, self.sel == n, label));
        if self.sel == n {
            ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), CornerRadius::same(4), SEL_ACTIVE);
        } else if resp.hovered() {
            ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), CornerRadius::same(4), HOVER);
        }
        let x = r.left() + 14.0 + depth as f32 * 19.0;
        let mut toggle = false;
        if let Some(o) = open {
            let cr = Rect::from_min_size(pos2(x - 2.0, r.center().y - 8.0), vec2(16.0, 16.0));
            paint_icon_tinted(ui, cr, if o { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, TEXT_MUTED);
            toggle = ui.interact(cr.expand(3.0), Id::new(("modify-chev", format!("{n:?}"))), Sense::CLICK).clicked();
        }
        let ir = Rect::from_min_size(pos2(x + 18.0, r.center().y - 8.0), vec2(16.0, 16.0));
        paint_icon(ui, ir, icon);
        let t = text_at(ui, pos2(ir.right() + 6.0, r.center().y), Align2::LEFT_CENTER, label, ui_font(13.0), color);
        if !detail.is_empty() {
            ui.painter().with_clip_rect(r).text(pos2(t.right() + 6.0, r.center().y + 0.5), Align2::LEFT_CENTER, detail, ui_font(11.5), TEXT_SECONDARY);
        }
        if resp.clicked() && !toggle {
            self.go(n);
        }
        if resp.double_clicked() && open.is_some() {
            toggle = true;
        }
        toggle
    }

    // -------------------------------------------------------------- forms

    fn form(&mut self, ui: &mut Ui, rect: Rect) {
        let (icon, title) = match self.sel {
            Node::Table | Node::Folder(_) => (ic::TABLE, self.cur.table.name.clone()),
            Node::Col(i) => (ic::COLUMN, self.cur.cols[i].name.clone()),
            Node::Con(i) => (
                match self.cur.cons[i].kind {
                    'p' => ic::KEY_PRIMARY,
                    'f' => ic::FOREIGN_KEY,
                    'c' => ic::CHECK,
                    _ => ic::KEY,
                },
                self.cur.cons[i].name.clone(),
            ),
            Node::Idx(i) => (if self.cur.idxs[i].unique { ic::INDEX_UNIQUE } else { ic::INDEX }, self.cur.idxs[i].name.clone()),
        };
        // the object's pill
        let g = ui.painter().layout_no_wrap(trunc(&title, 28), ui_font(13.0), TEXT);
        let pill = Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(g.size().x + 44.0, 26.0));
        ui.painter().rect(pill, CornerRadius::same(6), TAB_SEL_BG, Stroke::new(1.0, TAB_SEL_BORDER), egui::StrokeKind::Inside);
        paint_icon(ui, Rect::from_min_size(pill.min + vec2(9.0, 5.0), vec2(16.0, 16.0)), icon);
        ui.painter().galley(pos2(pill.left() + 32.0, pill.center().y - g.size().y / 2.0), g, TEXT);
        let body = Rect::from_min_max(pos2(rect.left(), pill.bottom() + 8.0), rect.max);
        rect_ui(ui, body, |ui| {
            egui::ScrollArea::vertical().id_salt(("modify-form", format!("{:?}", self.sel))).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.set_width(body.width() - 14.0);
                match self.sel {
                    Node::Table | Node::Folder(_) => self.table_form(ui),
                    Node::Col(i) => self.col_form(ui, i),
                    Node::Con(i) if self.cur.cons[i].kind == 'f' => self.fk_form(ui, i),
                    Node::Con(i) if self.cur.cons[i].kind == 'c' => self.check_form(ui, i),
                    Node::Con(i) => self.key_form(ui, i),
                    Node::Idx(i) => self.idx_form(ui, i),
                }
            });
        });
    }

    fn table_form(&mut self, ui: &mut Ui) {
        let rel = self.rel.as_ref().expect("loaded");
        let (t, o) = (&mut self.cur.table, &self.orig.table);
        text_row(ui, "Name", &mut t.name, Some(&o.name), true);
        text_row(ui, "Comment", &mut t.comment, Some(&o.comment), true);
        field(ui, "Persistence", t.persistence != o.persistence, |ui, w| {
            let names = [('p', "PERSISTENT"), ('u', "UNLOGGED"), ('t', "TEMPORARY")];
            let cur = names.iter().find(|x| x.0 == t.persistence).map_or("PERSISTENT", |x| x.1);
            ui.add_enabled_ui(o.persistence != 't', |ui| {
                egui::ComboBox::from_id_salt("modify-persistence").icon(combo_chevron).width(w * 0.55).selected_text(cur).show_ui(ui, |ui| {
                    for (c, n) in &names[..2] {
                        ui.selectable_value(&mut t.persistence, *c, *n);
                    }
                });
            });
            ui.add_space(12.0);
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "With OIDs"));
        });
        let mut part = rel.partition_key.clone();
        text_row(ui, "Partition Expression", &mut part, None, false);
        text_row(ui, "Partition Key", &mut String::new(), None, false);
        let mut opts = rel.options.clone();
        text_row(ui, "Options", &mut opts, None, false);
        combo_row(ui, "Access Method", &mut t.method, &rel.table_methods, Some(&o.method), false);
        combo_row(ui, "Tablespace", &mut t.tablespace, &rel.tablespaces, Some(&o.tablespace), true);
        combo_row(ui, "Owner", &mut t.owner, &rel.roles, Some(&o.owner), false);
        ui.label(egui::RichText::new("Grants").color(TEXT));
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 110.0), Sense::hover());
        ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
        row_ui(ui, Rect::from_min_size(r.min + vec2(4.0, 2.0), vec2(120.0, 28.0)), |ui| {
            for (i, tip) in [(ic::ADD, "Add"), (ic::REMOVE, "Remove"), (ic::MOVE_UP, "Move Up"), (ic::MOVE_DOWN, "Move Down")] {
                icon_btn_ex(ui, i, tip, false, false, false);
            }
        });
        text_at(ui, r.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, "Nothing to show", ui_font(13.0), TEXT_SECONDARY);
    }

    fn col_form(&mut self, ui: &mut Ui, i: usize) {
        let o = self.cur.cols[i].orig.map(|o| self.orig.cols[o].clone()).unwrap_or_default();
        let new = self.cur.cols[i].orig.is_none();
        let before = self.cur.cols[i].name.clone();
        let c = &mut self.cur.cols[i];
        text_row(ui, "Name", &mut c.name, (!new).then_some(&o.name), true);
        text_row(ui, "Comment", &mut c.comment, (!new).then_some(&o.comment), true);
        field(ui, "Data Type", !new && c.ty != o.ty, |ui, w| {
            ui.add(egui::TextEdit::singleline(&mut c.ty).font(mono(13.0)).desired_width(w - 34.0).margin(vec2(8.0, 5.0)));
            let b = icon_btn(ui, ic::CHEVRON_DOWN, "Data types");
            egui::Popup::menu(&b).show(|ui| {
                ui.set_min_width(200.0);
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for t in TYPES {
                        if ui.selectable_label(c.ty == t, t).clicked() {
                            c.ty = t.to_string();
                        }
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            { let ch = !new && c.not_null != o.not_null; ui.checkbox(&mut c.not_null, label_text("Not Null", ch)) };
        });
        text_row(ui, "Default Expression", &mut c.default, (!new).then_some(&o.default), true);
        let gen_on = new || !c.generated.is_empty();
        text_row(ui, "Generated Always As", &mut c.generated, (!new).then_some(&o.generated), gen_on);
        ui.horizontal(|ui| {
            { let ch = !new && c.identity != o.identity; ui.checkbox(&mut c.identity, label_text("Identity", ch)) };
        });
        // sequence options of an identity column (shown, not edited)
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal(|ui| {
                let w = ui.available_width();
                ui.add_sized(vec2(LABEL_W - 8.0, 26.0), egui::Label::new("Min"));
                ui.add(egui::TextEdit::singleline(&mut String::new()).desired_width(w * 0.35));
                ui.label("Max");
                ui.add(egui::TextEdit::singleline(&mut String::new()).desired_width(w * 0.12));
                ui.label("Step");
                ui.add(egui::TextEdit::singleline(&mut String::new()).desired_width(w * 0.12));
            });
            ui.horizontal(|ui| {
                let w = ui.available_width();
                ui.add_sized(vec2(LABEL_W - 8.0, 26.0), egui::Label::new("Cache Size"));
                ui.add(egui::TextEdit::singleline(&mut String::new()).desired_width(w * 0.35));
                ui.checkbox(&mut false, "Cycled");
            });
        });
        // constraints and indexes follow a renamed column
        let after = self.cur.cols[i].name.clone();
        if after != before {
            for c in &mut self.cur.cons {
                for x in c.cols.iter_mut().filter(|x| **x == before) {
                    *x = after.clone();
                }
            }
            for x in &mut self.cur.idxs {
                for c in x.cols.iter_mut().filter(|c| c.expr == before) {
                    c.expr = after.clone();
                }
            }
        }
    }

    fn names_of_cols(&self) -> Vec<String> {
        self.cur.cols.iter().map(|c| c.name.clone()).collect()
    }

    fn con_head(&mut self, ui: &mut Ui, i: usize) -> Con {
        let o = self.cur.cons[i].orig.map(|o| self.orig.cons[o].clone()).unwrap_or_default();
        let new = self.cur.cons[i].orig.is_none();
        let c = &mut self.cur.cons[i];
        let before = c.name.clone();
        text_row(ui, "Name", &mut c.name, (!new).then_some(&o.name), true);
        if c.name != before {
            c.auto = false;
        }
        text_row(ui, "Comment", &mut c.comment, (!new).then_some(&o.comment), true);
        o
    }

    fn key_form(&mut self, ui: &mut Ui, i: usize) {
        let o = self.con_head(ui, i);
        let new = self.cur.cons[i].orig.is_none();
        let cols = self.names_of_cols();
        let c = &mut self.cur.cons[i];
        let mut primary = c.kind == 'p';
        egui::Grid::new("modify-key-flags").num_columns(3).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
            if ui.checkbox(&mut primary, label_text("Primary", !new && (c.kind == 'p') != (o.kind == 'p'))).changed() {
                c.kind = if primary { 'p' } else { 'u' };
            }
            { let ch = !new && c.deferrable != o.deferrable; ui.checkbox(&mut c.deferrable, label_text("Deferrable", ch)) };
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Defined With Period"));
            ui.end_row();
            { let ch = !new && c.deferred != o.deferred; ui.checkbox(&mut c.deferred, label_text("Initially Deferred", ch)) };
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Not Enforced"));
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Not Validated"));
            ui.end_row();
        });
        ui.label("Columns");
        let part = &mut self.part;
        parts(ui, &mut c.cols, part, String::new, |s| s.clone(), |ui, s, w| {
            field_w(ui, "Column Name", 110.0, w, false, |ui, w| combo(ui, "modify-key-col", s, &cols, w, false));
            field_w(ui, "Exclude operator", 110.0, w, false, |ui, w| {
                ui.add_enabled(false, egui::TextEdit::singleline(&mut String::new()).desired_width(w));
            });
        });
    }

    fn fk_form(&mut self, ui: &mut Ui, i: usize) {
        let o = self.con_head(ui, i);
        let new = self.cur.cons[i].orig.is_none();
        let rel = self.rel.as_ref().expect("loaded");
        let cols: Vec<String> = self.cur.cols.iter().map(|c| c.name.clone()).collect();
        let tables: Vec<String> = rel.tables.iter().map(|t| t.0.clone()).collect();
        let c = &mut self.cur.cons[i];
        field(ui, "Target Table", !new && c.target != o.target, |ui, w| {
            let before = c.target.clone();
            combo(ui, "modify-fk-target", &mut c.target, &tables, w, false);
            if c.target != before {
                // the target's primary key by default
                c.ref_cols = rel.tables.iter().find(|t| t.0 == c.target).map(|t| t.2.clone()).unwrap_or_default();
                c.ref_cols.resize(c.cols.len().max(c.ref_cols.len()), String::new());
                c.cols.resize(c.ref_cols.len(), String::new());
            }
        });
        let target_cols = rel.tables.iter().find(|t| t.0 == c.target).map(|t| t.1.clone()).unwrap_or_default();
        ui.label("Columns");
        // pairs `column → target column`
        let mut pairs: Vec<(String, String)> = c.cols.iter().cloned().zip(c.ref_cols.iter().cloned().chain(std::iter::repeat(String::new()))).collect();
        let before = pairs.clone();
        parts(ui, &mut pairs, &mut self.part, || (String::new(), String::new()), |p| format!("{} → {}", p.0, p.1), |ui, p, w| {
            field_w(ui, "Column Name", 110.0, w, false, |ui, w| combo(ui, "modify-fk-col", &mut p.0, &cols, w, false));
            field_w(ui, "Target Name", 110.0, w, false, |ui, w| combo(ui, "modify-fk-ref", &mut p.1, &target_cols, w, false));
        });
        if pairs != before {
            c.cols = pairs.iter().map(|p| p.0.clone()).collect();
            c.ref_cols = pairs.iter().map(|p| p.1.clone()).collect();
        }
        egui::Grid::new("modify-fk-flags").num_columns(3).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
            { let ch = !new && c.deferrable != o.deferrable; ui.checkbox(&mut c.deferrable, label_text("Deferrable", ch)) };
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Defined With Period"));
            { let ch = !new && c.deferred != o.deferred; ui.checkbox(&mut c.deferred, label_text("Initially Deferred", ch)) };
            ui.end_row();
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Not Enforced"));
            { let ch = !new && c.not_valid != o.not_valid; ui.checkbox(&mut c.not_valid, label_text("Not Validated", ch)) };
            ui.end_row();
        });
        for (label, v, ov) in [("On Delete", &mut c.on_delete, o.on_delete), ("On Update", &mut c.on_update, o.on_update)] {
            field(ui, label, !new && *v != ov, |ui, w| {
                let cur = ACTIONS.iter().find(|a| a.0 == *v).map_or("no_action", |a| a.1);
                egui::ComboBox::from_id_salt(("modify-fk-action", label)).icon(combo_chevron).width(w * 0.55).selected_text(cur).show_ui(ui, |ui| {
                    for (ch, n, _) in ACTIONS {
                        ui.selectable_value(v, ch, n);
                    }
                });
            });
        }
    }

    fn check_form(&mut self, ui: &mut Ui, i: usize) {
        let o = self.con_head(ui, i);
        let new = self.cur.cons[i].orig.is_none();
        let c = &mut self.cur.cons[i];
        egui::Grid::new("modify-check-flags").num_columns(3).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Deferrable"));
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Initially Deferred"));
            { let ch = !new && c.no_inherit != o.no_inherit; ui.checkbox(&mut c.no_inherit, label_text("No Inherit", ch)) };
            ui.end_row();
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Not Enforced"));
            { let ch = !new && c.not_valid != o.not_valid; ui.checkbox(&mut c.not_valid, label_text("Not Validated", ch)) };
            ui.end_row();
        });
        field(ui, "Predicate", !new && c.predicate != o.predicate, |ui, w| {
            ui.add(egui::TextEdit::singleline(&mut c.predicate).font(mono(13.0)).desired_width(w).margin(vec2(8.0, 5.0)));
        });
    }

    fn idx_form(&mut self, ui: &mut Ui, i: usize) {
        let o = self.cur.idxs[i].orig.map(|o| self.orig.idxs[o].clone()).unwrap_or_default();
        let new = self.cur.idxs[i].orig.is_none();
        let cols = self.names_of_cols();
        let methods = self.rel.as_ref().map(|r| r.index_methods.clone()).unwrap_or_default();
        let x = &mut self.cur.idxs[i];
        let before = x.name.clone();
        text_row(ui, "Name", &mut x.name, (!new).then_some(&o.name), true);
        if x.name != before {
            x.auto = false;
        }
        text_row(ui, "Comment", &mut x.comment, (!new).then_some(&o.comment), true);
        ui.horizontal(|ui| {
            { let ch = !new && x.unique != o.unique; ui.checkbox(&mut x.unique, label_text("Unique", ch)) };
            ui.add_space(40.0);
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "Nulls Not Distinct"));
        });
        ui.label("Columns");
        parts(ui, &mut x.cols, &mut self.part, IdxCol::default, |c| format!("{}{}", c.expr, if c.desc { " DESC" } else { "" }), |ui, c, w| {
            field_w(ui, "Column Name", 110.0, w, false, |ui, w| combo(ui, "modify-idx-col", &mut c.expr, &cols, w, true));
            field_w(ui, "Order", 110.0, w, false, |ui, w| {
                let cur = if c.desc { "DESC" } else { "NONE" };
                egui::ComboBox::from_id_salt("modify-idx-order").icon(combo_chevron).width(w).selected_text(cur).show_ui(ui, |ui| {
                    for (d, n) in [(false, "NONE"), (false, "ASC"), (true, "DESC")] {
                        if ui.selectable_label(cur == n, n).clicked() {
                            c.desc = d;
                            c.nulls_first = d;
                        }
                    }
                });
            });
            field_w(ui, "Collation", 110.0, w, false, |ui, w| {
                let mut s = c.collation.clone().unwrap_or_else(|| "default".into());
                ui.add(egui::TextEdit::singleline(&mut s).desired_width(w).margin(vec2(8.0, 5.0)));
                c.collation = Some(s).filter(|s| !s.is_empty() && s != "default");
            });
            field_w(ui, "Operator Class", 110.0, w, false, |ui, w| {
                ui.add_enabled(false, egui::TextEdit::singleline(&mut String::new()).desired_width(w));
            });
        });
        text_row(ui, "Condition", &mut x.pred, (!new).then_some(&o.pred), true);
        text_row(ui, "Include Columns", &mut String::new(), None, false);
        let mut m = if x.method.is_empty() { String::new() } else { x.method.clone() };
        combo_row(ui, "Access Method", &mut m, &methods, (!new).then_some(&o.method), true);
        x.method = m;
        combo_row(ui, "Tablespace", &mut String::new(), &[], None, false);
    }

    // -------------------------------------------------------------- preview

    fn preview_area(&mut self, ui: &mut Ui, head: Rect, bottom: f32, out: &mut ModifyOut) {
        let resp = ui.interact(Rect::from_min_size(head.min, vec2(90.0, head.height())), Id::new("modify-preview-toggle"), Sense::click());
        paint_icon_tinted(
            ui,
            Rect::from_min_size(pos2(head.left(), head.center().y - 8.0), vec2(16.0, 16.0)),
            if self.preview { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT },
            TEXT_MUTED,
        );
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Preview"));
        let t = text_at(ui, pos2(head.left() + 22.0, head.center().y), Align2::LEFT_CENTER, "Preview", ui_font(13.5), TEXT);
        ui.painter().hline(t.right() + 8.0..=head.right() - 64.0, head.center().y, Stroke::new(1.0, BORDER));
        if resp.clicked() {
            self.preview = !self.preview;
        }
        let script = self.script();
        row_ui(ui, Rect::from_min_max(pos2(head.right() - 58.0, head.top()), head.max), |ui| {
            icon_btn_ex(ui, ic::SETTINGS, "Settings", false, false, true);
            if icon_btn_ex(ui, ic::EXPORT, "Open in Console", !script.is_empty(), false, false).clicked() {
                *out = ModifyOut::Console(script.clone());
            }
        });
        if !self.preview {
            return;
        }
        let code = Rect::from_min_max(pos2(head.left(), head.bottom() + 4.0), pos2(head.right(), bottom - 4.0));
        ui.painter().rect_stroke(code, CornerRadius::same(4), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
        rect_ui(ui, code.shrink(1.0), |ui| editor::code_view(ui, Id::new("modify-preview"), &script, Lang::Sql));
    }
}

/// Field label: blue when the value differs from the database's.
fn label_text(t: &str, changed: bool) -> egui::RichText {
    egui::RichText::new(t).color(if changed { LINK } else { TEXT })
}

fn field(ui: &mut Ui, label: &str, changed: bool, add: impl FnOnce(&mut Ui, f32)) {
    let w = ui.available_width();
    field_w(ui, label, LABEL_W, w, changed, add);
}

fn field_w(ui: &mut Ui, label: &str, label_w: f32, w: f32, changed: bool, add: impl FnOnce(&mut Ui, f32)) {
    ui.horizontal(|ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(label_w, 26.0), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
        text_at(ui, pos2(r.left(), r.center().y), Align2::LEFT_CENTER, label, ui_font(13.0), if changed { LINK } else { TEXT });
        let rest = (w - label_w - 8.0).max(60.0);
        add(ui, rest);
    });
}

/// `orig`: the database's value (`None` for a new object): the label turns blue once it differs.
fn text_row(ui: &mut Ui, label: &str, v: &mut String, orig: Option<&String>, enabled: bool) {
    let changed = orig.is_some_and(|o| o != v);
    field(ui, label, changed, |ui, w| {
        ui.add_enabled(enabled, egui::TextEdit::singleline(v).desired_width(w).margin(vec2(8.0, 5.0)));
    });
}

fn combo_row(ui: &mut Ui, label: &str, v: &mut String, opts: &[String], orig: Option<&String>, empty: bool) {
    let changed = orig.is_some_and(|o| o != v);
    field(ui, label, changed, |ui, w| combo(ui, &format!("modify-{label}"), v, opts, w, empty));
}

/// Combo box over `opts` (`free`: also an empty choice / typed value kept).
fn combo(ui: &mut Ui, id: &str, v: &mut String, opts: &[String], w: f32, free: bool) {
    if free {
        ui.add(egui::TextEdit::singleline(v).desired_width(w - 34.0).margin(vec2(8.0, 5.0)));
        let b = icon_btn(ui, ic::CHEVRON_DOWN, "Choose");
        egui::Popup::menu(&b).show(|ui| {
            ui.set_min_width(180.0);
            for o in opts {
                if ui.selectable_label(v == o, o).clicked() {
                    *v = o.clone();
                }
            }
        });
        return;
    }
    egui::ComboBox::from_id_salt(id).icon(combo_chevron).width(w).selected_text(v.as_str()).show_ui(ui, |ui| {
        for o in opts {
            ui.selectable_value(v, o.clone(), o);
        }
    });
}

/// Column list of a key / index / foreign key: + − ↑ ↓, the entries, and an editor of the selected one.
fn parts<T: Clone>(
    ui: &mut Ui,
    v: &mut Vec<T>,
    sel: &mut usize,
    new: impl Fn() -> T,
    label: impl Fn(&T) -> String,
    edit: impl FnOnce(&mut Ui, &mut T, f32),
) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 190.0), Sense::hover());
    let list = Rect::from_min_size(r.min, vec2(155.0, r.height()));
    ui.painter().rect_stroke(list, CornerRadius::same(4), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    let mut act = None;
    row_ui(ui, Rect::from_min_size(list.min + vec2(4.0, 2.0), vec2(list.width() - 8.0, 28.0)), |ui| {
        if icon_btn(ui, ic::ADD, "Add").clicked() {
            act = Some(0);
        }
        if icon_btn_ex(ui, ic::REMOVE, "Remove", !v.is_empty(), false, false).clicked() {
            act = Some(1);
        }
        if icon_btn_ex(ui, ic::MOVE_UP, "Move Up", *sel > 0 && *sel < v.len(), false, false).clicked() {
            act = Some(2);
        }
        if icon_btn_ex(ui, ic::MOVE_DOWN, "Move Down", *sel + 1 < v.len(), false, false).clicked() {
            act = Some(3);
        }
    });
    match act {
        Some(0) => {
            v.push(new());
            *sel = v.len() - 1;
        }
        Some(1) if *sel < v.len() => {
            v.remove(*sel);
            *sel = sel.saturating_sub(1);
        }
        Some(2) => {
            v.swap(*sel, *sel - 1);
            *sel -= 1;
        }
        Some(3) => {
            v.swap(*sel, *sel + 1);
            *sel += 1;
        }
        _ => {}
    }
    let items = Rect::from_min_max(list.min + vec2(1.0, 32.0), list.max - vec2(1.0, 1.0));
    if v.is_empty() {
        text_at(ui, items.center(), Align2::CENTER_CENTER, "No Columns", ui_font(13.0), TEXT_SECONDARY);
    }
    for (k, item) in v.iter().enumerate() {
        let row = Rect::from_min_size(items.min + vec2(0.0, k as f32 * 24.0), vec2(items.width(), 24.0));
        let resp = ui.interact(row, ui.id().with(("part", k)), Sense::click());
        if k == *sel {
            ui.painter().rect_filled(row, CornerRadius::ZERO, SEL_INACTIVE);
        }
        let t = label(item);
        let t = if t.trim().is_empty() || t.trim() == "→" { "<none>".to_string() } else { t };
        ui.painter().with_clip_rect(row).text(row.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, t, ui_font(13.0), TEXT);
        if resp.clicked() {
            *sel = k;
        }
    }
    if let Some(item) = v.get_mut(*sel) {
        let ed = Rect::from_min_max(pos2(list.right() + 8.0, r.top()), r.max);
        rect_ui(ui, ed, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            edit(ui, item, ed.width());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pg::{RelCol, RelCon, RelIdx};

    /// `customers` / `orders` of the seed, as the catalog describes them.
    fn rel(name: &str) -> Rel {
        let col = |n: &str, ty: &str, nn: bool, d: Option<&str>| RelCol {
            name: n.into(),
            ty: ty.into(),
            not_null: nn,
            default: d.map(String::from),
            ..RelCol::default()
        };
        let con = |n: &str, kind: char, cols: &[&str], def: &str| RelCon {
            name: n.into(),
            kind,
            cols: cols.iter().map(|c| c.to_string()).collect(),
            def: def.into(),
            on_update: 'a',
            on_delete: 'a',
            validated: true,
            ..RelCon::default()
        };
        let fk = |n: &str, c: &str, t: &str, rc: &str| RelCon {
            ref_schema: "public".into(),
            ref_table: t.into(),
            ref_cols: vec![rc.into()],
            ref_pk: true,
            ..con(n, 'f', &[c], "")
        };
        let idx = |n: &str, cols: &[&str], constraint: bool| RelIdx {
            name: n.into(),
            unique: constraint,
            method: "btree".into(),
            cols: cols.iter().map(|c| IdxCol { expr: c.to_string(), ..IdxCol::default() }).collect(),
            constraint,
            ..RelIdx::default()
        };
        let tables = vec![
            ("public.customers".into(), vec!["id".into(), "email".into()], vec!["id".into()]),
            ("public.products".into(), vec!["sku".into()], vec!["sku".into()]),
        ];
        let base = Rel { schema: "public".into(), name: name.into(), kind: 'r', owner: "postgres".into(), persistence: 'p', tables, ..Rel::default() };
        if name == "customers" {
            Rel {
                cols: vec![
                    col("id", "integer", true, Some("nextval('customers_id_seq'::regclass)")),
                    col("active", "boolean", true, Some("true")),
                    col("tier", "text", false, None),
                    col("note", "text", false, None),
                ],
                cons: vec![con("customers_pkey", 'p', &["id"], ""), con("customers_email_key", 'u', &["email"], "")],
                indexes: vec![idx("customers_pkey", &["id"], true), idx("customers_city_age", &["(address ->> 'city'::text)", "age"], false)],
                ..base
            }
        } else {
            Rel {
                cols: vec![col("id", "bigint", true, None), col("customer_id", "integer", true, None), col("status", "text", true, None)],
                cons: vec![
                    con("orders_pkey", 'p', &["id"], ""),
                    fk("orders_customer_id_fkey", "customer_id", "customers", "id"),
                    fk("orders_sku_fkey", "sku", "products", "sku"),
                    con("orders_qty_check", 'c', &["qty"], "CHECK ((qty > 0))"),
                ],
                indexes: vec![idx("orders_pkey", &["id"], true)],
                ..base
            }
        }
    }

    /// Expected texts: DataGrip's Modify Table preview for the same edits.
    #[test]
    fn preview_like_datagrip() {
        let r = rel("customers");
        let o = Model::of(&r);
        let mut m = o.clone();
        m.table.name = "customers2".into();
        m.table.comment = "tbl com".into();
        m.cols.retain(|c| c.name != "tier");
        let n = m.cols.iter_mut().find(|c| c.name == "note").unwrap();
        n.name = "note2".into();
        n.comment = "col c".into();
        n.ty = "varchar(100)".into();
        n.not_null = true;
        n.default = "'x'".into();
        m.cols.push(Col { name: "newcol".into(), ty: "integer".into(), default: "0".into(), not_null: true, comment: "new c".into(), ..Col::default() });
        assert_eq!(
            script(&o, &m, &r),
            "alter table customers
    drop column tier;

alter table customers
    rename column note to note2;

comment on column customers.note2 is 'col c';

alter table customers
    alter column note2 type varchar(100) using note2::varchar(100);

alter table customers
    alter column note2 set not null;

alter table customers
    alter column note2 set default 'x';

alter table customers
    rename to customers2;

comment on table customers2 is 'tbl com';

alter table customers2
    add newcol integer default 0 not null;

comment on column customers2.newcol is 'new c';"
        );

        let mut m = o.clone();
        m.cols[1].default.clear();
        m.cons.retain(|c| c.name != "customers_email_key");
        m.idxs[1].name = "city_a".into();
        assert_eq!(
            script(&o, &m, &r),
            "alter table customers
    alter column active drop default;

alter index customers_city_age rename to city_a;

alter table customers
    drop constraint customers_email_key;"
        );

        let r = rel("orders");
        let o = Model::of(&r);
        assert_eq!(o.cons[3].predicate, "qty > 0");
        let mut m = o.clone();
        m.cols[2].not_null = false;
        m.cons[0].name = "orders_pk".into();
        m.cons[1].name = "fk_cust".into();
        m.cons[1].on_delete = 'c';
        m.cons.remove(2);
        m.cons[2].predicate = "qty >= 1".into();
        m.cons.push(Con { kind: 'c', name: "chk_total".into(), predicate: "total >= 0".into(), on_delete: 'a', on_update: 'a', ..Con::default() });
        m.idxs.push(Idx { name: "idx_status".into(), cols: vec![IdxCol { expr: "status".into(), ..IdxCol::default() }], method: "btree".into(), ..Idx::default() });
        assert_eq!(
            script(&o, &m, &r),
            "alter table orders
    alter column status drop not null;

create index idx_status
    on orders (status);

alter table orders
    rename constraint orders_pkey to orders_pk;

alter table orders
    drop constraint orders_customer_id_fkey;

alter table orders
    add constraint fk_cust
        foreign key (customer_id) references customers
            on delete cascade;

alter table orders
    drop constraint orders_sku_fkey;

alter table orders
    drop constraint orders_qty_check;

alter table orders
    add constraint orders_qty_check
        check (qty >= 1);

alter table orders
    add constraint chk_total
        check (total >= 0);"
        );
        // nothing changed: no script; a dropped column takes its constraints along
        assert_eq!(script(&o, &o, &r), "");
        let mut m = o.clone();
        m.cols.retain(|c| c.name != "customer_id");
        m.cons.retain(|c| c.name != "orders_customer_id_fkey");
        assert_eq!(script(&o, &m, &r), "alter table orders\n    drop column customer_id;");
    }

    #[test]
    fn new_objects_are_named_like_datagrip() {
        let mut d = ModifyDlg::new(1, "datatired".into(), "public.customers".into(), "pg");
        d.loaded(rel("customers"));
        d.add(Folder::Keys);
        d.add(Folder::Fks);
        d.add(Folder::Indexes);
        d.auto_names();
        let names: Vec<&str> = d.cur.cons.iter().skip(2).map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["customers_pk", "customers___fk"]);
        assert_eq!(d.cur.idxs.last().unwrap().name, "customers__index");
        d.cur.idxs.last_mut().unwrap().cols.push(IdxCol { expr: "note".into(), ..IdxCol::default() });
        d.cur.idxs.last_mut().unwrap().unique = true;
        d.auto_names();
        assert_eq!(d.cur.idxs.last().unwrap().name, "customers_note_uindex");
        d.add(Folder::Columns);
        assert_eq!((d.cur.cols.last().unwrap().name.as_str(), d.cur.cols.last().unwrap().ty.as_str()), ("column_name", "integer"));
    }
}
