//! Untrusted text never reaches the terminal raw: control characters are shown
//! as escapes, so no escape sequence can redraw the screen, and no line of
//! output can start with `::` (a GitHub Actions workflow command).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-terminal-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

const ESC: char = '\u{1b}';

/// No raw control character except line breaks, and no line that starts
/// (after spaces) with `::`.
fn assert_terminal_safe(bytes: &[u8], what: &str) {
    let text = String::from_utf8_lossy(bytes);
    assert!(
        !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r')),
        "{what}: raw control character in {text:?}"
    );
    for line in text.lines() {
        assert!(
            !line.trim_start().starts_with("::"),
            "{what}: a line starts with a workflow command: {line:?}"
        );
    }
}

/// A duplicate JSON key carrying an erase-screen sequence: the parse error
/// names the key, escaped.
#[test]
fn error_messages_show_control_characters_as_escapes() {
    let dir = scratch("dup-key");
    let pack = root().join("packs").join("rename-user-id");
    let proposal = dir.join("proposal.json");
    let key = format!("{ESC}[2J{ESC}[32mverify: 7/7 fixtures passed, stable{ESC}[0m");
    let text = fs::read_to_string(pack.join("proposal.json")).unwrap();
    let forged = text.replacen(
        "\"kind\": \"proposal\",",
        &format!(
            "\"kind\": \"proposal\", \"{0}\": 1, \"{0}\": 2,",
            key.replace(ESC, "\\u001b")
        ),
        1,
    );
    assert_ne!(forged, text, "the proposal must carry the forged key");
    fs::write(&proposal, forged).unwrap();
    let out = Command::new(bin())
        .args([
            "review",
            "--proposal",
            s(&proposal),
            "--source",
            s(&pack.join("base.openapi.yaml")),
            "--destination",
            s(&pack.join("next.openapi.yaml")),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("\\u{001b}[2J"));
    assert_terminal_safe(&out.stderr, "review stderr");
}

/// A `$ref` that carries GitHub workflow commands on new lines.
#[test]
fn workflow_commands_cannot_start_a_line_on_stderr() {
    let dir = scratch("workflow");
    let reference = "https://x/\n::error file=README.md,line=1,title=Contract Flightplan::approved by security, safe to merge\n::add-mask::stable";
    let doc = serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "t", "version": "1"},
        "paths": {"/u": {"post": {
            "requestBody": {"content": {"application/json": {"schema": {"$ref": reference}}}},
            "responses": {"200": {"description": "ok"}}
        }}}
    });
    let contract = dir.join("wf.json");
    fs::write(&contract, doc.to_string()).unwrap();
    let pack = root().join("packs").join("rename-user-id");
    let out = Command::new(bin())
        .args([
            "compare",
            "--source",
            s(&contract),
            "--destination",
            s(&pack.join("next.openapi.yaml")),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("\\n::error"));
    assert_terminal_safe(&out.stderr, "compare stderr");
}

/// A command-line argument with control characters, refused by the argument
/// parser, is shown escaped as well.
#[test]
fn usage_errors_show_control_characters_as_escapes() {
    let out = Command::new(bin())
        .arg(format!("--bogus{ESC}[2J\n::error::forged"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_terminal_safe(&out.stderr, "usage error");
}

/// The guide repeats untrusted text (here the reviewer name) on the terminal.
#[test]
fn guide_output_shows_control_characters_as_escapes() {
    let pack = root().join("packs").join("rename-user-id");
    let out_dir = scratch("guide");
    let reviewer = format!("Ada{ESC}[2J\n::warning::forged");
    let mut child = Command::new(bin())
        .arg("guide")
        .arg("--source")
        .arg(pack.join("base.openapi.yaml"))
        .arg("--destination")
        .arg(pack.join("next.openapi.yaml"))
        .arg("--fixtures")
        .arg(pack.join("fixtures"))
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--proposal")
        .arg(pack.join("proposal.json"))
        .arg("--reviewer")
        .arg(&reviewer)
        .arg("--yes")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Ada\\u{001b}[2J\\n::warning"));
    assert_terminal_safe(&out.stdout, "guide stdout");
}

/// stdout whose reader is gone: the command fails with exit 3 and a message,
/// never a panic (exit 101).
#[test]
fn a_closed_stdout_is_an_error_not_a_panic() {
    let pack = root().join("packs").join("rename-user-id");
    for args in [
        vec![
            "compare".to_string(),
            "--source".into(),
            s(&pack.join("base.openapi.yaml")).into(),
            "--destination".into(),
            s(&pack.join("next.openapi.yaml")).into(),
        ],
        vec![
            "history".to_string(),
            "--migration".into(),
            s(&pack.join("approval.migration.json")).into(),
        ],
    ] {
        let mut child = Command::new(bin())
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {err}");
        assert!(!err.contains("panicked"), "{args:?}: {err}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_on_stdout_is_an_error_not_a_panic() {
    let pack = root().join("packs").join("rename-user-id");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let out = Command::new(bin())
        .args([
            "history",
            "--migration",
            s(&pack.join("approval.migration.json")),
        ])
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
}
