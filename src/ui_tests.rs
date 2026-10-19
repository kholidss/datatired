//! Headless UI flow (offscreen wgpu render) against the docker mongo on 127.0.0.1:27018.
//! Screenshots land in `target/ui-shots/`. Skipped when the local mongo is not running.

use crate::TEST_HOME as SERIAL;
use crate::projects::App;
use egui::{Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mongodb::bson::{Document, doc};
use std::path::PathBuf;
use std::time::Duration;

fn wait(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&Harness<'_, App>) -> bool) {
    for _ in 0..400 {
        h.step();
        if cond(h) {
            h.run_steps(4);
            return;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots/timeout.png");
    let _ = h.render().map(|i| i.save(out));
    panic!("timed out waiting for {what}");
}

fn type_chars(h: &mut Harness<'_, App>, text: &str) {
    for c in text.chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.step();
    }
}

fn settle(h: &mut Harness<'_, App>) {
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn click_at(h: &mut Harness<'_, App>, pos: Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
}

/// The connection is listed both in the Database Explorer and under Files › Database Consoles;
/// the explorer one is the leftmost.
fn explorer_node<'a>(h: &'a Harness<'_, App>, label: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by_label(label).min_by(|a, b| a.rect().left().total_cmp(&b.rect().left())).expect(label)
}

/// Project tab (topmost node with that label; the project folder is also a Files root).
fn project_tab<'a>(h: &'a Harness<'_, App>, label: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by_label(label).min_by(|a, b| a.rect().top().total_cmp(&b.rect().top())).expect(label)
}

fn mongo_up() -> bool {
    std::net::TcpStream::connect_timeout(&"127.0.0.1:27018".parse().unwrap(), Duration::from_millis(300)).is_ok()
}

/// App with a throwaway HOME so the user's config is never touched (first run = local docker source).
fn harness() -> Harness<'static, App> {
    harness_with(|| {}, 0.25)
}

