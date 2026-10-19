//! Connection worker: one thread per data source owning the MongoDB client and a QuickJS
//! context running the mongosh shim (`shell.js`). The UI talks to it through channels.

use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use mongodb::options::{ClientOptions, ResolverConfig};
use mongodb::sync::{Client, Database};
use rquickjs::{Context, Ctx, Function, Runtime, Value};
use serde_json::{Value as Json, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SHELL_JS: &str = include_str!("shell.js");
/// Rows fetched by `toArray()` / `forEach()` in scripts.
const SCRIPT_FETCH_CAP: u64 = 100_000;

pub enum Req {
    Connect,
    Databases,
    Collections {
        db: String,
    },
    Meta {
        db: String,
        coll: String,
    },
    /// `schema`: PostgreSQL search path for the console (MongoDB ignores it).
    Run {
        db: String,
        schema: String,
        script: String,
    },
    Page {
        spec: Document,
        offset: u64,
        limit: u64,
    },
    Find {
        db: String,
        coll: String,
        filter: String,
        sort: String,
        offset: u64,
        limit: u64,
    },
    Save {
        db: String,
        coll: String,
        edits: Vec<Edit>,
    },
    Command {
        db: String,
        cmd: Document,
    },
    /// What the SQL Generator needs of a PostgreSQL table / view.
    Ddl {
        db: String,
        coll: String,
    },
}

pub enum Edit {
    Set {
        id: Bson,
        field: String,
        text: String,
        as_string: bool,
    },
    /// (field, text, text is a plain string — else a shell literal).
    Insert {
        fields: Vec<(String, String, bool)>,
    },
    Delete {
        id: Bson,
    },
}

pub struct Resp {
    pub tag: u64,
    pub body: Result<Body, String>,
}

pub enum Body {
    Connected {
        version: String,
    },
    Databases(Vec<String>),
    /// `colls`: (schema, name, kind) with kind `r` table / collection, `v` view, `m` materialized view,
    /// `S` sequence (MongoDB: no schemas, schema empty).
    Collections {
        schemas: Vec<String>,
        colls: Vec<(String, String, char)>,
    },
    Meta(Meta),
    Ran(Vec<RunItem>),
    Rows(ResultSet),
    Done(String),
    Ddl(Box<crate::pg::Rel>),
}

/// Explorer children of a collection / table, by folder (`fields`, `columns`, `keys`, `indexes`, …).
pub type Meta = Vec<(String, Vec<MetaItem>)>;

#[derive(Clone)]
pub struct MetaItem {
    pub name: String,
    /// Gray text after the name: type, `(cols) UNIQUE`, `(col) → table (col)`, …
    pub detail: String,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ItemKind {
    Field,
    IdField,
    Column,
    PkColumn,
    NotNullColumn,
    PrimaryKey,
    Key,
    ForeignKey,
    Index,
    UniqueIndex,
    Check,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LogKind {
    Info,
    Echo,
    /// Echo of a SQL statement (highlighted as SQL in the output pane).
    EchoSql,
    Error,
}

pub enum RunItem {
    Log(String, LogKind),
    Result(ResultSet),
}

#[derive(Clone)]
pub struct ResultSet {
    pub title: String,
    pub docs: Vec<Document>,
    pub offset: u64,
    pub page: u64,
    pub more: bool,
    /// Query that produced the rows (for paging/refresh); `None` for literal values.
    pub spec: Option<Document>,
    pub exec_ms: u128,
    pub fetch_ms: u128,
    /// SQL results: (column, type) in select order; empty for MongoDB (columns come from the documents).
    pub cols: Vec<(String, String)>,
}

impl ResultSet {
    pub fn sql(&self) -> bool {
        !self.cols.is_empty()
    }

    /// `(db, coll)` when rows are an unprojected find over a collection (or SQL rows of one table with
    /// its primary key), i.e. editable.
    pub fn table(&self) -> Option<(String, String)> {
        let s = self.spec.as_ref()?;
        if let Ok(t) = s.get_str("table") {
            return Some((s.get_str("db").unwrap_or_default().into(), t.into()));
        }
        let plain = s.get_str("op") == Ok("find") && !matches!(s.get("projection"), Some(Bson::Document(p)) if !p.is_empty());
        plain.then(|| (s.get_str("db").unwrap_or_default().into(), s.get_str("coll").unwrap_or_default().into()))
    }

    /// Columns identifying a row for edits: the SQL primary key, `_id` for MongoDB.
    pub fn key(&self) -> Vec<String> {
        match self.spec.as_ref().and_then(|s| s.get_array("key").ok()) {
            Some(a) => a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect(),
            None => vec!["_id".into()],
        }
    }
}

/// Interrupts the statement in flight (PostgreSQL cancel request); set by the worker.
pub type Hook = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

pub struct Conn {
    tx: Sender<(u64, Req)>,
    pub cancel: Arc<AtomicBool>,
    hook: Hook,
}

impl Conn {
    pub fn open(uri: String, out: Sender<Resp>, wake: impl Fn() + Send + 'static) -> Conn {
        Conn::spawn(out, wake, move |cancel, _| {
            let mut w = Worker::new(&uri, cancel)?;
            Ok(move |req| w.handle(req))
        })
    }

    /// Worker thread: `start` builds the request handler (an error answers every request with it).
    pub fn spawn<H: FnMut(Req) -> Result<Body, String>>(
        out: Sender<Resp>,
        wake: impl Fn() + Send + 'static,
        start: impl FnOnce(Arc<AtomicBool>, Hook) -> Result<H, String> + Send + 'static,
    ) -> Conn {
        let (tx, rx) = channel::<(u64, Req)>();
        let cancel = Arc::new(AtomicBool::new(false));
        let hook: Hook = Arc::new(Mutex::new(None));
        let (cancel2, hook2) = (cancel.clone(), hook.clone());
        std::thread::spawn(move || {
            let mut handle = match start(cancel2.clone(), hook2) {
                Ok(h) => h,
                Err(e) => {
                    for (tag, _) in rx {
                        let _ = out.send(Resp { tag, body: Err(e.clone()) });
                        wake();
                    }
                    return;
                }
            };
            for (tag, req) in rx {
                cancel2.store(false, Ordering::Relaxed);
                // a bug must not leave the request unanswered (the console would stay "running")
                let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(req))).unwrap_or_else(|p| {
                    let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                    Err(format!("Internal error: {}", msg.unwrap_or_else(|| "panic".into())))
                });
                if out.send(Resp { tag, body }).is_err() {
                    return;
                }
                wake();
            }
        });
        Conn { tx, cancel, hook }
    }

    pub fn send(&self, tag: u64, req: Req) {
        let _ = self.tx.send((tag, req));
    }

    /// Cancel running statements: stops scripts between statements and interrupts a SQL query in flight.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(f) = &*self.hook.lock().unwrap_or_else(|e| e.into_inner()) {
            f();
        }
    }
}

