//! Integration tests for plain mode and exit codes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn hem() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hem"))
}

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = hem()
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("run hem");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn plain_prints_every_line_when_piped() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&[p.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 17);
    assert!(out.starts_with("2026-10-13 10:02:11.204 INFO"));
}

#[test]
fn level_filter_keeps_continuation_lines() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&[p.to_str().unwrap(), "--level", "error", "--plain"]);
    assert_eq!(code, 0);
    // 5 ERROR + 1 ERROR with 4 trace lines + 1 FATAL = 10
    assert_eq!(out.lines().count(), 10, "{out}");
    assert!(out.contains("\tat com.example.worker"));
    assert!(!out.contains("INFO"));
}

#[test]
fn include_exclude_and_literal() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&[
        p.to_str().unwrap(),
        "--filter",
        "logged in",
        "-x",
        "alice",
        "--plain",
    ]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1);
    assert!(out.contains("bob"));
    let (code, out, _) = run(&[
        p.to_str().unwrap(),
        "--filter",
        "(812ms)",
        "--literal",
        "--plain",
    ]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1);
}

#[test]
fn field_predicates_on_json() {
    let p = testdata("sample-json.log");
    let (code, out, _) = run(&[p.to_str().unwrap(), "--field", "status>=500", "--plain"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 3, "{out}");
    let (code, out, _) = run(&[p.to_str().unwrap(), "--field", "user=carol", "--plain"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1, "{out}");
}

#[test]
fn since_until_time_range() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&[
        p.to_str().unwrap(),
        "--since",
        "2026-10-13T10:02:16Z",
        "--until",
        "2026-10-13T10:02:18Z",
        "--plain",
    ]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 3, "{out}");
}

#[test]
fn collapse_flag_adds_counts() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&[p.to_str().unwrap(), "--collapse", "--plain"]);
    assert_eq!(code, 0);
    assert!(out.contains("×4 2026-10-13 10:02:15.001 ERROR"), "{out}");
    assert_eq!(out.lines().count(), 14, "{out}");
}

#[test]
fn stdin_source() {
    let mut child = hem()
        .args(["--level", "warn", "--plain"])
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            std::fs::read(testdata("sample-logfmt.log"))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 3, "{text}");
}

#[test]
fn merge_interleaves_sources() {
    let a = testdata("sample-json.log");
    let b = testdata("sample-plain.log");
    let (code, out, _) = run(&[
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "--merge",
        "--plain",
    ]);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 27, "{out}");
    assert!(
        lines[0].starts_with("sample-jso") || lines[0].starts_with("sample-pla"),
        "{}",
        lines[0]
    );
    // The plain file's 10:02:11.204 line comes right after the json 10:02:11.204 line.
    let pos_json = lines
        .iter()
        .position(|l| l.contains("server started\",\"port\""))
        .unwrap();
    let pos_plain = lines
        .iter()
        .position(|l| l.contains("[api] server started"))
        .unwrap();
    assert!(pos_json < pos_plain);
}

#[test]
fn templates_subcommand() {
    let p = testdata("sample-plain.log");
    let (code, out, _) = run(&["templates", p.to_str().unwrap(), "-n", "5"]);
    assert_eq!(code, 0);
    assert!(out.contains("count"), "{out}");
    assert!(out.contains("connection reset by peer"), "{out}");
    let (code, out, _) = run(&["templates", p.to_str().unwrap(), "--json", "-n", "2"]);
    assert_eq!(code, 0);
    assert!(
        out.lines().all(|l| l.starts_with('{') && l.ends_with('}')),
        "{out}"
    );
}

#[test]
fn glob_expansion() {
    let pat = testdata("sample-*.log");
    let (code, out, _) = run(&[pat.to_str().unwrap(), "--plain"]);
    assert_eq!(code, 0);
    assert!(out.lines().count() > 40, "{out}");
}

