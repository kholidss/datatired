//! Persisted projects, data sources and consoles. Passwords live in a user-only credentials file.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default, Debug)]
pub enum Driver {
    #[default]
    Mongo,
    Postgres,
}

impl Driver {
    pub fn name(self) -> &'static str {
        match self {
            Driver::Mongo => "MongoDB",
            Driver::Postgres => "PostgreSQL",
        }
    }

    pub fn default_port(self) -> &'static str {
        match self {
            Driver::Mongo => "27017",
            Driver::Postgres => "5432",
        }
    }

    /// Console file extension.
    pub fn ext(self) -> &'static str {
        match self {
            Driver::Mongo => "js",
            Driver::Postgres => "sql",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum ConnType {
    #[default]
    Default,
    Srv,
    UrlOnly,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum Auth {
    #[default]
    UserPassword,
    NoAuth,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct DataSource {
    pub id: u64,
    pub driver: Driver,
    pub name: String,
    pub comment: String,
    pub conn_type: ConnType,
    pub host: String,
    pub port: String,
    pub auth: Auth,
    pub user: String,
    pub database: String,
    pub read_pref: String,
    pub replica_set: String,
    pub url: String,
    pub color: Option<[u8; 3]>,
    /// Databases shown in the explorer; `None` = all.
    pub schemas: Option<Vec<String>>,
    /// The password was typed inside the URL: the dialog shows the URL with it again, like DataGrip,
    /// while the project file keeps the URL without it (the password is in the credentials file).
    pub url_password: bool,
}

impl Default for DataSource {
    fn default() -> Self {
        let mut d = DataSource {
            id: 0,
            driver: Driver::Mongo,
            name: "@localhost".into(),
            comment: String::new(),
            conn_type: ConnType::Default,
            host: "localhost".into(),
            port: "27017".into(),
            auth: Auth::UserPassword,
            user: String::new(),
            database: String::new(),
            read_pref: String::new(),
            replica_set: String::new(),
            url: String::new(),
            color: None,
            schemas: None,
            url_password: false,
        };
        d.url = d.generated_url();
        d
    }
}

impl DataSource {
    pub fn new(driver: Driver) -> Self {
        let mut d = DataSource { driver, port: driver.default_port().into(), ..Default::default() };
        d.url = d.generated_url();
        d
    }

    /// URL shown in the dialog (no password), regenerated whenever the fields change.
    pub fn generated_url(&self) -> String {
        if self.driver == Driver::Postgres {
            // DataGrip's template: jdbc:postgresql://{host::localhost}?[:{port::5432}][/{database::postgres}?]
            let db = if self.database.is_empty() { "postgres" } else { &self.database };
            return format!("jdbc:postgresql://{}:{}/{}", self.host, self.port, enc(db));
        }
        let user = match (&self.auth, self.user.is_empty()) {
            (Auth::UserPassword, false) => format!("{}@", enc(&self.user)),
            _ => String::new(),
        };
        let mut q = Vec::new();
        if !self.replica_set.is_empty() {
            q.push(format!("replicaSet={}", enc(&self.replica_set)));
        }
        if !self.read_pref.is_empty() {
            q.push(format!("readPreference={}", self.read_pref));
        }
        let (scheme, host) = match self.conn_type {
            ConnType::Srv => ("mongodb+srv", self.host.clone()),
            _ => ("mongodb", format!("{}:{}", self.host, self.port)),
        };
        let db = if self.database.is_empty() && q.is_empty() { String::new() } else { format!("/{}", self.database) };
        let q = if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) };
        format!("{scheme}://{user}{host}{db}{q}")
    }

    /// Connection string handed to the driver (password injected after the user name). PostgreSQL:
    /// a libpq URI (`postgresql://user:pw@host:port/db?…`) made from the JDBC URL.
    pub fn uri(&self, password: Option<&str>) -> String {
        let url = if self.url.is_empty() { self.generated_url() } else { self.url.clone() };
        if self.driver == Driver::Postgres {
            let url = url.strip_prefix("jdbc:").unwrap_or(&url);
            let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
            let has_user = rest[..rest.find(['/', '?']).unwrap_or(rest.len())].contains('@');
            let user = match (&self.auth, password.filter(|p| !p.is_empty())) {
                _ if has_user || self.user.is_empty() || self.auth == Auth::NoAuth => String::new(),
                (_, Some(pw)) => format!("{}:{}@", enc(&self.user), enc(pw)),
                _ => format!("{}@", enc(&self.user)),
            };
            return format!("{scheme}://{user}{rest}");
        }
        match (password, &self.auth) {
            (Some(pw), Auth::UserPassword) if !pw.is_empty() && !self.user.is_empty() => {
                url.replacen(&format!("{}@", enc(&self.user)), &format!("{}:{}@", enc(&self.user), enc(pw)), 1)
            }
            _ => url,
        }
    }
}

impl DataSource {
    /// User & Password auth with a user but neither a stored password nor one inside the URL:
    /// connecting would fail with an opaque "SCRAM failure: no password supplied".
    pub fn missing_password(&self, pw: Option<&str>) -> bool {
        let inline = self.url.contains("password=") || inline_password(&self.url);
        self.auth == Auth::UserPassword && !self.user.is_empty() && pw.is_none_or(str::is_empty) && !inline
    }

    /// PostgreSQL database the URL connects to (DataGrip's default `postgres`).
    pub fn pg_database(&self) -> String {
        let uri = self.uri(None);
        let path = uri.split_once("://").and_then(|(_, r)| r.split_once('/')).map_or("", |(_, p)| p);
        let db = dec(path.split('?').next().unwrap_or(""));
        if db.is_empty() { "postgres".into() } else { db }
    }

    /// The URL as the user typed it: with the password back in when it was typed there (dialog).
    pub fn url_with_password(&self, pw: Option<&str>) -> String {
        match pw.filter(|p| self.url_password && !p.is_empty()) {
            Some(pw) => with_password(&self.url, &enc(pw)),
            None => self.url.clone(),
        }
    }

    /// The Password field changed: a password typed in the URL follows it.
    pub fn set_url_password(&mut self, pw: &str) {
        if inline_password(&self.url) {
            self.url = with_password(&self.url, &enc(pw));
        }
    }

    /// The URL overrides the fields: fills connection type, host, port, user, database and the
    /// options that have a field from it, like DataGrip. Several hosts only fit `URL only`.
    pub fn sync_from_url(&mut self) {
        if self.conn_type == ConnType::UrlOnly {
            return;
        }
        let url = self.url.strip_prefix("jdbc:").unwrap_or(&self.url).to_string();
        let Some((scheme, rest)) = url.split_once("://") else { return };
        let (authority, tail) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
        let (userinfo, hosts) = match authority.rfind('@') {
            Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
            None => (None, authority),
        };
        if hosts.contains(',') {
            self.conn_type = ConnType::UrlOnly;
            return;
        }
        let (host, port) = match hosts.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, p.to_string()),
            _ => (hosts, self.driver.default_port().to_string()),
        };
        self.host = host.into();
        self.port = port;
        if let Some(u) = userinfo {
            self.user = dec(u.split(':').next().unwrap_or_default());
            self.auth = Auth::UserPassword;
        }
        let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
        self.database = dec(path.trim_start_matches('/'));
        if self.driver == Driver::Mongo {
            self.conn_type = if scheme == "mongodb+srv" { ConnType::Srv } else { ConnType::Default };
            self.replica_set.clear();
            self.read_pref.clear();
        }
        for kv in query.split('&') {
            match kv.split_once('=') {
                Some(("replicaSet", v)) if self.driver == Driver::Mongo => self.replica_set = dec(v),
                Some(("readPreference", v)) if self.driver == Driver::Mongo => self.read_pref = dec(v),
                Some(("user", v)) if self.driver == Driver::Postgres => self.user = dec(v),
                _ => {}
            }
        }
    }

    /// The URL rebuilt after a field changed, keeping what only the old URL had: a password typed
    /// in it and the options without a field (`retryWrites`, `appName`, `sslmode`, …).
    pub fn regenerated_url(&self, old: &str) -> String {
        let mut url = self.generated_url();
        let rest = old.split_once("://").map_or("", |(_, r)| r);
        let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
        if let Some((_, pw)) = authority.rfind('@').and_then(|at| authority[..at].split_once(':')) {
            url = with_password(&url, pw);
        }
        let own = ["replicaSet", "readPreference"];
        let extra: Vec<&str> = old
            .split_once('?')
            .map(|(_, q)| q.split('&').filter(|kv| !kv.is_empty() && !own.contains(&kv.split('=').next().unwrap_or_default())).collect())
            .unwrap_or_default();
        if !extra.is_empty() {
            if !url.split_once("://").is_some_and(|(_, r)| r.contains('/')) {
                url.push('/'); // mongodb://host/?options
            }
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&extra.join("&"));
        }
        url
    }

    /// A password typed into the URL (`scheme://user:pass@host`) is taken out of it, so it ends up
    /// in the credentials file instead of the project file. Returns the password.
    pub fn split_password(&mut self) -> Option<String> {
        let (scheme, rest) = self.url.split_once("://")?;
        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let at = rest[..auth_end].rfind('@')?;
        let (user, pass) = rest[..at].split_once(':')?;
        let (user, pass) = (dec(user), dec(pass));
        self.url = format!("{scheme}://{}@{}", enc(&user), &rest[at + 1..]);
        self.user = user;
        self.auth = Auth::UserPassword;
        Some(pass)
    }
}

