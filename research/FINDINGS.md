# DataGrip → DataTired findings (phase 1: MongoDB)

Target: DataGrip 2026.1.3 (DB-261.24374.56), macOS arm64, theme **Islands Dark** (New UI).
All live observation done only against `datatired-mongo@localhost` (docker, `127.0.0.1:27018`),
inside an isolated DataGrip project `~/DataGripProjects/datatired-local`.

## Evidence
| What | Source | Evidence |
|---|---|---|
| Mongo console runs real mongosh (`com.mongodb.mongosh.MongoShellEvaluator`, `com.dbschema.MongoJdbcDriver`) | `jdbc-drivers/MongoDB/1.24/mongo-jdbc-standalone-1.24.jar` | jar listing (observed) |
| Theme tokens, Island arcs/gaps | `lib/intellij.platform.ide.impl.jar` → `themes/islands/ManyIslandsDark.theme.json` | REA `extract_artifact` ev_f9a0084005d4… |
| Editor colors | same jar → `themes/islands/IslandSchemeDark.xml` | same |
| UI icons (Apache-2.0, intellij-community) | `lib/intellij.platform.ide.jar` → `expui/**` | unzip |
| Fonts | JBR `lib/fonts` JetBrains Mono + Inter (OFL) | ls |
| Window AX tree + screenshot | REA `observe_native_ui` pid 673 / window 10391 | ev_186b5b25eede… |
| Screens 01–18 | `research/screens/*.png` | screencapture of window 10391 |

## Layout (points)
- Main window bg `#26282C`; islands `#191A1C`, radius 10 (Island.arc 20 = diameter), gap ~6.
- Header 40 high: project widget, VCS widget, center tool-window toggles (Database, Services, Files, ⋯), right search/settings.
- Left island **Database Explorer**: header (title + locate/expand/collapse/⋯/minimize), toolbar
  `+ | props | refresh disconnect | console table DDL jump | eye`, tree rows 24px.
- Editor island: tabs (selected tab bg `#233558` border `#2E4D89`, radius 4), empty state shortcut list.
- Bottom island **Services**: left session tree (`Database > ds > customers 822 ms`, `console 820 ms`), right tabs `Output`, `shop.customers`.
- Status bar: breadcrumbs left (`Database > ds > shop > tables > customers`), right `1:66 LF UTF-8 4 spaces`.

## Tree
`ds@localhost  5` (green leaf when connected) → db (cube) → `collections 5` (folder) → collection (table grid)
→ `fields 12` → `{} name  Type` (types: ObjectId, Boolean, ISODate, String, Int64, Int32, Double, Decimal128, Object, list)
→ `indexes 3` → `i name  (cols asc/desc) UNIQUE`. Views (`paid_orders`) and `system.views` appear as collections.
Schema picker popup on the `0 of 5` badge (All databases / per-db checkboxes, Enter applies).

## Data editor / result grid
Toolbar: refresh, history, stop | + − revert submit | DDL | search … | right: `JSON ⌄` (Data Extractors), export, import, compare, `eye` (View as: Transpose / Table / Tree / Text), settings.
Filter bar: `find {}` + `sort`. Columns alphabetical with `_id` first; header `{} name ▽ ⇅`. Row numbers `#4B5059`.
Cells: ObjectId hex; `• true` / `false`; numbers right-aligned; ISO dates `2025-02-02T10:30:00.000Z`; objects as compact JSON
(keys and strings `#6AAB73`; keys are `#C77DBB` only in the Text view); `<null>`; grid line `#323438`. Floating pager `|< < 1-100 ⌄ of 101+ > >|` / `27 rows ⌄`.
Text view = Extended JSON array (`{"$oid"}`, `{"$date"}`, `{"$numberDecimal"}`), Tree view = rows expandable to fields.

## Console
Toolbar: Run ▶ (green), history, settings, output, stop, `Playground ⌄`, right: db selector `shop ⌄`.
⌘↩ runs statement at caret (chooser popup for nested sub-expressions). Gutter ✓ after success.
Output log: `[yyyy-MM-dd HH:mm:ss] Connected`, `shop> use shop`, `completed in 44 ms`, `shop> <stmt>`,
`27 rows retrieved starting from 1 in 401 ms (execution: 50 ms, fetching: 351 ms)`.