struct Worker {
    client: Client,
    _rt: Runtime,
    cx: Context,
    printed: Rc<RefCell<Vec<String>>>,
    cancel: Arc<AtomicBool>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The resolver couldn't even read the system DNS configuration (as opposed to the name not existing).
fn is_resolver_conf_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("dns resolution") && (m.contains("nameserver") || m.contains("resolv") || m.contains("system conf"))
}

impl Worker {
    fn new(uri: &str, cancel: Arc<AtomicBool>) -> Result<Worker, String> {
        let mut opts = match ClientOptions::parse(uri).run() {
            Ok(o) => o,
            // The system resolver config can hold entries the Rust DNS resolver can't parse (macOS
            // puts the router's link-local IPv6 there: `nameserver fe80::1%en0`). SRV/TXT records of
            // hosted clusters are public, so retry with public resolvers instead of failing.
            Err(e) if uri.starts_with("mongodb+srv://") && is_resolver_conf_error(&e.to_string()) => ClientOptions::parse(uri)
                .resolver_config(ResolverConfig::cloudflare())
                .run()
                .or_else(|_| ClientOptions::parse(uri).resolver_config(ResolverConfig::google()).run())
                .map_err(|e2| format!("{e} (retry with public DNS: {e2})"))?,
            Err(e) => return Err(err(e)),
        };
        opts.server_selection_timeout.get_or_insert(Duration::from_secs(5));
        opts.connect_timeout.get_or_insert(Duration::from_secs(5));
        opts.app_name = Some("DataTired".into());
        let client = Client::with_options(opts).map_err(err)?;

        let rt = Runtime::new().map_err(err)?;
        let c = cancel.clone();
        rt.set_interrupt_handler(Some(Box::new(move || c.load(Ordering::Relaxed))));
        let cx = Context::full(&rt).map_err(err)?;
        let printed = Rc::new(RefCell::new(Vec::new()));
        cx.with(|ctx| -> Result<(), String> {
            let g = ctx.globals();
            let (cl, stop) = (client.clone(), cancel.clone());
            let native = Function::new(ctx.clone(), move |s: String| native(&cl, &stop, &s).to_string()).map_err(err)?;
            g.set("__native", native).map_err(err)?;
            let p = printed.clone();
            let print = Function::new(ctx.clone(), move |s: String| p.borrow_mut().push(s)).map_err(err)?;
            g.set("__print", print).map_err(err)?;
            ctx.eval::<(), _>(SHELL_JS).map_err(|e| js_err(&ctx, e))
        })?;
        Ok(Worker { client, _rt: rt, cx, printed, cancel })
    }

    fn handle(&mut self, req: Req) -> Result<Body, String> {
        let c = &self.client;
        match req {
            Req::Connect => {
                let info = c.database("admin").run_command(doc! {"buildInfo": 1}).run().map_err(err)?;
                Ok(Body::Connected { version: info.get_str("version").unwrap_or("?").into() })
            }
            Req::Databases => {
                let mut names = c.list_database_names().run().map_err(err)?;
                names.sort();
                Ok(Body::Databases(names))
            }
            Req::Collections { db } => {
                let mut colls: Vec<(String, String, char)> = c
                    .database(&db)
                    .list_collections()
                    .run()
                    .map_err(err)?
                    .filter_map(Result::ok)
                    .map(|s| {
                        let view = matches!(s.collection_type, mongodb::results::CollectionType::View);
                        (String::new(), s.name, if view { 'v' } else { 'r' })
                    })
                    .collect();
                colls.sort();
                Ok(Body::Collections { schemas: vec![], colls })
            }
            Req::Meta { db, coll } => meta(c, &db, &coll),
            Req::Run { db, script, .. } => Ok(Body::Ran(self.run(&db, &script))),
            Req::Page { spec, offset, limit } => {
                let t = Instant::now();
                let (docs, more) = exec_spec(c, &spec, offset, Some(limit), &self.cancel)?;
                let ms = t.elapsed().as_millis();
                Ok(Body::Rows(ResultSet {
                    title: title_of(&spec),
                    docs,
                    offset,
                    page: limit,
                    more,
                    spec: Some(spec),
                    exec_ms: ms,
                    fetch_ms: 0,
                    cols: vec![],
                }))
            }
            Req::Find { db, coll, filter, sort, offset, limit } => {
                let mut spec = doc! {"db": &db, "coll": &coll, "op": "find", "filter": self.literal(&filter)?};
                if !sort.trim().is_empty() {
                    spec.insert("sort", self.literal(&sort)?);
                }
                self.handle(Req::Page { spec, offset, limit })
            }
            Req::Save { db, coll, edits } => {
                // Err only when nothing was written (the grid keeps its pending changes); a failure after
                // some writes (no transactions on a standalone server) is reported with the reload
                let d = c.database(&db);
                let mut n = 0;
                for e in edits {
                    let cmd = match e {
                        Edit::Set { id, field, text, as_string } => {
                            let v = self.cell_value(&text, as_string);
                            doc! {"update": &coll, "updates": [{"q": {"_id": id}, "u": {"$set": {field: v}}}]}
                        }
                        Edit::Insert { fields } => {
                            let mut doc = Document::new();
                            for (k, t, as_string) in fields {
                                doc.insert(k, self.cell_value(&t, as_string));
                            }
                            ensure_id(&mut doc);
                            doc! {"insert": &coll, "documents": [doc]}
                        }
                        Edit::Delete { id } => doc! {"delete": &coll, "deletes": [{"q": {"_id": id}, "limit": 1}]},
                    };
                    match write(&d, cmd) {
                        Ok(_) => n += 1,
                        Err(e) if n == 0 => return Err(e),
                        Err(e) => return Ok(Body::Done(format!("{n} row(s) affected, then failed: {e}"))),
                    }
                }
                Ok(Body::Done(format!("{n} row(s) affected")))
            }
            Req::Command { db, cmd } => {
                let r = c.database(&db).run_command(cmd).run().map_err(err)?;
                Ok(Body::Done(bson::Bson::Document(r).into_relaxed_extjson().to_string()))
            }
            Req::Ddl { .. } => Err("SQL Generator supports PostgreSQL only".into()),
        }
    }