/// `scheme://user:password@…`
fn inline_password(url: &str) -> bool {
    url.split_once("://").is_some_and(|(_, rest)| {
        let auth = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
        auth.rfind('@').is_some_and(|at| auth[..at].contains(':'))
    })
}

/// `scheme://user@…` → `scheme://user:pw@…` (`pw` already percent-encoded); an existing password is
/// replaced, a URL without a user is returned as is.
fn with_password(url: &str, pw: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
    let Some(at) = authority.rfind('@') else { return url.to_string() };
    let user = authority[..at].split(':').next().unwrap_or_default();
    format!("{scheme}://{user}:{pw}@{}", &rest[at + 1..])
}

/// Percent-decodes (invalid escapes are kept as typed).
fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything outside RFC 3986 unreserved characters.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub sources: Vec<DataSource>,
    /// Legacy (pre-files) console text per data source id; migrated to console files on load.
    #[serde(skip_serializing)]
    pub consoles: HashMap<u64, String>,
    /// Current database per console file path.
    pub console_db: HashMap<String, String>,
    /// Current schema per PostgreSQL console file path.
    pub console_schema: HashMap<String, String>,
    /// Directories attached to the Files view.
    pub attached: Vec<String>,
}

/// Query consoles are plain files: `<config>/consoles/<data source id>/<name>.js` (DataGrip keeps
/// them under `consoles/db/<uuid>/`).
pub fn consoles_dir(ds: u64) -> PathBuf {
    dir().join("consoles").join(ds.to_string())
}