/// Like `harness`, with `prepare` run in the fresh HOME before the app starts (e.g. writing a project);
/// `step_dt` = simulated seconds per step (kittest's default 0.25 is too slow for double clicks).
fn harness_with(prepare: impl FnOnce(), step_dt: f32) -> Harness<'static, App> {
    let home = std::env::temp_dir().join(format!("DataTired-ui-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let _ = std::fs::remove_dir_all(home.join("Library/Application Support/DataTired"));
    let _ = std::fs::remove_dir_all(home.join("DataTiredProjects"));
    // SAFETY: tests touching HOME are serialized by SERIAL; set before the app reads its config.
    unsafe { std::env::set_var("HOME", &home) };
    prepare();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(2.0)
        .with_step_dt(step_dt)
        .build_eframe(|cc| App::new(cc));
    settle(&mut h);
    h
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
    std::fs::create_dir_all(&out).unwrap();
    h.render().unwrap().save(out.join(name)).unwrap();
}

#[test]
fn explorer_data_editor_console_flow() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !mongo_up() {
        return eprintln!("skipped: no local mongo on 27018");
    }
    let mut h = harness();
    shot(&mut h, "01-start.png");
    // context menu on the connection under Files › Database Consoles: content-wide, at the pointer
    let files_ds = h.query_all_by_label("DataTired-mongo@localhost").max_by(|a, b| a.rect().left().total_cmp(&b.rect().left())).unwrap();
    let at = files_ds.rect().left_center() + egui::vec2(40.0, 0.0);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
    shot(&mut h, "01b-context-menu.png");
    let item = h.get_by_label("New Query Console").rect();
    assert!(item.width() < 320.0, "menu sized to content, got {}", item.width());
    assert!((item.left() - at.x).abs() < 24.0, "menu opens at the pointer: item.left {} pointer {}", item.left(), at.x);
    h.key_press(Key::Escape);
    settle(&mut h);
    // near the window's right edge: shifted just enough to fit, never flipped to the pointer's left
    let at = egui::pos2(1440.0 - 70.0, at.y);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
    let item = h.get_by_label("New Query Console").rect();
    assert!(item.right() <= 1440.0 && item.left() > at.x - item.width() + 40.0, "shifted, not flipped: {item:?} pointer {}", at.x);
    h.key_press(Key::Escape);
    settle(&mut h);

    // connect + expand via keyboard, like DataGrip's tree
    explorer_node(&h, "DataTired-mongo@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("shop").is_some());
    h.get_by_label("shop").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "collections folder", |h| h.query_by_label("collections").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "customers", |h| h.query_by_label("customers").is_some());
    h.get_by_label("customers").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "fields", |h| h.query_by_label("fields").is_some());
    h.get_by_label("fields").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "field rows", |h| h.query_by_label("balance").is_some());
    h.get_by_label("indexes").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "index rows", |h| h.query_by_label("email_1").is_some());
    shot(&mut h, "02-tree.png");

    // open the data editor
    h.get_by_label("customers").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "data editor", |h| h.query_by_label("Add Row").is_some());
    shot(&mut h, "03a-querying.png");
    settle(&mut h);
    settle(&mut h);
    shot(&mut h, "03-data-editor.png");
    // value panel on an array cell (row 1 → `tags`, the 10th column)
    click_at(&mut h, Pos2::new(560.0, 186.0));
    for _ in 0..9 {
        h.key_press(Key::ArrowRight);
        h.step();
    }
    assert!(h.query_by_label("Record").is_none(), "value panel is hidden by default");
    h.get_by_label("Value Editor").click();
    settle(&mut h);
    h.get_by_label("Value").click();
    settle(&mut h);
    shot(&mut h, "03b-value-panel.png");
    h.get_by_label("Value Editor").click();
    settle(&mut h);

    // speed search in the explorer: ⌘F, type, highlighted match selected, Enter opens it
    explorer_node(&h, "customers").click();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::F);
    type_chars(&mut h, "orde");
    settle(&mut h);
    shot(&mut h, "03c-speed-search.png");
    h.key_press(Key::Enter);
    wait(&mut h, "orders data editor", |h| h.query_by_label("orders [DataTired-mongo@localhost]").is_some());
    // typing straight into the tree opens it too; no match shows red and keeps the selection
    explorer_node(&h, "orders").click();
    h.step();
    type_chars(&mut h, "zzq");
    settle(&mut h);
    shot(&mut h, "03d-speed-search-nomatch.png");
    h.key_press(Key::Escape);
    settle(&mut h);

    // console: ⌘⇧L, type, ⌘↩
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::L);
    settle(&mut h);
    h.event(egui::Event::Text("db.customers.find({tier: \"gold\", age: {$gt: 30}}).sort({age: -1})".into()));
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    wait(&mut h, "console result tab", |h| h.query_by_label("shop.customers").is_some());
    settle(&mut h);
    shot(&mut h, "04-console.png");
    // the divider between the Services sessions tree and the results is draggable
    let out0 = h.get_by_label("Output").rect();
    let grip = egui::pos2(out0.left() - 9.0, out0.bottom() + 120.0);
    h.event(egui::Event::PointerMoved(grip));
    h.step();
    h.drag_at(grip);
    h.step();
    h.event(egui::Event::PointerMoved(grip + egui::vec2(-80.0, 0.0)));
    h.step();
    h.drop_at(grip + egui::vec2(-80.0, 0.0));
    settle(&mut h);
    let out1 = h.get_by_label("Output").rect();
    assert!((out1.left() - (out0.left() - 80.0)).abs() < 6.0, "results widened to the left: {} -> {}", out0.left(), out1.left());
    shot(&mut h, "04f-services-narrow-tree.png");
    // Files › Database Consoles lists the connection with its new console under it
    assert!(h.query_all_by_label("DataTired-mongo@localhost").count() >= 2, "explorer + Database Consoles (+ Services)");
    assert!(h.query_all_by_label("console [DataTired-mongo@localhost]").count() >= 2, "console tab + file under its connection");

    // Query History: executed statement is listed, persisted, and Paste inserts it into the console
    let q = "db.customers.find({tier: \"gold\", age: {$gt: 30}}).sort({age: -1})";
    assert_eq!(crate::config::history(1).last().map(String::as_str), Some(q), "persisted per connection");
    h.get_by_label("Browse Query History").click();
    settle(&mut h);
    assert!(h.query_by_label(q).is_some(), "listed in the dialog");
    shot(&mut h, "04d-query-history.png");
    h.get_by_label("Paste").click();
    settle(&mut h);
    assert!(h.query_by_label("Paste").is_none(), "dialog closed after paste");
    shot(&mut h, "04e-history-pasted.png");

    // completion popup in the console
    h.key_press(Key::Enter);
    type_chars(&mut h, "db.cus");
    settle(&mut h);
    shot(&mut h, "04b-completion.png");
    h.key_press(Key::Enter);
    settle(&mut h);
    type_chars(&mut h, ".fin");
    settle(&mut h);
    shot(&mut h, "04c-completion-methods.png");
    h.key_press(Key::Escape);
    settle(&mut h);

    // result views: Tree and Text
    let eyes: Vec<_> = h.query_all_by_label("View as").collect();
    eyes.last().unwrap().click();
    settle(&mut h);
    h.get_by_label("Tree").click();
    settle(&mut h);
    shot(&mut h, "05-tree-view.png");
    let eyes: Vec<_> = h.query_all_by_label("View as").collect();
    eyes.last().unwrap().click();
    settle(&mut h);
    h.get_by_label("Text").click();
    settle(&mut h);
    shot(&mut h, "06-text-view.png");

    // Output tab
    h.get_by_label("Output").click();
    settle(&mut h);
    shot(&mut h, "07-output.png");

    // data sources dialog
    h.key_press_modifiers(Modifiers::COMMAND, Key::Semicolon);
    settle(&mut h);
    shot(&mut h, "08-dialog.png");
}

