# DataTired

A DataGrip-style database IDE written in Rust (egui/eframe — native, no Electron, no webview).
Supports **MongoDB** (phase 1) and **PostgreSQL** (phase 2).

## Install (macOS, Apple Silicon)

```sh
curl -fsSL https://raw.githubusercontent.com/kholidss/datatired/main/install.sh | sh
```

The script checks that you are on macOS/arm64, downloads the latest release from GitHub,
verifies its SHA-256 checksum and puts `DataTired.app` in `/Applications`
(or `~/Applications` when `/Applications` is not writable). Run it again to update.

- **Specific version:** `curl -fsSL …/install.sh | DATATIRED_VERSION=v0.1.0 sh`
- **Uninstall:** `rm -rf /Applications/DataTired.app`

Release (maintainers): `scripts/bundle.sh` builds `DataTired.app` and `dist/DataTired-<version>-macos-arm64.zip`
(+ `.sha256`); upload both to a GitHub release tagged `v<version>`.

## Features (phase 1)

- **Database Explorer** — data sources → databases (`x of N` picker) → collections/views →
  sampled fields with BSON types → indexes (`(cols asc/desc) UNIQUE`). Keyboard: ↑↓, → expand,
  ← collapse, ↩ / ⌘↓ open data. Context menus: new/rename/drop collection, drop database, console.
- **Data editor** — paged grid (`1-100 ⌄ of 101+`), `find {}` filter + `sort`, header sort,
  inline editing with pending changes (edit / ⌘N add / ⌘⌫ delete / revert / ⌘↩ submit),
  copy with data extractors (CSV, TSV, JSON, Markdown, …), export to `~/Downloads`.
- **Query console** — mongosh syntax via an embedded JS engine (QuickJS) with a mongosh-compatible
  shim (`db.coll.find().sort()`, `aggregate`, CRUD, indexes, `runCommand`, `use`, `show dbs`,
  `ObjectId`, `ISODate`, `NumberLong`, …). ⌘↩ runs the statement at the caret (Playground) or the
  whole console (Script); syntax highlighting, autocomplete (collections, methods, fields, operators;
  ⌃Space), gutter status, database selector.
- **Value editor panel** — Record / Value tabs beside the grid (⇧⏎); arrays as an item list with + − ↶,
  other values as editable text; pending changes preview.
- **Files** — attached directories, *Scratches and Consoles › Database Consoles › data source › console*
  (consoles are `.js` files under `~/Library/Application Support/DataTired/consoles/`), scratch files,
  rename / delete / reveal in Finder. ⌘2 toggles it; ⌘⇧L creates a new console, the explorer's
  console button jumps to an existing one.
- **Projects** — like DataGrip's project widget: *New Project…* (name → `~/DataTiredProjects/<name>`,
  or a path), *Open…* (any folder), *Open Projects* / *Recent Projects* with coloured initials badges,
  *Close Project*. Each project keeps its own data sources (`<project>/.datatired/project.json`), editor
  tabs and connections; several open projects show as tabs under the toolbar (`project – tab`).
- **Services** — sessions with timings, Output log (`[time] db> stmt`, `N rows retrieved …`),
  result tabs with Table / Tree / Text / Transpose views.
- **Data Sources and Drivers** dialog — host/port, SRV, URL only, user & password (stored in the
  `~/Library/Application Support/DataTired/credentials.json`, readable only by you), database, read preference, replica set, Test Connection.
- Islands Dark look: theme tokens, New UI icons, JetBrains Mono/Inter fonts.

## PostgreSQL (phase 2)

- **Data source** — driver *PostgreSQL* in the dialog (and *+ › Data Source › PostgreSQL*): host/port `5432`,
  user & password, database, URL `jdbc:postgresql://host:5432/db` (overrides the fields; libpq options such as
  `?sslmode=require` work). TLS is tried first (sslmode `prefer`) without certificate verification, like libpq.
- **Explorer** — `ds` → databases (`x of N` picker) → schemas → `tables` / `views` / `materialized views` /
  `sequences` → table → `columns` (type `= default`, PK / NOT NULL icons), `keys`, `foreign keys`
  (`(col) → table (col)`), `indexes` (`(cols) UNIQUE`), `checks`. One connection per database, opened on demand.
- **Data editor** — `SELECT t.* FROM schema.table t` with `WHERE` / `ORDER BY` bar, 500-row pages, values in
  PostgreSQL text form, numbers right-aligned, header sort. Tables with a primary key are editable
  (edit / add / delete / Set NULL → `UPDATE` / `INSERT` / `DELETE` in one transaction, previewed as SQL).
- **SQL console** — `.sql` consoles (Files › Database Consoles), `database.schema` switcher (search path),
  statement at caret / whole script, `$$` / quotes / comments aware splitting, SQL highlighting and completion
  (key words, tables, columns, aliases), notices, `N rows affected`, errors as `[SQLSTATE] ERROR: …`,
  ⌘F2 sends a cancel request. Single-table query results are editable too.

## Run

```sh
docker compose -f docker/compose.yml up -d   # local MongoDB 7 on 127.0.0.1:27018 with seed data
# PostgreSQL test data (any local PostgreSQL ≥ 10 on 127.0.0.1:5432, user postgres / password `password`):
psql -h 127.0.0.1 -U postgres -c 'create database datatired' && psql -h 127.0.0.1 -U postgres -d datatired -f docker/pg-seed.sql
cargo run --release                           # or build the macOS app:
./scripts/bundle.sh && open target/release/bundle/DataTired.app
```

The app icon is `assets/icon.svg`; `scripts/bundle.sh` renders it (`cargo run --example icon`) into
`assets/DataTired.icns` and `assets/icon.png` (window/Dock icon).

On first start DataTired creates the project `~/DataTiredProjects/DataTired` with the data source
`DataTired-mongo@localhost` (127.0.0.1:27018). The project list lives in
`~/Library/Application Support/DataTired/projects.json`; consoles, history and scratches are shared.

## Tests

```sh
cargo test --release
```

Unit tests cover the statement splitter, URL building, formatting, highlighting and completion.
The mongo, PostgreSQL (`datatired` database; the worker test honours `DATATIRED_PG=postgresql://…`) and UI tests run
against the local databases (skipped when they are not running);
the UI tests render the app offscreen (egui_kittest + wgpu) and write screenshots to
`target/ui-shots/`.

## Layout

| File | Role |
|---|---|
| `src/mongo.rs` | connection worker thread: driver, QuickJS shell, paging, introspection |
| `src/shell.js` | mongosh-compatible shim evaluated inside QuickJS |
| `src/pg.rs` | PostgreSQL worker: per-database clients, introspection, paging, edits, SQL splitting/quoting |
| `src/app.rs` | window layout, explorer, tabs, services, dialogs, request routing |
| `src/grid.rs` | result grid + data editor views |
| `src/editor.rs` | code editor, highlighting, completion |
| `src/dialog.rs` | data source dialog |
| `src/config.rs` | projects, data sources, user-only credentials file |
| `research/FINDINGS.md` | what was observed in DataGrip (REA + screenshots) |