    /// JS literal (`{age: {$gt: 1}}`, `ObjectId("…")`) -> BSON, via the shell.
    fn literal(&self, src: &str) -> Result<Bson, String> {
        let src = if src.trim().is_empty() { "{}" } else { src };
        let s = self.cx.with(|ctx| -> Result<String, String> {
            let f: Function = ctx.globals().get("__eval_literal").map_err(err)?;
            f.call((src,)).map_err(|e| js_err(&ctx, e))
        })?;
        to_bson(serde_json::from_str(&s).map_err(err)?)
    }

    /// Grid cell text -> BSON. String columns keep text verbatim; otherwise try a JS literal.
    fn cell_value(&self, text: &str, as_string: bool) -> Bson {
        if as_string {
            return Bson::String(text.into());
        }
        self.literal(text).unwrap_or_else(|_| Bson::String(text.into()))
    }

    fn run(&mut self, db: &str, script: &str) -> Vec<RunItem> {
        let mut items = Vec::new();
        let mut db = db.to_string();
        for (a, b) in split_statements(script) {
            if self.cancel.load(Ordering::Relaxed) {
                items.push(RunItem::Log(stamp("Cancelled"), LogKind::Error));
                break;
            }
            let stmt = script[a..b].trim().trim_end_matches(';').trim();
            items.push(RunItem::Log(stamp(&format!("{db}> {stmt}")), LogKind::Echo));
            let t = Instant::now();
            match self.statement(&mut db, stmt) {
                Ok(Some(mut rs)) => {
                    rs.exec_ms = t.elapsed().as_millis() - rs.fetch_ms.min(t.elapsed().as_millis());
                    let msg = format!(
                        "{} row{} retrieved starting from 1 in {} ms (execution: {} ms, fetching: {} ms)",
                        rs.docs.len(),
                        if rs.docs.len() == 1 { "" } else { "s" },
                        t.elapsed().as_millis(),
                        rs.exec_ms,
                        rs.fetch_ms
                    );
                    self.flush_prints(&mut items);
                    items.push(RunItem::Result(rs));
                    items.push(RunItem::Log(stamp(&msg), LogKind::Info));
                }
                Ok(None) => {
                    self.flush_prints(&mut items);
                    items.push(RunItem::Log(stamp(&format!("completed in {} ms", t.elapsed().as_millis())), LogKind::Info));
                }
                Err(e) => {
                    self.flush_prints(&mut items);
                    items.push(RunItem::Log(stamp(&e), LogKind::Error));
                    break;
                }
            }
        }
        items
    }

    fn flush_prints(&self, items: &mut Vec<RunItem>) {
        for p in self.printed.borrow_mut().drain(..) {
            items.push(RunItem::Log(stamp(&p), LogKind::Info));
        }
    }

    fn statement(&mut self, db: &mut String, stmt: &str) -> Result<Option<ResultSet>, String> {
        let words: Vec<&str> = stmt.split_whitespace().collect();
        let lit = |docs: Vec<Document>, title: &str| ResultSet {
            title: title.into(),
            docs,
            offset: 0,
            page: 0,
            more: false,
            spec: None,
            exec_ms: 0,
            fetch_ms: 0,
            cols: vec![],
        };
        match words.as_slice() {
            ["use", name] => {
                *db = name.to_string();
                self.printed.borrow_mut().push(format!("switched to db {db}"));
                return Ok(None);
            }
            ["show", "dbs" | "databases"] => {
                let r = self.client.database("admin").run_command(doc! {"listDatabases": 1}).run().map_err(err)?;
                let docs =
                    r.get_array("databases").map(|a| a.iter().filter_map(|d| d.as_document().cloned()).collect()).unwrap_or_default();
                return Ok(Some(lit(docs, "databases")));
            }
            ["show", "collections" | "tables"] => {
                let mut names: Vec<String> = self.client.database(db).list_collection_names().run().map_err(err)?;
                names.sort();
                return Ok(Some(lit(names.into_iter().map(|n| doc! {"name": n}).collect(), "collections")));
            }
            _ => {}
        }
        // `db.c.find(…).toArray()` as a statement's result: page through the cursor like `find(…)`
        // instead of pulling every document (up to 100 000) through the JS engine first
        let lazy = (stmt.starts_with("db.") || stmt.starts_with("db[")) && stmt.ends_with(".toArray()");
        let src = if lazy { &stmt[..stmt.len() - ".toArray()".len()] } else { stmt };
        let desc = self.cx.with(|ctx| -> Result<String, String> {
            ctx.globals().set("__currentDb", db.as_str()).map_err(err)?;
            let mut v: Value = ctx.eval(src.as_bytes().to_vec()).map_err(|e| js_err(&ctx, e))?;
            if lazy {
                let finish: Function = ctx.globals().get("__finish").map_err(err)?;
                v = finish.call((v,)).map_err(|e| js_err(&ctx, e))?;
            }
            let f: Function = ctx.globals().get("__describe").map_err(err)?;
            let s = f.call::<_, String>((v,)).map_err(|e| js_err(&ctx, e))?;
            // keep `db` switches done from JS (db = db.getSiblingDB(...) is not supported, but use() via JS is)
            Ok(s)
        })?;
        let d: Json = serde_json::from_str(&desc).map_err(err)?;
        match d["kind"].as_str() {
            Some("cursor") => {
                let spec = match to_bson(d["spec"].clone())? {
                    Bson::Document(s) => s,
                    _ => return Err("bad cursor".into()),
                };
                let t = Instant::now();
                let (docs, more) = exec_spec(&self.client, &spec, 0, Some(crate::PAGE), &self.cancel)?;
                let fetch = t.elapsed().as_millis();
                Ok(Some(ResultSet {
                    title: title_of(&spec),
                    docs,
                    offset: 0,
                    page: crate::PAGE,
                    more,
                    spec: Some(spec),
                    exec_ms: 0,
                    fetch_ms: fetch,
                    cols: vec![],
                }))
            }
            Some("value") => match to_bson(d["value"].clone())? {
                Bson::Document(doc) => Ok(Some(lit(vec![doc], "result"))),
                Bson::Array(a) if a.iter().all(|x| matches!(x, Bson::Document(_))) && !a.is_empty() => {
                    Ok(Some(lit(a.into_iter().filter_map(|x| x.as_document().cloned()).collect(), "result")))
                }
                Bson::Array(a) => Ok(Some(lit(a.into_iter().map(|v| doc! {"value": v}).collect(), "result"))),
                v => {
                    self.printed.borrow_mut().push(display(&v));
                    Ok(None)
                }
            },
            _ => Ok(None),
        }
    }
}