#[test]
fn grid_edit_add_delete_submit() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !mongo_up() {
        return eprintln!("skipped: no local mongo on 27018");
    }
    let client = mongodb::sync::Client::with_uri_str("mongodb://127.0.0.1:27018").unwrap();
    let coll = client.database("DataTired_ui").collection::<Document>("items");
    let _ = coll.drop().run();
    coll.insert_many([doc! {"_id": 1, "a": 1, "s": "x"}, doc! {"_id": 2, "a": 2, "s": "y"}, doc! {"_id": 3, "a": 3, "s": "z"}])
        .run()
        .unwrap();

    let mut h = harness();
    explorer_node(&h, "DataTired-mongo@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("DataTired_ui").is_some());
    h.get_by_label("DataTired_ui").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "collections", |h| h.query_by_label("collections").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "items", |h| h.query_by_label("items").is_some());
    h.get_by_label("items").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "data editor", |h| h.query_by_label("Add Row").is_some());
    settle(&mut h);

    // row 1, column `_id` -> → `a`, edit to 42
    click_at(&mut h, Pos2::new(470.0, 186.0));
    h.key_press(Key::ArrowRight);
    h.step();
    h.key_press(Key::Enter);
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    type_chars(&mut h, "42");
    h.key_press(Key::Enter);
    settle(&mut h);
    shot(&mut h, "10-grid-pending-edit.png");
    // add a row: ⌘N edits the first non-_id column of the new row
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    settle(&mut h);
    type_chars(&mut h, "7");
    h.key_press(Key::Enter);
    settle(&mut h);
    // delete row 3
    click_at(&mut h, Pos2::new(470.0, 240.0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backspace);
    settle(&mut h);
    // edit row 2 `s` through the Record tab of the value panel
    click_at(&mut h, Pos2::new(470.0, 213.0));
    h.get_by_label("Value Editor").click();
    settle(&mut h);
    h.get_by_label("Record").click();
    settle(&mut h);
    h.get_by_label("s").click();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    type_chars(&mut h, "yy");
    h.key_press(Key::Enter);
    settle(&mut h);
    shot(&mut h, "11-grid-pending-all.png");
    // reload with unsubmitted changes asks first; "No" keeps them
    h.key_press_modifiers(Modifiers::COMMAND, Key::R);
    settle(&mut h);
    shot(&mut h, "11b-confirm-unsubmitted.png");
    h.get_by_label("No").click();
    settle(&mut h);
    assert!(h.query_by_label("No").is_none(), "dialog closed");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    for _ in 0..100 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
        if coll.count_documents(doc! {}).run().unwrap() == 3
            && coll.find_one(doc! {"_id": 1}).run().unwrap().unwrap().get_i32("a") == Ok(42)
            && coll.find_one(doc! {"_id": 2}).run().unwrap().unwrap().get_str("s") == Ok("yy")
        {
            break;
        }
    }
    settle(&mut h);
    shot(&mut h, "12-grid-submitted.png");
    // Clone Row (⌘D) on row 1 (`_id` 1: a 42, s "x") + submit: same values, a new ObjectId
    click_at(&mut h, Pos2::new(470.0, 186.0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    settle(&mut h);
    shot(&mut h, "12b-grid-clone-row.png");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    for _ in 0..100 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
        if coll.count_documents(doc! {}).run().unwrap() == 4 {
            break;
        }
    }
    let first = coll.find(doc! {}).run().unwrap().next().unwrap().unwrap();
    let clones: Vec<Document> =
        coll.find(doc! {"_id": {"$type": "objectId"}, "a": first.get("a").unwrap().clone()}).run().unwrap().map(Result::unwrap).collect();
    assert!(clones.iter().any(|d| d.get("s") == first.get("s")), "clone of {first:?} with a new _id, got {clones:?}");
    coll.delete_many(doc! {"_id": {"$in": clones.iter().map(|d| d.get("_id").unwrap().clone()).collect::<Vec<_>>()}}).run().unwrap();
    let docs: Vec<Document> = coll.find(doc! {}).sort(doc! {"a": 1}).run().unwrap().map(Result::unwrap).collect();
    let _ = client.database("DataTired_ui").drop().run();
    assert_eq!(docs.len(), 3, "{docs:?}");
    assert_eq!(docs.iter().find(|d| d.get_i32("_id") == Ok(1)).unwrap().get_i32("a"), Ok(42));
    assert!(docs.iter().all(|d| d.get_i32("_id") != Ok(3)), "row 3 deleted: {docs:?}");
    assert_eq!(docs.iter().find(|d| d.get_i32("_id") == Ok(2)).unwrap().get_str("s"), Ok("yy"), "record tab edit");
    assert!(
        docs.iter().any(|d| d.get_i32("a") == Ok(7) && d.get("_id").is_some_and(|i| matches!(i, mongodb::bson::Bson::ObjectId(_)))),
        "{docs:?}"
    );
}