## Data source dialog
"Data Sources and Drivers": Name, Color, Add comment; tabs General/Options/SSH/SSL/Schemas/Advanced;
Driver; Connection type `default | MongoDB Atlas (SRV protocol) | URL only`; Host/Port; Authentication (User & Password / No auth);
User; Password + Save; Database; Read preference; Replica set; URL ("Overrides settings above"); `Test Connection` → `MongoDB 7.0.x`; Cancel/Apply/OK.

## Collection context menu
New ›, Modify Table…, Copy/Paste ›, Edit Data ⌘↓, Refresh ⌘R, Rename… ⇧F6, Quick Documentation, Compare…, Navigation ›,
Find Usages, SQL Scripts ›, Object Actions ›, Tools ›, Import/Export ›, Diagrams ›, Diagnostics ›, Drop… ⌦.
DDL for Mongo collections: "No source text available" (not worth cloning).

## Not cloned in phase 1
Query history, statement chooser popup (⌘↩ on nested expressions), DDL/compare/diagrams, import,
charts, value-editor side panel, SSH/SSL and Options tabs, VCS widget, files/scratches, explain-plan view.
Cancel stops JS execution between/inside statements but not an in-flight server operation.

# Phase 2: PostgreSQL

Observed only against the local docker PostgreSQL 16 (`datatired` database seeded from
`docker/pg-seed.sql`) through the data source `datatired-pg@localhost` added to the isolated project
`~/DataGripProjects/datatired-local`.

## Evidence
| What | Source | Evidence |
|---|---|---|
| Introspection SQL (`ListDatabases`, `ListSchemas`, `RetrieveTables`, `RetrieveIndices`, `RetrieveConstraints`, …) | `plugins/DatabaseTools/lib/modules/intellij.database.dialects.postgres.jar` → `com/intellij/database/dialects/postgres/introspector/PgIntroQueries.sql` | REA `extract_artifact` ev_7bd2c793085a… |
| Extension points (introspector, `PgDvStructureExtension`, `PgSystemSchemasOption`, `postgres-drivers.xml`) | same jar → `intellij.database.dialects.postgres.xml` | same |
| Explorer tree, keys/indexes, foreign keys/checks | `research/screens/pg-02-tree.png`, `pg-03-keys-indexes.png`, `pg-04-fk-checks.png` | REA `observe_native_ui` ev_17532cb63dff…, ev_1cb2fce82435…, ev_c9326e6b334b… |
| Data editor | `research/screens/pg-05-data-editor.png` | ev_2ddd743205b8… |
| Console + result | `research/screens/pg-06-console.png` | ev_8091fbf97535…, ev_b382f86fad6b… |
| Data source dialog | `research/screens/pg-07-datasource-dialog.png` | ev_892d93a3d17d… |
| Credentials prompt (no stored password) | "Connecting to '…'…": `Please provide database credentials.` Authentication / User / Password / Save | ev_58fbce664eeb… |

