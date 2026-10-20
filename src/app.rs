//! Main window: header, Database Explorer, editor tabs (consoles / data editors),
//! Services (sessions, output, results) and status bar, laid out as New UI islands.

use crate::PAGE;
use crate::config::{self, DataSource, Driver};
use crate::dialog::{DialogOut, DsDialog, TestState, driver_icon, primary_btn};
use crate::editor::{self, Lang, Mark};
use crate::files::{self, FileAction, Files};
use crate::grid::{Grid, GridAction};
use crate::icons as ic;
use crate::modify::{ModifyDlg, ModifyOut};
use crate::mongo::{Body, Conn, ItemKind, LogKind, Meta, Req, Resp, RunItem, split_statements, stamp};
use crate::pg;
use crate::theme::*;
use crate::widgets::*;
use egui::{Align2, Color32, CornerRadius, Id, ImageSource, Key, Rect, Sense, Stroke, Ui, pos2, vec2};
use mongodb::bson::doc;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

const HEADER_H: f32 = 40.0;
const STATUS_H: f32 = 30.0;

enum ConnState {
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

struct CollNode {
    /// PostgreSQL schema (empty for MongoDB).
    schema: String,
    name: String,
    /// `r` table / collection, `v` view, `m` materialized view, `S` sequence.
    kind: char,
    meta: Option<Meta>,
}

impl CollNode {
    /// Identity used by requests and tree keys: `schema.table` (quoted as needed) or the collection name.
    fn qual(&self) -> String {
        if self.schema.is_empty() { self.name.clone() } else { pg::qualify(&self.schema, &self.name) }
    }
}

struct DbNode {
    name: String,
    /// PostgreSQL schemas (also the empty ones).
    schemas: Vec<String>,
    colls: Option<Vec<CollNode>>,
}

struct Source {
    ds: DataSource,
    conn: Option<Conn>,
    state: ConnState,
    dbs: Option<Vec<DbNode>>,
    all_dbs: Vec<String>,
    version: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Playground,
    Script,
}

struct Console {
    id: u64,
    ds: u64,
    path: PathBuf,
    saved: String,
    last_save: Instant,
    title: String,
    db: String,
    /// PostgreSQL search path of the console (`datatired.public`).
    schema: String,
    text: String,
    marks: Vec<(usize, Mark)>,
    running: bool,
    started: bool,
    mode: Mode,
    line_col: (usize, usize),
}

struct DataTab {
    id: u64,
    ds: u64,
    db: String,
    coll: String,
    filter: String,
    sort: String,
    grid: Grid,
    page: u64,
}

/// Plain file opened from the Files view (scratches, attached directories).
struct FileTab {
    id: u64,
    path: PathBuf,
    text: String,
    saved: String,
    last_save: Instant,
    line_col: (usize, usize),
}

enum Tab {
    Console(Console),
    Data(DataTab),
    File(FileTab),
    /// Console result opened with Execute in New Tab.
    Result(ResultTab),
}

impl Tab {
    fn id(&self) -> u64 {
        match self {
            Tab::Console(c) => c.id,
            Tab::Data(d) => d.id,
            Tab::File(f) => f.id,
            Tab::Result(r) => r.id,
        }
    }
    fn ds(&self) -> Option<u64> {
        match self {
            Tab::Console(c) => Some(c.ds),
            Tab::Data(d) => Some(d.ds),
            Tab::File(_) => None,
            Tab::Result(r) => Some(r.ds),
        }
    }
    /// Writes the backing file when it changed (consoles and files autosave like the IDE).
    fn flush(&mut self, force: bool) {
        let (path, text, saved, last) = match self {
            Tab::Console(c) => (&c.path, &c.text, &mut c.saved, &mut c.last_save),
            Tab::File(f) => (&f.path, &f.text, &mut f.saved, &mut f.last_save),
            Tab::Data(_) | Tab::Result(_) => return,
        };
        if text != saved && (force || last.elapsed().as_secs_f32() > 1.0) {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if std::fs::write(path, text).is_ok() {
                *saved = text.clone();
            }
            *last = Instant::now();
        }
    }
}

struct ResultTab {
    id: u64,
    console: u64,
    ds: u64,
    grid: Grid,
}

struct Session {
    key: u64,
    ds: u64,
    name: String,
    console: bool,
    ms: Option<u128>,
}

#[derive(Default)]
struct Services {
    output: Vec<(String, LogKind)>,
    results: Vec<ResultTab>,
    /// 0 = Output, n = results[n - 1]
    active: usize,
    sessions: Vec<Session>,
    soft_wrap: bool,
}

enum Pending {
    Connect(u64),
    Databases(u64),
    Colls(u64, String),
    Meta(u64, String, String),
    /// console, data source, start, keep the console's earlier result tabs
    Run(u64, u64, std::time::Instant, bool),
    DataPage(u64, std::time::Instant),
    ResultPage(u64),
    Save(u64, bool),
    Cmd(u64, String),
    Ddl(u64, String, String, DdlTo),
    /// Modify Table's script.
    Alter(u64, String),
    Test,
}

/// Where a generated DDL goes.
#[derive(Clone, Copy, PartialEq)]
enum DdlTo {
    /// The SQL Generator tool window.
    Panel,
    Clipboard,
    Console,
    /// The Modify Table dialog.
    Modify,
}

/// The SQL Generator tool window's object.
struct SqlGen {
    ds: u64,
    db: String,
    coll: String,
    /// `None` while it is read.
    rel: Option<pg::Rel>,
    error: Option<String>,
    /// SQL Generator Options shown (the gear).
    options: bool,
}

#[derive(Clone)]
enum Action {
    Connect(u64),
    Disconnect(u64),
    Refresh(u64),
    RefreshDb(u64, String),
    LoadMeta(u64, String, String),
    OpenData(u64, String, String),
    OpenConsole(u64, Option<String>),
    NewConsole(u64, Option<String>),
    /// Data Sources dialog: selected source, or a new one of that driver.
    Dialog(Option<u64>, Option<Driver>),
    RemoveSource(u64),
    /// "Remove Data Source…": asks first.
    AskRemoveSource(u64),
    RenameSource(u64),
    Drop(u64, String, Option<String>),
    Rename(u64, String, String),
    Create(u64, String),
    /// SQL Scripts › SQL Generator… / Generate DDL to Clipboard / … to Query Console.
    Ddl(u64, String, String, DdlTo),
    /// Modify Table… (PostgreSQL).
    Modify(u64, String, String),
}

struct HistoryDlg {
    ds: u64,
    /// Console the selected statement is pasted into.
    console: u64,
    title: String,
    items: Vec<String>,
    sel: Option<usize>,
    scroll: bool,
}

/// Waiting for "Ignore Unsubmitted Changes" confirmation.
enum Confirm {
    Grid(GridTarget, Vec<GridAction>),
    CloseTab(u64),
    CloseResult(u64),
}

enum Ask {
    RemoveSource(u64, String),
    RenameSource(u64, String),
    Drop(u64, String, Option<String>),
    Rename(u64, String, String, String),
    Create(u64, String, String),
    RenameFile(PathBuf, String),
    DeleteFile(PathBuf),
    AttachPath(String),
}

/// One Database Explorer row, built by `Workspace::tree_model`.
struct TreeRow {
    key: String,
    depth: usize,
    /// `None` = leaf.
    open: Option<bool>,
    icon: ImageSource<'static>,
    label: String,
    suffix: Option<String>,
    extra: NodeExtra,
    /// Gray placeholder text ("Loading…") instead of a node.
    info: bool,
    /// Requested when the node is expanded (connect, load children).
    load: Option<Action>,
    /// Double click / ↩ / ⌘↓ (Edit Data).
    activate: Option<Action>,
    menu: RowMenu,
    tip: Option<String>,
    /// Ancestor keys (speed search expands them).
    anc: Vec<String>,
}

impl TreeRow {
    fn new(key: String, depth: usize, open: Option<bool>, icon: ImageSource<'static>, label: &str, anc: &[String]) -> Self {
        TreeRow {
            key,
            depth,
            open,
            icon,
            label: label.into(),
            suffix: None,
            extra: NodeExtra::default(),
            info: false,
            load: None,
            activate: None,
            menu: RowMenu::None,
            tip: None,
            anc: anc.to_vec(),
        }
    }

    fn info(depth: usize, text: &str) -> Self {
        TreeRow { info: true, ..TreeRow::new(String::new(), depth, None, ic::FOLDER, text, &[]) }
    }
}

/// What the keyboard needs of a rendered explorer row.
#[derive(Clone)]
struct VisRow {
    key: String,
    expandable: bool,
    load: Option<Action>,
    activate: Option<Action>,
}

/// Context menu of an explorer row.
enum RowMenu {
    None,
    Ds(u64),
    Db(u64, String),
    Schema(u64, String),
    Coll(u64, String, String, char),
}

fn item_icon(k: ItemKind) -> ImageSource<'static> {
    match k {
        ItemKind::Field => ic::FIELD,
        ItemKind::IdField => ic::FIELD_KEY,
        ItemKind::Column => ic::COLUMN,
        ItemKind::PkColumn => ic::COLUMN_KEY,
        ItemKind::NotNullColumn => ic::COLUMN_NOT_NULL,
        ItemKind::PrimaryKey => ic::KEY_PRIMARY,
        ItemKind::Key => ic::KEY,
        ItemKind::ForeignKey => ic::FOREIGN_KEY,
        ItemKind::Index => ic::INDEX,
        ItemKind::UniqueIndex => ic::INDEX_UNIQUE,
        ItemKind::Check => ic::CHECK,
    }
}

/// `DROP` / `ALTER` object type of a PostgreSQL relation kind.
fn rel_kind(kind: char) -> &'static str {
    match kind {
        'v' => "VIEW",
        'm' => "MATERIALIZED VIEW",
        'S' => "SEQUENCE",
        _ => "TABLE",
    }
}

fn open_conn(ds: &DataSource, pw: Option<&str>, tx: Sender<Resp>, wake: impl Fn() + Send + 'static) -> Conn {
    match ds.driver {
        Driver::Mongo => Conn::open(ds.uri(pw), tx, wake),
        Driver::Postgres => pg::open(ds.uri(pw), tx, wake),
    }
}

/// Persists a console's current database (and PostgreSQL schema) by file path.
fn remember_console(cfg: &mut config::Config, c: &Console) {
    let key = c.path.display().to_string();
    cfg.console_db.insert(key.clone(), c.db.clone());
    if !c.schema.is_empty() {
        cfg.console_schema.insert(key, c.schema.clone());
    }
}

/// The project shown in a workspace (badge colour = index into the 9-colour palette).
#[derive(Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub color: usize,
}

/// One row of the project widget / one project tab, filled in by the multi-project shell.
#[derive(Clone)]
pub struct ProjectEntry {
    pub project: Project,
    pub open: bool,
    pub active: bool,
    /// `project – active editor tab`, the IDE window title.
    pub caption: String,
}

pub enum ProjectCmd {
    New,
    Open,
    Switch(PathBuf),
    Close(PathBuf),
    RemoveRecent(PathBuf),
}

/// Everything shown for one open project (DataGrip: one project window).
pub struct Workspace {
    pub project: Project,
    /// Open + recent projects, for the project widget and the project tab strip.
    pub shell: Vec<ProjectEntry>,
    /// Request for the multi-project shell (switch / open / close a project).
    pub cmd: Option<ProjectCmd>,
    cfg: config::Config,
    sources: Vec<Source>,
    tabs: Vec<Tab>,
    active: usize,
    services: Services,
    expanded: HashSet<String>,
    selected: Option<String>,
    dialog: Option<DsDialog>,
    passwords: HashMap<u64, String>,
    tx: Sender<Resp>,
    rx: Receiver<Resp>,
    next_tag: u64,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    left_w: f32,
    bottom_h: f32,
    show_explorer: bool,
    show_services: bool,
    /// The bottom area shows the SQL Generator instead of Services.
    show_gen: bool,
    sql_gen: Option<SqlGen>,
    gen_opts: pg::GenOpts,
    modify: Option<ModifyDlg>,
    show_files: bool,
    right_w: f32,
    /// Width of the Services sessions tree (divider is draggable).
    svc_left_w: f32,
    files: Files,
    picker: Option<Receiver<Option<String>>>,
    schema_popup: Option<(u64, egui::Pos2)>,
    ask: Option<Ask>,
    confirm: Option<Confirm>,
    test_conn: Option<Conn>,
    status: Option<String>,
    search: Option<String>,
    /// Console to focus, with an optional caret position (char index; `None` = end of text).
    focus_console: Option<(u64, Option<usize>)>,
    history: Option<HistoryDlg>,
    visible: Vec<VisRow>,
    tree_focus: bool,
    /// Database Explorer speed search (⌘F or typing in the tree).
    speed: Option<String>,
    /// Scroll the tree to the selected node on the next frame.
    scroll_tree: bool,
    tree_scroll_y: Option<f32>,
    /// Background introspection already requested for completion.
    requested: HashSet<String>,
}

impl Workspace {
    /// `uid` keeps tab / widget ids of different projects apart.
    pub fn new(project: Project, uid: u64) -> Self {
        let mut cfg = config::load_project(&project.path);
        let mut moved = false;
        for ds in &mut cfg.sources {
            if let Some(pw) = ds.split_password() {
                config::set_password(ds.id, &pw);
                ds.url_password = true;
                moved = true;
            } else if !ds.url_password
                && ds.auth == config::Auth::UserPassword
                && ds.url != ds.generated_url()
                && ds.url.contains(&format!("://{}@", ds.user))
                && config::password(ds.id).is_some()
            {
                // stored before `url_password` existed: a typed URL with the user in it had its
                // password taken out of it
                ds.url_password = true;
                moved = true;
            }
        }
        if moved {
            config::save_project(&project.path, &cfg);
        }
        let (tx, rx) = channel();
        let sources = cfg
            .sources
            .iter()
            .map(|ds| Source { ds: ds.clone(), conn: None, state: ConnState::Idle, dbs: None, all_dbs: vec![], version: String::new() })
            .collect();
        Workspace {
            project,
            shell: vec![],
            cmd: None,
            cfg,
            sources,
            tabs: vec![],
            active: 0,
            services: Services::default(),
            expanded: HashSet::new(),
            selected: None,
            dialog: None,
            passwords: HashMap::new(),
            tx,
            rx,
            next_tag: 1,
            next_id: uid << 32,
            pending: HashMap::new(),
            left_w: 420.0,
            bottom_h: 340.0,
            show_explorer: true,
            show_services: false,
            show_gen: false,
            sql_gen: None,
            gen_opts: pg::GenOpts::default(),
            modify: None,
            show_files: true,
            right_w: 300.0,
            svc_left_w: 240.0,
            files: Files::default(),
            picker: None,
            schema_popup: None,
            ask: None,
            confirm: None,
            test_conn: None,
            status: None,
            search: None,
            focus_console: None,
            history: None,
            visible: vec![],
            tree_focus: false,
            speed: None,
            scroll_tree: false,
            tree_scroll_y: None,
            requested: HashSet::new(),
        }
    }

    // ------------------------------------------------------------ requests

    fn send(&mut self, ds: u64, req: Req, p: Pending) {
        let tag = self.next_tag;
        self.next_tag += 1;
        let src = self.sources.iter().find(|s| s.ds.id == ds);
        if let Some(c) = src.and_then(|s| s.conn.as_ref()) {
            c.send(tag, req);
            self.pending.insert(tag, p);
        } else if matches!(
            p,
            Pending::Run(..) | Pending::DataPage(..) | Pending::ResultPage(_) | Pending::Save(..) | Pending::Cmd(..) | Pending::Ddl(..) | Pending::Alter(..)
        ) {
            // no connection (failed, or waiting for a password): answer with an error instead of
            // leaving a console "running" / a grid loading forever
            let why = match src.map(|s| &s.state) {
                Some(ConnState::Failed(e)) => e.clone(),
                _ => "not connected".into(),
            };
            let name = src.map_or(String::new(), |s| s.ds.name.clone());
            let _ = self.tx.send(Resp { tag, body: Err(format!("{name}: {why}")) });
            self.pending.insert(tag, p);
        }
    }