fn history_path(ds: u64) -> PathBuf {
    dir().join("history").join(format!("{ds}.json"))
}

/// Executed statements of a connection, oldest first (persisted like the IDE's query history).
pub fn history(ds: u64) -> Vec<String> {
    std::fs::read(history_path(ds)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_history(ds: u64, items: &[String]) {
    let p = history_path(ds);
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(b) = serde_json::to_vec_pretty(items) {
        let _ = std::fs::write(p, b);
    }
}

/// Appends a statement; a re-run moves it to the end instead of duplicating it. Keeps the last 500.
pub fn push_history(ds: u64, stmt: &str) {
    let mut h = history(ds);
    h.retain(|x| x != stmt);
    h.push(stmt.to_string());
    let extra = h.len().saturating_sub(500);
    h.drain(..extra);
    save_history(ds, &h);
}

pub fn scratches_dir() -> PathBuf {
    dir().join("scratches")
}

/// Files with the given extensions in `d`, sorted by name.
pub fn list_files(d: &std::path::Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(d)
        .map(|r| {
            r.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| exts.iter().any(|e| x == *e))).collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Every (non-hidden) file in the connection's console folder, so a console renamed without an
/// extension doesn't disappear from Database Consoles.
pub fn console_files(ds: u64) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(consoles_dir(ds))
        .map(|r| {
            r.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Creates the next free `console.js`, `console_1.js`, … (`.sql` for PostgreSQL) for a data source.
pub fn new_console(ds: u64, ext: &str, text: &str) -> PathBuf {
    new_file(&consoles_dir(ds), "console", ext, text)
}

pub fn new_file(dir: &std::path::Path, stem: &str, ext: &str, text: &str) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    let path =
        (0..).map(|i| dir.join(if i == 0 { format!("{stem}.{ext}") } else { format!("{stem}_{i}.{ext}") })).find(|p| !p.exists()).unwrap();
    let _ = std::fs::write(&path, text);
    path
}

fn dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/DataTired")
    } else {
        std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(home).join(".config")).join("DataTired")
    }
}

/// Pre-projects single config (`config.json`), migrated into the default project once.
fn load_legacy() -> Config {
    let path = dir().join("config.json");
    match std::fs::read(&path) {
        Ok(b) => {
            let mut cfg: Config = serde_json::from_slice(&b).unwrap_or_default();
            for (ds, text) in std::mem::take(&mut cfg.consoles) {
                if console_files(ds).is_empty() && !text.is_empty() {
                    new_console(ds, "js", &text);
                }
            }
            cfg
        }
        // First run: preconfigure the local docker test database.
        Err(_) => Config {
            sources: vec![{
                let mut d = DataSource {
                    id: 1,
                    name: "DataTired-mongo@localhost".into(),
                    port: "27018".into(),
                    auth: Auth::NoAuth,
                    ..Default::default()
                };
                d.url = d.generated_url();
                d
            }],
            ..Default::default()
        },
    }
}

// ------------------------------------------------------------------ projects

/// Where new projects are created by name (DataGrip: `~/DataGripProjects`).
pub fn projects_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("DataTiredProjects")
}

