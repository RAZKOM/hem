//! Rendering tests against ratatui's `TestBackend`.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use hem_core::Source;
use hem_tui::app::App;
use hem_tui::config::Config;
use hem_tui::{ui, RunOptions};

const SAMPLE: &str = "\
2026-10-13 10:02:11.204 INFO  server started on :8080
2026-10-13 10:02:14.880 WARN  slow query (812ms)
2026-10-13 10:02:15.001 ERROR connection reset by peer
2026-10-13 10:02:15.002 ERROR connection reset by peer
2026-10-13 10:02:15.003 ERROR connection reset by peer
2026-10-13 10:02:16.100 INFO  user bob logged in from 10.0.0.12
2026-10-13 10:02:17.300 DEBUG cache miss key=orders:42
2026-10-13 10:02:19.700 FATAL disk quota exceeded
2026-10-13 10:02:20.900 INFO  request completed status=200
";

fn app_with(text: &str, collapse: bool) -> App {
    let src = Source::from_bytes("app.log", text.as_bytes().to_vec());
    let opts = RunOptions {
        sources: vec![src],
        merge: false,
        config: Config::default(),
        config_path: None,
        ascii: false,
        mouse: true,
        theme: None,
        initial_filter: Default::default(),
        follow: false,
        collapse: Some(collapse),
        debug_log: None,
    };
    let app = App::new(opts).expect("app");
    wait_view(&app);
    app
}

fn wait_view(app: &App) {
    assert!(hem_core::view::wait_complete(
        &app.tab().view,
        Duration::from_secs(5)
    ));
}

fn render(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).expect("terminal");
    term.draw(|f| ui::render(app, f)).expect("draw");
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    buf.cell((x, y))
                        .map(|c| c.symbol().to_string())
                        .unwrap_or_default()
                })
                .collect::<String>()
        })
        .collect()
}

fn key(app: &mut App, c: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

fn key_code(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, c);
    }
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn main_view_renders_title_lines_and_status() {
    let mut app = app_with(SAMPLE, false);
    let lines = render(&mut app, 100, 20);
    assert!(lines[0].contains("hem"), "{}", lines[0]);
    assert!(lines[0].contains("app.log"), "{}", lines[0]);
    assert!(lines[0].contains("9 lines"), "{}", lines[0]);
    let body = joined(&lines);
    assert!(body.contains("server started on :8080"), "{body}");
    assert!(body.contains("disk quota exceeded"), "{body}");
    let status = &lines[19];
    assert!(status.contains("line 1"), "{status}");
    assert!(status.contains("help"), "{status}");
    assert!(status.contains("4 errors"), "{status}");
}

#[test]
fn scrolling_moves_cursor_and_status() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, 'j');
    key(&mut app, 'j');
    assert_eq!(app.tab().cursor, 2);
    let lines = render(&mut app, 100, 20);
    assert!(lines[19].contains("line 3"), "{}", lines[19]);
    key(&mut app, 'G');
    assert_eq!(app.tab().cursor, 8);
    key(&mut app, 'g');
    assert_eq!(app.tab().cursor, 0);
    key_code(&mut app, KeyCode::PageDown);
    assert_eq!(app.tab().cursor, 8);
}

#[test]
fn collapse_shows_count_badge() {
    let mut app = app_with(SAMPLE, true);
    let lines = render(&mut app, 100, 20);
    let body = joined(&lines);
    assert!(body.contains("×3"), "{body}");
    assert_eq!(app.tab().len(), 7);
    // Expanding the run shows the members.
    key(&mut app, 'j');
    key(&mut app, 'j');
    key(&mut app, 'x');
    let lines = render(&mut app, 100, 20);
    let n = lines
        .iter()
        .filter(|l| l.contains("connection reset by peer"))
        .count();
    assert_eq!(n, 3, "{}", joined(&lines));
}

#[test]
fn search_prompt_and_matches() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, '/');
    let lines = render(&mut app, 100, 20);
    assert!(
        joined(&lines).contains("regex (smart case)"),
        "{}",
        joined(&lines)
    );
    type_text(&mut app, "reset");
    key_code(&mut app, KeyCode::Enter);
    let search = app.tab().search.clone().expect("search started");
    assert!(hem_core::search::wait_complete(
        &search,
        Duration::from_secs(5)
    ));
    assert_eq!(search.count(), 3);
    key(&mut app, 'n');
    assert_eq!(app.tab().cursor, 2);
    key(&mut app, 'n');
    assert_eq!(app.tab().cursor, 3);
    key(&mut app, 'N');
    assert_eq!(app.tab().cursor, 2);
    let lines = render(&mut app, 100, 20);
    let body = joined(&lines);
    assert!(body.contains("/reset"), "chip missing: {body}");
    assert!(body.contains("3 matches"), "{body}");
}