    fn src(&mut self, ds: u64) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.ds.id == ds)
    }

    fn driver(&self, ds: u64) -> Driver {
        self.sources.iter().find(|s| s.ds.id == ds).map_or(Driver::Mongo, |s| s.ds.driver)
    }

    fn coll_kind(&self, ds: u64, db: &str, coll: &str) -> char {
        let s = self.sources.iter().find(|s| s.ds.id == ds);
        let c = s.and_then(|s| s.dbs.as_ref()).and_then(|d| d.iter().find(|d| d.name == db)).and_then(|d| d.colls.as_ref());
        c.and_then(|cs| cs.iter().find(|c| c.qual() == coll)).map_or('r', |c| c.kind)
    }

    /// Data tab caption: the table name without its schema, or the collection.
    fn short_name(&self, d: &DataTab) -> String {
        if self.driver(d.ds) == Driver::Postgres { pg::split_qual(&d.coll).1 } else { d.coll.clone() }
    }

    /// Data Sources dialog with the stored passwords filled in (so Test Connection uses them).
    fn ds_dialog(&self, items: Vec<DataSource>, sel: Option<u64>, create: Option<Driver>) -> DsDialog {
        let mut pws = self.passwords.clone();
        for ds in &items {
            if !pws.contains_key(&ds.id)
                && let Some(p) = config::password(ds.id)
            {
                pws.insert(ds.id, p);
            }
        }
        DsDialog::new(items, pws, sel, create)
    }

    fn connect(&mut self, ctx: &egui::Context, ds: u64) {
        let pw = self.passwords.get(&ds).cloned().or_else(|| config::password(ds));
        // like the IDE's credentials prompt: no password → open the data source to enter it
        if let Some(s) = self.sources.iter().find(|s| s.ds.id == ds)
            && s.conn.is_none()
            && s.ds.missing_password(pw.as_deref())
        {
            let msg = format!("Enter the password for user '{}' of {}", s.ds.user, s.ds.name);
            if let Some(s) = self.src(ds) {
                s.state = ConnState::Failed(msg.clone());
            }
            self.status = Some(msg);
            if self.dialog.is_none() {
                let items = self.sources.iter().map(|s| s.ds.clone()).collect();
                self.dialog = Some(self.ds_dialog(items, Some(ds), None));
            }
            return;
        }
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        let Some(s) = self.src(ds) else { return };
        if s.conn.is_some() {
            return;
        }
        s.conn = Some(open_conn(&s.ds, pw.as_deref(), tx, move || ctx2.request_repaint()));
        s.state = ConnState::Connecting;
        self.send(ds, Req::Connect, Pending::Connect(ds));
        self.send(ds, Req::Databases, Pending::Databases(ds));
    }

    fn disconnect(&mut self, ds: u64) {
        if let Some(s) = self.src(ds) {
            s.conn = None;
            s.state = ConnState::Idle;
            s.dbs = None;
        }
        self.services.sessions.retain(|x| x.ds != ds);
    }

    fn log(&mut self, line: String, kind: LogKind) {
        self.services.output.push((line, kind));
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(Resp { tag, body }) = self.rx.try_recv() {
            let Some(p) = self.pending.remove(&tag) else {
                continue;
            };
            match (p, body) {
                (Pending::Connect(ds), Ok(Body::Connected { version })) => {
                    if let Some(s) = self.src(ds) {
                        s.state = ConnState::Connected;
                        s.version = version;
                    }
                    self.log(stamp("Connected"), LogKind::Info);
                }
                (Pending::Connect(ds), Err(e)) | (Pending::Databases(ds), Err(e)) => {
                    let name = self.src(ds).map(|s| s.ds.name.clone()).unwrap_or_default();
                    if let Some(s) = self.src(ds) {
                        s.state = ConnState::Failed(e.clone());
                        s.conn = None;
                    }
                    self.status = Some(format!("{name}: {e}"));
                    self.log(stamp(&format!("[{name}] {e}")), LogKind::Error);
                }
                (Pending::Databases(ds), Ok(Body::Databases(names))) => {
                    let mut to_load = vec![];
                    if let Some(s) = self.src(ds) {
                        let keep = |n: &String| s.ds.schemas.as_ref().is_none_or(|v| v.contains(n));
                        let old: Vec<String> = s.dbs.iter().flatten().filter(|d| d.colls.is_some()).map(|d| d.name.clone()).collect();
                        s.dbs = Some(
                            names.iter().filter(|n| keep(n)).map(|n| DbNode { name: n.clone(), schemas: vec![], colls: None }).collect(),
                        );
                        s.all_dbs = names;
                        to_load = old;
                    }
                    for db in to_load {
                        self.send(ds, Req::Collections { db: db.clone() }, Pending::Colls(ds, db));
                    }
                }
                (Pending::Colls(ds, db), Ok(Body::Collections { schemas, colls })) => {
                    let mut reload = vec![];
                    if let Some(node) = self.src(ds).and_then(|s| s.dbs.as_mut()).and_then(|d| d.iter_mut().find(|d| d.name == db)) {
                        let old: Vec<String> = node.colls.iter().flatten().filter(|c| c.meta.is_some()).map(CollNode::qual).collect();
                        node.schemas = schemas;
                        node.colls =
                            Some(colls.into_iter().map(|(schema, name, kind)| CollNode { schema, name, kind, meta: None }).collect());
                        reload = old;
                    }
                    for c in reload {
                        self.send(ds, Req::Meta { db: db.clone(), coll: c.clone() }, Pending::Meta(ds, db.clone(), c));
                    }
                }
                (Pending::Meta(ds, db, coll), Ok(Body::Meta(meta))) => {
                    if let Some(c) = self.coll_node(ds, &db, &coll) {
                        c.meta = Some(meta);
                    }
                }
                (Pending::Colls(_, _), Err(e)) | (Pending::Meta(_, _, _), Err(e)) => self.status = Some(e),
                (Pending::Run(cid, ds, t0, new_tab), body) => self.on_run(cid, ds, t0, new_tab, body),
                (Pending::DataPage(tid, t0), body) => {
                    let ms = t0.elapsed().as_millis();
                    if let Some(Tab::Data(d)) = self.tabs.iter_mut().find(|t| t.id() == tid) {
                        match body {
                            Ok(Body::Rows(rs)) => d.grid.set(rs),
                            Err(e) => {
                                d.grid.loading = false;
                                d.grid.error = Some(e);
                            }
                            _ => {}
                        }
                    }
                    if let Some(Tab::Data(d)) = self.tabs.iter().find(|t| t.id() == tid) {
                        let (ds, name) = (d.ds, self.short_name(d));
                        self.session(tid, ds, &name, false, Some(ms));
                    }
                }
                (Pending::ResultPage(rid), body) => {
                    if let Some(r) = self.results_mut().find(|r| r.id == rid) {
                        match body {
                            Ok(Body::Rows(rs)) => r.grid.set(rs),
                            Err(e) => {
                                r.grid.loading = false;
                                r.grid.error = Some(e);
                            }
                            _ => {}
                        }
                    }
                }
                (Pending::Save(id, is_result), body) => {
                    match body {
                        Ok(Body::Done(msg)) => self.status = Some(msg),
                        Err(e) => {
                            // nothing was written: keep the pending changes so they can be fixed and resubmitted
                            self.status = Some(format!("Submit failed: {e}"));
                            self.log(stamp(&format!("Submit failed: {e}")), LogKind::Error);
                            self.show_services = true;
                            self.show_gen = false;
                            self.services.active = 0;
                            continue;
                        }
                        _ => {}
                    }
                    if is_result {
                        self.refresh_result(id, None);
                    } else {
                        self.load_data(id, None);
                    }
                }
                (Pending::Cmd(ds, db), body) => {
                    if let Err(e) = body {
                        self.status = Some(e.clone());
                        self.log(stamp(&e), LogKind::Error);
                    }
                    if db.is_empty() {
                        self.send(ds, Req::Databases, Pending::Databases(ds));
                    } else {
                        self.send(ds, Req::Collections { db: db.clone() }, Pending::Colls(ds, db));
                    }
                }
                (Pending::Ddl(ds, db, coll, DdlTo::Panel), body) => {
                    if let Some(g) = self.sql_gen.as_mut().filter(|g| g.ds == ds && g.db == db && g.coll == coll) {
                        match body {
                            Ok(Body::Ddl(r)) => (g.rel, g.error) = (Some(*r), None),
                            Err(e) => g.error = Some(e),
                            _ => {}
                        }
                    }
                }
                (Pending::Ddl(ds, db, coll, DdlTo::Modify), body) => {
                    if let Some(d) = self.modify.as_mut().filter(|d| d.ds == ds && d.db == db && d.coll == coll) {
                        match body {
                            Ok(Body::Ddl(r)) => d.loaded(*r),
                            Err(e) => d.failed(e),
                            _ => {}
                        }
                    }
                }
                (Pending::Alter(ds, db), body) => match body {
                    Err(e) => {
                        if let Some(d) = &mut self.modify {
                            d.failed(e);
                        }
                    }
                    _ => {
                        self.modify = None;
                        self.status = Some("Table modified".into());
                        self.send(ds, Req::Collections { db: db.clone() }, Pending::Colls(ds, db));
                    }
                },
                (Pending::Ddl(ds, db, coll, to), body) => match body {
                    Ok(Body::Ddl(r)) if to == DdlTo::Clipboard => {
                        ctx.copy_text(pg::ddl(&r, &self.gen_opts));
                        self.status = Some(format!("DDL of {coll} copied to the clipboard"));
                    }
                    Ok(Body::Ddl(r)) => self.ddl_console(ctx, ds, db, pg::ddl(&r, &self.gen_opts)),
                    Err(e) => self.status = Some(e),
                    _ => {}
                },
                (Pending::Test, body) => {
                    self.test_conn = None;
                    if let Some(d) = &mut self.dialog {
                        d.test = match body {
                            Ok(Body::Connected { version }) => TestState::Ok(version),
                            Err(e) => TestState::Err(e),
                            _ => TestState::Idle,
                        };
                    }
                }
                _ => {}
            }
        }
    }

    /// Generate DDL to Query Console / SQL Generator › Open in Console: a new console holding it.
    fn ddl_console(&mut self, ctx: &egui::Context, ds: u64, db: String, text: String) {
        let path = config::new_console(ds, self.driver(ds).ext(), &text);
        self.files.reveal_connection(ds);
        self.open_console_file(ctx, ds, path, Some(db));
    }

    /// ⌘8 / the Services button: Services shares the bottom area with the SQL Generator.
    fn toggle_services(&mut self) {
        self.show_services = !(self.show_services && !self.show_gen);
        self.show_gen = false;
    }

    fn coll_node(&mut self, ds: u64, db: &str, coll: &str) -> Option<&mut CollNode> {
        self.src(ds)?.dbs.as_mut()?.iter_mut().find(|d| d.name == db)?.colls.as_mut()?.iter_mut().find(|c| c.qual() == coll)
    }

    fn session(&mut self, key: u64, ds: u64, name: &str, console: bool, ms: Option<u128>) {
        match self.services.sessions.iter_mut().find(|s| s.key == key) {
            Some(s) => {
                s.ms = ms.or(s.ms);
                s.name = name.into();
            }
            None => self.services.sessions.push(Session { key, ds, name: name.into(), console, ms }),
        }
    }

    fn on_run(&mut self, cid: u64, ds: u64, t0: std::time::Instant, new_tab: bool, body: Result<Body, String>) {
        let ms = t0.elapsed().as_millis();
        let mut ok = true;
        let mut first_result = None;
        let items = match body {
            Ok(Body::Ran(items)) => items,
            Err(e) => vec![RunItem::Log(stamp(&e), LogKind::Error)],
            _ => vec![],
        };
        let mut new_results = vec![];
        for it in items {
            match it {
                RunItem::Log(l, k) => {
                    ok &= k != LogKind::Error;
                    self.services.output.push((l, k));
                }
                RunItem::Result(rs) => {
                    let mut grid = Grid::default();
                    grid.set(rs);
                    new_results.push(grid);
                }
            }
        }
        let mut opened = None;
        if new_tab {
            for grid in new_results {
                let id = self.next_id;
                self.next_id += 1;
                self.tabs.push(Tab::Result(ResultTab { id, console: cid, ds, grid }));
                opened.get_or_insert(self.tabs.len() - 1);
            }
        } else {
            if !new_results.is_empty() {
                self.services.results.retain(|r| r.console != cid);
            }
            for grid in new_results {
                let id = self.next_id;
                self.next_id += 1;
                self.services.results.push(ResultTab { id, console: cid, ds, grid });
                first_result.get_or_insert(self.services.results.len());
            }
        }
        if let Some(i) = opened {
            self.active = i;
        }
        // results in editor tabs leave Services alone unless something failed
        if opened.is_none() || !ok {
            self.services.active = if ok { first_result.unwrap_or(0) } else { 0 };
            self.show_services = true;
            self.show_gen = false;
        }
        let mut name = String::from("console");
        if let Some(Tab::Console(c)) = self.tabs.iter_mut().find(|t| t.id() == cid) {
            c.running = false;
            if let Some(m) = c.marks.last_mut() {
                m.1 = if ok { Mark::Ok } else { Mark::Err };
            }
            name = c.title.clone();
        }
        self.session(cid, ds, &name, true, Some(ms));
    }

    fn load_data(&mut self, tid: u64, page: Option<(u64, u64)>) {
        let Some(Tab::Data(d)) = self.tabs.iter_mut().find(|t| t.id() == tid) else {
            return;
        };
        let (offset, limit) = page.unwrap_or_else(|| (d.grid.rs.as_ref().map_or(0, |r| r.offset), d.page));
        d.page = limit;
        d.grid.loading = true;
        let req = Req::Find { db: d.db.clone(), coll: d.coll.clone(), filter: d.filter.clone(), sort: d.sort.clone(), offset, limit };
        let ds = d.ds;
        self.send(ds, req, Pending::DataPage(tid, std::time::Instant::now()));
    }

    fn refresh_result(&mut self, rid: u64, page: Option<(u64, u64)>) {
        let Some(r) = self.results_mut().find(|r| r.id == rid) else {
            return;
        };
        let Some(rs) = &r.grid.rs else { return };
        let Some(spec) = rs.spec.clone() else { return };
        let (offset, limit) = page.unwrap_or((rs.offset, rs.page.max(1)));
        r.grid.loading = true;
        let ds = r.ds;
        self.send(ds, Req::Page { spec, offset, limit }, Pending::ResultPage(rid));
    }

    /// Console results, in Services and opened as editor tabs.
    fn results_mut(&mut self) -> impl Iterator<Item = &mut ResultTab> {
        let tabs = self.tabs.iter_mut().filter_map(|t| match t {
            Tab::Result(r) => Some(r),
            _ => None,
        });
        self.services.results.iter_mut().chain(tabs)
    }

    fn grid_mut(&mut self, t: GridTarget) -> Option<&mut Grid> {
        match t {
            GridTarget::Data(id) => self.tabs.iter_mut().find_map(|x| match x {
                Tab::Data(d) if d.id == id => Some(&mut d.grid),
                _ => None,
            }),
            GridTarget::Result(id) => self.results_mut().find(|r| r.id == id).map(|r| &mut r.grid),
        }
    }

    fn grid_actions(&mut self, ctx: &egui::Context, target: GridTarget, acts: Vec<GridAction>) {
        let reloads =
            |a: &GridAction| matches!(a, GridAction::Refresh | GridAction::Page { .. } | GridAction::Sort(..) | GridAction::FilterBy(_));
        if acts.iter().any(reloads) && self.grid_mut(target).is_some_and(|g| g.dirty()) {
            let (later, now): (Vec<GridAction>, Vec<GridAction>) = acts.into_iter().partition(reloads);
            self.confirm = Some(Confirm::Grid(target, later));
            self.run_grid_actions(ctx, target, now);
            return;
        }
        self.run_grid_actions(ctx, target, acts);
    }

    fn run_grid_actions(&mut self, ctx: &egui::Context, target: GridTarget, acts: Vec<GridAction>) {
        for a in acts {
            match (a, target) {
                (GridAction::Refresh, GridTarget::Data(id)) => self.load_data(id, None),
                (GridAction::Refresh, GridTarget::Result(id)) => self.refresh_result(id, None),
                (GridAction::Page { offset, limit }, GridTarget::Data(id)) => self.load_data(id, Some((offset, limit))),
                (GridAction::Page { offset, limit }, GridTarget::Result(id)) => self.refresh_result(id, Some((offset, limit))),
                (GridAction::Sort(col, dir), GridTarget::Data(id)) => {
                    let pg =
                        self.tabs.iter().find(|t| t.id() == id).and_then(Tab::ds).is_some_and(|ds| self.driver(ds) == Driver::Postgres);
                    let mut page = PAGE;
                    if let Some(Tab::Data(d)) = self.tabs.iter_mut().find(|t| t.id() == id) {
                        d.sort = match (dir, pg) {
                            (0, _) => String::new(),
                            (_, true) => format!("{}{}", pg::ident(&col), if dir < 0 { " DESC" } else { "" }),
                            _ => format!("{{{}: {dir}}}", lit_key(&col)),
                        };
                        page = d.page;
                    }
                    self.load_data(id, Some((0, page)));
                }
                (GridAction::FilterBy(f), GridTarget::Data(id)) => {
                    let mut page = PAGE;
                    if let Some(Tab::Data(d)) = self.tabs.iter_mut().find(|t| t.id() == id) {
                        d.filter = f;
                        page = d.page;
                    }
                    self.load_data(id, Some((0, page)));
                }
                (GridAction::Submit(edits), t) => {
                    let (ds, table, id, res) = match t {
                        GridTarget::Data(id) => match self.tabs.iter().find(|x| x.id() == id) {
                            Some(Tab::Data(d)) => (d.ds, Some((d.db.clone(), d.coll.clone())), id, false),
                            _ => continue,
                        },
                        GridTarget::Result(id) => match self.results_mut().find(|r| r.id == id) {
                            Some(r) => (r.ds, r.grid.rs.as_ref().and_then(|x| x.table()), id, true),
                            None => continue,
                        },
                    };
                    if let Some((db, coll)) = table {
                        self.send(ds, Req::Save { db, coll, edits }, Pending::Save(id, res));
                    }
                }
                (GridAction::Stop, t) => {
                    let ds = match t {
                        GridTarget::Data(id) => self.tabs.iter().find(|x| x.id() == id).and_then(|x| x.ds()),
                        GridTarget::Result(id) => self.results_mut().find(|r| r.id == id).map(|r| r.ds),
                    };
                    if let Some(c) = ds.and_then(|ds| self.sources.iter().find(|s| s.ds.id == ds)).and_then(|s| s.conn.as_ref()) {
                        c.stop();
                    }
                }
                (GridAction::Export(text, ext), t) => {
                    let name = match t {
                        GridTarget::Data(id) => match self.tabs.iter().find(|x| x.id() == id) {
                            Some(Tab::Data(d)) => format!("{}.{}", d.db, d.coll),
                            _ => "export".into(),
                        },
                        GridTarget::Result(id) => self
                            .results_mut()
                            .find(|r| r.id == id)
                            .and_then(|r| r.grid.rs.as_ref())
                            .map_or("result".into(), |r| r.title.clone()),
                    };
                    let dir = config::home().join("Downloads");
                    let path = dir.join(format!("{name}.{ext}"));
                    self.status = Some(match std::fs::write(&path, text) {
                        Ok(_) => format!("Exported to {}", path.display()),
                        Err(e) => format!("Export failed: {e}"),
                    });
                }
                _ => {}
            }
        }
        let _ = ctx;
    }

    fn act(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::Connect(ds) => self.connect(ctx, ds),
            Action::Disconnect(ds) => self.disconnect(ds),
            Action::Refresh(ds) => {
                if self.src(ds).is_some_and(|s| s.conn.is_some()) {
                    self.send(ds, Req::Databases, Pending::Databases(ds));
                } else {
                    self.connect(ctx, ds);
                }
            }
            Action::RefreshDb(ds, db) => self.send(ds, Req::Collections { db: db.clone() }, Pending::Colls(ds, db)),
            Action::LoadMeta(ds, db, coll) => self.send(ds, Req::Meta { db: db.clone(), coll: coll.clone() }, Pending::Meta(ds, db, coll)),
            Action::OpenData(ds, db, coll) => self.open_data(ctx, ds, db, coll),
            Action::OpenConsole(ds, db) => self.open_console(ctx, ds, db),
            Action::NewConsole(ds, db) => self.new_console(ctx, ds, db),
            Action::Dialog(sel, create) => {
                let items = self.sources.iter().map(|s| s.ds.clone()).collect();
                self.dialog = Some(self.ds_dialog(items, sel, create));
            }
            Action::RemoveSource(ds) => {
                self.disconnect(ds);
                self.sources.retain(|s| s.ds.id != ds);
                self.tabs.retain(|t| t.ds() != Some(ds));
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
                self.save_cfg();
            }
            Action::AskRemoveSource(ds) => {
                let name = self.sources.iter().find(|s| s.ds.id == ds).map_or(String::new(), |s| s.ds.name.clone());
                self.ask = Some(Ask::RemoveSource(ds, name));
            }
            Action::RenameSource(ds) => {
                let name = self.sources.iter().find(|s| s.ds.id == ds).map_or(String::new(), |s| s.ds.name.clone());
                self.ask = Some(Ask::RenameSource(ds, name));
            }
            Action::Drop(ds, db, coll) => self.ask = Some(Ask::Drop(ds, db, coll)),
            Action::Rename(ds, db, coll) => {
                // a PostgreSQL table is renamed by its bare name (the schema stays)
                let name = if self.driver(ds) == Driver::Postgres { pg::split_qual(&coll).1 } else { coll.clone() };
                self.ask = Some(Ask::Rename(ds, db, coll, name))
            }
            Action::Create(ds, db) => self.ask = Some(Ask::Create(ds, db, String::new())),
            Action::Modify(ds, db, coll) => {
                let name = self.sources.iter().find(|s| s.ds.id == ds).map_or(String::new(), |s| s.ds.name.clone());
                self.modify = Some(ModifyDlg::new(ds, db.clone(), coll.clone(), &name));
                self.connect(ctx, ds);
                self.send(ds, Req::Ddl { db: db.clone(), coll: coll.clone() }, Pending::Ddl(ds, db, coll, DdlTo::Modify));
            }
            Action::Ddl(ds, db, coll, to) => {
                if to == DdlTo::Panel {
                    let options = self.sql_gen.as_ref().is_none_or(|g| g.options);
                    self.sql_gen = Some(SqlGen { ds, db: db.clone(), coll: coll.clone(), rel: None, error: None, options });
                    self.show_gen = true;
                }
                self.connect(ctx, ds);
                self.send(ds, Req::Ddl { db: db.clone(), coll: coll.clone() }, Pending::Ddl(ds, db, coll, to));
            }
        }
    }

    fn open_data(&mut self, ctx: &egui::Context, ds: u64, db: String, coll: String) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Data(d) if d.ds == ds && d.db == db && d.coll == coll)) {
            self.active = i;
            return;
        }
        self.connect(ctx, ds);
        let id = self.next_id;
        self.next_id += 1;
        let (filter, page) = if self.driver(ds) == Driver::Postgres { ("", pg::PAGE) } else { ("{}", PAGE) };
        let tab = DataTab { id, ds, db, coll, filter: filter.into(), sort: String::new(), grid: Grid::pending(), page };
        self.tabs.push(Tab::Data(tab));
        self.active = self.tabs.len() - 1;
        self.load_data(id, Some((0, page)));
    }

    /// Jump to the data source's console: an open one, else its first console file, else a new one.
    fn open_console(&mut self, ctx: &egui::Context, ds: u64, db: Option<String>) {
        if db.is_none()
            && let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Console(c) if c.ds == ds))
        {
            self.active = i;
            return;
        }
        let ext = self.driver(ds).ext();
        let path = config::console_files(ds).into_iter().next().unwrap_or_else(|| config::new_console(ds, ext, ""));
        self.open_console_file(ctx, ds, path, db);
    }

    fn new_console(&mut self, ctx: &egui::Context, ds: u64, db: Option<String>) {
        let path = config::new_console(ds, self.driver(ds).ext(), "");
        self.files.reveal_connection(ds);
        self.open_console_file(ctx, ds, path, db);
    }

    fn open_console_file(&mut self, ctx: &egui::Context, ds: u64, path: PathBuf, db: Option<String>) {
        self.connect(ctx, ds);
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Console(c) if c.path == path)) {
            self.active = i;
            return;
        }
        let src = self.sources.iter().find(|s| s.ds.id == ds);
        let pg = src.is_some_and(|s| s.ds.driver == Driver::Postgres);
        let key = path.display().to_string();
        let schema = if pg { self.cfg.console_schema.get(&key).cloned().unwrap_or_else(|| "public".into()) } else { String::new() };
        let db = db
            .or_else(|| self.cfg.console_db.get(&key).cloned())
            .or_else(|| src.filter(|_| pg).map(|s| s.ds.pg_database()))
            .or_else(|| src.map(|s| s.ds.database.clone()).filter(|d| !d.is_empty()))
            .or_else(|| {
                src.and_then(|s| s.dbs.as_ref())
                    .and_then(|d| d.iter().find(|d| !["admin", "config", "local"].contains(&d.name.as_str())))
                    .map(|d| d.name.clone())
            })
            .unwrap_or_else(|| "test".into());
        let id = self.next_id;
        self.next_id += 1;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let title = path.file_stem().map_or("console".into(), |s| s.to_string_lossy().into_owned());
        self.tabs.push(Tab::Console(Console {
            id,
            ds,
            saved: text.clone(),
            path,
            last_save: Instant::now(),
            title,
            db,
            schema,
            text,
            marks: vec![],
            running: false,
            started: false,
            mode: Mode::Playground,
            line_col: (0, 0),
        }));
        self.active = self.tabs.len() - 1;
        self.focus_console = Some((id, None));
    }

    fn open_file(&mut self, path: PathBuf) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::File(f) if f.path == path)) {
            self.active = i;
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let id = self.next_id;
                self.next_id += 1;
                self.tabs.push(Tab::File(FileTab { id, path, saved: text.clone(), text, last_save: Instant::now(), line_col: (0, 0) }));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => self.status = Some(format!("Cannot open {}: {e}", path.display())),
        }
    }

    fn file_action(&mut self, ctx: &egui::Context, a: FileAction) {
        match a {
            FileAction::OpenConsole(ds, p) => self.open_console_file(ctx, ds, p, None),
            FileAction::NewConsole(ds) => self.new_console(ctx, ds, None),
            FileAction::OpenFile(p) => self.open_file(p),
            FileAction::Rename(p) => {
                // consoles are renamed by name only (the IDE hides their extension); files show it
                let console = self.sources.iter().any(|s| p.starts_with(config::consoles_dir(s.ds.id)));
                let name = if console { p.file_stem() } else { p.file_name() }.map_or(String::new(), |n| n.to_string_lossy().into_owned());
                self.ask = Some(Ask::RenameFile(p, name));
            }
            FileAction::Delete(p) => self.ask = Some(Ask::DeleteFile(p)),
            FileAction::Reveal(p) => {
                if !p.exists() && p.extension().is_none() {
                    let _ = std::fs::create_dir_all(&p); // a connection's consoles folder may not exist yet
                }
                config::reveal(&p);
            }
            FileAction::Attach => {
                // native folder picker without blocking the UI thread
                let (tx, rx) = channel();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(config::pick_folder("Attach Directory"));
                    ctx2.request_repaint();
                });
                self.picker = Some(rx);
            }
            FileAction::Detach(d) => {
                self.cfg.attached.retain(|x| *x != d);
                self.save_cfg();
            }
            FileAction::NewScratch(ext) => {
                let text = if ext == "json" { "{\n}\n" } else { "" };
                let p = config::new_file(&config::scratches_dir(), "scratch", ext, text);
                self.open_file(p);
            }
            FileAction::Hide => self.show_files = false,
        }
    }

    fn run_console(&mut self, ctx: &egui::Context, idx: usize, cursor: Option<std::ops::Range<usize>>, new_tab: bool) {
        let Some(Tab::Console(c)) = self.tabs.get_mut(idx) else {
            return;
        };
        if c.running {
            return;
        }
        let byte = |ci: usize| c.text.char_indices().nth(ci).map_or(c.text.len(), |(b, _)| b);
        let sql = c.path.extension().is_some_and(|e| e == "sql");
        let split = if sql { pg::split_sql } else { split_statements };
        let (start, script) = match (&cursor, c.mode) {
            (Some(r), _) if r.start != r.end => (byte(r.start), c.text[byte(r.start)..byte(r.end)].to_string()),
            (_, Mode::Script) => (0, c.text.clone()),
            _ => {
                let caret = byte(cursor.map_or(0, |r| r.end));
                let stmts = split(&c.text);
                let hit = stmts
                    .iter()
                    .find(|&&(a, b)| caret >= a && caret <= b)
                    .or_else(|| stmts.iter().rev().find(|&&(a, _)| a <= caret))
                    .or(stmts.first());
                match hit {
                    Some(&(a, b)) => {
                        let lead = c.text[a..b].len() - c.text[a..b].trim_start().len();
                        (a + lead, c.text[a..b].trim().to_string())
                    }
                    None => return,
                }
            }
        };
        if script.trim().is_empty() {
            return;
        }
        let line = c.text[..start].matches('\n').count();
        c.marks.retain(|(l, _)| *l != line);
        c.marks.push((line, Mark::Ok));
        c.running = true;
        for (a, b) in split(&script) {
            if let Some(db) = script[a..b].trim().strip_prefix("use ").filter(|_| !sql) {
                c.db = db.trim().trim_end_matches(';').to_string();
            }
        }
        let (id, ds, db, schema) = (c.id, c.ds, c.db.clone(), c.schema.clone());
        config::push_history(ds, script.trim());
        if !c.started && !sql {
            c.started = true;
            self.log(stamp(&format!("{db}> use {db}")), LogKind::Echo);
            self.log(stamp("completed in 0 ms"), LogKind::Info);
        }
        if let Some(t) = self.tabs.get_mut(idx) {
            t.flush(true);
            if let Tab::Console(c) = t {
                remember_console(&mut self.cfg, c);
            }
        }
        // Execute (re)connects the console's data source, like DataGrip
        self.connect(ctx, ds);
        self.send(ds, Req::Run { db, schema, script }, Pending::Run(id, ds, std::time::Instant::now(), new_tab));
    }

    fn save_cfg(&mut self) {
        self.cfg.sources = self.sources.iter().map(|s| s.ds.clone()).collect();
        for t in &mut self.tabs {
            t.flush(true);
            if let Tab::Console(c) = t {
                remember_console(&mut self.cfg, c);
            }
        }
        config::save_project(&self.project.path, &self.cfg);
    }

    // ------------------------------------------------------------ layout

    fn header(&mut self, ui: &mut Ui, rect: Rect) {
        // project-coloured gradient on the left of the main toolbar (RecentProject.ColorN.MainToolbarGradientStart)
        let tint = project_toolbar_color(self.project.color).gamma_multiply(0.85);
        let mut mesh = egui::Mesh::default();
        let g = Rect::from_min_size(rect.min, vec2(600.0_f32.min(rect.width()), rect.height()));
        mesh.colored_vertex(g.left_top(), tint);
        mesh.colored_vertex(g.right_top(), Color32::TRANSPARENT);
        mesh.colored_vertex(g.left_bottom(), tint);
        mesh.colored_vertex(g.right_bottom(), Color32::TRANSPARENT);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);

        let drag = ui.interact(rect, Id::new("header-drag"), Sense::CLICK | Sense::DRAG);
        // the toolbar controls are not title bar (IntelliJ's toolbar components take the events): a press
        // on a button that drifts, or a fast click landing just beside it (egui counts clicks within
        // 6 px as a double click), must not move or zoom the window. Solid areas over the header, under
        // the controls, padded by that distance.
        let name_w = ui.fonts_mut(|f| f.layout_no_wrap(self.project.name.clone(), ui_font(13.0), TEXT).size().x) + 56.0;
        let pad = 6.0;
        for (i, (x0, x1)) in [
            (rect.left() + 80.0, rect.left() + 80.0 + name_w),
            (rect.center().x - 56.0, rect.center().x + 56.0),
            (rect.right() - 8.0 - 26.0, rect.right() - 8.0),
        ]
        .into_iter()
        .enumerate()
        {
            let r = Rect::from_x_y_ranges(x0 - pad..=x1 + pad, rect.y_range());
            ui.interact(r, Id::new(("header-solid", i)), Sense::CLICK | Sense::DRAG);
        }
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        // project widget: badge + name ⌄ → New Project… / Open… / Open Projects / Recent Projects
        row_ui(ui, Rect::from_min_max(rect.min + vec2(80.0, 6.0), pos2(rect.left() + 420.0, rect.bottom() - 6.0)), |ui| {
            let name = self.project.name.clone();
            let w = ui.fonts_mut(|f| f.layout_no_wrap(name.clone(), ui_font(13.0), TEXT).size().x) + 56.0;
            let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::CLICK);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Project"));
            if resp.hovered() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp)) {
                ui.painter().rect_filled(r, CornerRadius::same(6), TOOLBAR_HOVER);
            }
            project_badge(ui, Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(20.0, 20.0)), &name, self.project.color);
            text_at(ui, pos2(r.left() + 31.0, r.center().y), Align2::LEFT_CENTER, &name, ui_font(13.0), TEXT);
            chevron_down(ui, pos2(r.right() - 10.0, r.center().y), TEXT_MUTED);
            let mut cmd = None;
            let mut open_dialog = false;
            egui::Popup::menu(&resp).align(egui::RectAlign::BOTTOM_START).show(|ui| {
                ui.set_min_width(200.0);
                if menu_item(ui, Some(ic::ADD), "New Project…", "", true) {
                    cmd = Some(ProjectCmd::New);
                }
                if menu_item(ui, Some(ic::FOLDER_GRAY), "Open…", "", true) {
                    cmd = Some(ProjectCmd::Open);
                }
                let open: Vec<&ProjectEntry> = self.shell.iter().filter(|e| e.open).collect();
                let recent: Vec<&ProjectEntry> = self.shell.iter().filter(|e| !e.open).collect();
                for (title, list) in [("Open Projects", open), ("Recent Projects", recent)] {
                    if list.is_empty() {
                        continue;
                    }
                    ui.separator();
                    menu_section(ui, title);
                    for e in list {
                        match project_row(ui, e) {
                            Some(RowAction::Open) if !e.active => cmd = Some(ProjectCmd::Switch(e.project.path.clone())),
                            Some(RowAction::Remove) => cmd = Some(ProjectCmd::RemoveRecent(e.project.path.clone())),
                            _ => {}
                        }
                    }
                }
                ui.separator();
                open_dialog = menu_item(ui, Some(ic::DS_PROPERTIES), "Data Sources and Drivers…", "⌘;", true);
                let many = self.shell.iter().filter(|e| e.open).count() > 1;
                if menu_item(ui, None, "Close Project", "", many) {
                    cmd = Some(ProjectCmd::Close(self.project.path.clone()));
                }
            });
            if cmd.is_some() {
                self.cmd = cmd;
            }
            if open_dialog {
                self.act(ui.ctx(), Action::Dialog(None, None));
            }
        });
        // center: tool window toggles
        let cx = rect.center().x;
        row_ui(ui, Rect::from_center_size(pos2(cx, rect.center().y), vec2(3.0 * 32.0 + 2.0 * 8.0, 34.0)), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if toolbar_toggle(ui, ic::DATA_SCHEMA, "Database Explorer  ⌘1", self.show_explorer).clicked() {
                self.show_explorer = !self.show_explorer;
            }
            if toolbar_toggle(ui, ic::SERVICES, "Services  ⌘8", self.show_services && !self.show_gen).clicked() {
                self.toggle_services();
            }
            if toolbar_toggle(ui, ic::FOLDER_GRAY, "Files  ⌘2", self.show_files).clicked() {
                self.show_files = !self.show_files;
            }
        });
        row_ui(ui, Rect::from_min_max(pos2(rect.right() - 80.0, rect.top() + 6.0), rect.max - vec2(8.0, 6.0)), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, ic::SEARCH, "Search Everywhere  ⌘⇧O").clicked() {
                    self.search = Some(String::new());
                }
            });
        });
    }

    fn explorer(&mut self, ui: &mut Ui, rect: Rect) -> Vec<Action> {
        let mut acts = Vec::new();
        island(ui, rect);
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        tool_header(ui, head, "Database Explorer");
        row_ui(ui, Rect::from_min_max(pos2(head.right() - 150.0, head.top() + 7.0), head.right_bottom() - vec2(8.0, 7.0)), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, ic::HIDE, "Hide  ⇧⎋").clicked() {
                    self.show_explorer = false;
                }
                icon_btn(ui, ic::MORE_VERTICAL, "Options");
                if icon_btn(ui, ic::COLLAPSE_ALL, "Collapse All  ⌘-").clicked() {
                    self.expanded.clear();
                }
                if icon_btn(ui, ic::EXPAND_ALL, "Expand All  ⌘+").clicked() {
                    // data sources, databases, schemas and folders (not the collections themselves)
                    let keys: Vec<String> = self
                        .tree_model(true)
                        .into_iter()
                        .filter(|r| r.open.is_some() && ["ds:", "db:", "sch:", "colls:"].iter().any(|p| r.key.starts_with(p)))
                        .map(|r| r.key)
                        .collect();
                    self.expanded.extend(keys);
                }
                icon_btn_ex(ui, ic::LOCATE, "Select Opened Element", false, false, false);
            });
        });
        // toolbar
        let sel = self.selection();
        let tb = Rect::from_min_size(rect.min + vec2(8.0, 40.0), vec2(rect.width() - 16.0, 32.0));
        row_ui(ui, tb, |ui| {
            let plus = icon_btn_ex(ui, ic::ADD, "New", true, false, true);
            let mongo = sel.ds.is_some_and(|ds| self.driver(ds) == Driver::Mongo);
            egui::Popup::menu(&plus).show(|ui| new_menu(ui, &mut acts, sel.ds, sel.db.as_ref(), mongo, false));
            if icon_btn(ui, ic::DS_PROPERTIES, "Data Source Properties  ⌘;").clicked() {
                acts.push(Action::Dialog(sel.ds, None));
            }
            vsep(ui);
            if icon_btn_ex(ui, ic::REFRESH, "Refresh  ⌘R", sel.ds.is_some(), false, false).clicked() {
                if let (Some(ds), Some(db)) = (sel.ds, sel.db.clone()) {
                    acts.push(Action::RefreshDb(ds, db));
                } else if let Some(ds) = sel.ds {
                    acts.push(Action::Refresh(ds));
                }
            }
            let connected = sel.ds.is_some_and(|ds| self.sources.iter().any(|s| s.ds.id == ds && s.conn.is_some()));
            if icon_btn_ex(ui, ic::DISCONNECT, "Disconnect  ⌘F2", connected, false, false).clicked() {
                acts.push(Action::Disconnect(sel.ds.unwrap()));
            }
            vsep(ui);
            if icon_btn_ex(ui, ic::CONSOLE, "Jump to Query Console…  ⌘⇧F10", sel.ds.is_some(), false, false).clicked() {
                acts.push(Action::OpenConsole(sel.ds.unwrap(), sel.db.clone()));
            }
            if icon_btn_ex(ui, ic::TABLE, "Edit Data  ⌘↓", sel.coll.is_some(), false, false).clicked() {
                acts.push(Action::OpenData(sel.ds.unwrap(), sel.db.clone().unwrap(), sel.coll.clone().unwrap()));
            }
            text_btn(ui, "DDL", "Go to DDL  ⌘B (no source text for MongoDB)", false);
            icon_btn_ex(ui, ic::JUMP, "Jump to Console", false, false, false);
            vsep(ui);
            icon_btn_ex(ui, ic::SHOW, "View Options", true, false, true);
        });
        let tree = Rect::from_min_max(rect.min + vec2(0.0, 74.0), rect.max - vec2(0.0, 6.0));
        rect_ui(ui, tree, |ui| {
            let mut area = egui::ScrollArea::vertical().id_salt("explorer").auto_shrink([false, false]);
            if let Some(y) = self.tree_scroll_y.take() {
                // keep the revealed node in view, centred when it was outside
                let view = tree.height();
                let cur = egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id("explorer")).map_or(0.0, |st| st.offset.y);
                if y < cur + ROW_H || y > cur + view - ROW_H {
                    area = area.vertical_scroll_offset((y - view / 2.0).max(0.0));
                }
            }
            area.show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if self.sources.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Create a data source with ⌘N").color(TEXT_SECONDARY));
                        if ui.link("Create Data Source").clicked() {
                            acts.push(Action::Dialog(None, Some(Driver::Mongo)));
                        }
                    });
                }
                let q = self.speed.clone().unwrap_or_default();
                ui.data_mut(|d| d.insert_temp(Id::new("explorer-speed"), q));
                self.tree_rows(ui, &mut acts);
                ui.data_mut(|d| d.remove::<String>(Id::new("explorer-speed")));
            });
        });
        if ui.input(|i| i.pointer.any_pressed()) && !ui.rect_contains_pointer(tree) {
            self.tree_focus = false;
        }
        if !self.tree_focus {
            self.speed = None;
        }
        if self.tree_focus && ui.memory(|m| m.focused().is_none()) {
            self.speed_search_input(ui);
            self.tree_keys(ui, &mut acts);
            if self.speed.is_some() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.speed = None;
            }
        }
        if let Some(q) = &self.speed {
            speed_search_popup(ui, tree, q, !self.speed_matches(q).is_empty());
        }
        if let Some((ds, pos)) = self.schema_popup {
            self.schema_picker(ui.ctx(), ds, pos);
        }
        acts
    }

    /// Loaded explorer nodes in display order (expanded or not): (key, label, ancestor keys).
    fn tree_index(&self) -> Vec<(String, String, Vec<String>)> {
        self.tree_model(true).into_iter().filter(|r| !r.info).map(|r| (r.key, r.label, r.anc)).collect()
    }

    /// Database Explorer rows in display order; `all` lists every loaded node, expanded or not
    /// (speed search), without the "Loading…" placeholders.
    fn tree_model(&self, all: bool) -> Vec<TreeRow> {
        let mut out = vec![];
        let open = |k: &str| all || self.expanded.contains(k);
        for s in &self.sources {
            let id = s.ds.id;
            let pg = s.ds.driver == Driver::Postgres;
            let k = format!("ds:{id}");
            let badge = match (&s.state, &s.dbs) {
                (ConnState::Connected, Some(d)) => {
                    Some(if s.ds.schemas.is_some() { format!("{} of {}", d.len(), s.all_dbs.len()) } else { d.len().to_string() })
                }
                _ => None,
            };
            out.push(TreeRow {
                extra: NodeExtra {
                    badge,
                    spinner: matches!(s.state, ConnState::Connecting),
                    dot: s.conn.is_some() && matches!(s.state, ConnState::Connected),
                    error: matches!(s.state, ConnState::Failed(_)),
                },
                load: Some(Action::Connect(id)),
                menu: RowMenu::Ds(id),
                tip: if let ConnState::Failed(e) = &s.state { Some(e.clone()) } else { None },
                ..TreeRow::new(k.clone(), 0, Some(self.expanded.contains(&k)), driver_icon(s.ds.driver), &s.ds.name, &[])
            });
            if !open(&k) {
                continue;
            }
            if !all && s.dbs.as_ref().is_some_and(|d| d.is_empty()) {
                out.push(TreeRow::info(1, "No schemas selected"));
            }
            for db in s.dbs.iter().flatten() {
                let a1 = vec![k.clone()];
                let dk = format!("db:{id}:{}", db.name);
                out.push(TreeRow {
                    load: db.colls.is_none().then(|| Action::RefreshDb(id, db.name.clone())),
                    menu: RowMenu::Db(id, db.name.clone()),
                    ..TreeRow::new(
                        dk.clone(),
                        1,
                        Some(self.expanded.contains(&dk)),
                        if pg { ic::DATABASE } else { ic::MONGO_DB },
                        &db.name,
                        &a1,
                    )
                });
                if !open(&dk) {
                    continue;
                }
                let Some(colls) = &db.colls else {
                    if !all {
                        out.push(TreeRow::info(2, "Loading…"));
                    }
                    continue;
                };
                let a2 = vec![k.clone(), dk.clone()];
                if !pg {
                    let all_colls: Vec<&CollNode> = colls.iter().collect();
                    self.folder_rows(
                        &mut out,
                        all,
                        (id, &db.name),
                        format!("colls:{id}:{}", db.name),
                        "collections",
                        2,
                        &a2,
                        &all_colls,
                        ic::DATA_TABLES,
                    );
                    continue;
                }
                // PostgreSQL: schema → tables / views / materialized views / sequences
                for schema in &db.schemas {
                    let sk = format!("sch:{id}:{}:{schema}", db.name);
                    out.push(TreeRow {
                        menu: RowMenu::Schema(id, db.name.clone()),
                        ..TreeRow::new(sk.clone(), 2, Some(self.expanded.contains(&sk)), ic::SCHEMA, schema, &a2)
                    });
                    if !open(&sk) {
                        continue;
                    }
                    let a3 = vec![k.clone(), dk.clone(), sk.clone()];
                    for (kind, label, icon) in [
                        ('r', "tables", ic::DATA_TABLES),
                        ('v', "views", ic::VIEW),
                        ('m', "materialized views", ic::VIEW),
                        ('S', "sequences", ic::SEQUENCE),
                    ] {
                        let of: Vec<&CollNode> = colls.iter().filter(|c| c.schema == *schema && c.kind == kind).collect();
                        if !of.is_empty() {
                            self.folder_rows(
                                &mut out,
                                all,
                                (id, &db.name),
                                format!("colls:{id}:{}:{schema}:{label}", db.name),
                                label,
                                3,
                                &a3,
                                &of,
                                icon,
                            );
                        }
                    }
                }
            }
        }
        out
    }

    /// A `collections` / `tables` folder, its collections and their `fields` / `columns` / `keys` … folders.
    #[allow(clippy::too_many_arguments)]
    fn folder_rows(
        &self,
        out: &mut Vec<TreeRow>,
        all: bool,
        (id, db): (u64, &str),
        key: String,
        label: &str,
        depth: usize,
        anc: &[String],
        colls: &[&CollNode],
        icon: ImageSource<'static>,
    ) {
        let open = |k: &str| all || self.expanded.contains(k);
        let fopen = open(&key);
        out.push(TreeRow {
            suffix: Some(colls.len().to_string()),
            ..TreeRow::new(key.clone(), depth, Some(self.expanded.contains(&key)), ic::FOLDER, label, anc)
        });
        if !fopen {
            return;
        }
        let mut a = anc.to_vec();
        a.push(key);
        for c in colls {
            let q = c.qual();
            let ck = format!("coll:{id}:{db}:{q}");
            let leaf = c.kind == 'S';
            out.push(TreeRow {
                load: (c.meta.is_none() && !leaf).then(|| Action::LoadMeta(id, db.into(), q.clone())),
                activate: (!leaf).then(|| Action::OpenData(id, db.into(), q.clone())),
                menu: RowMenu::Coll(id, db.into(), q.clone(), c.kind),
                tip: (c.kind == 'v' && c.schema.is_empty()).then(|| "view".into()),
                ..TreeRow::new(ck.clone(), depth + 1, (!leaf).then(|| self.expanded.contains(&ck)), icon.clone(), &c.name, &a)
            });
            if leaf || !open(&ck) {
                continue;
            }
            let Some(meta) = &c.meta else {
                if !all {
                    out.push(TreeRow::info(depth + 2, "Loading…"));
                }
                continue;
            };
            let mut a2 = a.clone();
            a2.push(ck);
            for (label, items) in meta {
                let sk = format!("sec:{id}:{db}:{q}:{label}");
                out.push(TreeRow {
                    suffix: Some(items.len().to_string()),
                    ..TreeRow::new(sk.clone(), depth + 2, Some(self.expanded.contains(&sk)), ic::FOLDER, label, &a2)
                });
                if !open(&sk) {
                    continue;
                }
                let mut a3 = a2.clone();
                a3.push(sk);
                for it in items {
                    let ik = format!("o:{id}:{db}:{q}:{label}:{}", it.name);
                    out.push(TreeRow {
                        suffix: Some(it.detail.clone()).filter(|d| !d.is_empty()),
                        ..TreeRow::new(ik, depth + 3, None, item_icon(it.kind), &it.name, &a3)
                    });
                }
            }
        }
    }

    /// Positions in `tree_index()` whose label contains the query (case-insensitive).
    fn speed_matches(&self, q: &str) -> Vec<usize> {
        let q = q.to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        self.tree_index().iter().enumerate().filter(|(_, (_, l, _))| l.to_lowercase().contains(&q)).map(|(i, _)| i).collect()
    }

    /// Selects a match: `dir` 0 = first at/after the selection, 1 = next, -1 = previous (wrapping),
    /// expanding its ancestors and scrolling it into view.
    fn speed_jump(&mut self, dir: i32) {
        let Some(q) = self.speed.clone() else { return };
        let idx = self.tree_index();
        let m = self.speed_matches(&q);
        if m.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|k| idx.iter().position(|(key, _, _)| key == k)).unwrap_or(0);
        let pick = match dir {
            0 => m.iter().copied().find(|&i| i >= cur).unwrap_or(m[0]),
            1 => m.iter().copied().find(|&i| i > cur).unwrap_or(m[0]),
            _ => m.iter().rev().copied().find(|&i| i < cur).unwrap_or(*m.last().unwrap()),
        };
        let (key, _, anc) = &idx[pick];
        self.expanded.extend(anc.iter().cloned());
        self.selected = Some(key.clone());
        self.scroll_tree = true;
    }

    /// IntelliJ speed search: ⌘F or any typed character opens it; ↑/↓ move between matches,
    /// ⌫ edits, Esc closes (Enter is left to the tree and closes it afterwards).
    fn speed_search_input(&mut self, ui: &Ui) {
        let active = self.speed.is_some();
        let (open, typed, back, esc, up, down) = ui.input_mut(|i| {
            let open = i.consume_key(egui::Modifiers::COMMAND, Key::F);
            let mut typed = String::new();
            i.events.retain(|e| match e {
                egui::Event::Text(t) if !t.chars().any(char::is_control) && (active || !t.trim().is_empty()) => {
                    typed.push_str(t);
                    false
                }
                _ => true,
            });
            let on = active || open || !typed.is_empty();
            (
                open,
                typed,
                on && i.consume_key(egui::Modifiers::NONE, Key::Backspace),
                active && i.consume_key(egui::Modifiers::NONE, Key::Escape),
                active && i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                active && i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            )
        });
        if esc {
            self.speed = None;
            return;
        }
        if open || !typed.is_empty() || back {
            let q = self.speed.get_or_insert_with(String::new);
            q.push_str(&typed);
            if back {
                q.pop();
            }
            self.speed_jump(0);
        }
        if up {
            self.speed_jump(-1);
        }
        if down {
            self.speed_jump(1);
        }
    }

    /// Database Explorer keyboard navigation: ↑↓ select, → expand, ← collapse, ↩ / ⌘↓ open data.
    fn tree_keys(&mut self, ui: &Ui, acts: &mut Vec<Action>) {
        let (up, down, left, right, enter, edit) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown) && !i.modifiers.command,
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Enter),
                (i.modifiers.command && i.key_pressed(Key::ArrowDown)) || i.key_pressed(Key::F4),
            )
        });
        let Some(cur) = self.selected.clone() else {
            if down {
                self.selected = self.visible.first().map(|v| v.key.clone());
            }
            return;
        };
        let idx = self.visible.iter().position(|v| v.key == cur);
        if up && let Some(i) = idx.filter(|&i| i > 0) {
            self.selected = Some(self.visible[i - 1].key.clone());
        }
        if down && let Some(i) = idx.filter(|&i| i + 1 < self.visible.len()) {
            self.selected = Some(self.visible[i + 1].key.clone());
        }
        let Some(row) = idx.map(|i| self.visible[i].clone()) else { return };
        let open = self.expanded.contains(&cur);
        let toggle_enter = enter && row.activate.is_none();
        if (right || toggle_enter) && row.expandable && !open {
            self.expanded.insert(cur.clone());
            acts.extend(row.load);
        } else if right && open {
            if let Some(i) = idx.filter(|&i| i + 1 < self.visible.len()) {
                self.selected = Some(self.visible[i + 1].key.clone());
            }
        } else if (left || toggle_enter) && open {
            self.expanded.remove(&cur);
        }
        if enter || edit {
            acts.extend(row.activate);
        }
    }

    fn tree_rows(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let scroll_to = if std::mem::take(&mut self.scroll_tree) { self.selected.clone() } else { None };
        // content-space y of the node to reveal (applied as a scroll offset next frame)
        let top0 = ui.cursor().top();
        let mut scroll_y: Option<f32> = None;
        let mut vis: Vec<VisRow> = vec![];
        let mut toggles: Vec<(String, Option<Action>)> = vec![];
        let mut select: Option<String> = None;
        let mut badge_click: Option<(u64, egui::Pos2)> = None;
        for row in self.tree_model(false) {
            if row.info {
                leaf_text(ui, row.depth, &row.label);
                continue;
            }
            let r = node(ui, &self.selected, row.depth, &row.key, row.open, row.icon.clone(), &row.label, row.suffix.clone(), row.extra);
            vis.push(VisRow {
                key: row.key.clone(),
                expandable: row.open.is_some(),
                load: row.load.clone(),
                activate: row.activate.clone(),
            });
            if scroll_to.as_deref() == Some(row.key.as_str()) {
                scroll_y = Some(r.row.rect.center().y - top0);
            }
            if let Some(t) = &row.tip {
                r.row.clone().on_hover_text(t);
            }
            if let (Some(p), RowMenu::Ds(id)) = (r.badge_clicked, &row.menu) {
                badge_click = Some((*id, p));
            }
            let open = row.open == Some(true);
            if r.toggle || (r.row.double_clicked() && row.activate.is_none() && row.open.is_some()) {
                toggles.push((row.key.clone(), if open { None } else { row.load.clone() }));
            }
            if r.row.clicked() {
                select = Some(row.key.clone());
            }
            if r.row.double_clicked() {
                acts.extend(row.activate.clone());
            }
            match &row.menu {
                RowMenu::None => {}
                RowMenu::Ds(id) => {
                    let id = *id;
                    let connected = self.sources.iter().any(|s| s.ds.id == id && s.conn.is_some());
                    let mongo = self.driver(id) == Driver::Mongo;
                    // DataGrip's data source menu; what DataTired doesn't have yet is disabled
                    context_menu(&r.row, |ui| {
                        ui.set_min_width(260.0);
                        let sub = |ui: &mut Ui, t: &str| submenu(ui, None, t, false, |_| {});
                        if menu_item(ui, Some(ic::DS_PROPERTIES), "Properties", "⌘;", true) {
                            acts.push(Action::Dialog(Some(id), None));
                        }
                        ui.separator();
                        submenu(ui, Some(ic::ADD), "New", true, |ui| new_menu(ui, acts, Some(id), None, mongo, true));
                        sub(ui, "Copy/Paste");
                        if menu_item(ui, Some(ic::REFRESH), "Refresh", "⌘R", true) {
                            acts.push(Action::Refresh(id));
                        }
                        if menu_item(ui, Some(ic::DISCONNECT), "Deactivate", "⌘F2", connected) {
                            acts.push(Action::Disconnect(id));
                        }
                        ui.separator();
                        if menu_item(ui, None, "Rename…", "⇧F6", true) {
                            acts.push(Action::RenameSource(id));
                        }
                        sub(ui, "Move");
                        ui.separator();
                        menu_item(ui, None, "Quick Documentation", "F1", false);
                        menu_item(ui, Some(ic::JUMP), "Compare Schema Structure", "⌘D", false);
                        sub(ui, "Navigation");
                        ui.separator();
                        for t in ["SQL Scripts", "Object Actions", "Tools", "Import/Export", "Diagrams", "DDL Mapping"] {
                            sub(ui, t);
                        }
                        ui.separator();
                        sub(ui, "Diagnostics");
                        ui.separator();
                        if menu_item(ui, None, "Remove Data Source…", "⌦", true) {
                            acts.push(Action::AskRemoveSource(id));
                        }
                    });
                }
                RowMenu::Db(id, db) | RowMenu::Schema(id, db) => {
                    let mongo = self.driver(*id) == Driver::Mongo;
                    let schema = matches!(row.menu, RowMenu::Schema(..));
                    context_menu(&r.row, |ui| {
                        ui.set_min_width(180.0);
                        if mongo && menu_item(ui, Some(ic::NEW_COLLECTION), "New Collection…", "", true) {
                            acts.push(Action::Create(*id, db.clone()));
                        }
                        if menu_item(ui, Some(ic::CONSOLE), "New Query Console", "⌘⇧L", true) {
                            acts.push(Action::NewConsole(*id, Some(db.clone())));
                        }
                        if menu_item(ui, Some(ic::REFRESH), "Refresh", "⌘R", true) {
                            acts.push(Action::RefreshDb(*id, db.clone()));
                        }
                        if mongo && !schema {
                            ui.separator();
                            if menu_item(ui, Some(ic::DELETE), "Drop…", "⌦", true) {
                                acts.push(Action::Drop(*id, db.clone(), None));
                            }
                        }
                    });
                }
                RowMenu::Coll(id, db, coll, kind) => {
                    let driver = self.driver(*id);
                    context_menu(&r.row, |ui| coll_menu(ui, acts, *id, db, coll, driver, *kind));
                }
            }
        }
        self.visible = vis;
        self.tree_scroll_y = scroll_y;
        if let Some(s) = select {
            self.selected = Some(s);
            self.tree_focus = true;
        }
        for (k, a) in toggles {
            if !self.expanded.remove(&k) {
                self.expanded.insert(k);
            }
            if let Some(a) = a {
                acts.push(a);
            }
        }
        if let Some(b) = badge_click {
            self.schema_popup = Some(b);
        }
    }

    fn schema_picker(&mut self, ctx: &egui::Context, ds: u64, pos: egui::Pos2) {
        let Some(s) = self.sources.iter_mut().find(|s| s.ds.id == ds) else {
            return;
        };
        let mut close = false;
        let area = egui::Area::new(Id::new("schema-picker")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(8).show(ui, |ui| {
                ui.set_width(240.0);
                ui.label(egui::RichText::new(format!("{} databases", s.all_dbs.len())).color(TEXT_SECONDARY));
                ui.separator();
                let mut all = s.ds.schemas.is_none();
                if ui.checkbox(&mut all, "All databases").changed() {
                    s.ds.schemas = if all { None } else { Some(vec![]) };
                }
                for n in s.all_dbs.clone() {
                    let mut on = s.ds.schemas.as_ref().is_none_or(|v| v.contains(&n));
                    if ui.checkbox(&mut on, &n).changed() {
                        let mut v = s.ds.schemas.clone().unwrap_or_else(|| s.all_dbs.clone());
                        if on {
                            v.push(n.clone());
                        } else {
                            v.retain(|x| x != &n);
                        }
                        s.ds.schemas = (v.len() != s.all_dbs.len()).then_some(v);
                    }
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Press Enter or click outside the list to apply").size(11.5).color(TEXT_SECONDARY));
            });
        });
        let outside = ctx.input(|i| i.pointer.any_click())
            && !area.response.contains_pointer()
            && !ctx.input(|i| i.pointer.primary_down() && area.response.rect.contains(i.pointer.interact_pos().unwrap_or_default()));
        if ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape))
            || (outside && area.response.rect.width() > 0.0 && !area.response.hovered())
        {
            close = true;
        }
        if close {
            self.schema_popup = None;
            let names = s.all_dbs.clone();
            let keep = s.ds.schemas.clone();
            let old: HashMap<String, DbNode> = s.dbs.take().unwrap_or_default().into_iter().map(|d| (d.name.clone(), d)).collect();
            let mut old = old;
            s.dbs = Some(
                names
                    .into_iter()
                    .filter(|n| keep.as_ref().is_none_or(|v| v.contains(n)))
                    .map(|n| old.remove(&n).unwrap_or(DbNode { name: n, schemas: vec![], colls: None }))
                    .collect(),
            );
            self.save_cfg();
        }
    }

    fn selection(&self) -> Sel {
        let mut s = Sel::default();
        let Some(k) = &self.selected else {
            s.ds = self.sources.first().map(|x| x.ds.id);
            return s;
        };
        let parts: Vec<&str> = k.splitn(5, ':').collect();
        s.ds = parts.get(1).and_then(|x| x.parse().ok());
        if parts[0] != "ds" {
            s.db = parts.get(2).map(|x| x.to_string());
        }
        if ["coll", "sec", "o"].contains(&parts[0]) {
            s.coll = parts.get(3).map(|x| x.to_string());
        }
        s
    }

    fn editor_area(&mut self, ui: &mut Ui, rect: Rect) {
        island(ui, rect);
        if self.tabs.is_empty() {
            let hints = [
                ("Manage Data Sources", "⌘;"),
                ("Database Explorer", "⌘1"),
                ("Files View", "⌘2"),
                ("Services", "⌘8"),
                ("New Query Console", "⌘⇧L"),
                ("Search Everywhere", "⌘⇧O"),
            ];
            let top = rect.center().y - hints.len() as f32 * 18.0;
            let left = rect.center().x - 120.0;
            for (i, (t, k)) in hints.iter().enumerate() {
                let y = top + i as f32 * 36.0;
                let r = text_at(ui, pos2(left, y), Align2::LEFT_CENTER, t, ui_font(15.0), TEXT_SECONDARY);
                text_at(ui, pos2(r.right() + 10.0, y), Align2::LEFT_CENTER, k, ui_font(15.0), TEXT_MUTED);
            }
            return;
        }
        // tab strip
        let strip = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        let mut close = None;
        let mut clicked = None;
        let titles = self.tab_titles();
        // tabs overflowing the strip scroll horizontally (wheel or trackpad), clipped short of the ⋮ button
        let galleys: Vec<_> = titles.iter().map(|(_, t)| ui.painter().layout_no_wrap(t.clone(), ui_font(13.0), TEXT)).collect();
        let total = galleys.iter().map(|g| g.size().x + 66.0).sum::<f32>() - 6.0;
        let area = Rect::from_min_max(pos2(strip.left() + 8.0, strip.top()), strip.right_bottom() - vec2(44.0, 0.0));
        let scroll = egui::ScrollArea::horizontal().id_salt("editor-tabs").auto_shrink([false, false]);
        rect_ui(ui, area, |ui| {
            // thin, muted bar in the gap under the tabs
            let s = &mut ui.spacing_mut().scroll;
            (s.bar_width, s.bar_outer_margin, s.foreground_color) = (4.0, 1.0, false);
            scroll.show(ui, |ui| {
                let (row, _) = ui.allocate_exact_size(vec2(total, 40.0), Sense::hover());
                let mut x = row.left();
                for (i, t) in self.tabs.iter().enumerate() {
                    let (icon, title) = titles[i].clone();
                    let g = galleys[i].clone();
                    let w = g.size().x + 60.0;
                    let r = Rect::from_min_size(pos2(x, row.top() + 6.0), vec2(w, 28.0));
                    let resp = ui.interact(r, Id::new(("tab", t.id())), Sense::CLICK);
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, i == self.active, &title));
                    if i == self.active {
                        // bring a newly activated tab into view
                        let key = Id::new("editor-tabs-shown");
                        if ui.data(|d| d.get_temp::<u64>(key)) != Some(t.id()) {
                            ui.data_mut(|d| d.insert_temp(key, t.id()));
                            resp.scroll_to_me(None);
                        }
                        ui.painter().rect(r, CornerRadius::same(5), TAB_SEL_BG, Stroke::new(1.0, TAB_SEL_BORDER), egui::StrokeKind::Inside);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(5), TOOLBAR_HOVER);
                    }
                    paint_icon(ui, Rect::from_min_size(r.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), icon);
                    let col = if i == self.active { TEXT } else { TEXT_MUTED };
                    ui.painter().galley(pos2(r.left() + 30.0, r.center().y - g.size().y / 2.0), g, col);
                    let cr = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(16.0, 16.0));
                    let cresp = ui.interact(cr, Id::new(("tabx", t.id())), Sense::CLICK);
                    if i == self.active || resp.hovered() || cresp.hovered() {
                        if cresp.hovered() {
                            ui.painter().rect_filled(cr, CornerRadius::same(3), TOOLBAR_HOVER);
                        }
                        paint_icon_tinted(ui, cr, ic::CLOSE_SMALL, TEXT_MUTED);
                    }
                    if cresp.clicked() || resp.middle_clicked() {
                        close = Some(i);
                    } else if resp.clicked() {
                        clicked = Some(i);
                    }
                    x += w + 6.0;
                }
            })
        });
        if let Some(i) = clicked {
            self.active = i;
        }
        row_ui(ui, Rect::from_min_max(pos2(strip.right() - 40.0, strip.top() + 7.0), strip.right_bottom() - vec2(8.0, 7.0)), |ui| {
            icon_btn(ui, ic::MORE_VERTICAL, "");
        });
        if let Some(i) = close {
            self.close_tab(i);
            return;
        }
        let body = Rect::from_min_max(pos2(rect.left(), strip.bottom()), rect.max - vec2(0.0, 6.0));
        let ctx = ui.ctx().clone();
        let active = self.active.min(self.tabs.len() - 1);
        self.active = active;
        let tab_ds = self.tabs[active].ds();
        let pg = tab_ds.is_some_and(|ds| self.driver(ds) == Driver::Postgres);
        // console target choices: databases (MongoDB) or `database.schema` (PostgreSQL, loaded schemas)
        let databases: Vec<(String, String)> = self
            .sources
            .iter()
            .find(|s| Some(s.ds.id) == tab_ds)
            .map(|s| match pg {
                false => s.all_dbs.iter().map(|d| (d.clone(), String::new())).collect(),
                true => s.dbs.iter().flatten().flat_map(|d| d.schemas.iter().map(|sc| (d.name.clone(), sc.clone()))).collect(),
            })
            .unwrap_or_default();
        // completion data for the console's current database
        let (comp_colls, comp_fields) = match &self.tabs[active] {
            Tab::Console(c) => {
                let db = self
                    .sources
                    .iter()
                    .find(|s| s.ds.id == c.ds)
                    .and_then(|s| s.dbs.as_ref())
                    .and_then(|d| d.iter().find(|d| d.name == c.db));
                let colls: Vec<String> =
                    db.and_then(|d| d.colls.as_ref()).map(|cs| cs.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
                let fields: HashMap<String, Vec<(String, String)>> = db
                    .and_then(|d| d.colls.as_ref())
                    .map(|cs| {
                        cs.iter()
                            .filter_map(|c| {
                                let first = c.meta.as_ref()?.first()?;
                                Some((c.name.clone(), first.1.iter().map(|f| (f.name.clone(), f.detail.clone())).collect()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (colls, fields)
            }
            _ => (vec![], HashMap::new()),
        };
        // lazily introspect what the console needs for completion: the db's collections and
        // field types of collections referenced as `db.<name>`
        if let Tab::Console(c) = &self.tabs[active] {
            let (ds, db) = (c.ds, c.db.clone());
            let mut want: Vec<Action> = vec![];
            let node = self
                .sources
                .iter()
                .find(|s| s.ds.id == ds && s.conn.is_some())
                .and_then(|s| s.dbs.as_ref())
                .and_then(|d| d.iter().find(|d| d.name == db));
            match node.map(|d| &d.colls) {
                Some(None) => want.push(Action::RefreshDb(ds, db.clone())),
                Some(Some(cs)) if pg => {
                    // tables named anywhere in the console
                    let lower = c.text.to_lowercase();
                    let words: HashSet<&str> = lower.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).collect();
                    for x in cs.iter().filter(|x| x.meta.is_none() && x.kind != 'S' && words.contains(x.name.to_lowercase().as_str())) {
                        want.push(Action::LoadMeta(ds, db.clone(), x.qual()));
                    }
                }
                Some(Some(cs)) => {
                    for part in c.text.split("db.").skip(1) {
                        let name: String = part.chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
                        if cs.iter().any(|x| x.name == name && x.meta.is_none()) {
                            want.push(Action::LoadMeta(ds, db.clone(), name));
                        }
                    }
                }
                None => {}
            }
            for a in want {
                let key = match &a {
                    Action::RefreshDb(_, d) => format!("{ds}/{d}"),
                    Action::LoadMeta(_, d, c) => format!("{ds}/{d}/{c}"),
                    _ => continue,
                };
                if self.requested.insert(key) {
                    self.act(&ctx, a);
                }
            }
        }
        let mut run: Option<Option<std::ops::Range<usize>>> = None;
        let mut new_tab = false;
        let mut open_history: Option<(u64, u64)> = None;
        let mut grid_acts = None;
        let focus_req = self.focus_console.take();
        match &mut self.tabs[active] {
            Tab::Console(c) => {
                let tb = Rect::from_min_size(body.min + vec2(8.0, 0.0), vec2(body.width() - 16.0, 34.0));
                let mut stop = false;
                row_ui(ui, tb, |ui| {
                    if icon_btn_ex(ui, ic::RUN, "Execute  ⌘↩", !c.running, false, false).clicked() {
                        run = Some(None);
                    }
                    if icon_btn_ex(ui, ic::RUN_NEW_TAB, "Execute in New Tab", !c.running, false, false).clicked() {
                        (run, new_tab) = (Some(None), true);
                    }
                    if icon_btn(ui, ic::HISTORY, "Browse Query History  ⌥⌘E").clicked() {
                        open_history = Some((c.ds, c.id));
                    }
                    icon_btn_ex(ui, ic::SETTINGS, "Settings", false, false, false);
                    if icon_btn(ui, ic::OUTPUT, "Show Services").clicked() {
                        self.show_services = !(self.show_services && !self.show_gen);
                        self.show_gen = false;
                    }
                    if icon_btn_ex(ui, ic::STOP, "Cancel Running Statements  ⌘F2", c.running, false, false).clicked() {
                        stop = true;
                    }
                    vsep(ui);
                    let dd = dropdown(ui, None, if c.mode == Mode::Playground { "Playground" } else { "Script" }, "Execution mode");
                    egui::Popup::menu(&dd).show(|ui| {
                        ui.set_min_width(180.0);
                        if menu_check(ui, "Playground — run statement at caret", c.mode == Mode::Playground) {
                            c.mode = Mode::Playground;
                        }
                        if menu_check(ui, "Script — run the whole console", c.mode == Mode::Script) {
                            c.mode = Mode::Script;
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (icon, label, tip) = match pg {
                            true => (ic::SCHEMA, format!("{}.{}", c.db, c.schema), "Database and schema"),
                            false => (ic::MONGO_DB, c.db.clone(), "Database"),
                        };
                        let dd = dropdown(ui, Some(icon), &label, tip);
                        egui::Popup::menu(&dd).show(|ui| {
                            ui.set_min_width(180.0);
                            for (d, sc) in &databases {
                                let name = if pg { format!("{d}.{sc}") } else { d.clone() };
                                if menu_check(ui, &name, *d == c.db && *sc == c.schema) {
                                    c.db = d.clone();
                                    c.schema = sc.clone();
                                }
                            }
                        });
                    });
                });
                if stop && let Some(conn) = self.sources.iter().find(|s| s.ds.id == c.ds).and_then(|s| s.conn.as_ref()) {
                    conn.stop();
                }
                let er = Rect::from_min_max(pos2(body.left(), tb.bottom() + 2.0), body.max);
                let eid = Id::new(("console", c.id));
                let set_cursor = focus_req.filter(|(fid, _)| *fid == c.id).map(|(_, at)| at.unwrap_or_else(|| c.text.chars().count()));
                let completer = |before: &str| match pg {
                    true => editor::sql_completions(before, &comp_colls, &comp_fields),
                    false => editor::mongo_completions(before, &comp_colls, &comp_fields),
                };
                let lang = if pg { Lang::Sql } else { Lang::Js };
                let out = rect_ui(ui, er, |ui| editor::code_editor(ui, eid, &mut c.text, lang, &c.marks, set_cursor, Some(&completer)));
                c.line_col = out.line_col;
                if out.has_focus && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter)) {
                    run = Some(out.cursor.clone());
                }
                if run == Some(None) {
                    run = Some(out.cursor);
                }
            }
            Tab::Data(d) => {
                let tb = Rect::from_min_size(body.min, vec2(body.width(), 34.0));
                d.grid.blur_outside(ui, body);
                let acts = d.grid.toolbar(ui, tb, false);
                // the value panel (when shown) takes the right side from the filter bar row down
                let fb = Rect::from_min_size(
                    pos2(body.left(), tb.bottom()),
                    vec2(body.width() - d.grid.panel_width(body.width()), if d.grid.hide_filter { 0.0 } else { 30.0 }),
                );
                ui.painter().rect_filled(fb, 0.0, ISLAND);
                let mut reload = false;
                if !d.grid.hide_filter {
                    // DataGrip's filter bar: `funnel▾ find {…} ✕ | sort▾ sort`
                    let mid = fb.left() + fb.width() / 2.0;
                    let cy = fb.center().y;
                    ui.painter().hline(fb.x_range(), fb.top() + 0.5, Stroke::new(1.0, BORDER));
                    ui.painter().vline(mid, fb.y_range(), Stroke::new(1.0, BORDER));
                    let bar_icon = |ui: &Ui, left: f32, icon| {
                        let r = Rect::from_min_size(pos2(left, cy - 8.0), vec2(16.0, 16.0));
                        paint_icon(ui, r, icon);
                        let c = pos2(r.left() + 13.0, cy - 0.5);
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![c, c + vec2(4.5, 0.0), c + vec2(2.25, 2.25)],
                            ICON,
                            Stroke::NONE,
                        ));
                        r.left() + 24.5
                    };
                    // MongoDB: `find {…}` / `sort`; PostgreSQL: `WHERE` / `ORDER BY`
                    let (find_label, sort_label, empty) = if pg { ("WHERE", "ORDER BY", "") } else { ("find", "sort", "{}") };
                    let x = bar_icon(ui, fb.left() + 4.0, ic::FILTER);
                    let find = text_at(ui, pos2(x, cy), Align2::LEFT_CENTER, find_label, mono(13.0), TEXT_INFO);
                    let fid = Id::new(("filter", d.id));
                    let te = Rect::from_min_max(pos2(find.right() + 8.0, fb.top()), pos2(mid - 28.0, fb.bottom()));
                    let completer = |before: &str| match pg {
                        true => editor::where_completions(before, &d.grid.fields()),
                        false => editor::filter_completions(before, &d.grid.fields()),
                    };
                    let r = row_ui(ui, te, |ui| editor::completing_line(ui, fid, &mut d.filter, te.width(), &completer));
                    // `{}` is real text: focusing an empty filter puts the caret between the braces
                    if r.gained_focus() && d.filter == "{}" {
                        editor::set_caret(ui, fid, 1);
                    }
                    if r.lost_focus() && d.filter.trim().is_empty() {
                        d.filter = empty.into();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        reload = true;
                    }
                    let xr = Rect::from_center_size(pos2(mid - 14.0, cy), vec2(20.0, 20.0));
                    let clear = ui.interact(xr, fid.with("clear"), Sense::CLICK);
                    if clear.hovered() {
                        ui.painter().rect_filled(xr, CornerRadius::same(4), TOOLBAR_HOVER);
                    }
                    paint_icon(ui, xr, ic::CLOSE_SMALL);
                    clear.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Clear"));
                    if clear.on_hover_text("Clear").clicked() {
                        d.filter = empty.into();
                        reload = true;
                    }
                    let x = bar_icon(ui, mid + 6.5, ic::SORT);
                    let se = Rect::from_min_max(pos2(x, fb.top()), pos2(fb.right() - 8.0, fb.bottom()));
                    let r = row_ui(ui, se, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut d.sort)
                                .frame(egui::Frame::NONE)
                                .margin(egui::Margin::ZERO)
                                .hint_text(egui::RichText::new(sort_label).font(mono(13.0)).color(TEXT_INFO))
                                .font(mono(13.0))
                                .text_color(EDITOR_TEXT)
                                .desired_width(se.width()),
                        )
                    });
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        reload = true;
                    }
                }
                let gr = Rect::from_min_max(pos2(body.left(), fb.bottom()), body.max);
                let mut acts = acts;
                acts.extend(d.grid.body(ui, gr, Id::new(("data", d.id)), fb.top()));
                if reload {
                    d.grid.server_sort = parse_sort(&d.sort, pg);
                    acts.push(GridAction::Page { offset: 0, limit: d.page });
                }
                grid_acts = Some((GridTarget::Data(d.id), acts));
            }
            Tab::Result(r) => {
                let tb = Rect::from_min_size(body.min, vec2(body.width(), 34.0));
                r.grid.blur_outside(ui, body);
                let mut acts = r.grid.toolbar(ui, tb, true);
                let gr = Rect::from_min_max(pos2(body.left(), tb.bottom()), body.max);
                acts.extend(r.grid.body(ui, gr, Id::new(("result", r.id)), gr.top()));
                grid_acts = Some((GridTarget::Result(r.id), acts));
            }
            Tab::File(f) => {
                let lang = match f.path.extension().and_then(|e| e.to_str()) {
                    Some("json") => Lang::Json,
                    Some("sql") => Lang::Sql,
                    _ => Lang::Js,
                };
                let er = Rect::from_min_max(pos2(body.left(), body.top() + 2.0), body.max);
                let out = rect_ui(ui, er, |ui| editor::code_editor(ui, Id::new(("file", f.id)), &mut f.text, lang, &[], None, None));
                f.line_col = out.line_col;
                if out.has_focus && ui.input(|i| i.modifiers.command && i.key_pressed(Key::S)) {
                    f.last_save = Instant::now() - std::time::Duration::from_secs(5);
                }
            }
        }
        if let Some(cursor) = run {
            self.run_console(&ui.ctx().clone(), active, cursor, new_tab);
        }
        if let Some((ds, console)) = open_history {
            self.open_history(ds, console);
        }
        if let Some((t, a)) = grid_acts {
            self.grid_actions(&ctx, t, a);
        }
    }

    /// Tab captions like the IDE: `name [data source]`; when two collide the name is qualified
    /// (`db.coll [data source]`) and shortened in the middle.
    fn tab_titles(&self) -> Vec<(ImageSource<'static>, String)> {
        let ds_name = |id: u64| self.sources.iter().find(|s| s.ds.id == id).map_or(String::new(), |s| s.ds.name.clone());
        let base: Vec<(ImageSource<'static>, String, String)> = self
            .tabs
            .iter()
            .map(|t| match t {
                Tab::Console(c) => {
                    (driver_icon(self.driver(c.ds)), format!("{} [{}]", c.title, ds_name(c.ds)), format!("{} [{}]", c.title, ds_name(c.ds)))
                }
                Tab::Data(d) => {
                    (ic::TABLE, format!("{} [{}]", self.short_name(d), ds_name(d.ds)), format!("{}.{} [{}]", d.db, d.coll, ds_name(d.ds)))
                }
                Tab::Result(r) => {
                    let t = r.grid.rs.as_ref().map_or("result".into(), |x| x.title.clone());
                    (ic::TABLE, format!("{t} [{}]", ds_name(r.ds)), format!("{t} [{}]", ds_name(r.ds)))
                }
                Tab::File(f) => (
                    files::file_icon(&f.path),
                    f.path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()),
                    f.path.display().to_string(),
                ),
            })
            .collect();
        base.iter()
            .map(|(icon, short, long)| {
                let dup = base.iter().filter(|(_, s, _)| s == short).count() > 1;
                (icon.clone(), if dup { mid_ellipsis(long, 56) } else { mid_ellipsis(short, 56) })
            })
            .collect()
    }

    fn close_tab(&mut self, i: usize) {
        let grid = match &self.tabs[i] {
            Tab::Data(d) => Some(&d.grid),
            Tab::Result(r) => Some(&r.grid),
            _ => None,
        };
        if grid.is_some_and(|g| g.dirty()) {
            self.confirm = Some(Confirm::CloseTab(self.tabs[i].id()));
            return;
        }
        self.do_close_tab(i);
    }

    fn do_close_tab(&mut self, i: usize) {
        self.tabs[i].flush(true);
        if let Tab::Console(c) = &self.tabs[i] {
            remember_console(&mut self.cfg, c);
            let id = c.id;
            self.services.results.retain(|r| r.console != id);
            self.services.active = self.services.active.min(self.services.results.len());
        }
        let id = self.tabs[i].id();
        self.services.sessions.retain(|s| s.key != id);
        self.tabs.remove(i);
        if self.active >= i && self.active > 0 {
            self.active -= 1;
        }
    }

    fn services_area(&mut self, ui: &mut Ui, rect: Rect) {
        island(ui, rect);
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        tool_header(ui, head, "Services");
        row_ui(ui, Rect::from_min_max(pos2(head.right() - 80.0, head.top() + 7.0), head.right_bottom() - vec2(8.0, 7.0)), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, ic::HIDE, "Hide  ⇧⎋").clicked() {
                    self.show_services = false;
                }
                icon_btn(ui, ic::MORE_VERTICAL, "Options");
            });
        });
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max - vec2(0.0, 6.0));
        // draggable divider between the sessions tree and the output / results
        self.svc_left_w = self.svc_left_w.clamp(60.0, (body.width() - 280.0).max(60.0));
        let left_w = self.svc_left_w;
        let left = Rect::from_min_size(body.min, vec2(left_w, body.height()));
        // sessions tree
        let ctx = ui.ctx().clone();
        let mut stop_all = false;
        row_ui(ui, Rect::from_min_size(left.min + vec2(8.0, 0.0), vec2(left_w - 16.0, 30.0)), |ui| {
            if icon_btn_ex(ui, ic::STOP, "Stop", self.tabs.iter().any(|t| matches!(t, Tab::Console(c) if c.running)), false, false)
                .clicked()
            {
                stop_all = true;
            }
            vsep(ui);
            icon_btn_ex(ui, ic::ADD, "Add Service", false, false, true);
            icon_btn_ex(ui, ic::SHOW, "View Options", false, false, true);
            icon_btn_ex(ui, ic::OPEN_IN_TOOL_WINDOW, "Open in New Tab", false, false, false);
            icon_btn(ui, ic::EXPAND_ALL, "Expand All");
            icon_btn(ui, ic::COLLAPSE_ALL, "Collapse All");
        });
        if stop_all {
            for s in &self.sources {
                if let Some(c) = &s.conn {
                    c.stop();
                }
            }
        }
        let mut open_tab = None;
        rect_ui(ui, Rect::from_min_max(left.min + vec2(0.0, 34.0), left.max), |ui| {
            egui::ScrollArea::vertical().id_salt("sessions").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let none = None;
                let k0 = "svc:db".to_string();
                if self.services.sessions.is_empty() {
                    return;
                }
                node(ui, &none, 0, &k0, Some(true), ic::FOLDER, "Database", None, NodeExtra::default());
                let mut shown: Vec<u64> = vec![];
                for s in &self.services.sessions {
                    if !shown.contains(&s.ds) {
                        shown.push(s.ds);
                    }
                }
                for ds in shown {
                    let name = self.sources.iter().find(|x| x.ds.id == ds).map_or("?".into(), |x| x.ds.name.clone());
                    let dicon = driver_icon(self.driver(ds));
                    node(
                        ui,
                        &none,
                        1,
                        &format!("svc:{ds}"),
                        Some(true),
                        dicon.clone(),
                        &name,
                        None,
                        NodeExtra { dot: true, ..Default::default() },
                    );
                    for s in self.services.sessions.iter().filter(|s| s.ds == ds) {
                        let icon = if s.console { dicon.clone() } else { ic::TABLE };
                        let r = node(
                            ui,
                            &none,
                            2,
                            &format!("svc:s:{}", s.key),
                            None,
                            icon,
                            &s.name,
                            s.ms.map(|m| format!("{m} ms")),
                            NodeExtra::default(),
                        );
                        if r.row.double_clicked() || r.row.clicked() {
                            open_tab = Some(s.key);
                        }
                    }
                }
            });
        });
        if let Some(k) = open_tab
            && let Some(i) = self.tabs.iter().position(|t| t.id() == k)
        {
            self.active = i;
        }
        let split = Rect::from_center_size(pos2(left.right(), body.center().y), vec2(8.0, body.height()));
        let hot = ui.rect_contains_pointer(split) || ui.ctx().is_being_dragged(ui.id().with("split-svc"));
        self.svc_left_w += splitter(ui, "split-svc", split, true);
        ui.painter().vline(
            left.right(),
            body.y_range(),
            Stroke::new(if hot { 2.0 } else { 1.0 }, if hot { CONTROL_BORDER } else { BORDER }),
        );
        // right: tabs
        let right = Rect::from_min_max(pos2(left.right() + 1.0, body.top()), body.max);
        let strip = Rect::from_min_size(right.min, vec2(right.width(), 38.0));
        let mut x = strip.left() + 8.0;
        let mut close = None;
        let titles: Vec<(ImageSource<'static>, String)> = std::iter::once((ic::OUTPUT, "Output".to_string()))
            .chain(self.services.results.iter().map(|r| (ic::TABLE, r.grid.rs.as_ref().map_or("result".into(), |x| x.title.clone()))))
            .collect();
        for (i, (icon, title)) in titles.iter().enumerate() {
            let g = ui.painter().layout_no_wrap(title.clone(), ui_font(13.0), TEXT);
            let w = g.size().x + if i == 0 { 38.0 } else { 58.0 };
            let r = Rect::from_min_size(pos2(x, strip.top() + 5.0), vec2(w, 28.0));
            let resp = ui.interact(r, Id::new(("svc-tab", i)), Sense::CLICK);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, i == self.services.active, title));
            if i == self.services.active {
                ui.painter().rect(r, CornerRadius::same(5), TAB_SEL_BG, Stroke::new(1.0, TAB_SEL_BORDER), egui::StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(5), TOOLBAR_HOVER);
            }
            paint_icon(ui, Rect::from_min_size(r.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), icon.clone());
            ui.painter().galley(pos2(r.left() + 30.0, r.center().y - g.size().y / 2.0), g, TEXT);
            if i > 0 {
                let cr = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(16.0, 16.0));
                let cresp = ui.interact(cr, Id::new(("svc-x", i)), Sense::CLICK);
                paint_icon_tinted(ui, cr, ic::CLOSE_SMALL, TEXT_MUTED);
                if cresp.clicked() {
                    close = Some(i);
                }
            }
            if resp.clicked() && close.is_none() {
                self.services.active = i;
            }
            x += w + 6.0;
        }
        if let Some(i) = close
            && self.services.results[i - 1].grid.dirty()
        {
            self.confirm = Some(Confirm::CloseResult(self.services.results[i - 1].id));
        } else if let Some(i) = close {
            self.services.results.remove(i - 1);
            self.services.active = self.services.active.min(self.services.results.len());
            if self.services.active >= i {
                self.services.active = self
                    .services
                    .active
                    .saturating_sub(1)
                    .max(if self.services.results.is_empty() { 0 } else { 1 })
                    .min(self.services.results.len());
            }
        }
        let content = Rect::from_min_max(pos2(right.left(), strip.bottom()), right.max);
        if self.services.active == 0 {
            self.output_view(ui, content);
        } else if let Some(r) = self.services.results.get_mut(self.services.active - 1) {
            let tb = Rect::from_min_size(content.min, vec2(content.width(), 34.0));
            r.grid.blur_outside(ui, content);
            let mut acts = r.grid.toolbar(ui, tb, true);
            let gr = Rect::from_min_max(pos2(content.left(), tb.bottom()), content.max);
            acts.extend(r.grid.body(ui, gr, Id::new(("result", r.id)), gr.top()));
            let id = r.id;
            self.grid_actions(&ctx, GridTarget::Result(id), acts);
        }
    }

    /// DataGrip's SQL Generator tool window: options | generated script | copy, save, open in console.
    fn sql_gen_area(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        island(ui, rect);
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        tool_header(ui, head, "SQL Generator");
        row_ui(ui, Rect::from_min_max(pos2(head.right() - 80.0, head.top() + 7.0), head.right_bottom() - vec2(8.0, 7.0)), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_btn(ui, ic::HIDE, "Hide  ⇧⎋").clicked() {
                    self.show_gen = false;
                }
                icon_btn(ui, ic::MORE_VERTICAL, "Options");
            });
        });
        let Some(g) = self.sql_gen.as_mut() else { return };
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max - vec2(0.0, 6.0));
        let strip = Rect::from_min_size(body.min, vec2(40.0, body.height()));
        rect_ui(ui, strip.shrink2(vec2(7.0, 4.0)), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if icon_btn_ex(ui, ic::SETTINGS, "SQL Generator Options", true, g.options, false).clicked() {
                    g.options = !g.options;
                }
                icon_btn_ex(ui, ic::SAVE, "File Output Options", false, false, false);
            });
        });
        ui.painter().vline(strip.right(), body.y_range(), Stroke::new(1.0, BORDER));
        let mut left = strip.right() + 1.0;
        if g.options {
            let opts = Rect::from_min_max(pos2(left, body.top()), pos2((left + 470.0).min(body.right() - 240.0), body.bottom()));
            let o = &mut self.gen_opts;
            rect_ui(ui, opts.shrink2(vec2(10.0, 6.0)), |ui| {
                egui::ScrollArea::vertical().id_salt("sql-gen-opts").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    let combo = |ui: &mut Ui, label: &str, items: &[&str], v: &mut usize| {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            let w = ui.available_width() - 4.0;
                            egui::ComboBox::from_id_salt(("sql-gen", label)).icon(crate::dialog::combo_chevron).width(w).selected_text(items[*v]).show_ui(
                                ui,
                                |ui| {
                                    for (i, t) in items.iter().enumerate() {
                                        ui.selectable_value(v, i, *t);
                                    }
                                },
                            );
                        });
                    };
                    combo(ui, "Generate:", &pg::GENERATE, &mut o.generate);
                    combo(ui, "Qualify objects with schema names:", &pg::QUALIFY, &mut o.qualify);
                    combo(ui, "Place constraints:", &pg::PLACE, &mut o.place);
                    for (label, v) in o.flags() {
                        ui.checkbox(v, label);
                    }
                });
            });
            ui.painter().vline(opts.right(), body.y_range(), Stroke::new(1.0, BORDER));
            left = opts.right() + 1.0;
        }
        let side = Rect::from_min_max(pos2(body.right() - 36.0, body.top()), body.max);
        let code = Rect::from_min_max(pos2(left, body.top()), pos2(side.left(), body.bottom()));
        let text = g.rel.as_ref().map(|r| pg::ddl(r, &self.gen_opts));
        rect_ui(ui, code, |ui| match (&text, &g.error) {
            (_, Some(e)) => {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.colored_label(ERROR_OUT, e);
                });
            }
            (Some(t), _) => editor::code_view(ui, Id::new("sql-gen"), t, Lang::Sql),
            _ => {
                text_at(ui, code.center(), Align2::CENTER_CENTER, "Generating SQL script…", ui_font(13.0), TEXT_SECONDARY);
            }
        });
        let (mut save, mut console) = (false, false);
        rect_ui(ui, side.shrink2(vec2(5.0, 4.0)), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let ok = text.is_some();
                if icon_btn_ex(ui, ic::COPY, "Copy to Clipboard", ok, false, false).clicked() {
                    ctx.copy_text(text.clone().unwrap_or_default());
                }
                save = icon_btn_ex(ui, ic::SAVE, "Save into File", ok, false, false).clicked();
                console = icon_btn_ex(ui, ic::CONSOLE, "Open in Console", ok, false, false).clicked();
            });
        });
        let (ds, db, name) = (g.ds, g.db.clone(), pg::split_qual(&g.coll).1);
        let text = text.unwrap_or_default();
        if save {
            // like the grid's export: into Downloads
            let path = config::home().join("Downloads").join(format!("{name}.sql"));
            self.status = Some(match std::fs::write(&path, text) {
                Ok(_) => format!("Saved to {}", path.display()),
                Err(e) => format!("Save failed: {e}"),
            });
        } else if console {
            self.ddl_console(&ctx, ds, db, text);
        }
    }

    fn output_view(&mut self, ui: &mut Ui, rect: Rect) {
        let side = Rect::from_min_max(pos2(rect.right() - 36.0, rect.top()), rect.max);
        let main = Rect::from_min_max(rect.min, pos2(side.left(), rect.bottom()));
        rect_ui(ui, side.shrink2(vec2(5.0, 4.0)), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if icon_btn_ex(ui, ic::SOFT_WRAP, "Soft-Wrap", true, self.services.soft_wrap, false).clicked() {
                    self.services.soft_wrap = !self.services.soft_wrap;
                }
                icon_btn_ex(ui, ic::SCROLL_DOWN, "Scroll to the End", true, true, false);
                if icon_btn(ui, ic::COPY, "Copy Output").clicked() {
                    ui.ctx().copy_text(self.services.output.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n"));
                }
                if icon_btn(ui, ic::DELETE, "Clear All").clicked() {
                    self.services.output.clear();
                }
            });
        });
        let wrap = self.services.soft_wrap;
        rect_ui(ui, main, |ui| {
            egui::ScrollArea::both().id_salt("output").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(4.0);
                ui.style_mut().spacing.window_margin = egui::Margin::ZERO;
                for (line, kind) in &self.services.output {
                    let mut job = egui::text::LayoutJob::default();
                    let fmt = |c| egui::TextFormat {
                        font_id: mono(13.0),
                        color: c,
                        line_height: Some(editor::LINE_H),
                        valign: egui::Align::Center,
                        ..Default::default()
                    };
                    match kind {
                        LogKind::Echo | LogKind::EchoSql => {
                            let (ts, rest) = line.split_at(line.find("] ").map_or(0, |i| i + 2));
                            job.append(ts, 0.0, fmt(EDITOR_TEXT));
                            let (prompt, code) = rest.split_at(rest.find("> ").map_or(0, |i| i + 2));
                            job.append(prompt, 0.0, fmt(EDITOR_TEXT));
                            let lang = if *kind == LogKind::EchoSql { Lang::Sql } else { Lang::Js };
                            for s in editor::highlight(code, lang).sections {
                                job.append(&code[s.byte_range.start.0..s.byte_range.end.0], 0.0, fmt(s.format.color));
                            }
                        }
                        LogKind::Error => job.append(line, 0.0, fmt(ERROR_OUT)),
                        LogKind::Info => job.append(line, 0.0, fmt(EDITOR_TEXT)),
                    }
                    let mode = if wrap { egui::TextWrapMode::Wrap } else { egui::TextWrapMode::Extend };
                    ui.add(egui::Label::new(job).wrap_mode(mode));
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let mut crumbs: Vec<(Option<ImageSource<'static>>, String)> = vec![];
        let tab = self.tabs.get(self.active);
        let ds_name = |id: u64| self.sources.iter().find(|s| s.ds.id == id).map_or(String::new(), |s| s.ds.name.clone());
        match tab {
            Some(Tab::Console(c)) => {
                crumbs.push((None, "Database Consoles".into()));
                crumbs.push((None, ds_name(c.ds)));
                crumbs.push((Some(driver_icon(self.driver(c.ds))), format!("{} [{}]", c.title, ds_name(c.ds))));
            }
            Some(Tab::Data(d)) => {
                crumbs.push((None, "Database".into()));
                crumbs.push((Some(driver_icon(self.driver(d.ds))), ds_name(d.ds)));
                crumbs.push((None, d.db.clone()));
                if self.driver(d.ds) == Driver::Postgres {
                    crumbs.push((None, pg::split_qual(&d.coll).0));
                    crumbs.push((None, "tables".into()));
                } else {
                    crumbs.push((None, "collections".into()));
                }
                crumbs.push((Some(ic::TABLE), self.short_name(d)));
            }
            Some(Tab::Result(r)) => {
                crumbs.push((None, "Database".into()));
                crumbs.push((Some(driver_icon(self.driver(r.ds))), ds_name(r.ds)));
                crumbs.push((Some(ic::TABLE), r.grid.rs.as_ref().map_or("result".into(), |x| x.title.clone())));
            }
            Some(Tab::File(f)) => {
                let parts: Vec<String> = f.path.iter().map(|p| p.to_string_lossy().into_owned()).filter(|p| p != "/").collect();
                let n = parts.len();
                for (i, p) in parts.into_iter().enumerate().skip(n.saturating_sub(3)) {
                    crumbs.push(((i + 1 == n).then(|| files::file_icon(&f.path)), p));
                }
            }
            None => {
                if let Some(ds) = self.selection().ds {
                    crumbs.push((None, "Database".into()));
                    crumbs.push((Some(driver_icon(self.driver(ds))), ds_name(ds)));
                }
            }
        }
        let mut x = rect.left() + 14.0;
        for (i, (icon, t)) in crumbs.iter().enumerate() {
            if i > 0 {
                paint_icon_tinted(
                    ui,
                    Rect::from_min_size(pos2(x - 2.0, rect.center().y - 8.0), vec2(16.0, 16.0)),
                    ic::CHEVRON_RIGHT,
                    TEXT_MUTED,
                );
                x += 18.0;
            }
            if let Some(ic) = icon {
                paint_icon(ui, Rect::from_min_size(pos2(x, rect.center().y - 8.0), vec2(16.0, 16.0)), ic.clone());
                x += 19.0;
            }
            let r = text_at(ui, pos2(x, rect.center().y), Align2::LEFT_CENTER, t, ui_font(13.0), TEXT_MUTED);
            x = r.right() + 8.0;
        }
        if let Some(s) = &self.status {
            text_at(ui, pos2(x + 20.0, rect.center().y), Align2::LEFT_CENTER, s, ui_font(12.5), TEXT_SECONDARY);
        }
        let right = match tab {
            Some(Tab::Console(c)) => format!("{}:{}     LF     UTF-8     4 spaces", c.line_col.0 + 1, c.line_col.1 + 1),
            Some(Tab::Data(DataTab { grid, .. }) | Tab::Result(ResultTab { grid, .. })) => grid.rs.as_ref().map_or(String::new(), |r| {
                if grid.dirty() { "Pending changes — ⌘↩ to submit".into() } else { format!("{} rows", r.docs.len()) }
            }),
            Some(Tab::File(f)) => {
                format!("{}:{}     LF     UTF-8", f.line_col.0 + 1, f.line_col.1 + 1)
            }
            None => String::new(),
        };
        text_at(ui, pos2(rect.right() - 40.0, rect.center().y), Align2::RIGHT_CENTER, &right, ui_font(12.5), TEXT_MUTED);
        paint_icon_tinted(
            ui,
            Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(16.0, 16.0)),
            ic::NOTIFICATIONS,
            TEXT_MUTED,
        );
    }

    fn ask_dialog(&mut self, ctx: &egui::Context) {
        let pg = match &self.ask {
            Some(Ask::Drop(ds, ..) | Ask::Rename(ds, ..) | Ask::Create(ds, ..)) => self.driver(*ds) == Driver::Postgres,
            _ => false,
        };
        let Some(ask) = &mut self.ask else { return };
        let mut done: Option<bool> = None;
        let modal = egui::Modal::new(Id::new("ask"))
            .frame(egui::Frame::new().fill(POPUP_BG).stroke(Stroke::new(1.0, POPUP_BORDER)).corner_radius(10).inner_margin(20));
        let r = modal.show(ctx, |ui| {
            ui.set_width(420.0);
            match ask {
                Ask::RemoveSource(_, name) => {
                    ui.label(egui::RichText::new(format!("Remove data source \"{name}\"?")).font(bold_font(14.0)));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Its consoles and query history stay on disk.").color(TEXT_SECONDARY));
                }
                Ask::RenameSource(_, name) => {
                    ui.label(egui::RichText::new("Rename data source").font(bold_font(14.0)));
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(name).desired_width(380.0)).request_focus();
                }
                Ask::Drop(_, db, coll) => {
                    ui.label(
                        egui::RichText::new(match coll {
                            Some(c) if pg => format!("Drop \"{c}\"?"),
                            Some(c) => format!("Drop collection \"{db}.{c}\"?"),
                            None => format!("Drop database \"{db}\"?"),
                        })
                        .font(bold_font(14.0)),
                    );
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("This operation cannot be undone.").color(TEXT_SECONDARY));
                }
                Ask::Rename(_, _, _, to) => {
                    ui.label(egui::RichText::new(if pg { "Rename" } else { "Rename collection" }).font(bold_font(14.0)));
                    ui.add_space(6.0);
                    let r = ui.add(egui::TextEdit::singleline(to).desired_width(380.0));
                    r.request_focus();
                }
                Ask::Create(_, db, name) => {
                    ui.label(egui::RichText::new(format!("New collection in \"{db}\"")).font(bold_font(14.0)));
                    ui.add_space(6.0);
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("name").desired_width(380.0));
                    r.request_focus();
                }
                Ask::RenameFile(_, name) => {
                    ui.label(egui::RichText::new("Rename").font(bold_font(14.0)));
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(name).desired_width(380.0)).request_focus();
                }
                Ask::DeleteFile(p) => {
                    ui.label(
                        egui::RichText::new(format!(
                            "Delete \"{}\"?",
                            p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())
                        ))
                        .font(bold_font(14.0)),
                    );
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(p.display().to_string()).color(TEXT_SECONDARY));
                }
                Ask::AttachPath(path) => {
                    ui.label(egui::RichText::new("Attach Directory").font(bold_font(14.0)));
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(path).hint_text("/path/to/folder").desired_width(380.0)).request_focus();
                }
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match ask {
                    Ask::RemoveSource(..) => "Remove",
                    Ask::Drop(..) => "Drop",
                    Ask::DeleteFile(_) => "Delete",
                    _ => "OK",
                };
                if primary_btn(ui, label).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    done = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").min_size(vec2(72.0, 28.0))).clicked() {
                    done = Some(false);
                }
            });
        });
        if r.should_close() {
            done = done.or(Some(false));
        }
        if done == Some(true) {
            match self.ask.take().unwrap() {
                Ask::RemoveSource(ds, _) => self.act(ctx, Action::RemoveSource(ds)),
                Ask::RenameSource(ds, name) if !name.trim().is_empty() => {
                    if let Some(s) = self.src(ds) {
                        s.ds.name = name.trim().to_string();
                    }
                    self.save_cfg();
                }
                Ask::Drop(ds, db, Some(c)) if pg => {
                    self.tabs.retain(|t| !matches!(t, Tab::Data(d) if d.ds == ds && d.db == db && d.coll == c));
                    let sql = format!("DROP {} {c}", rel_kind(self.coll_kind(ds, &db, &c)));
                    self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"sql": sql} }, Pending::Cmd(ds, db));
                }
                Ask::Drop(ds, db, Some(c)) => {
                    self.tabs.retain(|t| !matches!(t, Tab::Data(d) if d.ds == ds && d.db == db && d.coll == c));
                    self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"drop": &c} }, Pending::Cmd(ds, db));
                }
                Ask::Rename(ds, db, from, to) if pg => {
                    if !to.is_empty() && to != pg::split_qual(&from).1 {
                        let sql = format!("ALTER {} {from} RENAME TO {}", rel_kind(self.coll_kind(ds, &db, &from)), pg::ident(&to));
                        self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"sql": sql} }, Pending::Cmd(ds, db));
                    }
                }
                Ask::Drop(ds, db, None) => {
                    self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"dropDatabase": 1} }, Pending::Cmd(ds, String::new()))
                }
                Ask::Rename(ds, db, from, to) if !to.is_empty() && to != from => self.send(
                    ds,
                    Req::Command { db: "admin".into(), cmd: doc! {"renameCollection": format!("{db}.{from}"), "to": format!("{db}.{to}")} },
                    Pending::Cmd(ds, db),
                ),
                Ask::Create(ds, db, name) if !name.is_empty() => {
                    self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"create": name} }, Pending::Cmd(ds, db))
                }
                Ask::RenameFile(from, name) if !name.is_empty() && !name.contains('/') => {
                    // typing a bare name keeps the original extension (console → KAI keeps `.js`)
                    let name = match from.extension() {
                        Some(ext) if !name.contains('.') => format!("{name}.{}", ext.to_string_lossy()),
                        _ => name,
                    };
                    let to = from.with_file_name(&name);
                    match std::fs::rename(&from, &to) {
                        Ok(_) => {
                            for t in &mut self.tabs {
                                match t {
                                    Tab::Console(c) if c.path == from => {
                                        c.path = to.clone();
                                        c.title = to.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
                                    }
                                    Tab::File(f) if f.path == from => f.path = to.clone(),
                                    _ => {}
                                }
                            }
                            if let Some(db) = self.cfg.console_db.remove(&from.display().to_string()) {
                                self.cfg.console_db.insert(to.display().to_string(), db);
                            }
                            if let Some(sc) = self.cfg.console_schema.remove(&from.display().to_string()) {
                                self.cfg.console_schema.insert(to.display().to_string(), sc);
                            }
                        }
                        Err(e) => self.status = Some(format!("Rename failed: {e}")),
                    }
                }
                Ask::DeleteFile(p) => {
                    self.tabs.retain(|t| !matches!(t, Tab::Console(c) if c.path == p) && !matches!(t, Tab::File(f) if f.path == p));
                    self.cfg.console_db.remove(&p.display().to_string());
                    self.cfg.console_schema.remove(&p.display().to_string());
                    if let Err(e) = std::fs::remove_file(&p) {
                        self.status = Some(format!("Delete failed: {e}"));
                    }
                }
                Ask::AttachPath(path) if std::path::Path::new(path.trim()).is_dir() => {
                    self.cfg.attached.push(path.trim().trim_end_matches('/').to_string());
                    self.save_cfg();
                }
                _ => {}
            }
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        } else if done == Some(false) {
            self.ask = None;
        }
    }

    /// "Ignore Unsubmitted Changes" (Yes discards pending edits and continues; No / Esc / Enter keep them).
    fn modal_open(&self) -> bool {
        self.confirm.is_some()
            || self.ask.is_some()
            || self.dialog.is_some()
            || self.search.is_some()
            || self.history.is_some()
            || self.modify.is_some()
    }

    fn open_history(&mut self, ds: u64, console: u64) {
        let items = config::history(ds);
        let sel = items.len().checked_sub(1);
        let title = self.sources.iter().find(|s| s.ds.id == ds).map_or(String::new(), |s| s.ds.name.clone());
        self.history = Some(HistoryDlg { ds, console, title, items, sel, scroll: true });
    }

    /// Inserts `text` at the console's caret (own lines, like pasting a query from history).
    fn paste_into_console(&mut self, ctx: &egui::Context, console: u64, text: &str) {
        let Some(i) = self.tabs.iter().position(|t| t.id() == console) else { return };
        self.active = i;
        let Tab::Console(c) = &mut self.tabs[i] else { return };
        let eid = Id::new(("console", c.id));
        let caret =
            egui::TextEdit::load_state(ctx, eid).and_then(|s| s.cursor.char_range()).map_or(c.text.chars().count(), |r| r.primary.index.0);
        let at = c.text.char_indices().nth(caret).map_or(c.text.len(), |(b, _)| b);
        let before = if at > 0 && !c.text[..at].ends_with('\n') { "\n" } else { "" };
        let after = if at < c.text.len() && !c.text[at..].starts_with('\n') { "\n" } else { "" };
        let ins = format!("{before}{text}{after}");
        c.text.insert_str(at, &ins);
        self.focus_console = Some((console, Some(caret + before.chars().count() + text.chars().count())));
    }

    /// "<connection> Query History": statements on the left (newest last), preview on the right.
    fn history_dialog(&mut self, ctx: &egui::Context) {
        let lang = match self.history.as_ref().map(|h| self.driver(h.ds)) {
            Some(Driver::Postgres) => Lang::Sql,
            _ => Lang::Js,
        };
        let Some(h) = &mut self.history else { return };
        let mut close = false;
        let mut paste: Option<String> = None;
        let mut remove = false;
        let n = h.items.len();
        ctx.input_mut(|i| {
            if let Some(s) = h.sel.as_mut() {
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) && *s > 0 {
                    *s -= 1;
                    h.scroll = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) && *s + 1 < n {
                    *s += 1;
                    h.scroll = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Delete) || i.consume_key(egui::Modifiers::NONE, Key::Backspace) {
                    remove = true;
                }
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
                paste = h.sel.and_then(|s| h.items.get(s).cloned());
            }
        });
        let size = vec2(560.0, 640.0).min(ctx.content_rect().size() - vec2(60.0, 60.0));
        let modal = egui::Modal::new(Id::new("query-history")).backdrop_color(Color32::from_black_alpha(40)).frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x2B, 0x2D, 0x30))
                .stroke(Stroke::new(1.0, POPUP_BORDER))
                .corner_radius(12)
                .inner_margin(egui::Margin::same(16)),
        );
        let r = modal.show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            text_at(
                ui,
                rect.left_top() + vec2(4.0, 12.0),
                Align2::LEFT_CENTER,
                &format!("{} Query History", h.title),
                bold_font(14.0),
                TEXT,
            );
            let body = Rect::from_min_max(rect.left_top() + vec2(0.0, 34.0), rect.right_bottom() - vec2(0.0, 52.0));
            let list = Rect::from_min_size(body.min, vec2((body.width() * 0.42).min(260.0), body.height()));
            let prev = Rect::from_min_max(pos2(list.right() + 14.0, body.top()), body.max);
            ui.painter().rect(list, 4.0, ISLAND, Stroke::new(1.0, POPUP_BORDER), egui::StrokeKind::Inside);
            ui.painter().rect(prev, 4.0, ISLAND, Stroke::new(1.0, POPUP_BORDER), egui::StrokeKind::Inside);
            // list: toolbar with "—" then statements
            row_ui(ui, Rect::from_min_size(list.min + vec2(4.0, 2.0), vec2(list.width() - 8.0, 28.0)), |ui| {
                if icon_btn_ex(ui, ic::REMOVE, "Remove from History  ⌦", h.sel.is_some(), false, false).clicked() {
                    remove = true;
                }
            });
            let rows = Rect::from_min_max(list.min + vec2(1.0, 32.0), list.max - vec2(1.0, 1.0));
            rect_ui(ui, rows, |ui| {
                egui::ScrollArea::vertical().id_salt("history-list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    if h.items.is_empty() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("  No queries yet").color(TEXT_SECONDARY));
                    }
                    for (k, q) in h.items.iter().enumerate() {
                        let (rr, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::CLICK);
                        let first = q.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, h.sel == Some(k), first));
                        if h.sel == Some(k) {
                            ui.painter().rect_filled(rr, 0.0, SEL_ACTIVE);
                            if h.scroll {
                                resp.scroll_to_me(None);
                            }
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rr, 0.0, HOVER);
                        }
                        ui.painter().with_clip_rect(rr).text(
                            pos2(rr.left() + 10.0, rr.center().y),
                            Align2::LEFT_CENTER,
                            first,
                            mono(13.0),
                            EDITOR_TEXT,
                        );
                        if resp.clicked() {
                            h.sel = Some(k);
                        }
                        if resp.double_clicked() {
                            paste = Some(q.clone());
                        }
                    }
                });
            });
            h.scroll = false;
            let text = h.sel.and_then(|s| h.items.get(s)).cloned().unwrap_or_default();
            rect_ui(ui, prev.shrink(1.0), |ui| editor::code_view(ui, Id::new("history-preview"), &text, lang));
            // buttons
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 40.0), rect.max);
            row_ui(ui, foot, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    if primary_btn(ui, "Paste").clicked() && !text.is_empty() {
                        paste = Some(text.clone());
                    }
                    if ui.add(egui::Button::new(egui::RichText::new("Close").color(TEXT)).min_size(vec2(84.0, 30.0))).clicked() {
                        close = true;
                    }
                });
            });
        });
        if r.should_close() {
            close = true;
        }
        if remove && let Some(s) = h.sel {
            h.items.remove(s);
            config::save_history(h.ds, &h.items);
            h.sel = if h.items.is_empty() { None } else { Some(s.min(h.items.len() - 1)) };
        }
        if let Some(q) = paste {
            let console = h.console;
            self.history = None;
            self.paste_into_console(ctx, console, &q);
        } else if close {
            self.history = None;
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        if self.confirm.is_none() {
            return;
        }
        let mut answer: Option<bool> = None;
        let modal = egui::Modal::new(Id::new("confirm-unsubmitted")).backdrop_color(Color32::from_black_alpha(40)).frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x2B, 0x2D, 0x30))
                .stroke(Stroke::new(1.0, POPUP_BORDER))
                .corner_radius(10)
                .inner_margin(egui::Margin { left: 20, right: 20, top: 22, bottom: 18 }),
        );
        let r = modal.show(ctx, |ui| {
            ui.set_width(330.0);
            ui.horizontal_top(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                egui::Image::new(ic::WARNING_DIALOG).paint_at(ui, ir);
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Ignore Unsubmitted Changes").font(bold_font(14.0)).color(TEXT));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Changes are not submitted. Data will be lost.\nContinue?").font(ui_font(13.5)).color(TEXT),
                    );
                });
            });
            ui.add_space(22.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if primary_btn(ui, "Yes").clicked() {
                    answer = Some(true);
                }
                // "No" is the focused (safe) default: blue focus ring, Enter / Esc pick it
                let no = ui.add(
                    egui::Button::new(egui::RichText::new("No").color(TEXT))
                        .min_size(vec2(72.0, 28.0))
                        .stroke(Stroke::new(2.0, BRAND))
                        .fill(Color32::TRANSPARENT),
                );
                if no.clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    answer = Some(false);
                }
            });
        });
        if r.should_close() {
            answer = answer.or(Some(false));
        }
        let Some(yes) = answer else { return };
        let pending = self.confirm.take().unwrap();
        match (pending, yes) {
            (Confirm::Grid(t, acts), true) => {
                if let Some(g) = self.grid_mut(t) {
                    g.revert();
                }
                self.run_grid_actions(ctx, t, acts);
            }
            (Confirm::Grid(GridTarget::Data(id), _), false) => {
                // keep the header sort indicator in sync with what is actually loaded
                let pg = self.tabs.iter().find(|x| x.id() == id).and_then(Tab::ds).is_some_and(|ds| self.driver(ds) == Driver::Postgres);
                if let Some(Tab::Data(d)) = self.tabs.iter_mut().find(|x| x.id() == id) {
                    d.grid.server_sort = parse_sort(&d.sort, pg);
                }
            }
            (Confirm::CloseTab(id), true) => {
                if let Some(i) = self.tabs.iter().position(|t| t.id() == id) {
                    self.do_close_tab(i);
                }
            }
            (Confirm::CloseResult(id), true) => {
                self.services.results.retain(|r| r.id != id);
                self.services.active = self.services.active.min(self.services.results.len());
            }
            _ => {}
        }
    }

    fn search_popup(&mut self, ctx: &egui::Context) {
        let Some(q) = &mut self.search else { return };
        let mut open: Option<(u64, String, String)> = None;
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        let modal = egui::Modal::new(Id::new("search"))
            .frame(egui::Frame::new().fill(POPUP_BG).stroke(Stroke::new(1.0, POPUP_BORDER)).corner_radius(10).inner_margin(12));
        let r = modal.show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                paint_icon(ui, Rect::from_min_size(ui.cursor().min + vec2(0.0, 6.0), vec2(16.0, 16.0)), ic::SEARCH);
                ui.add_space(22.0);
                ui.add(
                    egui::TextEdit::singleline(q).hint_text("Type collection or table name").frame(egui::Frame::NONE).desired_width(500.0),
                )
                .request_focus();
            });
            ui.separator();
            let ql = q.to_lowercase();
            let mut n = 0;
            for s in &self.sources {
                for d in s.dbs.iter().flatten() {
                    for c in d.colls.iter().flatten().filter(|c| c.kind != 'S') {
                        if n < 14 && (ql.is_empty() || c.name.to_lowercase().contains(&ql)) {
                            n += 1;
                            ui.horizontal(|ui| {
                                let r = ui.add(egui::Button::new(egui::RichText::new(&c.name)).frame(false));
                                let place = if c.schema.is_empty() { d.name.clone() } else { format!("{}.{}", d.name, c.schema) };
                                ui.label(egui::RichText::new(format!("{place} · {}", s.ds.name)).color(TEXT_SECONDARY));
                                if r.clicked() || (n == 1 && ui.input(|i| i.key_pressed(Key::Enter))) {
                                    open = Some((s.ds.id, d.name.clone(), c.qual()));
                                }
                            });
                        }
                    }
                }
            }
            if n == 0 {
                ui.label(egui::RichText::new("Nothing found (expand databases to index their collections)").color(TEXT_SECONDARY));
            }
        });
        close |= r.should_close();
        if let Some((ds, db, c)) = open {
            self.search = None;
            self.open_data(ctx, ds, db, c);
        } else if close {
            self.search = None;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (sources, explorer, services, console, close, search) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, Key::Semicolon),
                i.consume_key(egui::Modifiers::COMMAND, Key::Num1),
                i.consume_key(egui::Modifiers::COMMAND, Key::Num8),
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L),
                i.consume_key(egui::Modifiers::COMMAND, Key::W),
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::O),
            )
        });
        if sources {
            let sel = self.selection().ds;
            self.act(ctx, Action::Dialog(sel, None));
        }
        if explorer {
            self.show_explorer = !self.show_explorer;
        }
        if services {
            self.toggle_services();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Num2)) {
            self.show_files = !self.show_files;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::ALT, Key::E))
            && let Some(Tab::Console(c)) = self.tabs.get(self.active)
        {
            let (ds, id) = (c.ds, c.id);
            self.open_history(ds, id);
        }
        if console {
            let s = self.selection();
            if let Some(ds) = s.ds {
                self.new_console(ctx, ds, s.db);
            }
        }
        // ⌘F6: Modify Table… of the selected PostgreSQL table
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F6))
            && let Sel { ds: Some(ds), db: Some(db), coll: Some(coll) } = self.selection()
            && self.driver(ds) == Driver::Postgres
            && self.coll_kind(ds, &db, &coll) == 'r'
        {
            self.act(ctx, Action::Modify(ds, db, coll));
        }
        // SQL Scripts › Generate DDL to Clipboard / to Query Console / SQL Generator… of the selected table
        let alt_cmd = egui::Modifiers::COMMAND | egui::Modifiers::ALT;
        let to = ctx.input_mut(|i| {
            if i.consume_key(alt_cmd | egui::Modifiers::SHIFT, Key::G) {
                Some(DdlTo::Clipboard)
            } else if i.consume_key(alt_cmd | egui::Modifiers::SHIFT, Key::B) {
                Some(DdlTo::Console)
            } else if i.consume_key(alt_cmd, Key::G) {
                Some(DdlTo::Panel)
            } else {
                None
            }
        });
        if let (Some(to), Sel { ds: Some(ds), db: Some(db), coll: Some(coll) }) = (to, self.selection())
            && self.driver(ds) == Driver::Postgres
            && self.coll_kind(ds, &db, &coll) != 'S'
        {
            self.act(ctx, Action::Ddl(ds, db, coll, to));
        }
        if close && !self.tabs.is_empty() {
            self.close_tab(self.active);
        }
        if search {
            self.search = Some(String::new());
        }
    }

    fn modify_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.modify else { return };
        match d.show(ctx) {
            ModifyOut::None => {}
            ModifyOut::Close => self.modify = None,
            ModifyOut::Run(script) => {
                d.running = true;
                d.error = None;
                // the client is shared with consoles: name the table's schema for this script
                let (ds, db) = (d.ds, d.db.clone());
                let sql = format!("set search_path to {};\n{script}", pg::ident(&pg::split_qual(&d.coll).0));
                self.send(ds, Req::Command { db: db.clone(), cmd: doc! {"sql": sql} }, Pending::Alter(ds, db));
            }
            ModifyOut::Console(script) => {
                let (ds, db) = (d.ds, d.db.clone());
                self.modify = None;
                self.ddl_console(ctx, ds, db, script);
            }
        }
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else { return };
        match d.show(ctx) {
            DialogOut::None => {}
            DialogOut::Cancel => self.dialog = None,
            DialogOut::Test(ds, pw) if ds.missing_password(Some(&pw)) => {
                d.test = TestState::Err(format!("Password is empty: enter the password for user '{}'", ds.user));
            }
            DialogOut::Test(ds, pw) => {
                let tx = self.tx.clone();
                let c2 = ctx.clone();
                let conn = open_conn(&ds, Some(&pw), tx, move || c2.request_repaint());
                let tag = self.next_tag;
                self.next_tag += 1;
                conn.send(tag, Req::Connect);
                self.pending.insert(tag, Pending::Test);
                self.test_conn = Some(conn);
            }
            out @ (DialogOut::Apply | DialogOut::Ok) => {
                // a password typed into the URL goes to the credentials file, not the project file;
                // the dialog keeps showing the URL as typed
                let mut items = d.items.clone();
                for ds in &mut items {
                    let pw = ds.split_password();
                    ds.url_password = pw.is_some();
                    if let Some(pw) = pw {
                        d.passwords.insert(ds.id, pw);
                    }
                }
                let pws = d.passwords.clone();
                for (id, pw) in &pws {
                    if self.passwords.get(id) != Some(pw) {
                        config::set_password(*id, pw);
                    }
                }
                self.passwords = pws;
                let mut changed = vec![];
                let mut next: Vec<Source> = vec![];
                for ds in items {
                    match self.sources.iter().position(|s| s.ds.id == ds.id) {
                        Some(i) => {
                            let mut s = self.sources.remove(i);
                            if s.ds.uri(None) != ds.uri(None) {
                                changed.push(ds.id);
                            }
                            s.ds = ds;
                            next.push(s);
                        }
                        None => {
                            next.push(Source { ds, conn: None, state: ConnState::Idle, dbs: None, all_dbs: vec![], version: String::new() })
                        }
                    }
                }
                let removed: Vec<u64> = self.sources.iter().map(|s| s.ds.id).collect();
                self.sources = next;
                for id in removed {
                    self.tabs.retain(|t| t.ds() != Some(id));
                }
                for id in changed {
                    self.disconnect(id);
                }
                self.active = self.active.min(self.tabs.len().saturating_sub(1));
                self.save_cfg();
                if matches!(out, DialogOut::Ok) {
                    self.dialog = None;
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum GridTarget {
    Data(u64),
    Result(u64),
}

#[derive(Default)]
struct Sel {
    ds: Option<u64>,
    db: Option<String>,
    coll: Option<String>,
}

#[derive(Default, Clone)]
pub(crate) struct NodeExtra {
    badge: Option<String>,
    spinner: bool,
    dot: bool,
    error: bool,
}

pub(crate) struct NodeResp {
    pub row: egui::Response,
    pub toggle: bool,
    badge_clicked: Option<egui::Pos2>,
}

/// One explorer row: chevron, icon, name, gray suffix (count / type / index columns).
#[allow(clippy::too_many_arguments)]
pub(crate) fn node(
    ui: &mut Ui,
    selected: &Option<String>,
    depth: usize,
    key: &str,
    open: Option<bool>,
    icon: ImageSource<'static>,
    label: &str,
    suffix: Option<String>,
    extra: NodeExtra,
) -> NodeResp {
    let w = ui.available_width();
    let (r, row) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::CLICK);
    let sel = selected.as_deref() == Some(key);
    row.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, label));
    let hl = r.shrink2(vec2(10.0, 0.0));
    if sel {
        let focused = ui.ctx().memory(|m| m.focused().is_none());
        ui.painter().rect_filled(hl, CornerRadius::same(4), if focused { SEL_ACTIVE } else { SEL_INACTIVE });
    } else if row.hovered() {
        ui.painter().rect_filled(hl, CornerRadius::same(4), HOVER);
    }
    let x = r.left() + 16.0 + depth as f32 * 19.0;
    let mut toggle = false;
    if let Some(o) = open {
        let cr = Rect::from_min_size(pos2(x - 2.0, r.center().y - 8.0), vec2(16.0, 16.0));
        paint_icon_tinted(ui, cr, if o { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, TEXT_MUTED);
        let c = ui.interact(cr.expand(3.0), ui.id().with(("chev", key)), Sense::CLICK);
        toggle = c.clicked();
    }
    let ir = Rect::from_min_size(pos2(x + 18.0, r.center().y - 8.0), vec2(16.0, 16.0));
    if extra.spinner {
        egui::Spinner::new().size(13.0).color(TEXT_MUTED).paint_at(ui, ir);
    } else {
        paint_icon(ui, ir, icon);
    }
    if extra.dot {
        ui.painter().circle_filled(ir.right_bottom() + vec2(-2.0, -2.0), 2.6, SUCCESS);
    }
    if extra.error {
        ui.painter().circle_filled(ir.right_bottom() + vec2(-2.0, -2.0), 2.6, ERROR_OUT);
    }
    let q: String = ui.data(|d| d.get_temp(Id::new("explorer-speed"))).unwrap_or_default();
    let t = label_with_match(ui, pos2(ir.right() + 6.0, r.center().y), label, &q);
    let mut badge_clicked = None;
    if let Some(s) = suffix
        && extra.badge.is_none()
    {
        text_at(ui, pos2(t.right() + 7.0, r.center().y + 0.5), Align2::LEFT_CENTER, &s, ui_font(11.5), TEXT_SECONDARY);
    }
    if let Some(b) = extra.badge {
        let g = ui.painter().layout_no_wrap(b, ui_font(11.5), TEXT_SECONDARY);
        let br = Rect::from_min_size(pos2(t.right() + 7.0, r.center().y - 8.0), vec2(g.size().x + 8.0, 16.0));
        let resp = ui.interact(br, ui.id().with(("badge", key)), Sense::CLICK);
        ui.painter().rect_stroke(
            br,
            CornerRadius::same(3),
            Stroke::new(1.0, if resp.hovered() { TEXT_SECONDARY } else { CONTROL_BORDER }),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(br.center() - g.size() / 2.0, g, TEXT_SECONDARY);
        if resp.clicked() {
            badge_clicked = Some(br.left_bottom() + vec2(0.0, 4.0));
        }
    }
    NodeResp { row, toggle, badge_clicked }
}

pub(crate) fn leaf_text(ui: &mut Ui, depth: usize, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    text_at(ui, pos2(r.left() + 16.0 + depth as f32 * 19.0 + 18.0, r.center().y), Align2::LEFT_CENTER, text, ui_font(12.0), TEXT_SECONDARY);
}

/// DataGrip's New menu: the explorer's `+` and the data source's New ›. `context`: the data source
/// menu flavour (Virtual View, no Import from Clipboard). What DataTired can't create yet is disabled.
fn new_menu(ui: &mut Ui, acts: &mut Vec<Action>, ds: Option<u64>, db: Option<&String>, mongo: bool, context: bool) {
    ui.set_min_width(240.0);
    if menu_item(ui, Some(ic::CONSOLE), "Query Console", "⇧⌘L", ds.is_some())
        && let Some(ds) = ds
    {
        acts.push(Action::NewConsole(ds, db.cloned()));
    }
    menu_item(ui, Some(ic::SQL), "Query File…", "⇧⌘J", false);
    if let (Some(ds), Some(db)) = (ds, db)
        && mongo
        && menu_item(ui, Some(ic::NEW_COLLECTION), "Collection", "", true)
    {
        acts.push(Action::Create(ds, db.clone()));
    }
    ui.separator();
    if context {
        menu_item(ui, Some(ic::VIRTUAL_VIEW), "Virtual View", "", false);
        ui.separator();
    }
    submenu(ui, Some(ic::DATA_SOURCE), "Data Source", true, |ui| {
        ui.set_min_width(220.0);
        menu_section(ui, "Complete Support");
        for d in [Driver::Mongo, Driver::Postgres] {
            if menu_item(ui, Some(driver_icon(d)), d.name(), "", true) {
                acts.push(Action::Dialog(None, Some(d)));
            }
        }
    });
    submenu(ui, Some(ic::CLOUD), "Data Source from Cloud Provider", false, |_| {});
    menu_item_ex(ui, Some(ic::TEMPLATE), "Data Source Templates", "", false, Some("Beta"));
    menu_item(ui, Some(ic::FOLDER_GRAY), "Data Source from File/Folder", "", false);
    menu_item(ui, Some(ic::PLUG), "Data Source from URL", "", false);
    menu_item(ui, Some(ic::DDL_SOURCE), "DDL Data Source", "", false);
    if !context {
        menu_item(ui, Some(ic::PASTE), "Import from Clipboard", "", false);
    }
    ui.separator();
    menu_item(ui, Some(ic::FOLDER_GRAY), "Folder", "", false);
    menu_item(ui, Some(ic::DRIVER), "Driver", "", false);
}

fn coll_menu(ui: &mut Ui, acts: &mut Vec<Action>, ds: u64, db: &str, coll: &str, driver: Driver, kind: char) {
    ui.set_min_width(180.0);
    let pg = driver == Driver::Postgres;
    let sub = |ui: &mut Ui, t: &str| menu_item(ui, None, &format!("{t}  ›"), "", false);
    if !pg && menu_item(ui, Some(ic::ADD), "New Collection…", "", true) {
        acts.push(Action::Create(ds, db.into()));
    }
    if menu_item(ui, None, "Modify Table…", "⌘F6", pg && kind == 'r') {
        acts.push(Action::Modify(ds, db.into(), coll.into()));
    }
    if menu_item(ui, None, "Copy Reference", "⌥⇧⌘C", true) {
        ui.ctx().copy_text(if pg { coll.to_string() } else { format!("{db}.{coll}") });
    }
    if menu_item(ui, Some(ic::TABLE), "Edit Data", "⌘↓", kind != 'S') {
        acts.push(Action::OpenData(ds, db.into(), coll.into()));
    }
    if menu_item(ui, Some(ic::CONSOLE), "New Query Console", "⌘⇧L", true) {
        acts.push(Action::NewConsole(ds, Some(db.into())));
    }
    if menu_item(ui, Some(ic::REFRESH), "Refresh", "⌘R", true) {
        acts.push(Action::LoadMeta(ds, db.into(), coll.into()));
    }
    ui.separator();
    if menu_item(ui, None, "Rename…", "⇧F6", true) {
        acts.push(Action::Rename(ds, db.into(), coll.into()));
    }
    ui.separator();
    menu_item(ui, None, "Quick Documentation", "F1", false);
    menu_item(ui, Some(ic::JUMP), "Compare Schema Structure With…", "⌘D", false);
    sub(ui, "Navigation");
    menu_item(ui, None, "Find Usages", "⌥F7", false);
    ui.separator();
    if pg {
        submenu(ui, None, "SQL Scripts", true, |ui| {
            ui.set_min_width(300.0);
            let ok = kind != 'S';
            let mut ddl = |to| acts.push(Action::Ddl(ds, db.into(), coll.into(), to));
            if menu_item(ui, Some(ic::SQL_GENERATOR), "SQL Generator…", "⌥⌘G", ok) {
                ddl(DdlTo::Panel);
            }
            ui.separator();
            menu_item(ui, Some(ic::EXPORT), "Request and Copy Original DDL", "", false);
            if menu_item(ui, Some(ic::EXPORT), "Generate DDL to Clipboard", "⌥⇧⌘G", ok) {
                ddl(DdlTo::Clipboard);
            }
            if menu_item(ui, Some(ic::EXPORT), "Generate DDL to Query Console", "⌥⇧⌘B", ok) {
                ddl(DdlTo::Console);
            }
            ui.separator();
            for t in [
                "Select all rows from a table",
                "Select first N rows from a table",
                "Insert rows into a table",
                "Update values in a table",
                "Delete rows from a table",
                "New view definition",
            ] {
                menu_item(ui, None, t, "", false);
            }
        });
    }
    sub(ui, "Tools");
    sub(ui, "Import/Export");
    sub(ui, "Diagnostics");
    ui.separator();
    if menu_item(ui, Some(ic::DELETE), "Drop…", "⌦", true) {
        acts.push(Action::Drop(ds, db.into(), Some(coll.into())));
    }
}

fn lit_key(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { k.into() } else { format!("{k:?}") }
}

/// `{age: -1}` / `age DESC` -> ("age", -1) for the header sort indicator (one column only).
fn parse_sort(s: &str, pg: bool) -> Option<(String, i32)> {
    if pg {
        let s = s.trim();
        if s.is_empty() || s.contains(',') {
            return None;
        }
        let (col, dir) = match s.rsplit_once(' ') {
            Some((c, d)) if d.eq_ignore_ascii_case("desc") => (c, -1),
            Some((c, d)) if d.eq_ignore_ascii_case("asc") => (c, 1),
            _ => (s, 1),
        };
        return Some((pg::split_qual(col.trim()).1, dir));
    }
    let inner = s.trim().strip_prefix('{')?.strip_suffix('}')?;
    let (k, v) = inner.split_once(':')?;
    let dir = v.trim().parse::<i32>().ok()?;
    Some((k.trim().trim_matches('"').trim_matches('\'').to_string(), dir.signum()))
}

impl Workspace {
    /// Caption of the active editor tab (for the window title / project tab).
    pub fn current_title(&self) -> Option<String> {
        self.tab_titles().get(self.active).map(|(_, t)| t.clone())
    }

    /// Renders the whole project window into `ui` (the shell has already polled connections).
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        native_scroll(&ctx);
        // modal dialogs first (they render on the foreground layer anyway) so they get the keyboard,
        // then the main window doesn't see keys while one is open (no Enter/⌘R leaking to the grid)
        let modal = self.modal_open();
        self.dialog(&ctx);
        self.ask_dialog(&ctx);
        self.confirm_dialog(&ctx);
        self.history_dialog(&ctx);
        self.search_popup(&ctx);
        self.modify_dialog(&ctx);
        if modal && !self.modal_open() {
            // the dialog's buttons/fields are gone; don't let their stale focus swallow the keyboard
            ctx.memory_mut(|m| {
                if let Some(f) = m.focused() {
                    m.surrender_focus(f)
                }
            });
        }
        if modal || self.modal_open() {
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                    )
                })
            });
        }
        self.shortcuts(&ctx);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, MAIN_BG);
        let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
        let status = Rect::from_min_max(pos2(full.left(), full.bottom() - STATUS_H), full.max);
        let mut top = header.bottom();
        // several open projects: one tab per project under the toolbar (DataGrip's window tabs)
        if self.shell.iter().filter(|e| e.open).count() > 1 {
            let strip = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), 32.0));
            self.project_tabs(ui, strip);
            top = strip.bottom() + 2.0;
        }
        let content = Rect::from_min_max(pos2(full.left() + GAP, top), pos2(full.right() - GAP, status.top()));
        self.header(ui, header);

        let mut acts = vec![];
        // like the IDE: the bottom tool window (Services) spans the full width under
        // Database Explorer | editor | Files
        let mut content = content;
        let generator = self.show_gen && self.sql_gen.is_some();
        if self.show_services || generator {
            self.bottom_h = self.bottom_h.clamp(140.0, (content.height() - 160.0).max(140.0));
            let bottom = Rect::from_min_max(pos2(content.left(), content.bottom() - self.bottom_h), content.max);
            content.max.y = bottom.top() - GAP;
            let split = Rect::from_min_max(pos2(content.left(), content.bottom()), pos2(content.right(), bottom.top()));
            self.bottom_h -= splitter(ui, "split-bottom", split, false);
            if generator {
                self.sql_gen_area(ui, bottom);
            } else {
                self.services_area(ui, bottom);
            }
        }
        let mut right = content;
        let mut file_acts = vec![];
        if self.show_files {
            self.right_w = self.right_w.clamp(200.0, (content.width() * 0.4).max(200.0));
            let files_rect = Rect::from_min_max(pos2(content.right() - self.right_w, content.top()), content.max);
            right.max.x = files_rect.left() - GAP;
            let split = Rect::from_min_max(pos2(right.right(), content.top()), pos2(files_rect.left(), content.bottom()));
            self.right_w -= splitter(ui, "split-right", split, true);
            let sources: Vec<(u64, String, ImageSource<'static>)> =
                self.sources.iter().map(|s| (s.ds.id, s.ds.name.clone(), driver_icon(s.ds.driver))).collect();
            let root = self.project.path.display().to_string();
            let mut roots = vec![root.clone()];
            roots.extend(self.cfg.attached.iter().filter(|a| **a != root).cloned());
            file_acts = self.files.show(ui, files_rect, &sources, &roots);
        }
        if self.show_explorer {
            self.left_w = self.left_w.clamp(220.0, (content.width() - 300.0).max(220.0));
            let left = Rect::from_min_size(content.min, vec2(self.left_w, content.height()));
            acts = self.explorer(ui, left);
            right.min.x = left.right() + GAP;
            let split = Rect::from_min_max(pos2(left.right(), content.top()), pos2(right.left(), content.bottom()));
            self.left_w += splitter(ui, "split-left", split, true);
        }
        self.editor_area(ui, right);
        self.status_bar(ui, status);
        for a in acts {
            self.act(&ctx, a);
        }
        for a in file_acts {
            self.file_action(&ctx, a);
        }
        if let Some(rx) = &self.picker
            && let Ok(res) = rx.try_recv()
        {
            self.picker = None;
            match res {
                Some(p) if !p.is_empty() => {
                    if !self.cfg.attached.contains(&p) {
                        self.cfg.attached.push(p);
                        self.save_cfg();
                    }
                }
                // no native picker (or cancelled with error): ask for a path
                None if !cfg!(target_os = "macos") && !cfg!(windows) => self.ask = Some(Ask::AttachPath(String::new())),
                _ => {}
            }
        }
        for t in &mut self.tabs {
            t.flush(false);
        }
    }

    pub fn save(&mut self) {
        self.save_cfg();
    }

    pub fn set_status(&mut self, s: String) {
        self.status = Some(s);
    }

    /// DataGrip's window tabs: equal-width tabs; the active one is an island-coloured pill, the
    /// others blend into the main background.
    fn project_tabs(&mut self, ui: &mut Ui, strip: Rect) {
        let open: Vec<ProjectEntry> = self.shell.iter().filter(|e| e.open).cloned().collect();
        let inner = strip.shrink2(vec2(GAP, 3.0));
        let gap = 6.0;
        let w = (inner.width() - gap * (open.len() - 1) as f32) / open.len() as f32;
        for (i, e) in open.iter().enumerate() {
            let r = Rect::from_min_size(pos2(inner.left() + i as f32 * (w + gap), inner.top()), vec2(w, inner.height()));
            let resp = ui.interact(r, Id::new(("project-tab", &e.project.path)), Sense::CLICK);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, e.active, &e.caption));
            if e.active {
                ui.painter().rect_filled(r, CornerRadius::same(8), ISLAND);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(8), HOVER);
            }
            let col = if e.active { TEXT } else { TEXT_MUTED };
            ui.painter().with_clip_rect(r.shrink2(vec2(30.0, 0.0))).text(r.center(), Align2::CENTER_CENTER, &e.caption, ui_font(13.5), col);
            // close button on the left, shown on hover (macOS window tab)
            let cr = Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(18.0, 18.0));
            if resp.hovered() || ui.rect_contains_pointer(cr) {
                let c = ui.interact(cr, Id::new(("project-tab-x", &e.project.path)), Sense::CLICK);
                if c.hovered() {
                    ui.painter().rect_filled(cr, CornerRadius::same(4), TOOLBAR_HOVER);
                }
                paint_icon_tinted(ui, cr, ic::CLOSE_SMALL, TEXT_MUTED);
                if c.clicked() {
                    self.cmd = Some(ProjectCmd::Close(e.project.path.clone()));
                    continue;
                }
            }
            if resp.clicked() && !e.active {
                self.cmd = Some(ProjectCmd::Switch(e.project.path.clone()));
            }
        }
    }
}