## Tree
`ds@localhost  1 of 6` (non-template databases; only the URL's database by default) → `datatired  1 of 4`
(schemas; `public` by default, `pg_toast` not counted) → `public` (schema icon) → `tables 3` / `views 1` /
`sequences 2` → table → `columns 12` (`id  integer = nextval('customers…`, `name  text`, `balance  numeric(10,2)`,
`created_at  timestamp with time zone = now()`, `tags  text[]`; PK / NOT NULL / plain column icons) →
`keys 2` (`customers_pkey (id)` gold key, `customers_email_key (email)`) → `foreign keys 2`
(`orders_customer_id_fkey (customer_id) → customers (id)`) → `indexes 3` (`customers_email_key (email) UNIQUE`,
`customers_city_age ((address ->> 'city'::text) asc, age desc)`) → `checks 1` (`orders_qty_check (qty > 0)`).
Empty folders are not shown. Below the schemas: `Database Objects`, `virtual views`; below the databases `Server Objects`.

## Data editor
Same toolbar as MongoDB plus `Tx: Auto ⌄`; extractor defaults to `CSV`. Filter bar `WHERE` | `ORDER BY`.
Columns in table order, header icons per column kind. Query `SELECT t.*\nFROM public.orders t\nLIMIT 501`
(page size 500), logged as `[time] datatired.public> SELECT t.* …` + `60 rows retrieved starting from 1 in 397 ms
(execution: 15 ms, fetching: 382 ms)`. Values in PostgreSQL text form (`33.30`, `2026-01-02`,
`{"zip": "10043", "city": "Medan"}`, `{vip,newsletter}`), numbers right-aligned, booleans `• true` / `false`.
Breadcrumbs `Database > ds > datatired > public > tables > orders`.

## Console
Elephant tab icon, toolbar Run, Explain, history, settings, `Tx: Auto ⌄`, stop, `Playground ⌄`; right: schema
switcher `datatired.public ⌄`. Result tab `datatired.public.customers` for a single-table query, `Result N` otherwise.

## Data source dialog
Driver `PostgreSQL`; Connection type `default | URL only`; Host / Port `5432`; Authentication; User; Password + Save;
Database (combo); URL `jdbc:postgresql://localhost:5432/datatired` ("Overrides settings above");
Test Connection → `PostgreSQL 16.15`.

## Clone Row (⌘D)
| What | Source | Evidence |
|---|---|---|
| Action: enabled only with exactly one selected row, `AddRowAction.canAddRow`, not editing; refuses rows with truncated LOBs; submits first in "update immediately" mode | `lib/intellij.grid.impl.jar` → `com/intellij/database/run/actions/CloneRowAction.class` | REA `extract_artifact` ev_684255a41a5b… (bytecode read with `javap`) |
| Row built by `DatabaseMutatorImpl.createRow(rowToClone)`: every column = pending edit or current value; virtual / rowid columns `COMPUTED`; **primary key columns → `DbImplUtil.getDefaultNullValue` (`<default>`)**; inserted at the page end | `plugins/DatabaseTools/lib/modules/intellij.database.connectivity.jar` → `DatabaseMutatorImpl.class` | same (jar extracted from the DataGrip bundle) |
| PostgreSQL `orders`: clone of row 2 appended as row 61, `id <default>`, other values copied, caret on the same column (`61:6`), pending until submit | `research/screens/pg-08-clone-row.png` | REA `observe_native_ui` ev_8e90348cffaa… (reverted, table unchanged) |
| MongoDB `customers`: clone appended as row 101 with the **same `_id`** (DataGrip's Mongo model has no primary key) → submit would fail on the duplicate key | `research/screens/mongo-clone-row.png` | ev_549eb49a928a… (reverted) |

DataTired follows `createRow`, treating `_id` as the key for MongoDB (new ObjectId instead of a duplicate).

## Copy Using Data Extractor (PostgreSQL)
| What | Source | Evidence |
|---|---|---|
| Built-in SQL extractors: `SqlInsertsExtractorFactory` (`sql.inserts`, `.sql`) → `SqlInsertValuesExtractor`: per row `INSERT INTO <qualified table or MY_TABLE> (<selected columns>) VALUES (<literals>);` + newline; null literal `null`; rowid / auto-generated / computed columns dropped unless configured | `intellij.database.connectivity.jar` → `com/intellij/database/extractors/*` | REA `extract_artifact` ev_87c3b017b5be… |
| Extractor list (Data Extractors popup): Built-in SQL Inserts, SQL Updates, Where Clause, Skip Computed / Generated Columns; CSV: CSV, TSV, Pipe-, Semicolon-separated; Scripted: CSV, HTML, JSON, Markdown, One-row, Pretty, Python-DataFrame, SQL-Insert-Multirow, SQL-Insert-Statements, XML | `research/screens/pg-09-data-extractors.png` | `screencapture -l` of the popup window |
| Clipboard (⌘C) on `public.products` / `public.customers`: whole row → `INSERT INTO public.products (sku, name, price, stock, category, discontinued) VALUES ('CB-USB', 'USB-C Cable', 9.99, 1000, 'accessory', false);`; one statement per selected row; a cell block → only its columns; a single cell → the raw value whatever the extractor; serial `id` kept; numbers / booleans bare, timestamps / json / arrays quoted | observed in the isolated project | clipboard read after ⌘C (restored afterwards) |
| SQL Updates: `UPDATE public.customers SET <non-key columns> = … WHERE id = 107;` — Where Clause: `id = 107 AND name = 'Customer 107' AND …` per row, rows joined by `\nOR ` | same | same |
| Scripted SQL-Insert-Statements / SQL-Insert-Multirow: `NULL`, unquoted `schema.table`, multirow `INSERT INTO t (…)\nVALUES  (…),\n        (…);` | `extensions/com.intellij.database/data/extractors/*.sql.groovy` | file contents |

## SQL Generator (PostgreSQL)
| What | Source | Evidence |
|---|---|---|
| Table context menu `SQL Scripts ›`: SQL Generator… ⌥⌘G · Request and Copy Original DDL · Generate DDL to Clipboard ⌥⇧⌘G · Generate DDL to Query Console ⌥⇧⌘B · Select all rows / first N rows / Insert / Update / Delete rows · New view definition | `research/screens/pg-10-sql-scripts-menu.png` | `screencapture -l` of the submenu window |
| Tool window `SQL Generator` (`scriptGen.ui.*`): left strip SQL Generator Options / File Output Options; options Generate (8 modes), Qualify objects with schema names (Never / Always / Auto), Place constraints (Inside column / Inside table / After table), 9 checkboxes (Reformat generated code on); right strip Copy to Clipboard / Save into File / Open in Console | `research/screens/pg-11-sql-generator.png`, `DatabaseBundle.properties` | `plugins/DatabaseTools/lib/database-plugin.jar` |
| Script format: lower-case keywords; columns aligned in name / type / default / `not null` parts (each part aligned over the rows that have it); serial / identity columns without `not null`; single-column constraints under their column (8-space indent), others after the columns in the order PK, unique, FK, check; names left out when they are PostgreSQL's default (`t_pkey`, `t_c_key`, `t_c_fkey`) — check names always shown; FK target columns omitted when they are the referenced PK; then `comment on …`, `alter table … owner to …`, non-constraint indexes (`asc` written once any column is `desc`, `using` unless btree, index collation); every statement followed by a blank line | Copy to Clipboard of customers / orders / products / paid_orders and a temporary `gen_demo` table (dropped afterwards) under each option | clipboard read (restored); exact texts in `pg::tests` |
| Modes: as is = completely; RDBMS = `-- No native definition for element: t (table)` + `pg_get_indexdef` per index; Enable/Disable = `alter table t\n    enable constraint c\n;` per constraint; Truncate = `truncate table t;`; refresh / execute: empty for a table | same | same |

## Modify Table (PostgreSQL)
| What | Source | Evidence |
|---|---|---|
| Table menu `Modify Table… ⌘F6` opens the `Modify` dialog: tree (table `datatired.public [ds]` › columns N, keys, foreign keys, indexes — including the ones backing keys —, checks, virtual columns / foreign keys); toolbar Add (adds the selected kind) / Remove / Duplicate ⌘D / Move Up / Move Down / Edit On Selection / Back ⌘[ / Forward ⌘]; changed objects blue, new green; form header pill; Preview (collapsible, live, settings + open-in-new); Cancel with changes → "You're about to lose your changes" | `research/screens/pg-12-modify-table.png`, `pg-13-modify-fk.png` | `screencapture -l` of the dialog in the isolated project (always cancelled; schema checked unchanged afterwards) |
| Forms — table: Name, Comment, Persistence (PERSISTENT…) + With OIDs, Partition Expression, Partition Key, Options, Access Method, Tablespace, Owner, Grants; column: Name, Comment, Data Type, Not Null, Default Expression, Generated Always As, Identity + Min / Max / Step / Cache Size / Cycled; key: Primary, Deferrable, Defined With Period, Initially Deferred, Not Enforced, Not Validated, Columns (Column Name, Exclude operator); foreign key: Target Table, Columns (`col → target`), the same flags, On Delete / On Update (`no_action`, `restrict`, `set_null`, `set_default`, `cascade`); index: Unique, Nulls Not Distinct, Columns (Column Name, Order NONE/ASC/DESC, Collation, Operator Class), Condition, Include Columns, Access Method, Tablespace; check: Deferrable, Initially Deferred, No Inherit, Not Enforced, Not Validated, Predicate | same | same |
| Preview statements (copied with ⌘A ⌘C): `alter table t\n    drop column c;` · `rename column a to b` · `comment on column t.c is '…';` · `alter column c type T using c::T` · `set / drop not null` · `set default x` / `drop default` · `rename to t2` (later statements use the new name) · `comment on table t2 is '…';` · `add c integer default 0 not null` · `alter index a rename to b;` · `rename constraint a to b` · changed key / foreign key / check → `drop constraint` + `add constraint n\n        foreign key (c) references customers\n            on delete cascade` / `check (…)`; order: columns, table, new columns, indexes, keys, foreign keys, checks | same | clipboard read (restored) |
| New object names: `column_name integer`, `check_name`, templates `{table}_pk`, `{table}_{ref_table}_{ref_columns}_fk`, `{table}_{columns}_{unique?u:}index` (e.g. `customers__index`, `customers___fk`) | `modules/intellij.database.sql.jar` strings | `unzip -p … \| grep` |
| DataGrip quirks not copied: a phantom `create sequence "";` after adding a column / index; a dropped foreign key's statement vanishing once an index is added | same | observed |