/// A project is a folder holding `.datatired/project.json` (DataGrip: `.idea/dataSources.xml`).
pub fn project_file(dir: &Path) -> PathBuf {
    dir.join(".datatired").join("project.json")
}

pub fn load_project(dir: &Path) -> Config {
    std::fs::read(project_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_project(dir: &Path, cfg: &Config) {
    let f = project_file(dir);
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    if let Ok(s) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(f, s);
    }
}

/// Makes `dir` a project (creating the folder) unless it already is one.
pub fn ensure_project(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if !project_file(dir).exists() {
        save_project(dir, &Config::default());
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RecentProject {
    pub path: String,
    /// Index into the 9-colour project palette (DataGrip: `RecentProjectColorInfo.associatedIndex`).
    pub color: usize,
}

/// Recent / open projects (DataGrip: `options/recentProjects.xml`).
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Registry {
    /// Most recently used first.
    pub recent: Vec<RecentProject>,
    pub open: Vec<String>,
    pub active: Option<String>,
}

impl Registry {
    pub fn color(&self, path: &str) -> usize {
        self.recent.iter().find(|r| r.path == path).map_or_else(|| color_for(path), |r| r.color)
    }

    /// Moves (or adds) a project to the front of the recent list.
    pub fn touch(&mut self, path: &str) {
        let color = self.color(path);
        self.recent.retain(|r| r.path != path);
        self.recent.insert(0, RecentProject { path: path.into(), color });
        self.recent.truncate(50);
    }
}

fn registry_path() -> PathBuf {
    dir().join("projects.json")
}

/// Loads the project registry; on first start turns the old single config into the default
/// project `~/DataTiredProjects/DataTired`.
pub fn load_registry() -> Registry {
    if let Some(r) = std::fs::read(registry_path()).ok().and_then(|b| serde_json::from_slice::<Registry>(&b).ok()) {
        return r;
    }
    let def = projects_home().join("DataTired");
    if !project_file(&def).exists() {
        save_project(&def, &load_legacy());
        let legacy = dir().join("config.json");
        if legacy.exists() {
            let _ = std::fs::rename(&legacy, dir().join("config.json.migrated"));
        }
    }
    let p = def.display().to_string();
    let mut r = Registry { open: vec![p.clone()], active: Some(p.clone()), ..Default::default() };
    r.touch(&p);
    save_registry(&r);
    r
}

pub fn save_registry(r: &Registry) {
    let _ = std::fs::create_dir_all(dir());
    if let Ok(b) = serde_json::to_vec_pretty(r) {
        let _ = std::fs::write(registry_path(), b);
    }
}

/// Palette slot for a project without a stored colour (IntelliJ derives it from the path).
fn color_for(path: &str) -> usize {
    path.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % 9
}

/// Avatar initials like IntelliJ's `AvatarUtils.initials`: first letters of the first and last
/// word when the name has several (`datatired-local` → `DL`), else its capitals (`DataTired` → `DT`),
/// else the first letter (`shop` → `S`).
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.trim().split([' ', ',', '`', '\'', '"', '-', '_', '.']).filter(|w| !w.is_empty()).collect();
    let first = |w: &str| w.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => String::new(),
        [w] => {
            let caps: String = w.chars().filter(|c| c.is_uppercase()).take(2).collect();
            if caps.chars().count() == 2 { caps } else { first(w) }
        }
        [a, .., b] => first(a) + &first(b),
    }
}

/// Data source ids are unique across projects (they key console folders and query history).
pub fn new_ds_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_micros() as u64;
    let prev = LAST.fetch_max(now, Ordering::SeqCst);
    if prev >= now { LAST.fetch_add(1, Ordering::SeqCst) + 1 } else { now }
}