#[test]
fn level_cycle_adds_chip_and_filters() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, 'l'); // debug
    key(&mut app, 'l'); // info
    key(&mut app, 'l'); // warn
    wait_view(&app);
    let lines = render(&mut app, 100, 20);
    let body = joined(&lines);
    assert!(body.contains("level ≥ WARN"), "{body}");
    assert_eq!(app.tab().len(), 5);
    assert!(!body.contains("cache miss"), "{body}");
    // Clicking the chip's area removes it.
    let chip_hit = app
        .hits
        .iter()
        .find(|(_, h)| matches!(h, hem_tui::app::Hit::Chip(0)))
        .map(|(r, _)| *r)
        .expect("chip hit region");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: chip_hit.x + 1,
        row: chip_hit.y,
        modifiers: KeyModifiers::NONE,
    });
    wait_view(&app);
    assert_eq!(app.tab().len(), 9);
}

#[test]
fn include_filter_prompt() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, '&');
    type_text(&mut app, "logged in");
    key_code(&mut app, KeyCode::Enter);
    wait_view(&app);
    assert_eq!(app.tab().len(), 1);
    key(&mut app, 'X');
    wait_view(&app);
    assert_eq!(app.tab().len(), 9);
}

#[test]
fn next_error_jumps() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, 'e');
    assert_eq!(app.tab().cursor, 2);
    key(&mut app, 'e');
    assert_eq!(app.tab().cursor, 3);
    key(&mut app, 'G');
    key(&mut app, 'E');
    assert_eq!(app.tab().cursor, 7);
}

#[test]
fn help_modal_lists_keys() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, '?');
    let lines = render(&mut app, 100, 40);
    let body = joined(&lines);
    assert!(body.contains("Help"), "{body}");
    assert!(body.contains("Navigation"), "{body}");
    assert!(body.contains("Next error"), "{body}");
    key_code(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());
}

#[test]
fn palette_filters_and_runs_actions() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, ':');
    type_text(&mut app, "wrap");
    let lines = render(&mut app, 100, 30);
    let body = joined(&lines);
    assert!(body.contains("Command palette"), "{body}");
    assert!(body.contains("Toggle line wrap"), "{body}");
    assert!(!app.tab().wrap);
    key_code(&mut app, KeyCode::Enter);
    assert!(app.tab().wrap);
    assert!(app.modal.is_none());
}

#[test]
fn templates_view_renders_table() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, 't');
    let c = app.tab().clusters.clone().expect("clusters");
    assert!(hem_core::cluster::wait_complete(&c, Duration::from_secs(5)));
    let lines = render(&mut app, 120, 20);
    let body = joined(&lines);
    assert!(body.contains("count"), "{body}");
    assert!(body.contains("template"), "{body}");
    assert!(body.contains("connection reset by peer"), "{body}");
    // Enter applies the selected template as a filter.
    key_code(&mut app, KeyCode::Enter);
    assert!(!app.templates_mode);
    assert!(app.tab().filter.template.is_some());
    wait_view(&app);
    assert!(!app.tab().is_empty() && app.tab().len() < 9);
}

#[test]
fn detail_modal_shows_context() {
    let mut app = app_with(SAMPLE, false);
    key(&mut app, 'j');
    key_code(&mut app, KeyCode::Enter);
    let lines = render(&mut app, 100, 30);
    let body = joined(&lines);
    assert!(body.contains("Line 2"), "{body}");
    assert!(body.contains("WARN"), "{body}");
    assert!(body.contains("Context"), "{body}");
}

#[test]
fn mouse_click_selects_row_and_minimap_jumps() {
    let mut app = app_with(SAMPLE, false);
    let _ = render(&mut app, 100, 20);
    let row_hit = app
        .hits
        .iter()
        .find(|(_, h)| matches!(h, hem_tui::app::Hit::Row(4)))
        .map(|(r, _)| *r)
        .expect("row hit");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: row_hit.x + 5,
        row: row_hit.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tab().cursor, 4);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tab().cursor, 7);
}

#[test]
fn goto_line_percent_and_time() {
    let mut app = app_with(SAMPLE, false);
    app.goto("5");
    assert_eq!(app.tab().cursor, 4);
    app.goto("100%");
    assert_eq!(app.tab().cursor, 8);
    app.goto("2026-10-13T10:02:16Z");
    assert_eq!(app.tab().cursor, 5);
}

#[test]
fn tiny_terminal_does_not_panic() {
    let mut app = app_with(SAMPLE, true);
    let _ = render(&mut app, 8, 2);
    let _ = render(&mut app, 20, 3);
    key(&mut app, '?');
    let _ = render(&mut app, 20, 5);
}

#[test]
fn ascii_mode_renders_without_box_drawing() {
    let src = Source::from_bytes("a.log", SAMPLE.as_bytes().to_vec());
    let opts = RunOptions {
        sources: vec![src],
        merge: false,
        config: Config::default(),
        config_path: None,
        ascii: true,
        mouse: true,
        theme: Some("light".into()),
        initial_filter: Default::default(),
        follow: false,
        collapse: Some(true),
        debug_log: None,
    };
    let mut app = App::new(opts).unwrap();
    wait_view(&app);
    key(&mut app, '?');
    let lines = render(&mut app, 100, 30);
    let body = joined(&lines);
    assert!(!body.contains('╭') && !body.contains('─'), "{body}");
    assert!(body.contains('+'), "{body}");
}