fn title_of(spec: &Document) -> String {
    match spec.get_str("coll") {
        Ok(c) => format!("{}.{}", spec.get_str("db").unwrap_or("?"), c),
        Err(_) => spec.get_str("db").unwrap_or("result").into(),
    }
}

fn js_err(ctx: &Ctx, e: rquickjs::Error) -> String {
    if matches!(e, rquickjs::Error::Exception) {
        let ex = ctx.catch();
        if let Some(x) = ex.as_exception() {
            let msg = x.message().unwrap_or_else(|| "exception".into());
            return match x.get::<_, String>("name") {
                Ok(n) if n != "Error" => format!("{n}: {msg}"),
                _ => msg,
            };
        }
        if let Some(s) = ex.as_string() {
            return s.to_string().unwrap_or_default();
        }
        return format!("{ex:?}");
    }
    if matches!(e, rquickjs::Error::Unknown) {
        return "Interrupted".into();
    }
    e.to_string()
}

/// Local wall-clock prefix `[yyyy-MM-dd HH:mm:ss] `, like DataGrip's output pane.
pub fn stamp(msg: &str) -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] {msg}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Plain-text rendering of a scalar for the output pane.
pub fn display(v: &Bson) -> String {
    match v {
        Bson::String(s) => s.clone(),
        Bson::Null => "null".into(),
        other => other.clone().into_relaxed_extjson().to_string(),
    }
}

fn to_bson(v: Json) -> Result<Bson, String> {
    Bson::try_from(v).map_err(err)
}

fn to_doc(v: Option<&Json>) -> Result<Document, String> {
    match v {
        None | Some(Json::Null) => Ok(Document::new()),
        Some(v) => match to_bson(v.clone())? {
            Bson::Document(d) => Ok(d),
            other => Err(format!("expected a document, got {}", display(&other))),
        },
    }
}

/// BSON -> JSON handed to JS. Int64 stays tagged so it survives a round trip.
fn js(b: &Bson) -> Json {
    match b {
        Bson::Int32(i) => json!(i),
        Bson::Double(f) if f.is_finite() => json!(f),
        Bson::Int64(i) => json!({"$numberLong": i.to_string()}),
        Bson::Document(d) => Json::Object(d.iter().map(|(k, v)| (k.clone(), js(v))).collect()),
        Bson::Array(a) => Json::Array(a.iter().map(js).collect()),
        other => other.clone().into_relaxed_extjson(),
    }
}

/// Runs a command; insert / update / delete report rejected documents (duplicate key, validation)
/// in `writeErrors` with `ok: 1`, which become errors here instead of silent successes.
fn write(db: &Database, cmd: Document) -> Result<Document, String> {
    let r = db.run_command(cmd).run().map_err(err)?;
    let first_error = r.get_array("writeErrors").ok().and_then(|a| a.first()).and_then(Bson::as_document).cloned();
    if let Some(e) = first_error.or_else(|| r.get_document("writeConcernError").ok().cloned()) {
        return Err(e.get_str("errmsg").unwrap_or("write error").to_string());
    }
    Ok(r)
}

fn ensure_id(doc: &mut Document) {
    if !doc.contains_key("_id") {
        let mut d = doc! {"_id": ObjectId::new()};
        d.extend(std::mem::take(doc));
        *doc = d;
    }
}

fn num(v: Option<&Bson>) -> u64 {
    match v {
        Some(Bson::Int32(i)) => i.unsigned_abs() as u64,
        Some(Bson::Int64(i)) => i.unsigned_abs(),
        Some(Bson::Double(f)) => f.abs() as u64,
        _ => 0,
    }
}