/// `RecentProject.ColorN.Avatar.Start/End` (Islands Dark), N = 1..9.
const PROJECT_AVATAR: [(u32, u32); 9] = [
    (0xE08855, 0xE9806F),
    (0xB08B14, 0xBB7F19),
    (0xA1A359, 0x87AA59),
    (0x3B92B8, 0x6183EC),
    (0x3574F0, 0x7A64F0),
    (0xC84D8F, 0xA956CF),
    (0x955AE0, 0xA84DE0),
    (0x24A394, 0x279CCD),
    (0x5FAD65, 0x3D968B),
];
/// `RecentProject.ColorN.MainToolbarGradientStart` (Islands Dark).
const PROJECT_TOOLBAR: [u32; 9] = [0x664832, 0x5E4724, 0x45522F, 0x335661, 0x394773, 0x613861, 0x503D70, 0x325959, 0x365439];

fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn project_toolbar_color(i: usize) -> Color32 {
    hex(PROJECT_TOOLBAR[i % 9])
}

/// Rounded square with the project's diagonal gradient and white initials.
pub fn project_badge(ui: &Ui, r: Rect, name: &str, color: usize) {
    let (a, b) = PROJECT_AVATAR[color % 9];
    let (a, b) = (hex(a), hex(b));
    let rad = r.width() * 0.25;
    // rounded-rect outline as a fan mesh, each vertex coloured along the top-left → bottom-right diagonal
    let mut pts = Vec::new();
    for (cx, cy, start) in [
        (r.right() - rad, r.top() + rad, -90.0f32),
        (r.right() - rad, r.bottom() - rad, 0.0),
        (r.left() + rad, r.bottom() - rad, 90.0),
        (r.left() + rad, r.top() + rad, 180.0),
    ] {
        for k in 0..=6 {
            let t = (start + k as f32 * 15.0).to_radians();
            pts.push(pos2(cx + rad * t.cos(), cy + rad * t.sin()));
        }
    }
    let col = |p: egui::Pos2| {
        let t = (((p.x - r.left()) + (p.y - r.top())) / (r.width() + r.height())).clamp(0.0, 1.0);
        a.lerp_to_gamma(b, t)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.center(), col(r.center()));
    for p in &pts {
        mesh.colored_vertex(*p, col(*p));
    }
    for i in 0..pts.len() as u32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % pts.len() as u32);
    }
    ui.painter().add(mesh);
    let size = if r.width() > 24.0 { 13.0 } else { 10.0 };
    text_at(ui, r.center(), Align2::CENTER_CENTER, &config::initials(name), bold_font(size), Color32::WHITE);
}