#[test]
fn multi_project_widget() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = harness();
    // single project: no project tab strip
    assert!(h.query_by_label("DataTired – ").is_none());
    h.get_by_label("Project").click();
    settle(&mut h);
    shot(&mut h, "20-project-widget.png");
    let w = h.get_by_label("Data Sources and Drivers…").rect().width();
    assert!(w < 330.0, "project popup sized to content, got {w}");
    h.get_by_label("New Project…").click();
    settle(&mut h);
    type_chars(&mut h, "datatired-local");
    settle(&mut h);
    shot(&mut h, "21-new-project.png");
    h.key_press(Key::Enter);
    settle(&mut h);
    let dir = crate::config::projects_home().join("datatired-local");
    assert!(crate::config::project_file(&dir).exists(), "project created on disk");
    // two open projects → project tabs; the new one is active and empty
    assert!(project_tab(&h, "datatired-local").rect().top() < 90.0, "tab of the new project");
    assert!(h.query_all_by_label("DataTired-mongo@localhost").count() == 0, "new project has no data sources");
    h.get_by_label("Project").click();
    settle(&mut h);
    shot(&mut h, "22-open-projects.png");
    h.key_press(Key::Escape);
    settle(&mut h);
    // switch back through the project tab
    project_tab(&h, "DataTired").click();
    settle(&mut h);
    assert!(h.query_all_by_label("DataTired-mongo@localhost").count() >= 1, "back in the default project");
    shot(&mut h, "23-project-tabs.png");
    // close the second project from the widget: it moves to Recent Projects
    project_tab(&h, "datatired-local").click();
    settle(&mut h);
    h.get_by_label("Project").click();
    settle(&mut h);
    h.get_by_label("Close Project").click();
    settle(&mut h);
    assert!(h.query_all_by_label("datatired-local").all(|n| n.rect().top() > 90.0), "tab strip gone with one project");
    let reg = crate::config::load_registry();
    assert_eq!(reg.open.len(), 1);
    assert!(reg.recent.iter().any(|r| r.path.ends_with("datatired-local")), "kept as recent");
}

/// App whose default project has only `datatired-pg@localhost` (password in the credentials file).
fn pg_harness() -> Harness<'static, App> {
    harness_with(
        || {
            use crate::config::{Auth, Config, DataSource, Driver};
            let mut ds = DataSource {
                id: 2,
                name: "datatired-pg@localhost".into(),
                host: "127.0.0.1".into(),
                user: "postgres".into(),
                auth: Auth::UserPassword,
                database: "datatired".into(),
                ..DataSource::new(Driver::Postgres)
            };
            ds.url = ds.generated_url();
            crate::config::save_project(
                &crate::config::projects_home().join("DataTired"),
                &Config { sources: vec![ds], ..Default::default() },
            );
            crate::config::set_password(2, "password");
        },
        0.25,
    )
}

/// Text the app put on the clipboard in the last frame.
fn copied(h: &Harness<'_, App>) -> Option<String> {
    h.output().platform_output.commands.iter().find_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t.clone()),
        _ => None,
    })
}

fn right_click(h: &mut Harness<'_, App>, pos: Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    settle(h);
}

/// Local PostgreSQL with the seeded `datatired` database (`docker/pg-seed.sql`) on 127.0.0.1:5432.
fn pg_client() -> Option<postgres::Client> {
    postgres::Client::connect("postgresql://postgres:password@127.0.0.1:5432/datatired?connect_timeout=2", postgres::NoTls).ok()
}

