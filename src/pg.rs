//! PostgreSQL connection worker: one thread per data source holding a client per database
//! (connected on first use). Rows come back in text form through the simple query protocol, so the
//! grid shows what the server prints (`33.30`, `{vip,newsletter}`, jsonb); a `prepare` of the same
//! statement supplies column types and the source table (for editing).

use crate::mongo::{Body, Conn, Edit, Hook, ItemKind, LogKind, MetaItem, Req, Resp, ResultSet, RunItem, stamp};
use mongodb::bson::{Bson, Document, doc};
use postgres::error::ErrorPosition;
use postgres::{Client, Config, SimpleQueryMessage, Statement};
use postgres_native_tls::MakeTlsConnector;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Rows per page (DataGrip: `LIMIT 501` for PostgreSQL tables).
pub const PAGE: u64 = 500;

pub fn open(uri: String, out: Sender<Resp>, wake: impl Fn() + Send + 'static) -> Conn {
    Conn::spawn(out, wake, move |cancel, hook| {
        let mut w = Worker::new(&uri, cancel, hook)?;
        Ok(move |req| w.handle(req))
    })
}

struct Worker {
    cfg: Config,
    tls: MakeTlsConnector,
    default_db: String,
    clients: HashMap<String, Client>,
    cancel: Arc<AtomicBool>,
    hook: Hook,
    /// `RAISE NOTICE` & co. of the running statement.
    notices: Arc<Mutex<Vec<String>>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// `[42P01] ERROR: relation "x" does not exist` (+ detail / hint / position), like DataGrip's output.
fn pg_err(e: postgres::Error) -> String {
    let Some(d) = e.as_db_error() else { return e.to_string() };
    let mut s = format!("[{}] {}: {}", d.code().code(), d.severity(), d.message());
    if let Some(x) = d.detail() {
        s += &format!("\n  Detail: {x}");
    }
    if let Some(x) = d.hint() {
        s += &format!("\n  Hint: {x}");
    }
    if let Some(ErrorPosition::Original(p)) = d.position() {
        s += &format!("\n  Position: {p}");
    }
    s
}

impl Worker {
    fn new(uri: &str, cancel: Arc<AtomicBool>, hook: Hook) -> Result<Worker, String> {
        let mut cfg: Config = uri.parse().map_err(err)?;
        if cfg.get_dbname().is_none() {
            cfg.dbname("postgres");
        }
        cfg.connect_timeout(Duration::from_secs(5));
        cfg.application_name("DataTired");
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n = notices.clone();
        cfg.notice_callback(move |d| n.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{}: {}", d.severity(), d.message())));
        // sslmode prefer (default) / require: TLS without verifying the certificate, as libpq does
        let tls = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .map_err(err)?;
        let default_db = cfg.get_dbname().unwrap_or("postgres").to_string();
        Ok(Worker { cfg, tls: MakeTlsConnector::new(tls), default_db, clients: HashMap::new(), cancel, hook, notices })
    }

    /// Client for a database (empty = the URL's), reconnecting when the connection dropped.
    fn client(&mut self, db: &str) -> Result<&mut Client, String> {
        let db = if db.is_empty() { self.default_db.clone() } else { db.to_string() };
        if self.clients.get(&db).is_some_and(Client::is_closed) {
            self.clients.remove(&db);
        }
        if !self.clients.contains_key(&db) {
            let mut cfg = self.cfg.clone();
            cfg.dbname(&db);
            let c = cfg.connect(self.tls.clone()).map_err(pg_err)?;
            self.clients.insert(db.clone(), c);
            // ⌘F2 cancels whatever runs (one statement at a time per worker)
            let tokens: Vec<postgres::CancelToken> = self.clients.values().map(Client::cancel_token).collect();
            let tls = self.tls.clone();
            *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
                for t in tokens.clone() {
                    let tls = tls.clone();
                    std::thread::spawn(move || t.cancel_query(tls));
                }
            }));
        }
        Ok(self.clients.get_mut(&db).unwrap())
    }

    fn handle(&mut self, req: Req) -> Result<Body, String> {
        match req {
            Req::Connect => {
                let rows = text(self.client("")?, "show server_version")?;
                let v = rows.first().and_then(|r| r[0].clone()).unwrap_or_default();
                Ok(Body::Connected { version: v.split_whitespace().next().unwrap_or("?").into() })
            }
            Req::Databases => {
                let sql = "select datname from pg_database where not datistemplate and datallowconn order by datname";
                Ok(Body::Databases(text(self.client("")?, sql)?.into_iter().filter_map(|r| r[0].clone()).collect()))
            }
            Req::Collections { db } => {
                let c = self.client(&db)?;
                let user = "nspname !~ '^pg_' and nspname <> 'information_schema'";
                let schemas = text(c, &format!("select nspname from pg_namespace where {user} order by nspname"))?;
                let rels = text(
                    c,
                    &format!(
                        "select nspname, relname, relkind from pg_class c join pg_namespace n on n.oid = c.relnamespace
                         where relkind in ('r', 'p', 'v', 'm', 'f', 'S') and not relispartition and {user} order by 1, 2"
                    ),
                )?;
                let s = |v: &Option<String>| v.clone().unwrap_or_default();
                let colls = rels
                    .iter()
                    .map(|r| {
                        let kind = match s(&r[2]).as_str() {
                            "v" => 'v',
                            "m" => 'm',
                            "S" => 'S',
                            _ => 'r',
                        };
                        (s(&r[0]), s(&r[1]), kind)
                    })
                    .collect();
                Ok(Body::Collections { schemas: schemas.iter().map(|r| s(&r[0])).collect(), colls })
            }
            Req::Meta { db, coll } => Ok(Body::Meta(meta(self.client(&db)?, &coll)?)),
            Req::Run { db, schema, script } => Ok(Body::Ran(self.run(&db, &schema, &script))),
            Req::Page { spec, offset, limit } => {
                let (db, schema) = (spec.get_str("db").unwrap_or_default().to_string(), spec.get_str("schema").unwrap_or_default());
                let c = self.client(&db)?;
                set_schema(c, schema);
                Ok(Body::Rows(fetch(c, spec, offset, limit)?))
            }
            Req::Find { db, coll, filter, sort, offset, limit } => {
                let mut sql = format!("SELECT t.*\nFROM {coll} t");
                if !filter.trim().is_empty() {
                    sql += &format!("\nWHERE {}", filter.trim());
                }
                if !sort.trim().is_empty() {
                    sql += &format!("\nORDER BY {}", sort.trim());
                }
                let c = self.client(&db)?;
                let st = c.prepare(&sql).map_err(pg_err)?;
                let mut spec = doc! {"db": &db, "sql": &sql};
                if let Some((table, key)) = target(c, &st) {
                    spec.insert("table", table);
                    spec.insert("key", key);
                }
                Ok(Body::Rows(fetch(c, spec, offset, limit)?))
            }
            Req::Save { db, coll, edits } => {
                let c = self.client(&db)?;
                let mut tx = c.transaction().map_err(pg_err)?;
                let mut n = 0;
                for e in &edits {
                    n += tx.execute(&edit_sql(&coll, e), &[]).map_err(pg_err)?;
                }
                tx.commit().map_err(pg_err)?;
                Ok(Body::Done(format!("{n} row(s) affected")))
            }
            Req::Command { db, cmd } => {
                self.client(&db)?.batch_execute(cmd.get_str("sql").map_err(err)?).map_err(pg_err)?;
                Ok(Body::Done("completed".into()))
            }
            Req::Ddl { db, coll } => Ok(Body::Ddl(Box::new(rel(self.client(&db)?, &coll)?))),
        }
    }

    fn run(&mut self, db: &str, schema: &str, script: &str) -> Vec<RunItem> {
        let mut items = Vec::new();
        let prompt = if schema.is_empty() { db.to_string() } else { format!("{db}.{schema}") };
        let cancel = self.cancel.clone();
        let notices = self.notices.clone();
        let c = match self.client(db) {
            Ok(c) => c,
            Err(e) => return vec![RunItem::Log(stamp(&e), LogKind::Error)],
        };
        set_schema(c, schema);
        let flush = |items: &mut Vec<RunItem>| {
            for n in notices.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                items.push(RunItem::Log(stamp(&n), LogKind::Info));
            }
        };
        for (a, b) in split_sql(script) {
            if cancel.load(Ordering::Relaxed) {
                items.push(RunItem::Log(stamp("Cancelled"), LogKind::Error));
                break;
            }
            let stmt = script[a..b].trim().trim_end_matches(';').trim();
            items.push(RunItem::Log(stamp(&format!("{prompt}> {stmt}")), LogKind::EchoSql));
            let t = Instant::now();
            let res = statement(c, db, schema, stmt);
            flush(&mut items);
            let ms = t.elapsed().as_millis();
            match res {
                Ok(Done::Rows(rs)) => {
                    let n = rs.docs.len();
                    let msg = format!(
                        "{n} row{} retrieved starting from 1 in {ms} ms (execution: {} ms, fetching: {} ms)",
                        if n == 1 { "" } else { "s" },
                        rs.exec_ms,
                        ms.saturating_sub(rs.exec_ms)
                    );
                    items.push(RunItem::Result(*rs));
                    items.push(RunItem::Log(stamp(&msg), LogKind::Info));
                }
                Ok(Done::Affected(n)) => {
                    items.push(RunItem::Log(stamp(&format!("{n} row{} affected in {ms} ms", if n == 1 { "" } else { "s" })), LogKind::Info))
                }
                Ok(Done::Completed) => items.push(RunItem::Log(stamp(&format!("completed in {ms} ms")), LogKind::Info)),
                Err(e) => {
                    items.push(RunItem::Log(stamp(&e), LogKind::Error));
                    break;
                }
            }
        }
        items
    }
}