enum RowAction {
    Open,
    Remove,
}

/// Two-line project row in the widget popup: badge, name, `~/path`; recent ones can be removed.
fn project_row(ui: &mut Ui, e: &ProjectEntry) -> Option<RowAction> {
    let home = config::home().display().to_string();
    let path = e.project.path.display().to_string();
    let path = if !home.is_empty() && path.starts_with(&home) { format!("~{}", &path[home.len()..]) } else { path };
    let natural = 44.0 + text_w(ui, &e.project.name, 13.0).max(text_w(ui, &path, 12.0)) + if e.open { 14.0 } else { 34.0 };
    let w = menu_row_width(ui, natural);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 48.0), Sense::CLICK);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, e.active, &e.project.name));
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(4), SEL_ACTIVE);
    }
    project_badge(ui, Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 13.0), vec2(26.0, 26.0)), &e.project.name, e.project.color);
    text_at(ui, pos2(r.left() + 44.0, r.top() + 15.0), Align2::LEFT_CENTER, &e.project.name, ui_font(13.0), TEXT);
    ui.painter().with_clip_rect(r.shrink(2.0)).text(
        pos2(r.left() + 44.0, r.top() + 34.0),
        Align2::LEFT_CENTER,
        path,
        ui_font(12.0),
        TEXT_SECONDARY,
    );
    if !e.open && resp.hovered() {
        let xr = Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(18.0, 18.0));
        let x = ui.interact(xr, ui.id().with(("recent-x", &e.project.path)), Sense::CLICK).on_hover_text("Remove from Recent Projects");
        paint_icon_tinted(ui, xr, ic::CLOSE_SMALL, TEXT_MUTED);
        if x.clicked() {
            ui.close();
            return Some(RowAction::Remove);
        }
    }
    if resp.clicked() {
        ui.close();
        return Some(RowAction::Open);
    }
    None
}

