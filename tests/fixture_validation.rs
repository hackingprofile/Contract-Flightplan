//! `approve` binds only fixtures it can read: every catalogued fixture must
//! be UTF-8 JSON (strict: no duplicate keys) before any digest is bound, and
//! the refusal names the fixture.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-fixture-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
            + "-"
            + &{
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_string()
            }
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Review then approve, as a reviewer would. When the proposal itself is
/// refused, no record is written and approve refuses the proposal first.
fn approve(dir: &Path) -> Output {
    let record = dir.join("approve.review.json");
    let _ = Command::new(bin())
        .args([
            "review",
            "--proposal",
            p(&dir.join("proposal.json")),
            "--source",
            p(&dir.join("base.openapi.yaml")),
            "--destination",
            p(&dir.join("next.openapi.yaml")),
            "--out",
            p(&record),
        ])
        .output()
        .unwrap();
    Command::new(bin())
        .args([
            "approve",
            "--review",
            p(&record),
            "--proposal",
            p(&dir.join("proposal.json")),
            "--source",
            p(&dir.join("base.openapi.yaml")),
            "--destination",
            p(&dir.join("next.openapi.yaml")),
            "--fixtures",
            p(&dir.join("fixtures")),
            "--reviewer",
            "local-reviewer",
            "--approved-at",
            "2026-09-30T00:00:00Z",
            "--out",
            p(&dir.join("migration.json")),
        ])
        .output()
        .unwrap()
}

/// Copy the rename pack and replace the bytes of one catalogued fixture.
fn pack_with_fixture(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = scratch(name);
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    fs::write(dir.join("fixtures/request.positive.json"), bytes).unwrap();
    dir
}

fn assert_refused_naming_the_fixture(dir: &Path, out: &Output) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("request-positive"), "names the item: {err}");
    assert!(
        err.contains("request.positive.json"),
        "names the file: {err}"
    );
    assert!(!dir.join("migration.json").exists(), "nothing was bound");
}

#[test]
fn approve_refuses_a_utf16_fixture_and_names_it() {
    let text = r#"{"id": "u-1", "name": "Ada"}"#;
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let dir = pack_with_fixture("utf16", &bytes);
    let out = approve(&dir);
    assert_refused_naming_the_fixture(&dir, &out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("UTF-8"));
}

#[test]
fn approve_refuses_a_fixture_that_is_not_json_and_names_it() {
    let dir = pack_with_fixture("not-json", b"id: u-1\nname: Ada\n");
    let out = approve(&dir);
    assert_refused_naming_the_fixture(&dir, &out);
}

#[test]
fn approve_refuses_a_fixture_with_a_duplicate_key_and_names_it() {
    let dir = pack_with_fixture("dup-key", br#"{"id": "u-1", "id": "u-2", "name": "Ada"}"#);
    let out = approve(&dir);
    assert_refused_naming_the_fixture(&dir, &out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate key `id`"));
}