#[test]
fn postgres_explorer_data_editor_console_flow() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut pg) = pg_client() else {
        return eprintln!("skipped: no local postgres with the datatired database");
    };
    pg.batch_execute(
        "drop schema if exists ui_t cascade; create schema ui_t;
         create table ui_t.items (id serial primary key, a int, s text);
         insert into ui_t.items (a, s) values (1, 'x'), (2, 'y'), (3, 'z');",
    )
    .unwrap();
    let mut h = pg_harness();

    // explorer: data source → database → schema → tables → table → columns / keys / indexes
    explorer_node(&h, "datatired-pg@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("datatired").is_some());
    h.get_by_label("datatired").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "schemas", |h| h.query_by_label("public").is_some() && h.query_by_label("ui_t").is_some());
    h.get_by_label("public").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "tables folder", |h| h.query_by_label("tables").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "customers", |h| h.query_by_label("customers").is_some());
    assert!(h.query_by_label("views").is_some() && h.query_by_label("sequences").is_some(), "views / sequences folders");
    h.get_by_label("customers").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "columns", |h| h.query_by_label("columns").is_some());
    for (folder, child) in [("columns", "balance"), ("keys", "customers_email_key"), ("indexes", "customers_city_age")] {
        h.get_by_label(folder).click();
        h.step();
        h.key_press(Key::ArrowRight);
        wait(&mut h, child, |h| h.query_by_label(child).is_some());
    }
    shot(&mut h, "pg-02-tree.png");

    // data editor of customers: SQL rows in table order
    h.get_by_label("customers").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "data editor", |h| h.query_by_label("customers [datatired-pg@localhost]").is_some());
    settle(&mut h);
    settle(&mut h);
    shot(&mut h, "pg-03-data-editor.png");

    // edit / add / delete rows of ui_t.items through the grid, then submit
    h.get_by_label("public").click();
    h.step();
    h.key_press(Key::ArrowLeft);
    settle(&mut h);
    h.get_by_label("ui_t").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "ui_t tables", |h| h.query_by_label("tables").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "items", |h| h.query_by_label("items").is_some());
    h.get_by_label("items").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "items editor", |h| h.query_by_label("items [datatired-pg@localhost]").is_some());
    settle(&mut h);
    click_at(&mut h, Pos2::new(470.0, 186.0)); // row 1, `id`
    h.key_press(Key::ArrowRight);
    h.step();
    h.key_press(Key::Enter);
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    type_chars(&mut h, "42");
    h.key_press(Key::Enter);
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::N); // edits `a` of the new row (id has a default)
    settle(&mut h);
    type_chars(&mut h, "7");
    h.key_press(Key::Enter);
    settle(&mut h);
    click_at(&mut h, Pos2::new(470.0, 240.0)); // row 3
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backspace);
    settle(&mut h);
    shot(&mut h, "pg-04-pending.png");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    let rows = |pg: &mut postgres::Client| -> Vec<(i32, Option<i32>)> {
        pg.query("select id, a from ui_t.items order by id", &[]).unwrap().iter().map(|r| (r.get(0), r.get(1))).collect()
    };
    for _ in 0..200 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
        if rows(&mut pg).len() == 3 && rows(&mut pg)[0] == (1, Some(42)) {
            break;
        }
    }
    settle(&mut h);
    shot(&mut h, "pg-05-submitted.png");
    assert_eq!(rows(&mut pg), [(1, Some(42)), (2, Some(2)), (4, Some(7))]);
    // Clone Row (⌘D) on the first row, then submit: a new id, the other values copied
    click_at(&mut h, Pos2::new(560.0, 186.0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    settle(&mut h);
    shot(&mut h, "pg-05b-clone-row.png");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    let all = |pg: &mut postgres::Client| -> Vec<(i32, Option<i32>, Option<String>)> {
        pg.query("select id, a, s from ui_t.items order by id", &[]).unwrap().iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect()
    };
    for _ in 0..200 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
        if all(&mut pg).len() == 4 {
            break;
        }
    }
    let items = all(&mut pg);
    let clone = items.last().unwrap().clone();
    assert!(clone.0 > 4 && items[..3].iter().any(|r| (r.1, &r.2) == (clone.1, &clone.2)), "cloned with a new id: {items:?}");

    // console: ⌘⇧L on the database → `.sql` console on datatired.public, ⌘↩ runs the statement
    explorer_node(&h, "datatired").click();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::L);
    settle(&mut h);
    h.event(egui::Event::Text("select id, name, balance, address, tags from customers where age > 60 order by id;".into()));
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    wait(&mut h, "console result", |h| h.query_by_label("datatired.public.customers").is_some());
    settle(&mut h);
    shot(&mut h, "pg-06-console.png");
    assert!(h.query_all_by_label("console [datatired-pg@localhost]").count() >= 2, "console tab + file under its connection");
    let console = crate::config::console_files(2);
    assert!(console.iter().any(|p| p.extension().is_some_and(|e| e == "sql")), "{console:?}");
    h.get_by_label("Output").click();
    settle(&mut h);
    shot(&mut h, "pg-07-output.png");

    h.key_press_modifiers(Modifiers::COMMAND, Key::Semicolon);
    settle(&mut h);
    shot(&mut h, "pg-08-dialog.png");
    pg.batch_execute("drop schema ui_t cascade").unwrap();
}

#[test]
fn new_menu_like_datagrip() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = harness();
    h.get_by_label("New").click();
    settle(&mut h);
    for item in ["Query Console", "Query File…", "Data Source from URL", "Import from Clipboard", "Driver"] {
        assert!(h.query_by_label(item).is_some(), "{item}");
    }
    // hovering `Data Source ›` opens the driver list
    let row = h.get_by_label("Data Source").rect();
    h.event(egui::Event::PointerMoved(row.center()));
    settle(&mut h);
    shot(&mut h, "30-new-menu.png");
    h.get_by_label("PostgreSQL").click();
    settle(&mut h);
    assert!(h.query_by_label("Test Connection").is_some(), "Data Sources dialog with a new PostgreSQL source");
    shot(&mut h, "31-new-postgres-source.png");
}

/// Press + release in one frame (one step).
fn tap(h: &mut Harness<'_, App>, pos: Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.step();
}