/// Data source passwords, outside the project files, in a file only the user can read (like
/// `~/.pgpass`). Not the macOS keychain: an unsigned app gets a keychain password prompt after
/// every rebuild.
fn credentials_path() -> PathBuf {
    dir().join("credentials.json")
}

fn credentials() -> HashMap<u64, String> {
    std::fs::read(credentials_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn password(id: u64) -> Option<String> {
    credentials().remove(&id)
}

pub fn set_password(id: u64, pw: &str) {
    let mut all = credentials();
    if pw.is_empty() {
        all.remove(&id);
    } else {
        all.insert(id, pw.to_string());
    }
    let _ = std::fs::create_dir_all(dir());
    let Ok(bytes) = serde_json::to_vec_pretty(&all) else { return };
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    if let Ok(mut f) = o.open(credentials_path()) {
        let _ = std::io::Write::write_all(&mut f, &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_initials_like_intellij() {
        assert_eq!(initials("datatired-local"), "DL");
        assert_eq!(initials("shop"), "S");
        assert_eq!(initials("DataTired"), "DT");
        assert_eq!(initials("my project name"), "MN");
        let (a, b) = (new_ds_id(), new_ds_id());
        assert!(b > a);
    }

    #[test]
    fn numbers_new_files() {
        let d = std::env::temp_dir().join(format!("DataTired-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = new_file(&d, "console", "js", "1");
        let b = new_file(&d, "console", "js", "2");
        assert_eq!((a.file_name().unwrap(), b.file_name().unwrap()), ("console.js".as_ref(), "console_1.js".as_ref()));
        assert_eq!(list_files(&d, &["js"]), vec![a, b]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn detects_missing_password() {
        let d = DataSource { url: "mongodb+srv://app@c.example.net".into(), user: "app".into(), ..Default::default() };
        assert!(d.missing_password(None));
        assert!(d.missing_password(Some("")));
        assert!(!d.missing_password(Some("x")));
        let inline = DataSource { url: "mongodb+srv://app:x@c.example.net".into(), user: "app".into(), ..Default::default() };
        assert!(!inline.missing_password(None));
        let noauth = DataSource { auth: Auth::NoAuth, ..Default::default() };
        assert!(!noauth.missing_password(None));
    }

    #[test]
    fn moves_url_password_out() {
        let mut d =
            DataSource { url: "mongodb+srv://app_user:s3cr%40t@cluster.example.net/?retryWrites=true".into(), ..Default::default() };
        assert_eq!(d.split_password().as_deref(), Some("s3cr@t"));
        assert_eq!(d.url, "mongodb+srv://app_user@cluster.example.net/?retryWrites=true");
        assert_eq!(d.user, "app_user");
        assert_eq!(d.uri(Some("s3cr@t")), "mongodb+srv://app_user:s3cr%40t@cluster.example.net/?retryWrites=true");
        let mut plain = DataSource { url: "mongodb://host:1".into(), ..Default::default() };
        assert_eq!(plain.split_password(), None);
    }

    #[test]
    fn stores_passwords_user_only() {
        let _g = crate::TEST_HOME.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("DataTired-cred-{}", std::process::id()));
        // SAFETY: HOME-dependent tests are serialized by TEST_HOME.
        unsafe { std::env::set_var("HOME", &home) };
        set_password(7, "pw1");
        set_password(8, "pw2");
        set_password(7, "");
        assert_eq!(password(7), None);
        assert_eq!(password(8).as_deref(), Some("pw2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(credentials_path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn builds_urls() {
        let mut d = DataSource { host: "h".into(), port: "1".into(), user: "a b".into(), ..Default::default() };
        assert_eq!(d.generated_url(), "mongodb://a%20b@h:1");
        d.url = d.generated_url();
        assert_eq!(d.uri(Some("p@ss")), "mongodb://a%20b:p%40ss@h:1");
        d.auth = Auth::NoAuth;
        d.replica_set = "rs0".into();
        d.database = "shop".into();
        assert_eq!(d.generated_url(), "mongodb://h:1/shop?replicaSet=rs0");
        d.conn_type = ConnType::Srv;
        assert_eq!(d.generated_url(), "mongodb+srv://h/shop?replicaSet=rs0");
    }

    #[test]
    fn url_typed_with_password_round_trips() {
        let typed = "mongodb+srv://backend_dev_3:p%40ss@vernoss.example.net/shop?retryWrites=true&replicaSet=rs0&appName=X";
        let mut d = DataSource { url: typed.into(), ..Default::default() };
        let pw = d.split_password();
        d.url_password = pw.is_some();
        assert_eq!(d.url, "mongodb+srv://backend_dev_3@vernoss.example.net/shop?retryWrites=true&replicaSet=rs0&appName=X");
        assert_eq!(d.url_with_password(pw.as_deref()), typed, "the dialog shows the URL as typed");
        d.sync_from_url();
        assert!(d.conn_type == ConnType::Srv && d.host == "vernoss.example.net" && d.user == "backend_dev_3");
        assert_eq!((d.database.as_str(), d.replica_set.as_str()), ("shop", "rs0"));
        // editing a field rebuilds the URL but keeps the typed password and the other options
        let shown = d.url_with_password(pw.as_deref());
        d.database = "logs".into();
        assert_eq!(
            d.regenerated_url(&shown),
            "mongodb+srv://backend_dev_3:p%40ss@vernoss.example.net/logs?replicaSet=rs0&retryWrites=true&appName=X"
        );
        let mut many = DataSource { url: "mongodb://a:1,b:2/?replicaSet=rs".into(), ..Default::default() };
        many.sync_from_url();
        assert!(many.conn_type == ConnType::UrlOnly, "several hosts only fit URL only");
        let mut pg =
            DataSource { url: "jdbc:postgresql://db.example.com:6543/shop?sslmode=require".into(), ..DataSource::new(Driver::Postgres) };
        pg.sync_from_url();
        assert_eq!((pg.host.as_str(), pg.port.as_str(), pg.database.as_str()), ("db.example.com", "6543", "shop"));
        pg.port = "5432".into();
        assert_eq!(pg.regenerated_url(&pg.url), "jdbc:postgresql://db.example.com:5432/shop?sslmode=require");
    }

    #[test]
    fn builds_postgres_urls() {
        let mut d = DataSource::new(Driver::Postgres);
        assert_eq!(d.url, "jdbc:postgresql://localhost:5432/postgres");
        d.user = "app user".into();
        assert_eq!(d.uri(Some("p@ss")), "postgresql://app%20user:p%40ss@localhost:5432/postgres");
        assert_eq!(d.uri(None), "postgresql://app%20user@localhost:5432/postgres");
        assert!(d.missing_password(None));
        d.url = "jdbc:postgresql://db.example.com/shop?sslmode=require&password=x".into();
        assert!(!d.missing_password(None));
        assert_eq!(d.uri(Some("p")), "postgresql://app%20user:p@db.example.com/shop?sslmode=require&password=x");
        assert_eq!(d.pg_database(), "shop");
        d.auth = Auth::NoAuth;
        assert_eq!(d.uri(Some("p")), "postgresql://db.example.com/shop?sslmode=require&password=x");
    }
}