/// `db_pegasus.public.sourc...onfigs [DS]`: shortens the part before ` [`, keeping the data source.
fn mid_ellipsis(s: &str, max: usize) -> String {
    let (head, tail) = s.find(" [").map_or((s, ""), |i| s.split_at(i));
    let room = max.saturating_sub(tail.chars().count()).max(12);
    let n = head.chars().count();
    if n <= room {
        return s.to_string();
    }
    let keep = room - 3;
    let a: String = head.chars().take(keep * 3 / 4).collect();
    let b: String = head.chars().skip(n - (keep - keep * 3 / 4)).collect();
    format!("{a}...{b}{tail}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn shortens_tab_titles_in_the_middle() {
        assert_eq!(super::mid_ellipsis("shop.customers [local]", 48), "shop.customers [local]");
        let t = super::mid_ellipsis("db_pegasus.public.source_service_configs [Meowlong Production]", 56);
        assert!(t.starts_with("db_pegasus.public.s") && t.contains("...") && t.ends_with("onfigs [Meowlong Production]"), "{t}");
    }
}

/// Scroll exactly by this frame's wheel/trackpad events, like native macOS views. egui's own
/// smoothing delays momentum deltas >= 8 px by ~0.1 s and drops what is pending when the finger
/// lifts, which made scrolling feel sticky.
fn native_scroll(ctx: &egui::Context) {
    let line = ctx.options(|o| o.input_options.line_scroll_speed);
    ctx.input_mut(|i| {
        let page = i.viewport_rect().height();
        let mut d = egui::Vec2::ZERO;
        for e in &i.events {
            if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                if modifiers.command || modifiers.ctrl {
                    return; // zoom gesture: leave egui's handling alone
                }
                d += match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => *delta * line,
                    egui::MouseWheelUnit::Page => *delta * page,
                };
            }
        }
        i.smooth_scroll_delta = d;
    });
}