#[test]
fn header_double_click_needs_both_clicks_on_empty_header() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = harness_with(|| {}, 1.0 / 60.0);
    let zoomed = |h: &Harness<'_, App>| {
        h.output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Maximized(_) | egui::ViewportCommand::StartDrag)))
    };
    let pause = |h: &mut Harness<'_, App>| h.run_steps(60); // > egui's triple-click window
    // fast clicks on a tool window toggle that slip just off it (gap / strip above): no window zoom
    let svc = h.get_by_label("Services").rect();
    for off in [egui::pos2(svc.right() + 3.0, svc.center().y), egui::pos2(svc.center().x, svc.top() - 3.0)] {
        pause(&mut h);
        let edge = egui::pos2(off.x.clamp(svc.left() + 1.0, svc.right() - 1.0), off.y.clamp(svc.top() + 1.0, svc.bottom() - 1.0));
        tap(&mut h, edge);
        assert!(!zoomed(&h));
        tap(&mut h, off);
        assert!(!zoomed(&h), "button click + click just beside it ({off:?}) is not a title bar double click");
    }
    // pressing a button and drifting off it doesn't drag the window either
    pause(&mut h);
    let c = svc.center();
    h.event(egui::Event::PointerMoved(c));
    h.event(egui::Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerMoved(c + egui::vec2(0.0, 12.0)));
    h.step();
    assert!(!zoomed(&h), "no window drag from a toolbar button");
    h.event(egui::Event::PointerButton {
        pos: c + egui::vec2(0.0, 12.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.step();
    // a real double click on the empty header still zooms, like a title bar
    pause(&mut h);
    let empty = egui::pos2(1000.0, 20.0);
    tap(&mut h, empty);
    tap(&mut h, empty);
    assert!(zoomed(&h), "double click on the empty header zooms the window");
}

#[test]
fn data_source_context_menu_like_datagrip() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = harness();
    let right_click = |h: &mut Harness<'_, App>| {
        let at = explorer_node(h, "DataTired-mongo@localhost").rect().left_center() + egui::vec2(60.0, 0.0);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
        settle(h);
    };
    right_click(&mut h);
    for item in [
        "Properties",
        "New",
        "Copy/Paste",
        "Refresh",
        "Deactivate",
        "Rename…",
        "Move",
        "Quick Documentation",
        "Navigation",
        "DDL Mapping",
        "Diagnostics",
        "Remove Data Source…",
    ] {
        assert!(h.query_all_by_label(item).next().is_some(), "{item}");
    }
    // the menu row (the explorer's `+` button is also labelled New)
    let new = h.query_all_by_label("New").map(|n| n.rect()).max_by(|a, b| a.width().total_cmp(&b.width())).unwrap();
    h.event(egui::Event::PointerMoved(new.center()));
    settle(&mut h);
    assert!(h.query_by_label("Virtual View").is_some() && h.query_by_label("Query Console").is_some(), "New › submenu");
    shot(&mut h, "32-data-source-menu.png");
    h.key_press(Key::Escape);
    settle(&mut h);

    // Rename… renames the data source
    right_click(&mut h);
    h.get_by_label("Rename…").click();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    type_chars(&mut h, "renamed-mongo");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.query_all_by_label("renamed-mongo").next().is_some(), "renamed in the explorer");

    // Remove Data Source… asks first; Cancel keeps it
    let at = explorer_node(&h, "renamed-mongo").rect().left_center() + egui::vec2(60.0, 0.0);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    settle(&mut h);
    h.get_by_label("Remove Data Source…").click();
    settle(&mut h);
    shot(&mut h, "33-remove-data-source.png");
    h.get_by_label("Cancel").click();
    settle(&mut h);
    assert!(h.query_all_by_label("renamed-mongo").next().is_some(), "kept after Cancel");
}

#[test]
fn clone_row_submit_reports_write_errors() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !mongo_up() {
        return eprintln!("skipped: no local mongo on 27018");
    }
    let client = mongodb::sync::Client::with_uri_str("mongodb://127.0.0.1:27018").unwrap();
    let coll = client.database("shop").collection::<Document>("customers");
    let before = coll.count_documents(doc! {}).run().unwrap();
    let mut h = harness();
    explorer_node(&h, "DataTired-mongo@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("shop").is_some());
    h.get_by_label("shop").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "collections", |h| h.query_by_label("collections").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "customers", |h| h.query_by_label("customers").is_some());
    h.get_by_label("customers").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "editor", |h| h.query_by_label("Add Row").is_some());
    settle(&mut h);
    settle(&mut h);
    // clone row 2 (`_id` column) and submit with the toolbar arrow: the copied email breaks the
    // unique index, MongoDB answers ok:1 + writeErrors -> an error, the clone stays pending
    click_at(&mut h, Pos2::new(560.0, 213.0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    settle(&mut h);
    h.get_by_label("Submit").click();
    for _ in 0..60 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
    }
    settle(&mut h);
    shot(&mut h, "13-clone-duplicate-key.png");
    assert_eq!(coll.count_documents(doc! {}).run().unwrap(), before, "nothing inserted");
    assert!(!h.get_by_label("Submit").accesskit_node().is_disabled(), "the clone is still pending");
    // give the clone its own email (7th column) and submit again: created
    for _ in 0..6 {
        h.key_press(Key::ArrowRight);
        h.step();
    }
    h.key_press(Key::Enter);
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    type_chars(&mut h, "clone@example.com");
    h.key_press(Key::Enter);
    settle(&mut h);
    h.get_by_label("Submit").click();
    for _ in 0..100 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
        if coll.count_documents(doc! {}).run().unwrap() == before + 1 {
            break;
        }
    }
    let clone = coll.find_one(doc! {"email": "clone@example.com"}).run().unwrap();
    let _ = coll.delete_many(doc! {"email": "clone@example.com"}).run();
    let clone = clone.expect("clone inserted after fixing the email");
    assert_eq!(clone.get_str("name"), Ok("Customer 2"));
    assert!(matches!(clone.get("_id"), Some(mongodb::bson::Bson::ObjectId(_))));
}