/// Executes a find/aggregate spec. `page` = rows wanted (one extra is fetched to report `more`).
/// Documents of a cursor spec (`page`: at most that many, plus one to tell whether there are more).
/// `cancel` (Stop / ⌘F2) aborts between documents, so a big fetch doesn't block the connection.
pub fn exec_spec(c: &Client, spec: &Document, offset: u64, page: Option<u64>, cancel: &AtomicBool) -> Result<(Vec<Document>, bool), String> {
    let take = |cur: mongodb::sync::Cursor<Document>| -> Result<Vec<Document>, String> {
        let mut docs = vec![];
        for d in cur {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            docs.push(d.map_err(err)?);
        }
        Ok(docs)
    };
    let db = c.database(spec.get_str("db").unwrap_or("test"));
    let skip = num(spec.get("skip")) + offset;
    let user_limit = num(spec.get("limit"));
    let mut want = page.map(|p| p + 1);
    if user_limit > 0 {
        let left = user_limit.saturating_sub(offset);
        want = Some(want.map_or(left, |w| w.min(left)));
    }
    if want == Some(0) {
        return Ok((vec![], false));
    }
    let docs: Vec<Document> = if spec.get_str("op") == Ok("aggregate") {
        let mut pipeline: Vec<Document> =
            spec.get_array("pipeline").map(|a| a.iter().filter_map(|d| d.as_document().cloned()).collect()).unwrap_or_default();
        let writes = pipeline.last().is_some_and(|s| s.contains_key("$out") || s.contains_key("$merge"));
        if !writes {
            if skip > 0 {
                pipeline.push(doc! {"$skip": skip as i64});
            }
            if let Some(w) = want {
                pipeline.push(doc! {"$limit": w as i64});
            }
        }
        let cur = match spec.get_str("coll") {
            Ok(coll) => db.collection::<Document>(coll).aggregate(pipeline).run(),
            Err(_) => db.aggregate(pipeline).run(),
        };
        take(cur.map_err(err)?)?
    } else {
        let coll = db.collection::<Document>(spec.get_str("coll").map_err(err)?);
        let mut f = coll.find(to_doc_b(spec.get("filter"))).skip(skip);
        if let Ok(p) = spec.get_document("projection") {
            f = f.projection(p.clone());
        }
        if let Ok(s) = spec.get_document("sort") {
            f = f.sort(s.clone());
        }
        if let Some(w) = want {
            f = f.limit(w as i64);
        }
        take(f.run().map_err(err)?)?
    };
    let mut docs = docs;
    let more = page.is_some_and(|p| docs.len() as u64 > p);
    if let Some(p) = page {
        docs.truncate(p as usize);
    }
    Ok((docs, more))
}

fn to_doc_b(v: Option<&Bson>) -> Document {
    v.and_then(|b| b.as_document().cloned()).unwrap_or_default()
}

/// Field types (sampled) and indexes of a collection, as shown in the explorer.
fn meta(c: &Client, db: &str, coll: &str) -> Result<Body, String> {
    let d = c.database(db);
    let col = d.collection::<Document>(coll);
    let mut fields: Vec<(String, String)> = Vec::new();
    for doc in col.aggregate([doc! {"$sample": {"size": 100}}]).run().map_err(err)?.filter_map(Result::ok) {
        for (k, v) in &doc {
            match fields.iter_mut().find(|(n, _)| n == k) {
                Some((_, t)) if t == "Null" => *t = type_name(v).into(),
                Some(_) => {}
                None => fields.push((k.clone(), type_name(v).into())),
            }
        }
    }
    fields.sort_by(|a, b| (a.0 != "_id", &a.0).cmp(&(b.0 != "_id", &b.0)));
    let fields = fields
        .into_iter()
        .map(|(name, t)| MetaItem { kind: if name == "_id" { ItemKind::IdField } else { ItemKind::Field }, name, detail: t })
        .collect();
    let mut indexes = Vec::new();
    if let Ok(r) = d.run_command(doc! {"listIndexes": coll}).run() {
        let batch = r.get_document("cursor").and_then(|c| c.get_array("firstBatch")).cloned().unwrap_or_default();
        for i in batch.iter().filter_map(|b| b.as_document()) {
            let name = i.get_str("name").unwrap_or("?").to_string();
            let key = i.get_document("key").cloned().unwrap_or_default();
            let cols: Vec<String> = key
                .iter()
                .map(|(k, v)| match v {
                    Bson::Int32(n) if *n < 0 => format!("{k} desc"),
                    Bson::Int64(n) if *n < 0 => format!("{k} desc"),
                    Bson::Double(n) if *n < 0.0 => format!("{k} desc"),
                    Bson::String(s) => format!("{k} {s}"),
                    _ if key.len() > 1 => format!("{k} asc"),
                    _ => k.clone(),
                })
                .collect();
            let unique = name == "_id_" || i.get_bool("unique").unwrap_or(false);
            let detail = format!("({}){}", cols.join(", "), if unique { " UNIQUE" } else { "" });
            indexes.push(MetaItem { name, detail, kind: if unique { ItemKind::UniqueIndex } else { ItemKind::Index } });
        }
    }
    Ok(Body::Meta(vec![("fields".into(), fields), ("indexes".into(), indexes)]))
}

pub fn type_name(v: &Bson) -> &'static str {
    match v {
        Bson::ObjectId(_) => "ObjectId",
        Bson::Boolean(_) => "Boolean",
        Bson::DateTime(_) => "ISODate",
        Bson::String(_) => "String",
        Bson::Int64(_) => "Int64",
        Bson::Int32(_) => "Int32",
        Bson::Double(_) => "Double",
        Bson::Decimal128(_) => "Decimal128",
        Bson::Document(_) => "Object",
        Bson::Array(_) => "list",
        Bson::Null => "Null",
        Bson::Binary(_) => "BinData",
        Bson::Timestamp(_) => "Timestamp",
        Bson::RegularExpression(_) => "Regex",
        _ => "Any",
    }
}

/// Bridge entry point called from JS (`__native`). Every request returns `{ok}` or `{error}`.
fn native(c: &Client, cancel: &AtomicBool, s: &str) -> Json {
    match op(c, cancel, s) {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    }
}