/// Label with the speed-search match on `SearchMatch.startBackground` (#BA9752), dark text on it.
fn label_with_match(ui: &Ui, pos: egui::Pos2, label: &str, q: &str) -> Rect {
    let t = text_at(ui, pos, Align2::LEFT_CENTER, label, ui_font(13.0), TEXT);
    let lower = label.to_lowercase();
    if q.is_empty() || lower.len() != label.len() {
        return t;
    }
    if let Some(start) = lower.find(&q.to_lowercase()) {
        let end = start + q.len();
        let x0 = ui.fonts_mut(|f| f.layout_no_wrap(label[..start].to_string(), ui_font(13.0), TEXT).size().x);
        let g = ui.fonts_mut(|f| f.layout_no_wrap(label[start..end].to_string(), ui_font(13.0), TEXT));
        let hr = Rect::from_min_size(pos2(t.left() + x0 - 1.0, t.center().y - 9.0), vec2(g.size().x + 2.0, 18.0));
        ui.painter().rect_filled(hr, CornerRadius::same(3), Color32::from_rgb(0xBA, 0x97, 0x52));
        ui.painter().galley(pos2(t.left() + x0, t.center().y - g.size().y / 2.0), g, Color32::from_rgb(0x1E, 0x1F, 0x22));
    }
    t
}