#[test]
fn postgres_copy_using_data_extractor() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if pg_client().is_none() {
        return eprintln!("skipped: no local postgres with the datatired database");
    }
    let mut h = pg_harness();
    explorer_node(&h, "datatired-pg@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("datatired").is_some());
    h.get_by_label("datatired").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "public", |h| h.query_by_label("public").is_some());
    h.get_by_label("public").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "tables", |h| h.query_by_label("tables").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "products", |h| h.query_by_label("products").is_some());
    h.get_by_label("products").click();
    h.step();
    h.key_press(Key::Enter);
    wait(&mut h, "products editor", |h| h.query_by_label("products [datatired-pg@localhost]").is_some());
    settle(&mut h);
    settle(&mut h);
    // row 5 (CB-USB) by its row number, then the cell menu keeps that selection
    click_at(&mut h, Pos2::new(448.0, 294.0));
    right_click(&mut h, Pos2::new(620.0, 294.0));
    shot(&mut h, "pg-09a-cell-menu.png");
    assert!(h.query_by_label("Copy Using Data Extractor (CSV)").is_some());
    let change = h.get_by_label("Change Data Extractor (CSV)").rect();
    h.event(egui::Event::PointerMoved(change.center()));
    settle(&mut h);
    shot(&mut h, "pg-09-change-data-extractor.png");
    h.get_by_label("SQL Inserts").click();
    settle(&mut h);
    right_click(&mut h, Pos2::new(620.0, 294.0));
    h.get_by_label("Copy Using Data Extractor (SQL Inserts)").click();
    h.step();
    assert_eq!(
        copied(&h).as_deref(),
        Some(
            "INSERT INTO public.products (sku, name, price, stock, category, discontinued) VALUES ('CB-USB', 'USB-C Cable', 9.99, 1000, 'accessory', false);\n"
        ),
        "the statement DataGrip copies for that row"
    );
    // ⌘C on a single cell copies just its value
    click_at(&mut h, Pos2::new(520.0, 186.0));
    h.event(egui::Event::Copy);
    h.step();
    assert_eq!(copied(&h).as_deref(), Some("KB-01"));
}

