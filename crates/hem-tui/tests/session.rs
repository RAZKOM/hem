//! Saving a session and restoring it onto a fresh app.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hem_core::source::SourceOptions;
use hem_core::Source;
use hem_tui::app::{App, PinPos};
use hem_tui::config::Config;
use hem_tui::session::{self, Session};
use hem_tui::RunOptions;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hem-session-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_logs(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for (name, offset) in [("api.log", 0), ("db.log", 1)] {
        let mut text = String::new();
        for i in 0..200 {
            let level = if i % 10 == 0 { "ERROR" } else { "INFO " };
            text.push_str(&format!(
                "2026-10-13 10:{:02}:{:02}.000 {level} {name} event {i}\n",
                (i * 2 + offset) / 60,
                (i * 2 + offset) % 60
            ));
        }
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        paths.push(p);
    }
    paths
}

fn app_for(paths: &[PathBuf], merge: bool, session: Option<Session>) -> App {
    let sources = paths
        .iter()
        .map(|p| Source::open(p, SourceOptions::default()).unwrap())
        .collect();
    let opts = RunOptions {
        sources,
        merge,
        config: Config::default(),
        config_path: None,
        ascii: false,
        mouse: true,
        theme: None,
        initial_filter: Default::default(),
        follow: false,
        collapse: Some(false),
        debug_log: None,
        session_name: None,
        session,
    };
    App::new(opts).unwrap()
}

/// Tick until the active tab's view, pins and anchor have settled.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if !app.busy() {
            app.tick();
            return;
        }
        assert!(Instant::now() < deadline, "app never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, KeyCode::Char(c));
    }
}

#[test]
fn a_saved_session_restores_filters_pins_search_and_position() {
    let dir = temp_dir("restore");
    let paths = write_logs(&dir);
    let mut app = app_for(&paths, true, None);
    settle(&mut app);
    assert_eq!(app.tab().name, "merged");
    assert_eq!(app.tab().len(), 400);

    // Filter to errors, search, highlight, pin two lines, move the cursor.
    type_text(&mut app, "&event");
    key(&mut app, KeyCode::Enter);
    // Level cycles all -> debug -> info -> warn -> error.
    type_text(&mut app, "llll");
    settle(&mut app);
    assert_eq!(app.tab().len(), 40, "{:?}", app.tab().filter.spec);
    type_text(&mut app, "/event 1");
    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "*db.log");
    key(&mut app, KeyCode::Enter);
    app.set_cursor(3);
    type_text(&mut app, "p");
    app.set_cursor(10);
    type_text(&mut app, "p");
    app.set_cursor(7);
    settle(&mut app);
    let cursor_row = app.tab().view.row(7).unwrap();
    let pinned: Vec<(u16, u64)> = app.tab().pins.clone();
    assert_eq!(pinned.len(), 2);

    let snap = app.session_snapshot();
    assert!(snap.merge);
    assert_eq!(snap.active, session::MERGED);
    assert_eq!(snap.files.len(), 2);
    // Per-file tabs were never shown, so only the merged tab is saved.
    assert_eq!(snap.tabs.len(), 1, "{:?}", snap.tabs);

    // Through TOML and back, as on disk.
    let snap: Session = toml::from_str(&toml::to_string(&snap).unwrap()).unwrap();
    let mut back = app_for(&paths, true, Some(snap.clone()));
    settle(&mut back);
    let t = back.tab();
    assert_eq!(t.name, "merged");
    assert_eq!(t.filter.spec.include, ["event"]);
    assert_eq!(t.filter.min_level, Some(hem_core::Level::Error));
    assert_eq!(t.len(), 40);
    assert_eq!(t.search_text, "event 1");
    assert!(
        t.search.is_some(),
        "restored search starts on the shown tab"
    );
    assert_eq!(
        t.highlight.as_ref().map(|m| m.text.as_str()),
        Some("db.log")
    );
    assert_eq!(t.pins, pinned);
    assert!(t.pin_pos.iter().all(|p| matches!(p, PinPos::Row(_))));
    assert_eq!(t.view.row(t.cursor), Some(cursor_row));

    // A rewritten file drops its pins rather than pinning unrelated lines.
    let db = &paths[1];
    let text = std::fs::read_to_string(db)
        .unwrap()
        .replace("event", "EVENT");
    std::fs::write(db, text).unwrap();
    let back = app_for(&paths, true, Some(snap));
    // Errors alternate api, db, api, ... so one pin was in each file.
    assert_eq!(back.tab().pins.len(), 1, "the db.log pin was dropped");
    assert!(back
        .notices
        .iter()
        .any(|n| n.text.contains("1 pin(s) dropped")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_and_listing_sessions_on_disk() {
    let dir = temp_dir("disk");
    // Only this test touches the session directory.
    std::env::set_var("HEM_SESSION_DIR", dir.join("sessions"));
    let paths = write_logs(&dir);
    let mut app = app_for(&paths, false, None);
    settle(&mut app);
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    type_text(&mut app, "incident-42");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.session_name.as_deref(), Some("incident-42"));
    let s = session::load("incident-42").unwrap().expect("saved");
    assert_eq!(s.files.len(), 2);
    assert_eq!(s.name, "incident-42");
    assert_eq!(s.active, session::absolute(&paths[0]));

    // Quitting saves both the named session and `last`.
    app.save_session_on_exit().unwrap();
    let last = session::load(session::LAST).unwrap().expect("last saved");
    assert_eq!(last.name, "incident-42");
    let names: Vec<String> = session::list()
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(names.contains(&"incident-42".to_string()), "{names:?}");
    assert!(names.contains(&"last".to_string()), "{names:?}");

    // Bad names are refused with a notice, not saved.
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    for _ in 0.."incident-42".len() {
        key(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "../evil");
    key(&mut app, KeyCode::Enter);
    assert!(app
        .notices
        .iter()
        .any(|n| n.text.contains("bad session name")));
    assert!(session::delete("incident-42").unwrap());
    assert!(session::load("incident-42").unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quit_needs_a_second_press_when_confirm_is_on_and_esc_never_quits() {
    let dir = temp_dir("quit");
    let paths = write_logs(&dir);
    let mut app = app_for(&paths[..1], false, None);
    key(&mut app, KeyCode::Esc);
    assert!(!app.quit, "Esc does not quit");
    app.config.defaults.confirm_quit = true;
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(app.notices.iter().any(|n| n.text.contains("again to quit")));
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);

    let mut app = app_for(&paths[..1], false, None);
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit, "without confirm, q quits at once");
    let _ = std::fs::remove_dir_all(&dir);
}