enum Done {
    Rows(Box<ResultSet>),
    Affected(u64),
    Completed,
}

/// The console's schema becomes the search path (ignored when it fails, e.g. in an aborted
/// transaction: the user must still be able to run ROLLBACK).
fn set_schema(c: &mut Client, schema: &str) {
    if !schema.is_empty() {
        let _ = c.batch_execute(&format!("SET search_path TO {}", ident(schema)));
    }
}

fn first_word(stmt: &str) -> String {
    let mut s = stmt.trim_start();
    // leading comments
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.split_once('\n').map_or("", |x| x.1).trim_start();
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.split_once("*/").map_or("", |x| x.1).trim_start();
        } else {
            break;
        }
    }
    s.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase()
}

/// One console statement. Queries are paged (`SELECT * FROM (…) q LIMIT 501`); other row-returning
/// statements (`SHOW`, `EXPLAIN`, `… RETURNING`) return all their rows.
fn statement(c: &mut Client, db: &str, schema: &str, stmt: &str) -> Result<Done, String> {
    let st = c.prepare(stmt).ok();
    let kw = first_word(stmt);
    let lower = stmt.to_lowercase();
    let modifies =
        ["insert", "update", "delete", "merge"].iter().any(|w| lower.split(|c: char| !c.is_alphanumeric() && c != '_').any(|x| x == *w));
    let pageable = st.as_ref().is_some_and(|s| !s.columns().is_empty())
        && (matches!(kw.as_str(), "select" | "values" | "table") || (kw == "with" && !modifies));
    if pageable {
        let mut spec = doc! {"db": db, "schema": schema, "sql": format!("SELECT * FROM (\n{stmt}\n) q")};
        if let Some((table, key)) = st.as_ref().and_then(|s| target(c, s)) {
            spec.insert("table", table);
            spec.insert("key", key);
        }
        return fetch(c, spec, 0, PAGE).map(|rs| Done::Rows(Box::new(rs)));
    }
    let t = Instant::now();
    let (cols, docs, n) = text_rows(c, stmt, st.as_ref())?;
    if cols.is_empty() {
        return Ok(if matches!(kw.as_str(), "insert" | "update" | "delete" | "merge" | "copy") {
            Done::Affected(n)
        } else {
            Done::Completed
        });
    }
    let page = docs.len() as u64;
    let exec_ms = t.elapsed().as_millis();
    Ok(Done::Rows(Box::new(ResultSet {
        title: "result".into(),
        docs,
        offset: 0,
        page,
        more: false,
        spec: None,
        exec_ms,
        fetch_ms: 0,
        cols,
    })))
}

/// One page of a pageable query (`spec.sql` without LIMIT); one extra row is fetched for `more`.
fn fetch(c: &mut Client, spec: Document, offset: u64, limit: u64) -> Result<ResultSet, String> {
    let sql = spec.get_str("sql").map_err(err)?;
    let mut q = format!("{sql}\nLIMIT {}", limit + 1);
    if offset > 0 {
        q += &format!(" OFFSET {offset}");
    }
    let t = Instant::now();
    let st = c.prepare(&q).map_err(pg_err)?;
    let (cols, mut docs, _) = text_rows(c, &q, Some(&st))?;
    let exec_ms = t.elapsed().as_millis();
    let more = docs.len() as u64 > limit;
    docs.truncate(limit as usize);
    let title = match spec.get_str("table") {
        Ok(tb) => format!("{}.{tb}", spec.get_str("db").unwrap_or_default()),
        Err(_) => "result".into(),
    };
    Ok(ResultSet { title, docs, offset, page: limit, more, spec: Some(spec), exec_ms, fetch_ms: 0, cols })
}

/// Rows as documents in column order (text values, NULL, booleans), the (column, type) list and the
/// command's row count. Repeated column names get ` (2)`, ` (3)`… so every cell keeps its own key.
/// (column, type) list, rows, command row count.
type TextRows = (Vec<(String, String)>, Vec<Document>, u64);

fn text_rows(c: &mut Client, sql: &str, st: Option<&Statement>) -> Result<TextRows, String> {
    let types: Vec<String> = st.map(|s| s.columns().iter().map(|c| c.type_().name().to_string()).collect()).unwrap_or_default();
    let mut cols: Vec<(String, String)> = vec![];
    let mut docs = vec![];
    let mut n = 0;
    for m in c.simple_query(sql).map_err(pg_err)? {
        match m {
            SimpleQueryMessage::RowDescription(desc) => {
                cols.clear();
                for (i, col) in desc.iter().enumerate() {
                    let mut name = col.name().to_string();
                    let mut k = 2;
                    while cols.iter().any(|(c, _)| *c == name) {
                        name = format!("{} ({k})", col.name());
                        k += 1;
                    }
                    cols.push((name, types.get(i).cloned().unwrap_or_default()));
                }
            }
            SimpleQueryMessage::Row(r) => {
                let mut d = Document::new();
                for (i, (name, ty)) in cols.iter().enumerate() {
                    let v = match r.get(i) {
                        None => Bson::Null,
                        Some(v) if ty == "bool" => Bson::Boolean(v == "t"),
                        Some(v) => Bson::String(v.into()),
                    };
                    d.insert(name, v);
                }
                docs.push(d);
            }
            SimpleQueryMessage::CommandComplete(k) => n = k,
            _ => {}
        }
    }
    Ok((cols, docs, n))
}

/// Text result of an internal query (introspection), NULL as `None`.
fn text(c: &mut Client, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    let (cols, docs, _) = text_rows(c, sql, None)?;
    Ok(docs
        .iter()
        .map(|d| {
            cols.iter()
                .map(|(k, _)| match d.get(k) {
                    Some(Bson::String(s)) => Some(s.clone()),
                    Some(Bson::Boolean(b)) => Some(if *b { "t" } else { "f" }.into()),
                    _ => None,
                })
                .collect()
        })
        .collect())
}

/// Table (qualified name) and primary key when every column comes straight from one table with a
/// primary key that is fully selected, under the column's own names: rows can then be edited.
fn target(c: &mut Client, st: &Statement) -> Option<(String, Vec<String>)> {
    let oid = st.columns().first()?.table_oid().filter(|o| *o != 0)?;
    if st.columns().iter().any(|col| col.table_oid() != Some(oid)) {
        return None;
    }
    let rel =
        text(c, &format!("select nspname, relname from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.oid = {oid}"))
            .ok()?;
    let atts = text(
        c,
        &format!(
            "select a.attnum, a.attname, coalesce(a.attnum = any(i.indkey), false) from pg_attribute a
             left join pg_index i on i.indrelid = a.attrelid and i.indisprimary where a.attrelid = {oid} and a.attnum > 0"
        ),
    )
    .ok()?;
    let att = |n: i16| atts.iter().find(|r| r[0].as_deref() == Some(&n.to_string())).and_then(|r| r[1].clone());
    if st.columns().iter().any(|col| col.column_id().and_then(att).as_deref() != Some(col.name())) {
        return None;
    }
    let key: Vec<String> = atts.iter().filter(|r| r[2].as_deref() == Some("t")).filter_map(|r| r[1].clone()).collect();
    if key.is_empty() || key.iter().any(|k| !st.columns().iter().any(|col| col.name() == k)) {
        return None;
    }
    let r = rel.first()?;
    Some((qualify(r[0].as_deref()?, r[1].as_deref()?), key))
}