#[test]
fn postgres_sql_generator() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if pg_client().is_none() {
        return eprintln!("skipped: no local postgres with the datatired database");
    }
    let mut h = pg_harness();
    explorer_node(&h, "datatired-pg@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("datatired").is_some());
    h.get_by_label("datatired").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "public", |h| h.query_by_label("public").is_some());
    h.get_by_label("public").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "tables", |h| h.query_by_label("tables").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "customers", |h| h.query_by_label("customers").is_some());
    h.get_by_label("customers").click();
    h.step();
    // right click › SQL Scripts › SQL Generator…: the tool window with DataGrip's options and script
    let row = h.get_by_label("customers").rect().center();
    right_click(&mut h, row);
    let scripts = h.get_by_label("SQL Scripts").rect();
    h.event(egui::Event::PointerMoved(scripts.center()));
    settle(&mut h);
    for item in ["Generate DDL to Clipboard", "Generate DDL to Query Console", "Select first N rows from a table"] {
        assert!(h.query_by_label(item).is_some(), "{item}");
    }
    shot(&mut h, "40-sql-scripts-menu.png");
    h.get_by_label("SQL Generator…").click();
    let loaded = |h: &Harness<'_, App>| h.query_by_label("Copy to Clipboard").is_some_and(|n| !n.accesskit_node().is_disabled());
    wait(&mut h, "generated script", loaded);
    for opt in ["Generate:", "Qualify objects with schema names:", "Place constraints:", "Reformat generated code"] {
        assert!(h.query_by_label(opt).is_some(), "{opt}");
    }
    shot(&mut h, "41-sql-generator.png");
    h.get_by_label("Copy to Clipboard").click();
    h.step();
    let text = copied(&h).unwrap_or_default();
    assert!(text.starts_with("create table customers\n(\n    id         serial\n        primary key,\n"), "{text}");
    assert!(text.contains("alter table customers\n    owner to postgres;"), "{text}");
    // options apply at once
    h.get_by_label("Ignore owner").click();
    h.step();
    h.get_by_label("Copy to Clipboard").click();
    h.step();
    assert!(!copied(&h).unwrap_or_default().contains("owner to"));
    h.get_by_label("Ignore owner").click();
    h.step();
    // ⌥⇧⌘G: Generate DDL to Clipboard of the selected table
    h.get_by_label("products").click();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT, Key::G);
    let mut ddl = None;
    for _ in 0..200 {
        h.step();
        if let Some(t) = copied(&h) {
            ddl = Some(t);
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(ddl.unwrap_or_default().starts_with("create table products\n"));
    // Services takes the bottom area back
    h.get_by_label("Services").click();
    settle(&mut h);
    assert!(h.query_by_label("Output").is_some() && h.query_by_label("Copy to Clipboard").is_none());
}

#[test]
fn postgres_modify_table() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut pg) = pg_client() else {
        return eprintln!("skipped: no local postgres with the datatired database");
    };
    pg.batch_execute("drop schema if exists ui_mod cascade; create schema ui_mod; create table ui_mod.items (id int primary key, a int, note text);")
        .unwrap();
    let mut h = pg_harness();
    explorer_node(&h, "datatired-pg@localhost").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "databases", |h| h.query_by_label("datatired").is_some());
    h.get_by_label("datatired").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "ui_mod", |h| h.query_by_label("ui_mod").is_some());
    h.get_by_label("ui_mod").click();
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "tables", |h| h.query_by_label("tables").is_some());
    h.key_press(Key::ArrowDown);
    h.step();
    h.key_press(Key::ArrowRight);
    wait(&mut h, "items", |h| h.query_by_label("items").is_some());
    let row = h.get_by_label("items").rect().center();
    right_click(&mut h, row);
    h.get_by_label("Modify Table…").click();
    wait(&mut h, "Modify dialog", |h| h.query_by_label("columns").is_some() && h.query_by_label("note").is_some());
    for label in ["keys", "foreign keys", "indexes", "checks", "Persistence", "Access Method", "Owner", "Preview"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    // drop column `a`, make `note` not null
    h.get_by_label("a").click();
    h.step();
    assert!(h.query_by_label("Data Type").is_some() && h.query_by_label("Identity").is_some(), "column form");
    h.get_all_by_label("Remove").next().unwrap().click();
    h.step();
    h.get_by_label("note").click();
    h.step();
    h.get_by_label("Not Null").click();
    settle(&mut h);
    shot(&mut h, "42-modify-table.png");
    // Cancel with changes asks first
    h.get_by_label("Cancel").click();
    settle(&mut h);
    assert!(h.query_by_label("You're about to lose your changes").is_some());
    h.get_all_by_label("Cancel").last().unwrap().click();
    settle(&mut h);
    h.get_by_label("OK").click();
    wait(&mut h, "dialog closed", |h| h.query_by_label("Preview").is_none());
    let cols: Vec<(String, bool)> = pg
        .query("select attname::text, attnotnull from pg_attribute where attrelid = 'ui_mod.items'::regclass and attnum > 0 and not attisdropped order by attnum", &[])
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    assert_eq!(cols, [("id".into(), true), ("note".into(), true)]);
    pg.batch_execute("drop schema ui_mod cascade").unwrap();
}

/// Execute on a data source that isn't connected (here: no password yet) reports why in Output instead
/// of leaving the console "running" with the request silently dropped.
#[test]
fn console_execute_without_connection_reports_error() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = harness_with(
        || {
            use crate::config::{Auth, Config, DataSource, Driver};
            let mut ds = DataSource {
                id: 3,
                name: "locked-mongo".into(),
                host: "127.0.0.1".into(),
                port: "27018".into(),
                user: "nobody".into(),
                auth: Auth::UserPassword,
                ..DataSource::new(Driver::Mongo)
            };
            ds.url = ds.generated_url();
            crate::config::save_project(&crate::config::projects_home().join("DataTired"), &Config { sources: vec![ds], ..Default::default() });
        },
        0.25,
    );
    explorer_node(&h, "locked-mongo").click();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::L);
    settle(&mut h);
    // the console asks for the password first; dismiss it
    if h.query_by_label("Cancel").is_some() {
        h.get_by_label("Cancel").click();
        settle(&mut h);
    }
    click_at(&mut h, Pos2::new(800.0, 300.0));
    h.event(egui::Event::Text("db.import_requests.find().toArray()".into()));
    h.step();
    h.get_by_label("Execute").click();
    wait(&mut h, "error in Output", |h| h.query_by_label_contains("Enter the password for user 'nobody'").is_some());
    assert!(!h.get_by_label("Execute").accesskit_node().is_disabled(), "console is not left running");
}