fn op(c: &Client, cancel: &AtomicBool, s: &str) -> Result<Json, String> {
    let r: Json = serde_json::from_str(s).map_err(err)?;
    let name = r["op"].as_str().unwrap_or_default();
    let dbn = r["db"].as_str().unwrap_or("test");
    let db = c.database(dbn);
    let coll = r["coll"].as_str().unwrap_or_default();
    let o = &r["options"];
    let cmd = |db: &Database, cmd: Document| write(db, cmd);
    let doc_of = |k: &str| to_doc(r.get(k));
    let n = |d: &Document, k: &str| d.get(k).map(|v| num(Some(v))).unwrap_or(0);
    Ok(match name {
        "newOid" => json!(ObjectId::new().to_hex()),
        "newUuid" => json!(bson::Uuid::new().to_string()),
        "cursor" => {
            let spec = doc_of("spec")?;
            let (docs, _) = exec_spec(c, &spec, 0, Some(SCRIPT_FETCH_CAP), cancel)?;
            Json::Array(docs.into_iter().map(|d| js(&Bson::Document(d))).collect())
        }
        "count" => {
            let spec = doc_of("spec")?;
            let res = cmd(
                &c.database(spec.get_str("db").unwrap_or("test")),
                doc! {
                    "count": spec.get_str("coll").map_err(err)?,
                    "query": to_doc_b(spec.get("filter")),
                    "skip": num(spec.get("skip")) as i64,
                    "limit": num(spec.get("limit")) as i64,
                },
            )?;
            json!(n(&res, "n"))
        }
        "explain" => {
            let spec = doc_of("spec")?;
            let inner = doc! {"find": spec.get_str("coll").map_err(err)?, "filter": to_doc_b(spec.get("filter"))};
            js(&Bson::Document(cmd(&db, doc! {"explain": inner, "verbosity": r["verbosity"].as_str().unwrap_or("queryPlanner")})?))
        }
        "insertOne" | "insertMany" => {
            let docs: Vec<Document> = if name == "insertOne" {
                vec![doc_of("doc")?]
            } else {
                match to_bson(r["docs"].clone())? {
                    Bson::Array(a) => a.into_iter().filter_map(|d| d.as_document().cloned()).collect(),
                    _ => return Err("insertMany expects an array of documents".into()),
                }
            };
            let docs: Vec<Document> = docs
                .into_iter()
                .map(|mut d| {
                    ensure_id(&mut d);
                    d
                })
                .collect();
            let ids: Vec<Json> = docs.iter().map(|d| js(d.get("_id").unwrap_or(&Bson::Null))).collect();
            cmd(&db, doc! {"insert": coll, "documents": docs, "ordered": o["ordered"].as_bool().unwrap_or(true)})?;
            if name == "insertOne" {
                json!({"acknowledged": true, "insertedId": ids[0]})
            } else {
                let map: serde_json::Map<String, Json> = ids.into_iter().enumerate().map(|(i, v)| (i.to_string(), v)).collect();
                json!({"acknowledged": true, "insertedIds": map})
            }
        }
        "updateOne" | "updateMany" | "replaceOne" => {
            let u = if name == "replaceOne" { Bson::Document(doc_of("doc")?) } else { to_bson(r["update"].clone())? };
            let mut st =
                doc! {"q": doc_of("filter")?, "u": u, "upsert": o["upsert"].as_bool().unwrap_or(false), "multi": name == "updateMany"};
            if let Some(af) = o.get("arrayFilters") {
                st.insert("arrayFilters", to_bson(af.clone())?);
            }
            let res = cmd(&db, doc! {"update": coll, "updates": [st]})?;
            let up =
                res.get_array("upserted").ok().and_then(|a| a.first()).and_then(|u| u.as_document()).and_then(|u| u.get("_id")).cloned();
            let upn = up.is_some() as u64;
            json!({
                "acknowledged": true,
                "insertedId": up.as_ref().map(js),
                "matchedCount": n(&res, "n") - upn,
                "modifiedCount": n(&res, "nModified"),
                "upsertedCount": upn,
            })
        }
        "deleteOne" | "deleteMany" => {
            let res = cmd(&db, doc! {"delete": coll, "deletes": [{"q": doc_of("filter")?, "limit": (name == "deleteOne") as i32}]})?;
            json!({"acknowledged": true, "deletedCount": n(&res, "n")})
        }
        "findOneAndUpdate" | "findOneAndReplace" | "findOneAndDelete" => {
            let mut fam = doc! {"findAndModify": coll, "query": doc_of("filter")?};
            match name {
                "findOneAndDelete" => fam.insert("remove", true),
                "findOneAndReplace" => fam.insert("update", doc_of("doc")?),
                _ => fam.insert("update", to_bson(r["update"].clone())?),
            };
            let after = o["returnDocument"].as_str() == Some("after") || o["returnNewDocument"].as_bool() == Some(true);
            fam.insert("new", after);
            fam.insert("upsert", o["upsert"].as_bool().unwrap_or(false));
            if let Some(s) = o.get("sort") {
                fam.insert("sort", to_bson(s.clone())?);
            }
            if let Some(p) = o.get("projection") {
                fam.insert("fields", to_bson(p.clone())?);
            }
            js(cmd(&db, fam)?.get("value").unwrap_or(&Bson::Null))
        }
        "countDocuments" => {
            let res = cmd(&db, doc! {"count": coll, "query": doc_of("filter")?})?;
            json!(n(&res, "n"))
        }
        "estimatedDocumentCount" => json!(n(&cmd(&db, doc! {"count": coll})?, "n")),
        "distinct" => {
            let res = cmd(&db, doc! {"distinct": coll, "key": r["key"].as_str().unwrap_or_default(), "query": doc_of("filter")?})?;
            js(res.get("values").unwrap_or(&Bson::Array(vec![])))
        }
        "createIndex" => {
            let keys = doc_of("keys")?;
            let mut idx = to_doc(Some(o))?;
            let name = match idx.get_str("name") {
                Ok(n) => n.to_string(),
                Err(_) => keys.iter().map(|(k, v)| format!("{k}_{}", display(v))).collect::<Vec<_>>().join("_"),
            };
            idx.insert("key", keys);
            idx.insert("name", &name);
            cmd(&db, doc! {"createIndexes": coll, "indexes": [idx]})?;
            json!(name)
        }
        "getIndexes" => {
            let res = cmd(&db, doc! {"listIndexes": coll})?;
            js(res.get_document("cursor").ok().and_then(|c| c.get("firstBatch")).unwrap_or(&Bson::Array(vec![])))
        }
        "dropIndex" | "dropIndexes" => {
            let index = match (&r["name"], &r["keys"]) {
                (Json::String(s), _) => Bson::String(s.clone()),
                (_, k) if k.is_object() => to_bson(k.clone())?,
                _ => Bson::String("*".into()),
            };
            js(&Bson::Document(cmd(&db, doc! {"dropIndexes": coll, "index": index})?))
        }
        "drop" => json!(cmd(&db, doc! {"drop": coll}).is_ok()),
        "renameCollection" => {
            let to = r["to"].as_str().unwrap_or_default();
            js(&Bson::Document(cmd(
                &c.database("admin"),
                doc! {
                    "renameCollection": format!("{dbn}.{coll}"),
                    "to": format!("{dbn}.{to}"),
                    "dropTarget": r["dropTarget"].as_bool().unwrap_or(false),
                },
            )?))
        }
        "listCollections" => {
            let res =
                cmd(&db, doc! {"listCollections": 1, "filter": doc_of("filter")?, "nameOnly": r["nameOnly"].as_bool().unwrap_or(false)})?;
            js(res.get_document("cursor").ok().and_then(|c| c.get("firstBatch")).unwrap_or(&Bson::Array(vec![])))
        }
        "listDatabases" => js(cmd(&c.database("admin"), doc! {"listDatabases": 1})?.get("databases").unwrap_or(&Bson::Array(vec![]))),
        "createCollection" => {
            let mut cc = doc! {"create": r["name"].as_str().unwrap_or_default()};
            cc.extend(to_doc(Some(o))?);
            js(&Bson::Document(cmd(&db, cc)?))
        }
        "dropDatabase" => js(&Bson::Document(cmd(&db, doc! {"dropDatabase": 1})?)),
        "runCommand" => js(&Bson::Document(cmd(&db, doc_of("cmd")?)?)),
        _ => return Err(format!("{name} is not supported")),
    })
}