/// Speed search box over the top of the tree: search icon + query, red when nothing matches.
fn speed_search_popup(ui: &Ui, tree: Rect, q: &str, found: bool) {
    let w = (ui.fonts_mut(|f| f.layout_no_wrap(q.to_string(), ui_font(13.0), TEXT).size().x) + 46.0).clamp(120.0, tree.width() - 16.0);
    let r = Rect::from_min_size(tree.left_top() + vec2(10.0, 2.0), vec2(w, 28.0));
    egui::Area::new(Id::new("explorer-speed-popup")).order(egui::Order::Foreground).fixed_pos(r.min).interactable(false).show(
        ui.ctx(),
        |ui| {
            let (r, _) = ui.allocate_exact_size(r.size(), Sense::hover());
            ui.painter().rect(r, CornerRadius::same(6), ISLAND, Stroke::new(1.0, CONTROL_BORDER), egui::StrokeKind::Inside);
            paint_icon_tinted(ui, Rect::from_min_size(r.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), ic::SEARCH, TEXT_MUTED);
            let col = if found || q.is_empty() { TEXT } else { Color32::from_rgb(0xF5, 0x7E, 0x84) };
            let t = text_at(ui, pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, q, ui_font(13.0), col);
            // caret
            if (ui.input(|i| i.time) * 2.0) as i64 % 2 == 0 {
                ui.painter().vline(t.right() + 1.0, r.y_range().shrink(7.0), Stroke::new(1.2, CARET));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        },
    );
}