#[test]
fn exit_codes() {
    let (code, _, err) = run(&["--definitely-not-a-flag"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = run(&["/definitely/missing/file.log", "--plain"]);
    assert_eq!(code, 1);
    assert!(err.contains("cannot open"), "{err}");
    let p = testdata("sample-plain.log");
    let (code, _, err) = run(&[p.to_str().unwrap(), "--filter", "(", "--plain"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid regex"), "{err}");
    let (code, _, err) = run(&[p.to_str().unwrap(), "--level", "loud", "--plain"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = run(&[p.to_str().unwrap(), "--format", "xml", "--plain"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("unknown format"), "{err}");
}

#[test]
fn bad_config_names_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[defaults]\ncollaps = true\n").unwrap();
    let p = testdata("sample-plain.log");
    let (code, _, err) = run(&[
        p.to_str().unwrap(),
        "--config",
        cfg.to_str().unwrap(),
        "--plain",
    ]);
    assert_eq!(code, 1);
    assert!(err.contains("collaps"), "{err}");
    assert!(err.contains("line 2"), "{err}");
}

#[test]
fn preset_and_custom_format_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(
        &cfg,
        "[presets.errs]\nlevel = \"error\"\n\n[formats.bracket]\nbase = \"plain\"\nlevel_regex = \"<(\\\\w+)>\"\n",
    )
    .unwrap();
    let p = testdata("sample-plain.log");
    let (code, out, err) = run(&[
        p.to_str().unwrap(),
        "--config",
        cfg.to_str().unwrap(),
        "--preset",
        "errs",
        "--plain",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().count(), 10, "{out}");
    let log = dir.path().join("custom.log");
    std::fs::write(
        &log,
        "2026-10-13 10:00:00 <boom> one\n2026-10-13 10:00:01 <ok> two\n",
    )
    .unwrap();
    let (code, out, err) = run(&[
        log.to_str().unwrap(),
        "--config",
        cfg.to_str().unwrap(),
        "--format",
        "bracket",
        "--level",
        "error",
        "--plain",
    ]);
    assert_eq!(code, 0, "{err}");
    // "boom" is not a level name, so nothing is at error level.
    assert_eq!(out.lines().count(), 0, "{out}");
}

#[test]
fn handles_binary_garbage_and_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("junk.log");
    let mut data = Vec::new();
    data.extend_from_slice(b"ok line\r\n");
    data.extend((0..=255u8).cycle().take(5000));
    data.extend_from_slice(b"\r\nERROR after junk\r\n");
    std::fs::write(&log, &data).unwrap();
    let (code, out, _) = run(&[log.to_str().unwrap(), "--level", "error", "--plain"]);
    assert_eq!(code, 0);
    assert!(out.contains("ERROR after junk"), "{out}");
    let (code, _, _) = run(&["templates", log.to_str().unwrap()]);
    assert_eq!(code, 0);
}

#[test]
fn gzip_source_is_decompressed() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("sample.log.gz");
    let raw = std::fs::read(testdata("sample-plain.log")).unwrap();
    let f = std::fs::File::create(&gz).unwrap();
    let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    enc.write_all(&raw).unwrap();
    enc.finish().unwrap();
    let (code, out, err) = run(&[gz.to_str().unwrap(), "--plain"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().count(), 17, "{out}");
}

#[test]
fn merging_many_files_prints_every_line() {
    // Regression: with several sources the merge was still running when
    // the view processed its first batch, and the view (and plain output)
    // stopped after 32 768 lines.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut paths = Vec::new();
    for f in 0..12 {
        let p = dir.path().join(format!("svc{f}.log"));
        let mut file = std::fs::File::create(&p).expect("create");
        for i in 0..20_000 {
            writeln!(
                file,
                "2026-10-13 10:{:02}:{:02}.{:03} INFO svc{f} line {i}",
                i / 6000,
                (i / 100) % 60,
                (i % 100) * 10 + f
            )
            .expect("write");
        }
        paths.push(p.to_string_lossy().into_owned());
    }
    for _ in 0..3 {
        let mut args: Vec<&str> = paths.iter().map(String::as_str).collect();
        args.extend(["--merge", "--plain"]);
        let (code, out, err) = run(&args);
        assert_eq!(code, 0, "{err}");
        assert_eq!(out.lines().count(), 12 * 20_000);
    }
}

#[test]
fn sessions_list_resume_and_reopen_files_in_plain_mode() {
    let dir = std::env::temp_dir().join(format!("hem-cli-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dir.join("store");
    let with_store = |args: &[&str]| {
        let out = hem()
            .args(args)
            .env("NO_COLOR", "1")
            .env("HEM_SESSION_DIR", &store)
            .output()
            .expect("run hem");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    let (code, out, _) = with_store(&["sessions"]);
    assert_eq!(code, 0);
    assert!(out.contains("no saved sessions"), "{out}");
    let (code, _, err) = with_store(&["--resume", "--plain"]);
    assert_eq!(code, 1);
    assert!(err.contains("no session to resume"), "{err}");

    // A session file as the TUI writes it; plain mode reopens its files.
    let log = testdata("sample-plain.log");
    let gone = dir.join("rotated-away.log");
    std::fs::create_dir_all(&store).unwrap();
    let toml = format!(
        "version = 1\nsaved = \"2026-10-15 09:00:00\"\nmerge = false\nactive = \"\"\n\n\
         [[files]]\npath = {:?}\n\n[[files]]\npath = {:?}\n",
        log.to_string_lossy(),
        gone.to_string_lossy()
    );
    std::fs::write(store.join("inc.toml"), toml).unwrap();
    let (code, out, _) = with_store(&["sessions"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("inc") && out.contains("sample-plain.log"),
        "{out}"
    );
    let (code, out, err) = with_store(&["--session", "inc", "--level", "error", "--plain"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().count(), 10, "{out}");
    assert!(err.contains("missing, skipped"), "{err}");

    let (code, _, err) = with_store(&["--session", "../x", "--plain"]);
    assert_eq!(code, 1);
    assert!(err.contains("bad session name"), "{err}");
    let (code, out, _) = with_store(&["sessions", "--delete", "inc"]);
    assert_eq!(code, 0);
    assert!(out.contains("deleted"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run hem until it exits, failing (instead of hanging the test run) if it
/// takes longer than `secs`.
fn run_with_timeout(args: &[&str], secs: u64) -> (i32, String, String) {
    let mut child = hem()
        .args(args)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("hem {args:?} did not exit within {secs}s");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Start `hem ARGS -f --plain`, let it settle, run `act`, then stop it and
/// return what it printed.
fn follow(args: &[&str], act: impl FnOnce()) -> (String, String) {
    let mut child = hem()
        .args(args)
        .args(["-f", "--plain"])
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    act();
    std::thread::sleep(std::time::Duration::from_millis(2000));
    let _ = child.kill();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn multibyte_text_in_a_syslog_source_does_not_crash() {
    // Bytes 3 and 9 of the last line are ' ' and ':' and byte 15 is inside
    // 'é': the syslog field parser used to slice there, panic on the view
    // thread and leave plain mode waiting forever.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sys.log");
    let mut text = String::new();
    for i in 1..=3 {
        text.push_str(&format!("Oct 13 10:02:1{i} web01 sshd[1234]: ok {i}\n"));
    }
    text.push_str("Oct 13 10:0000é host app: msg\n");
    std::fs::write(&p, text).unwrap();
    let (code, out, err) = run_with_timeout(&[p.to_str().unwrap(), "--field", "app=sshd"], 20);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().count(), 3, "{out}");
}

#[test]
fn collapsed_counts_are_final_in_plain_mode() {
    // Runs longer than a view chunk (32 768 rows) used to be printed before
    // the next chunk added to their count.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("runs.log");
    let mut text = String::new();
    for word in ["alpha", "beta", "gamma"] {
        for i in 0..40_000 {
            text.push_str(&format!("{word} request {i} handled\n"));
        }
    }
    std::fs::write(&p, text).unwrap();
    let (code, out, err) = run_with_timeout(&[p.to_str().unwrap(), "--collapse"], 60);
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "×40000 alpha request 0 handled",
            "×40000 beta request 0 handled",
            "×40000 gamma request 0 handled"
        ]
    );
}

#[test]
fn corrupt_gzip_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bad.log.gz");
    std::fs::write(&p, b"this is not gzip").unwrap();
    let (code, _, err) = run_with_timeout(&[p.to_str().unwrap()], 20);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("gunzip"), "{err}");
}

#[test]
fn follow_prints_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.log");
    let b = dir.path().join("b.log");
    std::fs::write(&a, "a one\n").unwrap();
    std::fs::write(&b, "b one\n").unwrap();
    let (out, err) = follow(&[a.to_str().unwrap(), b.to_str().unwrap()], || {
        let mut f = std::fs::OpenOptions::new().append(true).open(&b).unwrap();
        f.write_all(b"b two\n").unwrap();
    });
    assert!(out.contains("==> a.log <==\na one\n"), "{out}\n{err}");
    assert!(
        out.contains("==> b.log <==\nb one\nb two\n"),
        "{out}\n{err}"
    );
}

#[test]
fn follow_after_truncation_prints_the_new_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("app.log");
    std::fs::write(&p, "a1\na2\na3\na4\na5\n").unwrap();
    // Truncated and refilled past its old length before hem's next poll.
    let (out, err) = follow(&[p.to_str().unwrap()], || {
        std::fs::write(&p, "b1\nb2\nb3\nb4\nb5\nb6\nb7\n").unwrap();
    });
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        ["a1", "a2", "a3", "a4", "a5", "b1", "b2", "b3", "b4", "b5", "b6", "b7"],
        "{err}"
    );
    assert!(err.contains("truncated"), "{err}");
}

#[test]
fn follow_counts_repeats_that_arrive_later() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("app.log");
    std::fs::write(&p, "same 1\n").unwrap();
    let (out, err) = follow(&[p.to_str().unwrap(), "--collapse"], || {
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"same 2\nsame 3\nother\n").unwrap();
    });
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["same 1", "×2 same 1", "other"], "{err}");
}

#[cfg(unix)]
#[test]
fn follow_keeps_the_old_files_last_lines_on_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("app.log");
    std::fs::write(&p, "a1\na2\n").unwrap();
    let (out, err) = follow(&[p.to_str().unwrap()], || {
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"a3\n").unwrap();
        std::fs::rename(&p, dir.path().join("app.log.1")).unwrap();
        std::fs::write(&p, "n1\n").unwrap();
    });
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["a1", "a2", "a3", "n1"], "{err}");
}