/// Splits a script into top-level statement byte ranges. A statement ends at `;` or at a
/// newline at bracket depth 0, unless the next line continues a call chain (`.method()`) or the
/// current line ends with an operator.
pub fn split_statements(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut depth, mut i) = (0usize, 0i32, 0usize);
    let mut last_sig = 0u8;
    let push = |a: usize, z: usize, out: &mut Vec<(usize, usize)>| {
        if !src[a..z].trim().trim_matches(';').trim().is_empty() {
            out.push((a, z));
        }
    };
    while i < b.len() {
        let ch = b[i];
        match ch {
            b'"' | b'\'' | b'`' => {
                i += 1;
                while i < b.len() && b[i] != ch {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                last_sig = ch;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                last_sig = ch;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                last_sig = ch;
            }
            b';' if depth <= 0 => {
                push(start, i + 1, &mut out);
                start = i + 1;
                last_sig = 0;
            }
            b'\n' if depth <= 0 && last_sig != 0 => {
                let next = src[i + 1..].trim_start();
                let cont = next.starts_with('.') || b",([{.+-*/=&|?:<>!".contains(&last_sig);
                if !cont {
                    push(start, i, &mut out);
                    start = i + 1;
                    last_sig = 0;
                }
            }
            c if !c.is_ascii_whitespace() => last_sig = c,
            _ => {}
        }
        i += 1;
    }
    push(start, b.len().min(src.len()), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unreadable_resolver_config() {
        assert!(is_resolver_conf_error(
            "Kind: An error occurred during DNS resolution: protocol error: failed to parse nameserver address: invalid IP address syntax"
        ));
        assert!(!is_resolver_conf_error("Kind: An error occurred during DNS resolution: no record found for Query"));
        assert!(!is_resolver_conf_error("Server selection timeout"));
    }

    #[test]
    fn splits_statements() {
        let s = "use shop\ndb.a.find({x: 1})\n  .sort({x: -1})\n\nvar y = 'a;b'; db.b.count()\n// note\nprint(y)";
        let parts: Vec<&str> = split_statements(s).iter().map(|&(a, b)| s[a..b].trim().trim_end_matches(';')).collect();
        assert_eq!(parts, ["use shop", "db.a.find({x: 1})\n  .sort({x: -1})", "var y = 'a;b'", "db.b.count()", "// note\nprint(y)"]);
    }

    /// Needs the docker mongo from `docker/compose.yml` (127.0.0.1:27018); skipped when absent.
    #[test]
    fn shell_against_local_mongo() {
        let (tx, rx) = channel();
        let conn = Conn::open("mongodb://127.0.0.1:27018/?serverSelectionTimeoutMS=1500".into(), tx, || {});
        conn.send(1, Req::Connect);
        match rx.recv().unwrap().body {
            Ok(Body::Connected { version }) => assert!(!version.is_empty()),
            Err(e) => return eprintln!("skipped, no local mongo: {e}"),
            _ => panic!(),
        }
        let script = "use DataTired_test\ndb.t.drop()\ndb.t.insertMany([{a: 1, d: ISODate('2025-01-01'), n: NumberLong(5)}, {a: 2}])\ndb.t.updateOne({a: 2}, {$set: {b: 'x'}})\ndb.t.find({a: {$gte: 1}}).sort({a: -1})\ndb.t.find(\n  {a: 2},\n  {b: 1}\n)\ndb.t.countDocuments()\nprint(db.t.findOne({a: 1}).n.toString())";
        conn.send(2, Req::Run { db: "test".into(), schema: String::new(), script: script.into() });
        let Ok(Body::Ran(items)) = rx.recv().unwrap().body else { panic!() };
        let logs: Vec<String> =
            items.iter().filter_map(|i| if let RunItem::Log(l, _) = i { Some(l[22..].to_string()) } else { None }).collect();
        let results: Vec<&ResultSet> = items.iter().filter_map(|i| if let RunItem::Result(r) = i { Some(r) } else { None }).collect();
        assert!(!logs.iter().any(|l| l.contains("Error") || l.contains("error")), "{logs:#?}");
        let find = results.iter().find(|r| r.title == "DataTired_test.t").expect("find result");
        assert_eq!(find.docs.len(), 2);
        assert_eq!(find.docs[0].get_i32("a").unwrap(), 2);
        assert_eq!(find.docs[0].get_str("b").unwrap(), "x");
        assert!(matches!(find.docs[1].get("n"), Some(Bson::Int64(5))));
        assert!(matches!(find.docs[1].get("d"), Some(Bson::DateTime(_))));
        // projection: only the selected fields (+ _id)
        let proj = results.iter().filter(|r| r.title == "DataTired_test.t").nth(1).expect("projected find");
        assert_eq!(proj.docs.len(), 1, "{:#?} {logs:#?}", proj.docs);
        assert_eq!(proj.docs[0].keys().collect::<Vec<_>>(), ["_id", "b"]);
        assert!(logs.iter().any(|l| l == "2"), "{logs:#?}");
        assert!(logs.iter().any(|l| l == "5"), "{logs:#?}");

        conn.send(
            3,
            Req::Find { db: "DataTired_test".into(), coll: "t".into(), filter: "{a: 1}".into(), sort: String::new(), offset: 0, limit: 10 },
        );
        let Ok(Body::Rows(rs)) = rx.recv().unwrap().body else { panic!() };
        assert_eq!(rs.docs.len(), 1);
        let id = rs.docs[0].get("_id").unwrap().clone();
        conn.send(
            4,
            Req::Save {
                db: "DataTired_test".into(),
                coll: "t".into(),
                edits: vec![Edit::Set { id, field: "a".into(), text: "42".into(), as_string: false }],
            },
        );
        assert!(rx.recv().unwrap().body.is_ok());
        conn.send(5, Req::Meta { db: "DataTired_test".into(), coll: "t".into() });
        let Ok(Body::Meta(meta)) = rx.recv().unwrap().body else { panic!() };
        let fields: Vec<(&str, &str)> = meta[0].1.iter().map(|f| (f.name.as_str(), f.detail.as_str())).collect();
        assert_eq!(fields[0], ("_id", "ObjectId"));
        assert!(fields.contains(&("n", "Int64")));
        assert_eq!((meta[1].1[0].name.as_str(), meta[1].1[0].detail.as_str()), ("_id_", "(_id) UNIQUE"));
        conn.send(6, Req::Run { db: "DataTired_test".into(), schema: String::new(), script: "db.dropDatabase()".into() });
        rx.recv().unwrap();

        // errors surface as red log lines and stop the script, never panic (write errors too)
        for (script, needle) in [
            ("db.t.find({\n", "SyntaxError"),
            ("db.t.find({$bad: 1})", "$bad"),
            ("nosuch()", "not defined"),
            ("db.u.insertOne({_id: 1}); db.u.insertOne({_id: 1})", "E11000"),
        ] {
            conn.send(7, Req::Run { db: "DataTired_test".into(), schema: String::new(), script: script.into() });
            let Ok(Body::Ran(items)) = rx.recv().unwrap().body else { panic!() };
            let errs: Vec<&String> =
                items.iter().filter_map(|i| if let RunItem::Log(l, LogKind::Error) = i { Some(l) } else { None }).collect();
            assert!(errs.iter().any(|e| e.contains(needle)), "{script}: {errs:?}");
        }
        conn.send(8, Req::Run { db: "DataTired_test".into(), schema: String::new(), script: "db.dropDatabase()".into() });
        rx.recv().unwrap();
    }
}

#[cfg(test)]
mod to_array {
    use super::*;
    use std::sync::mpsc::channel;

    /// `db.c.find(…).sort(…).toArray()` (as typed in a console) shows the cursor's first page instead of
    /// pulling the whole collection through JS; Needs the docker mongo (127.0.0.1:27018).
    #[test]
    fn to_array_pages_like_find() {
        let (tx, rx) = channel();
        let conn = Conn::open("mongodb://127.0.0.1:27018/?serverSelectionTimeoutMS=1500".into(), tx, || {});
        conn.send(1, Req::Connect);
        if let Err(e) = rx.recv().unwrap().body {
            return eprintln!("skipped, no local mongo: {e}");
        }
        let run = |tag, script: &str| {
            conn.send(tag, Req::Run { db: "DataTired_to_array".into(), schema: String::new(), script: script.into() });
            let Ok(Body::Ran(items)) = rx.recv_timeout(Duration::from_secs(60)).unwrap().body else { panic!() };
            items
        };
        run(2, "db.import_requests.drop(); db.import_requests.insertMany(Array.from({length: 250}, (_, i) => ({i})))");
        let items = run(
            3,
            "db.getCollection(\"import_requests\").find({\n  _id: {\n    $gte: ObjectId(\"000000000000000000000000\")\n  }\n}).sort({\n  _id: -1\n}).toArray()",
        );
        let rs = items.iter().find_map(|i| if let RunItem::Result(r) = i { Some(r) } else { None }).expect("result");
        assert_eq!((rs.title.as_str(), rs.docs.len(), rs.more), ("DataTired_to_array.import_requests", crate::PAGE as usize, true));
        assert_eq!(rs.docs[0].get_i32("i"), Ok(249), "sorted by _id desc");
        assert!(rs.spec.is_some(), "next pages come from the cursor");
        // toArray() inside an expression is still a real array
        let items = run(4, "db.import_requests.find({i: {$lt: 7}}).toArray().length");
        assert!(items.iter().any(|i| matches!(i, RunItem::Log(l, LogKind::Info) if l.ends_with("] 7"))));
        run(5, "db.dropDatabase()");
    }
}