/// Explorer children of a table / view: columns, keys, foreign keys, indexes, checks (empty folders
/// besides `columns` are left out, like DataGrip).
fn meta(c: &mut Client, coll: &str) -> Result<crate::mongo::Meta, String> {
    let rel = format!("to_regclass({})", lit(coll));
    let item = |name: &Option<String>, detail: String, kind| MetaItem { name: name.clone().unwrap_or_default(), detail, kind };
    let columns = text(
        c,
        &format!(
            "select a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, pg_get_expr(d.adbin, d.adrelid),
                    coalesce(a.attnum = any(i.indkey), false)
             from pg_attribute a
               left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
               left join pg_index i on i.indrelid = a.attrelid and i.indisprimary
             where a.attrelid = {rel} and a.attnum > 0 and not a.attisdropped order by a.attnum"
        ),
    )?
    .iter()
    .map(|r| {
        let ty = r[1].clone().unwrap_or_default();
        let detail = match &r[3] {
            Some(def) => format!("{ty} = {def}"),
            None => ty,
        };
        let kind = match (r[4].as_deref(), r[2].as_deref()) {
            (Some("t"), _) => ItemKind::PkColumn,
            (_, Some("t")) => ItemKind::NotNullColumn,
            _ => ItemKind::Column,
        };
        item(&r[0], detail, kind)
    })
    .collect();
    let cols = |k: &str| {
        format!(
            "(select string_agg(a.attname, ', ' order by k.o) from unnest({k}) with ordinality k(n, o) join pg_attribute a on a.attrelid = c.{} and a.attnum = k.n)",
            if k == "c.conkey" { "conrelid" } else { "confrelid" }
        )
    };
    let cons = text(
        c,
        &format!(
            "select conname, contype, {}, confrelid::regclass, {}, pg_get_constraintdef(c.oid) from pg_constraint c
             where conrelid = {rel} order by contype <> 'p', conname",
            cols("c.conkey"),
            cols("c.confkey")
        ),
    )?;
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let (mut keys, mut fks, mut checks) = (vec![], vec![], vec![]);
    for r in &cons {
        match r[1].as_deref() {
            Some("p") => keys.push(item(&r[0], format!("({})", s(&r[2])), ItemKind::PrimaryKey)),
            Some("u") => keys.push(item(&r[0], format!("({})", s(&r[2])), ItemKind::Key)),
            Some("f") => fks.push(item(&r[0], format!("({}) → {} ({})", s(&r[2]), s(&r[3]), s(&r[4])), ItemKind::ForeignKey)),
            Some("c") => {
                // `CHECK ((qty > 0))` → `(qty > 0)`
                let def = s(&r[5]);
                let body = def.strip_prefix("CHECK ").unwrap_or(&def);
                let body = body.strip_prefix('(').and_then(|b| b.strip_suffix(')')).unwrap_or(body);
                checks.push(item(&r[0], body.to_string(), ItemKind::Check))
            }
            _ => {}
        }
    }
    let indexes = text(
        c,
        &format!(
            "select i.relname, x.indisunique, pg_get_indexdef(x.indexrelid) from pg_index x join pg_class i on i.oid = x.indexrelid
             where x.indrelid = {rel} order by i.relname"
        ),
    )?
    .iter()
    .map(|r| {
        // `CREATE UNIQUE INDEX … USING btree (email)` → `(email) UNIQUE`
        let def = s(&r[2]);
        let cols = def.split_once(" USING ").and_then(|(_, m)| m.split_once(' ')).map_or(def.clone(), |(_, c)| c.to_string());
        let unique = r[1].as_deref() == Some("t");
        let kind = if unique { ItemKind::UniqueIndex } else { ItemKind::Index };
        item(&r[0], if unique { format!("{cols} UNIQUE") } else { cols }, kind)
    })
    .collect::<Vec<_>>();
    let mut out = vec![("columns".to_string(), columns)];
    for (label, v) in [("keys", keys), ("foreign keys", fks), ("indexes", indexes), ("checks", checks)] {
        if !v.is_empty() {
            out.push((label.into(), v));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ SQL Generator

/// A table / view as the SQL Generator sees it. The worker reads the catalog once; `ddl` turns it
/// into text on the UI thread, so changing an option re-renders without a round trip.
#[derive(Clone, Debug, Default)]
pub struct Rel {
    pub schema: String,
    pub name: String,
    /// `r` table, `p` partitioned table, `v` view, `m` materialized view, `f` foreign table.
    pub kind: char,
    pub owner: String,
    pub comment: Option<String>,
    pub cols: Vec<RelCol>,
    /// Primary key, unique, foreign keys, checks (in that order).
    pub cons: Vec<RelCon>,
    /// All indexes (`constraint`: backs a key).
    pub indexes: Vec<RelIdx>,
    /// `pg_get_viewdef` of a view.
    pub view: Option<String>,
    /// `p` permanent, `u` unlogged, `t` temporary.
    pub persistence: char,
    pub tablespace: String,
    pub access_method: String,
    /// `reloptions`: `fillfactor=70, …`.
    pub options: String,
    pub partition_key: String,
    /// Choices of the Modify dialog's combo boxes.
    pub roles: Vec<String>,
    pub tablespaces: Vec<String>,
    pub table_methods: Vec<String>,
    pub index_methods: Vec<String>,
    /// Tables of the database (qualified) with their columns and primary key: foreign key targets.
    pub tables: Vec<(String, Vec<String>, Vec<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct RelCol {
    pub name: String,
    pub ty: String,
    pub default: Option<String>,
    pub not_null: bool,
    /// `always` / `by default` (identity column).
    pub identity: Option<String>,
    /// Stored generated column (`default` is its expression).
    pub generated: bool,
    /// Integer column fed by a sequence it owns: shown as `serial`.
    pub serial: bool,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RelCon {
    pub name: String,
    /// `p` / `u` / `f` / `c`.
    pub kind: char,
    pub cols: Vec<String>,
    /// `pg_get_constraintdef` (checks use its expression).
    pub def: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_cols: Vec<String>,
    /// The referenced columns are the referenced table's primary key (DataGrip then leaves them out).
    pub ref_pk: bool,
    /// `confupdtype` / `confdeltype`: `a` no action, `r` restrict, `c` cascade, `n` set null, `d` set default.
    pub on_update: char,
    pub on_delete: char,
    pub comment: Option<String>,
    pub deferrable: bool,
    pub deferred: bool,
    pub validated: bool,
    pub no_inherit: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RelIdx {
    pub name: String,
    pub unique: bool,
    pub method: String,
    pub cols: Vec<IdxCol>,
    pub pred: Option<String>,
    /// `pg_get_indexdef` ("Definitions provided by RDBMS server").
    pub native: String,
    /// Backs a primary key / unique constraint.
    pub constraint: bool,
    pub comment: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdxCol {
    /// Column or expression (`(address ->> 'city'::text)`).
    pub expr: String,
    /// Quoted collation when not the default one.
    pub collation: Option<String>,
    pub desc: bool,
    pub nulls_first: bool,
}

fn rel(c: &mut Client, coll: &str) -> Result<Rel, String> {
    const SEP: char = '\u{1}';
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let list = |v: &Option<String>| v.as_deref().filter(|x| !x.is_empty()).map_or(vec![], |x| x.split(SEP).map(String::from).collect());
    let t = |v: &Option<String>| v.as_deref() == Some("t");
    let ch = |v: &Option<String>| v.as_deref().and_then(|x| x.chars().next()).unwrap_or(' ');
    let rows = text(
        c,
        &format!(
            "select c.oid, n.nspname, c.relname, c.relkind, pg_get_userbyid(c.relowner), obj_description(c.oid, 'pg_class'),
                    case when c.relkind in ('v', 'm') then pg_get_viewdef(c.oid, true) end, c.relpersistence,
                    (select spcname from pg_tablespace where oid = c.reltablespace), (select amname from pg_am where oid = c.relam),
                    array_to_string(c.reloptions, ', '), case when c.relkind = 'p' then pg_get_partkeydef(c.oid) end
             from pg_class c join pg_namespace n on n.oid = c.relnamespace where c.oid = to_regclass({})",
            lit(coll)
        ),
    )?;
    let r = rows.first().ok_or_else(|| format!("{coll}: no such table"))?;
    let oid = s(&r[0]);
    let mut rel = Rel {
        schema: s(&r[1]),
        name: s(&r[2]),
        kind: ch(&r[3]),
        owner: s(&r[4]),
        comment: r[5].clone(),
        view: r[6].clone(),
        persistence: ch(&r[7]),
        tablespace: s(&r[8]),
        access_method: s(&r[9]),
        options: s(&r[10]),
        partition_key: s(&r[11]),
        ..Rel::default()
    };
    let mut column = |sql: &str| -> Result<Vec<String>, String> { Ok(text(c, sql)?.iter().map(|r| s(&r[0])).collect()) };
    rel.roles = column("select rolname from pg_roles where rolname !~ '^pg_' order by 1")?;
    rel.tablespaces = column("select spcname from pg_tablespace order by 1")?;
    rel.table_methods = column("select amname from pg_am where amtype = 't' order by 1")?;
    rel.index_methods = column("select amname from pg_am where amtype = 'i' order by 1")?;
    rel.tables = text(
        c,
        "select n.nspname, c.relname,
                (select string_agg(attname, chr(1) order by attnum) from pg_attribute where attrelid = c.oid and attnum > 0 and not attisdropped),
                (select string_agg(a.attname, chr(1) order by k.o) from pg_constraint p
                   cross join lateral unnest(p.conkey) with ordinality k(n, o) join pg_attribute a on a.attrelid = p.conrelid and a.attnum = k.n
                 where p.conrelid = c.oid and p.contype = 'p')
         from pg_class c join pg_namespace n on n.oid = c.relnamespace
         where c.relkind in ('r', 'p') and n.nspname !~ '^pg_' and n.nspname <> 'information_schema' order by 1, 2",
    )?
    .iter()
    .map(|r| (qualify(&s(&r[0]), &s(&r[1])), list(&r[2]), list(&r[3])))
    .collect();
    rel.cols = text(
        c,
        &format!(
            "select a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, pg_get_expr(d.adbin, d.adrelid), a.attidentity,
                    a.attgenerated = 's', col_description(a.attrelid, a.attnum),
                    a.attidentity = '' and d.adbin is not null and a.atttypid in (21, 23, 20)
                      and pg_get_serial_sequence(a.attrelid::regclass::text, a.attname) is not null
             from pg_attribute a left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum
             where a.attrelid = {oid} and a.attnum > 0 and not a.attisdropped order by a.attnum"
        ),
    )?
    .iter()
    .map(|r| RelCol {
        name: s(&r[0]),
        ty: short_type(&s(&r[1])),
        default: r[3].clone(),
        not_null: t(&r[2]),
        identity: match r[4].as_deref() {
            Some("a") => Some("always".into()),
            Some("d") => Some("by default".into()),
            _ => None,
        },
        generated: t(&r[5]),
        serial: t(&r[7]),
        comment: r[6].clone(),
    })
    .collect();
    let names = |k: &str, rel: &str| {
        format!(
            "(select string_agg(a.attname, chr(1) order by k.o) from unnest({k}) with ordinality k(n, o)
              join pg_attribute a on a.attrelid = {rel} and a.attnum = k.n)"
        )
    };
    rel.cons = text(
        c,
        &format!(
            "select x.conname, x.contype, {}, fn.nspname, fc.relname, {}, pg_get_constraintdef(x.oid), x.confupdtype, x.confdeltype,
                    coalesce(x.confkey = (select p.conkey from pg_constraint p where p.conrelid = x.confrelid and p.contype = 'p'), false),
                    obj_description(x.oid, 'pg_constraint'), x.condeferrable, x.condeferred, x.convalidated, x.connoinherit
             from pg_constraint x left join pg_class fc on fc.oid = x.confrelid left join pg_namespace fn on fn.oid = fc.relnamespace
             where x.conrelid = {oid} and x.contype in ('p', 'u', 'f', 'c') order by position(x.contype in 'pufc'), x.conname",
            names("x.conkey", "x.conrelid"),
            names("x.confkey", "x.confrelid")
        ),
    )?
    .iter()
    .map(|r| RelCon {
        name: s(&r[0]),
        kind: ch(&r[1]),
        cols: list(&r[2]),
        ref_schema: s(&r[3]),
        ref_table: s(&r[4]),
        ref_cols: list(&r[5]),
        def: s(&r[6]),
        on_update: ch(&r[7]),
        on_delete: ch(&r[8]),
        ref_pk: t(&r[9]),
        comment: r[10].clone(),
        deferrable: t(&r[11]),
        deferred: t(&r[12]),
        validated: t(&r[13]),
        no_inherit: t(&r[14]),
    })
    .collect();
    rel.indexes = text(
        c,
        &format!(
            "select i.relname, x.indisunique, am.amname, pg_get_expr(x.indpred, x.indrelid), pg_get_indexdef(x.indexrelid),
                    (select string_agg(pg_get_indexdef(x.indexrelid, k, true) || chr(2)
                              || coalesce((select quote_ident(collname) from pg_collation where oid = x.indcollation[k - 1] and oid <> 100), '')
                              || chr(2) || (x.indoption[k - 1] & 1) || (x.indoption[k - 1] & 2), chr(1) order by k)
                     from generate_series(1, x.indnkeyatts) k),
                    exists (select 1 from pg_constraint where conindid = x.indexrelid and conrelid = {oid}), obj_description(x.indexrelid, 'pg_class')
             from pg_index x join pg_class i on i.oid = x.indexrelid join pg_am am on am.oid = i.relam
             where x.indrelid = {oid} order by x.indexrelid"
        ),
    )?
    .iter()
    .map(|r| RelIdx {
        name: s(&r[0]),
        unique: t(&r[1]),
        method: s(&r[2]),
        pred: r[3].clone(),
        native: s(&r[4]),
        cols: list(&r[5])
            .iter()
            .map(|x| {
                let p: Vec<&str> = x.split('\u{2}').collect();
                let opt = p.get(2).copied().unwrap_or("00");
                IdxCol {
                    expr: p[0].to_string(),
                    collation: p.get(1).filter(|c| !c.is_empty()).map(|c| c.to_string()),
                    desc: opt.starts_with('1'),
                    nulls_first: opt.ends_with('2'),
                }
            })
            .collect(),
        constraint: t(&r[6]),
        comment: r[7].clone(),
    })
    .collect();
    Ok(rel)
}

/// `format_type` text as DataGrip writes it: `character varying(16)` → `varchar(16)`,
/// `numeric(10,2)` → `numeric(10, 2)`, `timestamp without time zone` → `timestamp`.
fn short_type(t: &str) -> String {
    let t = t.replace(" without time zone", "");
    let t = if let Some(r) = t.strip_prefix("character varying") {
        format!("varchar{r}")
    } else if let Some(r) = t.strip_prefix("character") {
        format!("char{r}")
    } else {
        t
    };
    t.replace(',', ", ")
}

/// SQL Generator › Generate.
pub const GENERATE: [&str; 8] = [
    "Creation script completely",
    "Definitions 'as is'",
    "Definitions provided by RDBMS server",
    "Enable constraints and triggers",
    "Disable constraints and triggers",
    "Refresh materialized views",
    "Execute procedures and functions",
    "Truncate table",
];
/// SQL Generator › Qualify objects with schema names.
pub const QUALIFY: [&str; 3] = ["Never", "Always", "Auto"];
/// SQL Generator › Place constraints.
pub const PLACE: [&str; 3] = ["Inside column", "Inside table", "After table"];

/// SQL Generator options (DataGrip's defaults).
#[derive(Clone, Debug, PartialEq)]
pub struct GenOpts {
    pub generate: usize,
    pub qualify: usize,
    pub place: usize,
    pub surrogate: bool,
    pub or_replace: bool,
    pub if_not_exists: bool,
    /// No PostgreSQL syntax for it: kept for the panel.
    pub disabling: bool,
    pub ignore_owner: bool,
    /// Grants aren't generated yet: kept for the panel.
    pub skip_grants: bool,
    pub show_grants: bool,
    pub regenerate: bool,
    pub reformat: bool,
}

impl Default for GenOpts {
    fn default() -> Self {
        GenOpts {
            generate: 0,
            qualify: 2,
            place: 0,
            surrogate: false,
            or_replace: false,
            if_not_exists: false,
            disabling: false,
            ignore_owner: false,
            skip_grants: false,
            show_grants: false,
            regenerate: false,
            reformat: true,
        }
    }
}

impl GenOpts {
    /// The panel's checkboxes, in DataGrip's order.
    pub fn flags(&mut self) -> [(&'static str, &mut bool); 9] {
        [
            ("Use surrogate (auto-generated) names", &mut self.surrogate),
            ("Use CREATE OR REPLACE syntax", &mut self.or_replace),
            ("Use CREATE IF NOT EXISTS syntax", &mut self.if_not_exists),
            ("With disabling clauses or statements", &mut self.disabling),
            ("Ignore owner", &mut self.ignore_owner),
            ("Skip grants", &mut self.skip_grants),
            ("Show grants in user definition", &mut self.show_grants),
            ("Regenerate database-provided code", &mut self.regenerate),
            ("Reformat generated code", &mut self.reformat),
        ]
    }
}

/// The name PostgreSQL gives a constraint created without one (DataGrip leaves those out).
fn default_con_name(table: &str, c: &RelCon) -> String {
    match c.kind {
        'p' => format!("{table}_pkey"),
        'u' => format!("{table}_{}_key", c.cols.join("_")),
        'f' => format!("{table}_{}_fkey", c.cols.join("_")),
        _ => String::new(),
    }
}

/// `(a > b)` wraps the whole text (not `(a) + (b)`); quotes are skipped.
pub fn wrapped(s: &str) -> bool {
    if !s.starts_with('(') {
        return false;
    }
    let (mut depth, mut quote) = (0, false);
    for (i, ch) in s.char_indices() {
        match ch {
            '\'' => quote = !quote,
            '(' if !quote => depth += 1,
            ')' if !quote => {
                depth -= 1;
                if depth == 0 {
                    return i == s.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// `pg_get_viewdef` laid out like DataGrip's formatter: clauses at the margin, the select list
/// under its first item.
// ponytail: line-based (top-level clauses / select list only); nested queries keep the server's layout
fn format_view(def: &str) -> String {
    const CLAUSES: [&str; 12] =
        ["SELECT", "FROM", "WHERE", "GROUP BY", "HAVING", "WINDOW", "ORDER BY", "LIMIT", "OFFSET", "UNION", "EXCEPT", "INTERSECT"];
    def.trim_end()
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = l.len() - t.len();
            if indent < 4 && CLAUSES.iter().any(|k| t.starts_with(k)) {
                t.to_string()
            } else if indent == 4 {
                format!("       {t}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `create index …` statement (SQL Generator and Modify Table): `unit` is one indent.
pub fn create_index(i: &RelIdx, table: &str, if_not_exists: bool, unit: &str) -> String {
    let desc = i.cols.iter().any(|c| c.desc);
    let cols: Vec<String> = i
        .cols
        .iter()
        .map(|c| {
            let mut s = c.expr.clone();
            if let Some(co) = &c.collation {
                s += &format!(" collate {co}");
            }
            if desc {
                s += if c.desc { " desc" } else { " asc" };
            }
            // NULLS FIRST is the default of DESC only
            if c.nulls_first != c.desc {
                s += if c.nulls_first { " nulls first" } else { " nulls last" };
            }
            s
        })
        .collect();
    let using = if i.method == "btree" || i.method.is_empty() { String::new() } else { format!(" using {}", i.method) };
    let mut s = format!(
        "create {}index {}{}\n{unit}on {table}{using} ({})",
        if i.unique { "unique " } else { "" },
        if if_not_exists { "if not exists " } else { "" },
        ident(&i.name),
        cols.join(", ")
    );
    if let Some(p) = i.pred.as_ref().filter(|p| !p.trim().is_empty()) {
        s += &format!("\n{unit}where {p}");
    }
    s + ";"
}

/// SQL Generator text of `r`, like DataGrip's for PostgreSQL: every statement is followed by a blank line.
pub fn ddl(r: &Rel, o: &GenOpts) -> String {
    let unit = if o.reformat { "    " } else { "\t" };
    let ind = |n: usize| unit.repeat(n);
    // Auto: no schema names when everything is in one schema, else only for non-current schemas
    let one_schema = r.cons.iter().all(|c| c.kind != 'f' || c.ref_schema == r.schema);
    let name = |schema: &str, n: &str| match o.qualify {
        1 => qualify(schema, n),
        2 if !one_schema && schema != "public" => qualify(schema, n),
        _ => ident(n),
    };
    let t = name(&r.schema, &r.name);
    let view = matches!(r.kind, 'v' | 'm');
    let table_word = if r.kind == 'm' { "materialized view" } else { "table" };
    let mut out: Vec<String> = vec![];
    match o.generate {
        2 => {
            let what = match r.kind {
                'v' => "view",
                'm' => "materialized view",
                _ => "table",
            };
            out.push(format!("-- No native definition for element: {} ({what})", r.name));
            for i in r.indexes.iter().filter(|i| !i.constraint) {
                out.push(format!("-- {}: index\n{};", i.name, i.native));
            }
        }
        3 | 4 if !view => {
            let verb = if o.generate == 3 { "enable" } else { "disable" };
            for c in &r.cons {
                out.push(format!("alter table {t}\n{}{verb} constraint {}\n;", ind(1), ident(&c.name)));
            }
        }
        5 if r.kind == 'm' => out.push(format!("refresh materialized view {t};")),
        7 if !view => out.push(format!("truncate table {t};")),
        0 | 1 => {
            let named = |c: &RelCon| o.surrogate || c.kind == 'c' || c.name != default_con_name(&r.name, c);
            // a constraint as lines at `level` (`prefix`: `add ` after the table); `inline`: under its column
            let con_lines = |c: &RelCon, level: usize, prefix: &str, inline: bool| -> Vec<String> {
                let cols = c.cols.iter().map(|x| ident(x)).collect::<Vec<_>>().join(", ");
                let mut body = match c.kind {
                    'p' if inline => vec!["primary key".to_string()],
                    'p' => vec![format!("primary key ({cols})")],
                    'u' if inline => vec!["unique".to_string()],
                    'u' => vec![format!("unique ({cols})")],
                    'f' => {
                        let mut target = name(&c.ref_schema, &c.ref_table);
                        if !c.ref_pk {
                            target += &format!(" ({})", c.ref_cols.iter().map(|x| ident(x)).collect::<Vec<_>>().join(", "));
                        }
                        let mut v = vec![if inline { format!("references {target}") } else { format!("foreign key ({cols}) references {target}") }];
                        for (when, a) in [("update", c.on_update), ("delete", c.on_delete)] {
                            let act = match a {
                                'r' => "restrict",
                                'c' => "cascade",
                                'n' => "set null",
                                'd' => "set default",
                                _ => continue,
                            };
                            v.push(format!("on {when} {act}"));
                        }
                        v
                    }
                    _ => {
                        let e = c.def.strip_prefix("CHECK ").unwrap_or(&c.def);
                        let e = e.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(e);
                        vec![if wrapped(e) { format!("check {e}") } else { format!("check ({e})") }]
                    }
                };
                let mut lines = vec![];
                let mut level = level;
                if named(c) {
                    lines.push(format!("{}{prefix}constraint {}", ind(level), ident(&c.name)));
                    level += 1;
                } else {
                    body[0] = format!("{prefix}{}", body[0]);
                }
                lines.push(format!("{}{}", ind(level), body[0]));
                lines.extend(body[1..].iter().map(|b| format!("{}{b}", ind(level + 1))));
                lines
            };
            let inline = |c: &RelCon| o.place == 0 && c.cols.len() == 1;
            if let (true, Some(def)) = (view, &r.view) {
                let replace = if o.or_replace && r.kind == 'v' { "or replace " } else { "" };
                let body = if o.reformat { format_view(def) } else { def.trim().to_string() };
                let body = body.trim_end().trim_end_matches(';');
                if r.kind == 'm' {
                    out.push(format!("create materialized view {t} as\n{body};"));
                } else {
                    let cols = r.cols.iter().map(|c| ident(&c.name)).collect::<Vec<_>>().join(", ");
                    out.push(format!("create {replace}view {t}({cols}) as\n{body};"));
                }
            } else if !view {
                // columns: name | type | default | not null, each part aligned over the rows having it
                let parts: Vec<(String, String, Option<String>, bool)> = r
                    .cols
                    .iter()
                    .map(|c| {
                        let ty = match (c.serial, c.ty.as_str()) {
                            (true, "integer") => "serial".to_string(),
                            (true, "bigint") => "bigserial".to_string(),
                            (true, "smallint") => "smallserial".to_string(),
                            _ => c.ty.clone(),
                        };
                        let def = match (&c.identity, c.generated, &c.default) {
                            (Some(how), ..) => Some(format!("generated {how} as identity")),
                            (None, true, Some(e)) => Some(format!("generated always as ({e}) stored")),
                            _ if c.serial => None,
                            (.., d) => d.as_ref().map(|d| format!("default {d}")),
                        };
                        (ident(&c.name), ty, def, c.not_null && !c.serial && c.identity.is_none())
                    })
                    .collect();
                let w = |s: &str| s.chars().count();
                let name_w = parts.iter().map(|p| w(&p.0)).max().unwrap_or(0) + 1;
                let def_at = parts.iter().filter(|p| p.2.is_some()).map(|p| w(&p.1) + 1).max().unwrap_or(0);
                let nn_at = parts
                    .iter()
                    .filter(|p| p.3)
                    .map(|p| p.2.as_ref().map_or(w(&p.1), |d| def_at + w(d)) + 1)
                    .max()
                    .unwrap_or(0);
                let mut items: Vec<String> = vec![];
                for (i, (n, ty, def, nn)) in parts.iter().enumerate() {
                    let mut line = if o.reformat { format!("{n:name_w$}{ty}", name_w = name_w) } else { format!("{n} {ty}") };
                    if let Some(d) = def {
                        line = if o.reformat { format!("{line:width$}{d}", width = name_w + def_at) } else { format!("{line} {d}") };
                    }
                    if *nn {
                        line = if o.reformat { format!("{line:width$}not null", width = name_w + nn_at) } else { format!("{line} not null") };
                    }
                    let mut lines = vec![format!("{}{line}", ind(1))];
                    for c in r.cons.iter().filter(|c| inline(c) && c.cols[0] == r.cols[i].name) {
                        lines.extend(con_lines(c, 2, "", true));
                    }
                    items.push(lines.join("\n"));
                }
                if o.place < 2 {
                    for c in r.cons.iter().filter(|c| !inline(c)) {
                        items.push(con_lines(c, 1, "", false).join("\n"));
                    }
                }
                let ine = if o.if_not_exists { "if not exists " } else { "" };
                out.push(format!("create {table_word} {ine}{t}\n(\n{}\n);", items.join(",\n")));
            }
            if let Some(cm) = &r.comment {
                out.push(format!("comment on {} {t} is {};", if r.kind == 'v' { "view" } else { table_word }, lit(cm)));
            }
            for c in r.cols.iter().filter(|c| c.comment.is_some()) {
                out.push(format!("comment on column {t}.{} is {};", ident(&c.name), lit(c.comment.as_deref().unwrap_or_default())));
            }
            if !o.ignore_owner && !r.owner.is_empty() {
                out.push(format!("alter {table_word} {t}\n{}owner to {};", ind(1), ident(&r.owner)));
            }
            for i in r.indexes.iter().filter(|i| !i.constraint) {
                out.push(create_index(i, &t, o.if_not_exists, unit));
            }
            if o.place == 2 && !view {
                for c in &r.cons {
                    let mut lines = con_lines(c, 1, "add ", false);
                    if let Some(l) = lines.last_mut() {
                        l.push(';');
                    }
                    out.push(format!("alter table {t}\n{}", lines.join("\n")));
                }
            }
        }
        _ => {}
    }
    out.iter().map(|s| format!("{s}\n\n")).collect()
}

// ------------------------------------------------------------------ SQL text helpers

/// PostgreSQL reserved key words (they must be quoted as identifiers).
const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "authorization",
    "binary",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "collation",
    "column",
    "concurrently",
    "constraint",
    "create",
    "cross",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "freeze",
    "from",
    "full",
    "grant",
    "group",
    "having",
    "ilike",
    "in",
    "initially",
    "inner",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "lateral",
    "leading",
    "left",
    "like",
    "limit",
    "localtime",
    "localtimestamp",
    "natural",
    "not",
    "notnull",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "overlaps",
    "placing",
    "primary",
    "references",
    "returning",
    "right",
    "select",
    "session_user",
    "similar",
    "some",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "verbose",
    "when",
    "where",
    "window",
    "with",
];

/// Identifier as SQL: bare when it is a plain lower-case name, else double-quoted.
pub fn ident(s: &str) -> String {
    let plain = s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$')
        && !RESERVED.contains(&s);
    if plain { s.into() } else { format!("\"{}\"", s.replace('"', "\"\"")) }
}

/// `schema.table`, each part quoted when needed: the identity of a table across the app.
pub fn qualify(schema: &str, name: &str) -> String {
    format!("{}.{}", ident(schema), ident(name))
}

/// Inverse of `qualify`: (schema, name); the schema is empty for a bare name.
pub fn split_qual(q: &str) -> (String, String) {
    let mut parts = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut it = q.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' if quoted && it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            '"' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    let name = cur;
    (parts.pop().unwrap_or_default(), name)
}

/// String literal (`standard_conforming_strings`, the default since 9.1).
pub fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Grid cell text → SQL value: `null` (the grid's Set NULL) is NULL, anything else a literal the
/// server casts to the column type.
fn value(t: &str) -> String {
    if t == "null" { "NULL".into() } else { lit(t) }
}

/// `WHERE` condition of a row identity (primary key column → value).
fn cond(id: &Bson) -> String {
    let parts: Vec<String> = match id {
        Bson::Document(d) => d
            .iter()
            .map(|(k, v)| match v {
                Bson::Null => format!("{} IS NULL", ident(k)),
                Bson::Boolean(b) => format!("{} = {b}", ident(k)),
                Bson::String(s) => format!("{} = {}", ident(k), lit(s)),
                v => format!("{} = {}", ident(k), lit(&v.to_string())),
            })
            .collect(),
        _ => vec![],
    };
    if parts.is_empty() { "false".into() } else { parts.join(" AND ") }
}

/// The statement Submit runs for a pending grid change (also shown by Preview Pending Changes).
pub fn edit_sql(table: &str, e: &Edit) -> String {
    match e {
        Edit::Set { id, field, text, .. } => format!("UPDATE {table} SET {} = {} WHERE {}", ident(field), value(text), cond(id)),
        Edit::Insert { fields } if fields.is_empty() => format!("INSERT INTO {table} DEFAULT VALUES"),
        Edit::Insert { fields } => format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            fields.iter().map(|(k, ..)| ident(k)).collect::<Vec<_>>().join(", "),
            fields.iter().map(|(_, v, _)| value(v)).collect::<Vec<_>>().join(", ")
        ),
        Edit::Delete { id } => format!("DELETE FROM {table} WHERE {}", cond(id)),
    }
}

/// Splits a script into statement byte ranges at `;`, skipping quotes ('…', E'…', "…", $tag$…$tag$)
/// and comments (`--`, nested `/* */`).
pub fn split_sql(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    let push = |a: usize, z: usize, out: &mut Vec<(usize, usize)>| {
        if !src[a..z].trim().trim_matches(';').trim().is_empty() {
            out.push((a, z));
        }
    };
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                let esc = i > 0 && (b[i - 1] == b'E' || b[i - 1] == b'e') && !(i > 1 && word(b[i - 2]));
                i += 1;
                while i < b.len() {
                    if esc && b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            b'$' if !(i > 0 && word(b[i - 1])) => {
                // $tag$ … $tag$ (not $1 parameters)
                let tag_end = src[i + 1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|k| i + 1 + k);
                if let Some(e) = tag_end.filter(|&e| b[e] == b'$' && !b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
                    let tag = &src[i..=e];
                    i = src[e + 1..].find(tag).map_or(b.len(), |k| e + 1 + k + tag.len() - 1);
                }
            }
            b';' => {
                push(start, i + 1, &mut out);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    push(start, b.len(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn splits_sql_statements() {
        let s = "select 1; select ';' as x, \"a;b\" from t -- c;\n;\ncreate function f() returns int as $$ select 1; $$ language sql;\n/* x; /* y; */ */ select E'\\';'; select $1";
        let parts: Vec<&str> = split_sql(s).iter().map(|&(a, b)| s[a..b].trim().trim_end_matches(';').trim()).collect();
        assert_eq!(
            parts,
            [
                "select 1",
                "select ';' as x, \"a;b\" from t -- c;",
                "create function f() returns int as $$ select 1; $$ language sql",
                "/* x; /* y; */ */ select E'\\';'",
                "select $1"
            ]
        );
    }

    #[test]
    fn quotes_identifiers_and_edits() {
        assert_eq!(ident("orders"), "orders");
        assert_eq!(ident("Orders"), "\"Orders\"");
        assert_eq!(ident("order"), "\"order\"");
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(qualify("public", "my table"), "public.\"my table\"");
        assert_eq!(split_qual("public.\"my.ta\"\"ble\""), ("public".into(), "my.ta\"ble".into()));
        assert_eq!(split_qual("orders"), (String::new(), "orders".into()));
        let id = Bson::Document(doc! {"id": "7"});
        let set = Edit::Set { id: id.clone(), field: "note".into(), text: "it's".into(), as_string: false };
        assert_eq!(edit_sql("public.orders", &set), "UPDATE public.orders SET note = 'it''s' WHERE id = '7'");
        let null = Edit::Set { id: id.clone(), field: "note".into(), text: "null".into(), as_string: false };
        assert_eq!(edit_sql("t", &null), "UPDATE t SET note = NULL WHERE id = '7'");
        let ins = Edit::Insert { fields: vec![("qty".into(), "2".into(), false), ("user".into(), "x".into(), true)] };
        assert_eq!(edit_sql("t", &ins), "INSERT INTO t (qty, \"user\") VALUES ('2', 'x')");
        assert_eq!(edit_sql("t", &Edit::Insert { fields: vec![] }), "INSERT INTO t DEFAULT VALUES");
        assert_eq!(edit_sql("t", &Edit::Delete { id: Bson::Null }), "DELETE FROM t WHERE false");
    }

    /// `gen_demo` as the catalog describes it (see `ddl_like_datagrip`).
    fn gen_demo() -> Rel {
        let col = |name: &str, ty: &str, default: Option<&str>, not_null: bool| RelCol {
            name: name.into(),
            ty: ty.into(),
            default: default.map(String::from),
            not_null,
            ..RelCol::default()
        };
        let con = |name: &str, kind: char, cols: &[&str], def: &str| RelCon {
            name: name.into(),
            kind,
            cols: cols.iter().map(|c| c.to_string()).collect(),
            def: def.into(),
            on_update: 'a',
            on_delete: 'a',
            ..RelCon::default()
        };
        let fk = |name: &str, col: &str, table: &str, ref_col: &str, pk: bool, del: char| RelCon {
            ref_schema: "public".into(),
            ref_table: table.into(),
            ref_cols: vec![ref_col.into()],
            ref_pk: pk,
            on_delete: del,
            ..con(name, 'f', &[col], "")
        };
        let mut a = col("a", "integer", None, true);
        a.comment = Some("col a".into());
        Rel {
            schema: "public".into(),
            name: "gen_demo".into(),
            kind: 'r',
            owner: "postgres".into(),
            comment: Some("demo table".into()),
            cols: vec![
                a,
                col("b", "text", Some("'x'::text"), true),
                col("c", "integer", None, false),
                col("d", "varchar(16)", None, false),
                col("e", "text", None, false),
                RelCol { identity: Some("always".into()), ..col("f", "integer", None, true) },
                col("g", "numeric", None, false),
                col("h", "text", None, false),
                col("i", "text", None, false),
                col("j", "integer", Some("5"), false),
            ],
            cons: vec![
                con("gen_demo_pk", 'p', &["a", "b"], "PRIMARY KEY (a, b)"),
                con("gen_demo_ce_uniq", 'u', &["c", "e"], "UNIQUE (c, e)"),
                fk("gen_demo_c_fkey", "c", "customers", "id", true, 'c'),
                fk("gen_demo_d_fkey", "d", "products", "sku", true, 'a'),
                fk("gen_demo_i_fkey", "i", "customers", "email", false, 'a'),
                con("g_positive", 'c', &["g"], "CHECK ((g > (0)::numeric))"),
                con("gen_demo_check", 'c', &["a", "c"], "CHECK ((a < c))"),
            ],
            indexes: vec![
                RelIdx {
                    name: "gen_demo_e_idx".into(),
                    unique: true,
                    method: "btree".into(),
                    cols: vec![IdxCol { expr: "lower(e)".into(), ..IdxCol::default() }],
                    pred: Some("(e IS NOT NULL)".into()),
                    native: "CREATE UNIQUE INDEX gen_demo_e_idx ON public.gen_demo USING btree (lower(e)) WHERE (e IS NOT NULL)".into(),
                    ..RelIdx::default()
                },
                RelIdx {
                    name: "gen_demo_h_idx".into(),
                    method: "hash".into(),
                    cols: vec![IdxCol { expr: "h".into(), collation: Some("\"C\"".into()), ..IdxCol::default() }],
                    native: "CREATE INDEX gen_demo_h_idx ON public.gen_demo USING hash (h)".into(),
                    ..RelIdx::default()
                },
            ],
            ..Rel::default()
        }
    }

    /// Expected texts are what DataGrip 2025's SQL Generator shows for the same table.
    #[test]
    fn ddl_like_datagrip() {
        let mut r = gen_demo();
        let o = GenOpts::default();
        let tail = "comment on table gen_demo is 'demo table';

comment on column gen_demo.a is 'col a';

alter table gen_demo
    owner to postgres;

create unique index gen_demo_e_idx
    on gen_demo (lower(e))
    where (e IS NOT NULL);

create index gen_demo_h_idx
    on gen_demo using hash (h collate \"C\");

";
        assert_eq!(
            ddl(&r, &o),
            "create table gen_demo
(
    a integer                   not null,
    b text    default 'x'::text not null,
    c integer
        references customers
            on delete cascade,
    d varchar(16)
        references products,
    e text,
    f integer generated always as identity,
    g numeric
        constraint g_positive
            check (g > (0)::numeric),
    h text,
    i text
        references customers (email),
    j integer default 5,
    constraint gen_demo_pk
        primary key (a, b),
    constraint gen_demo_ce_uniq
        unique (c, e),
    constraint gen_demo_check
        check (a < c)
);

"
            .to_string()
                + tail
        );
        let inside_table = GenOpts { place: 1, ..o.clone() };
        assert!(ddl(&r, &inside_table).starts_with(
            "create table gen_demo
(
    a integer                   not null,
    b text    default 'x'::text not null,
    c integer,
    d varchar(16),
    e text,
    f integer generated always as identity,
    g numeric,
    h text,
    i text,
    j integer default 5,
    constraint gen_demo_pk
        primary key (a, b),
    constraint gen_demo_ce_uniq
        unique (c, e),
    foreign key (c) references customers
        on delete cascade,
    foreign key (d) references products,
    foreign key (i) references customers (email),
    constraint g_positive
        check (g > (0)::numeric),
    constraint gen_demo_check
        check (a < c)
);

"
        ));
        let after = ddl(&r, &GenOpts { place: 2, ..o.clone() });
        assert!(after.contains("    j integer default 5\n);\n\n"), "{after}");
        assert!(after.ends_with(
            "alter table gen_demo
    add constraint gen_demo_pk
        primary key (a, b);

alter table gen_demo
    add constraint gen_demo_ce_uniq
        unique (c, e);

alter table gen_demo
    add foreign key (c) references customers
        on delete cascade;

alter table gen_demo
    add foreign key (d) references products;

alter table gen_demo
    add foreign key (i) references customers (email);

alter table gen_demo
    add constraint g_positive
        check (g > (0)::numeric);

alter table gen_demo
    add constraint gen_demo_check
        check (a < c);

"
        ));
        let plain = GenOpts { surrogate: true, if_not_exists: true, ignore_owner: true, reformat: false, ..o.clone() };
        assert_eq!(
            ddl(&r, &plain),
            "create table if not exists gen_demo
(
\ta integer not null,
\tb text default 'x'::text not null,
\tc integer
\t\tconstraint gen_demo_c_fkey
\t\t\treferences customers
\t\t\t\ton delete cascade,
\td varchar(16)
\t\tconstraint gen_demo_d_fkey
\t\t\treferences products,
\te text,
\tf integer generated always as identity,
\tg numeric
\t\tconstraint g_positive
\t\t\tcheck (g > (0)::numeric),
\th text,
\ti text
\t\tconstraint gen_demo_i_fkey
\t\t\treferences customers (email),
\tj integer default 5,
\tconstraint gen_demo_pk
\t\tprimary key (a, b),
\tconstraint gen_demo_ce_uniq
\t\tunique (c, e),
\tconstraint gen_demo_check
\t\tcheck (a < c)
);

comment on table gen_demo is 'demo table';

comment on column gen_demo.a is 'col a';

create unique index if not exists gen_demo_e_idx
\ton gen_demo (lower(e))
\twhere (e IS NOT NULL);

create index if not exists gen_demo_h_idx
\ton gen_demo using hash (h collate \"C\");

"
        );
        let mode = |g: usize| ddl(&r, &GenOpts { generate: g, ..o.clone() });
        assert_eq!(mode(1), ddl(&r, &o));
        assert_eq!(
            mode(2),
            "-- No native definition for element: gen_demo (table)

-- gen_demo_e_idx: index
CREATE UNIQUE INDEX gen_demo_e_idx ON public.gen_demo USING btree (lower(e)) WHERE (e IS NOT NULL);

-- gen_demo_h_idx: index
CREATE INDEX gen_demo_h_idx ON public.gen_demo USING hash (h);

"
        );
        assert!(mode(4).starts_with("alter table gen_demo\n    disable constraint gen_demo_pk\n;\n\nalter table gen_demo\n"));
        assert_eq!(mode(4).matches("disable constraint").count(), 7);
        assert_eq!(mode(5), "");
        assert_eq!(mode(7), "truncate table gen_demo;\n\n");
        assert!(ddl(&r, &GenOpts { qualify: 1, ..o.clone() }).starts_with("create table public.gen_demo\n"));
        // Auto: schema names once the objects span schemas (but not for the current schema)
        r.schema = "logs".into();
        let auto = ddl(&r, &o);
        assert!(auto.starts_with("create table logs.gen_demo\n") && auto.contains("references customers\n"), "{auto}");
        r.cons.retain(|c| c.kind != 'f');
        assert!(ddl(&r, &o).starts_with("create table gen_demo\n"));
        assert_eq!(format_view(" SELECT id,\n    sku\n   FROM orders\n  WHERE status = 'paid'::text;"), "SELECT id,\n       sku\nFROM orders\nWHERE status = 'paid'::text;");
        assert_eq!((short_type("character varying(16)[]"), short_type("numeric(10,2)")), ("varchar(16)[]".into(), "numeric(10, 2)".into()));
        assert!(wrapped("(a > b)") && !wrapped("(a) + (b)") && !wrapped("f(x)") && wrapped("(s <> ')')"));
    }

    /// Local PostgreSQL with the `datatired` database from `docker/pg-seed.sql`; skipped when absent.
    pub fn pg_url() -> String {
        std::env::var("DATATIRED_PG").unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:5432/datatired".into())
    }

    #[test]
    fn worker_against_local_postgres() {
        let (tx, rx) = channel();
        let conn = open(pg_url(), tx, || {});
        let mut tag = 0;
        let mut ask = |req: Req| {
            tag += 1;
            conn.send(tag, req);
            rx.recv().unwrap().body
        };
        match ask(Req::Connect) {
            Ok(Body::Connected { version }) => assert!(version.starts_with(|c: char| c.is_ascii_digit()), "{version}"),
            Err(e) => return eprintln!("skipped, no local postgres: {e}"),
            _ => panic!(),
        }
        let Ok(Body::Databases(dbs)) = ask(Req::Databases) else { panic!() };
        assert!(dbs.contains(&"datatired".to_string()), "{dbs:?}");
        let Ok(Body::Collections { schemas, colls }) = ask(Req::Collections { db: "datatired".into() }) else { panic!() };
        assert_eq!(schemas, ["logs", "public"]);
        for want in
            [("public", "customers", 'r'), ("public", "paid_orders", 'v'), ("public", "orders_id_seq", 'S'), ("logs", "events", 'r')]
        {
            assert!(colls.contains(&(want.0.into(), want.1.into(), want.2)), "{want:?} in {colls:?}");
        }
        let Ok(Body::Meta(m)) = ask(Req::Meta { db: "datatired".into(), coll: "public.orders".into() }) else { panic!() };
        let labels: Vec<&str> = m.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["columns", "keys", "foreign keys", "indexes", "checks"]);
        let find = |l: &str, n: &str| m.iter().find(|x| x.0 == l).unwrap().1.iter().find(|i| i.name == n).unwrap().clone();
        assert_eq!(find("columns", "id").detail, "bigint = nextval('orders_id_seq'::regclass)");
        assert!(find("columns", "id").kind == ItemKind::PkColumn);
        assert_eq!(find("foreign keys", "orders_customer_id_fkey").detail, "(customer_id) → customers (id)");
        assert_eq!(find("checks", "orders_qty_check").detail, "(qty > 0)");
        assert_eq!(find("indexes", "orders_pkey").detail, "(id) UNIQUE");

        // SQL Generator: the catalog read gives DataGrip's text for the seed tables
        let mut generate = |coll: &str| match ask(Req::Ddl { db: "datatired".into(), coll: coll.into() }) {
            Ok(Body::Ddl(r)) => ddl(&r, &GenOpts::default()),
            Err(e) => panic!("{e}"),
            _ => panic!(),
        };
        let owner = "\n\nalter table customers\n    owner to postgres;\n\n";
        assert_eq!(
            generate("public.customers"),
            "create table customers
(
    id         serial
        primary key,
    name       text                                   not null,
    email      text                                   not null
        unique,
    age        integer,
    active     boolean                  default true  not null,
    tier       text,
    balance    numeric(10, 2),
    visits     bigint,
    created_at timestamp with time zone default now() not null,
    address    jsonb,
    tags       text[],
    note       text
);"
            .to_string()
                + owner
                + "create index customers_city_age
    on customers ((address ->> 'city'::text) asc, age desc);

"
        );
        assert!(generate("public.orders").starts_with(
            "create table orders
(
    id          bigserial
        primary key,
    customer_id integer     not null
        references customers,
    sku         varchar(16) not null
        references products,
    qty         integer     not null
        constraint orders_qty_check
            check (qty > 0),
    total       numeric(10, 2),
    status      text        not null,
    ordered_at  date
);

alter table orders
"
        ));
        assert!(generate("public.products").starts_with(
            "create table products
(
    sku          varchar(16)           not null
        primary key,
    name         text                  not null,
    price        numeric(8, 2)         not null,
    stock        integer default 0     not null,
    category     text,
    discontinued boolean default false not null
);
"
        ));
        assert_eq!(
            generate("public.paid_orders"),
            "create view paid_orders(id, customer_id, sku, qty, total, status, ordered_at) as
SELECT id,
       customer_id,
       sku,
       qty,
       total,
       status,
       ordered_at
FROM orders
WHERE status = 'paid'::text;

alter table paid_orders
    owner to postgres;

"
        );
        assert!(generate("logs.events").starts_with("create table events\n(\n    id   bigserial\n        primary key,\n    at   timestamp default now() not null,\n"));

        // data editor: WHERE / ORDER BY, typed text columns, editable through the primary key
        let find_req = |filter: &str, sort: &str| Req::Find {
            db: "datatired".into(),
            coll: "public.orders".into(),
            filter: filter.into(),
            sort: sort.into(),
            offset: 0,
            limit: 10,
        };
        let Ok(Body::Rows(rs)) = ask(find_req("status = 'paid'", "id DESC")) else { panic!() };
        assert_eq!(rs.cols[0], ("id".into(), "int8".into()));
        assert_eq!(rs.docs.len(), 10);
        assert!(rs.more);
        assert_eq!(rs.docs[0].get_str("status"), Ok("paid"));
        assert_eq!(rs.docs[0].get_str("total").map(|t| t.split_once('.').map(|x| x.1.len())), Ok(Some(2)), "numeric keeps its scale");
        assert_eq!(rs.table(), Some(("datatired".into(), "public.orders".into())));
        assert_eq!(rs.key(), ["id"]);
        let Err(e) = ask(find_req("nosuch = 1", "")) else { panic!() };
        assert!(e.starts_with("[42703] ERROR: column \"nosuch\" does not exist"), "{e}");

        // console: statements, notices, paging, edits through Save, errors stop the script
        let run = |script: &str| Req::Run { db: "datatired".into(), schema: "public".into(), script: script.into() };
        let script = "drop schema if exists ut cascade; create schema ut;\ncreate table ut.t (id int primary key, s text, b bool);\n\
                      insert into ut.t select i, 's' || i, i % 2 = 0 from generate_series(1, 600) i;\n\
                      do $$ begin raise notice 'hello'; end $$;\nselect * from ut.t order by id;\nselect count(*), 'x' as x, 'y' as x from ut.t";
        let Ok(Body::Ran(items)) = ask(run(script)) else { panic!() };
        let logs: Vec<String> =
            items.iter().filter_map(|i| if let RunItem::Log(l, _) = i { Some(l[22..].to_string()) } else { None }).collect();
        assert!(!items.iter().any(|i| matches!(i, RunItem::Log(_, LogKind::Error))), "{logs:#?}");
        assert!(logs.contains(&"datatired.public> create table ut.t (id int primary key, s text, b bool)".into()), "{logs:#?}");
        assert!(logs.iter().any(|l| l.starts_with("600 rows affected")), "{logs:#?}");
        assert!(logs.iter().any(|l| l == "NOTICE: hello"), "{logs:#?}");
        let results: Vec<&ResultSet> = items.iter().filter_map(|i| if let RunItem::Result(r) = i { Some(r) } else { None }).collect();
        assert_eq!((results[0].title.as_str(), results[0].docs.len(), results[0].more), ("datatired.ut.t", 500, true));
        assert_eq!(results[0].docs[1].get("b"), Some(&Bson::Boolean(true)));
        assert_eq!(results[1].cols.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["count", "x", "x (2)"]);
        assert_eq!(results[1].table(), None, "aggregate is read-only");
        let spec = results[0].spec.clone().unwrap();
        let Ok(Body::Rows(p2)) = ask(Req::Page { spec, offset: 500, limit: 500 }) else { panic!() };
        assert_eq!((p2.docs.len(), p2.more, p2.docs[0].get_str("id")), (100, false, Ok("501")));
        let id = |n: &str| Bson::Document(doc! {"id": n});
        let edits = vec![
            Edit::Set { id: id("1"), field: "s".into(), text: "it's".into(), as_string: true },
            Edit::Set { id: id("2"), field: "s".into(), text: "null".into(), as_string: false },
            Edit::Insert { fields: vec![("id".into(), "1000".into(), false), ("b".into(), "true".into(), false)] },
            Edit::Delete { id: id("3") },
        ];
        assert!(ask(Req::Save { db: "datatired".into(), coll: "ut.t".into(), edits }).is_ok());
        let Ok(Body::Ran(items)) = ask(run("select id, s, b from ut.t where id in (1, 2, 3, 1000) order by id")) else { panic!() };
        let Some(RunItem::Result(rs)) = items.iter().find(|i| matches!(i, RunItem::Result(_))) else { panic!() };
        let rows: Vec<(String, Bson)> = rs.docs.iter().map(|d| (d.get_str("id").unwrap().into(), d.get("s").unwrap().clone())).collect();
        assert_eq!(rows, [("1".into(), Bson::String("it's".into())), ("2".into(), Bson::Null), ("1000".into(), Bson::Null)]);
        let Ok(Body::Ran(items)) = ask(run("select 1; select * from nosuch; select 2")) else { panic!() };
        let errs: Vec<&String> =
            items.iter().filter_map(|i| if let RunItem::Log(l, LogKind::Error) = i { Some(l) } else { None }).collect();
        assert!(errs.len() == 1 && errs[0].contains("[42P01] ERROR: relation \"nosuch\" does not exist"), "{errs:?}");
        assert!(!items.iter().any(|i| matches!(i, RunItem::Log(l, _) if l.ends_with("> select 2"))), "stops at the error");
        assert!(ask(run("drop schema ut cascade")).is_ok());
    }
}