#[test]
fn follow_sticks_to_bottom_as_source_grows() {
    let src = Source::from_bytes("grow.log", b"one\ntwo\n".to_vec());
    let opts = RunOptions {
        sources: vec![src.clone()],
        merge: false,
        config: Config::default(),
        config_path: None,
        ascii: false,
        mouse: true,
        theme: None,
        initial_filter: Default::default(),
        follow: true,
        collapse: Some(false),
        debug_log: None,
    };
    let mut app = App::new(opts).unwrap();
    wait_view(&app);
    let _ = render(&mut app, 60, 10);
    src.append_bytes(b"three\nfour\n");
    let start = Instant::now();
    while app.tab().len() < 4 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    app.tick();
    assert_eq!(app.tab().cursor, 3);
    let lines = render(&mut app, 60, 10);
    assert!(lines[0].contains("following"), "{}", lines[0]);
}

/// A long log with one distinctive line near the top.
fn long_log() -> String {
    let mut s = String::from("2026-10-13 10:00:00.000 ERROR the line worth keeping\n");
    for i in 1..200 {
        s.push_str(&format!(
            "2026-10-13 10:{:02}:{:02}.000 INFO  routine line {i}\n",
            i / 60,
            i % 60
        ));
    }
    s
}

#[test]
fn pinned_lines_stay_on_screen_and_are_jumpable() {
    let mut app = app_with(&long_log(), false);
    key(&mut app, 'p');
    app.tick();
    key(&mut app, 'G');
    let lines = render(&mut app, 100, 30);
    let body = joined(&lines);
    // Scrolled to the bottom, the pinned line is still shown in the strip.
    assert!(body.contains("the line worth keeping"), "{body}");
    assert!(body.contains("1 pinned"), "{body}");
    assert!(body.contains("routine line 199"), "{body}");
    // ']' wraps around to the pin.
    key(&mut app, ']');
    assert_eq!(app.tab().cursor, 0);
    // Clicking the pin in the strip jumps there too.
    key(&mut app, 'G');
    let _ = render(&mut app, 100, 30);
    let pin_hit = app
        .hits
        .iter()
        .find(|(_, h)| matches!(h, hem_tui::app::Hit::Pin(0)))
        .map(|(r, _)| *r)
        .expect("pin hit");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pin_hit.x + 5,
        row: pin_hit.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tab().cursor, 0);
    // The minimap marks the pin.
    let lines = render(&mut app, 100, 30);
    assert!(joined(&lines).contains('◆'));
}

#[test]
fn pins_hidden_by_filters_are_reported() {
    let mut app = app_with(&long_log(), false);
    key(&mut app, 'j');
    key(&mut app, 'p'); // pin "routine line 1"
    key(&mut app, 'l');
    key(&mut app, 'l');
    key(&mut app, 'l');
    key(&mut app, 'l'); // level >= error
    wait_view(&app);
    app.tick();
    let lines = render(&mut app, 100, 30);
    let body = joined(&lines);
    assert!(body.contains("filtered out"), "{body}");
    assert!(body.contains("routine line 1"), "{body}");
}

fn json_log() -> String {
    let mut s = String::new();
    for i in 0..60 {
        let extra = if i % 2 == 0 { ",\"user\":\"bob\"" } else { "" };
        s.push_str(&format!(
            "{{\"time\":\"2026-10-13T10:00:{:02}Z\",\"level\":\"info\",\"status\":{},\"msg\":\"request {i}\"{extra}}}\n",
            i,
            200 + i % 3
        ));
    }
    s
}

#[test]
fn field_columns_have_a_stable_header() {
    let mut app = app_with(&json_log(), false);
    key(&mut app, 'C');
    let lines = render(&mut app, 120, 20);
    let header = lines
        .iter()
        .find(|l| l.contains("status") && l.contains("msg"))
        .cloned()
        .expect("header");
    let col = |l: &str, needle: &str| l.find(needle);
    // Values sit under their header.
    let row = lines
        .iter()
        .find(|l| l.contains("request 0 "))
        .cloned()
        .expect("row");
    assert_eq!(
        col(&header, "msg"),
        col(&row, "request 0"),
        "{header}\n{row}"
    );
    // Scrolling does not change the columns.
    let before = app.tab().col_layout.clone();
    for _ in 0..30 {
        key(&mut app, 'j');
    }
    let lines = render(&mut app, 120, 20);
    assert_eq!(app.tab().col_layout, before);
    // Compare the log column only (the scrollbar moves).
    let log = |l: &str| l.chars().take(100).collect::<String>();
    assert!(
        lines.iter().any(|l| log(l) == log(&header)),
        "{}",
        joined(&lines)
    );
    assert!(header.find("time") < header.find("level"), "{header}");
}
