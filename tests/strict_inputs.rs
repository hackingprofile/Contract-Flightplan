//! Inputs that carry approval (proposal, migration, fixture catalog) are read
//! strictly: a duplicate key is a parse error naming the key, and a field the
//! published schema does not define is refused, never silently ignored.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-strict-{name}-{}-{}",
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

fn pack(name: &str) -> PathBuf {
    let dir = scratch(name);
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    dir
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn review(dir: &Path) -> Output {
    Command::new(bin())
        .args([
            "review",
            "--proposal",
            p(&dir.join("proposal.json")),
            "--source",
            p(&dir.join("base.openapi.yaml")),
            "--destination",
            p(&dir.join("next.openapi.yaml")),
            "--out",
            p(&dir.join("review.json")),
        ])
        .output()
        .unwrap()
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

fn verify(dir: &Path) -> Output {
    Command::new(bin())
        .args([
            "verify",
            "--migration",
            p(&dir.join("migration.json")),
            "--proposal",
            p(&dir.join("proposal.json")),
            "--source",
            p(&dir.join("base.openapi.yaml")),
            "--destination",
            p(&dir.join("next.openapi.yaml")),
            "--fixtures",
            p(&dir.join("fixtures")),
            "--out",
            p(&dir.join("verify.json")),
        ])
        .output()
        .unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Replace the first occurrence of `from` in a file; the text must be there.
fn edit_text(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from:?} not in {}", path.display());
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn edit_json(path: &Path, change: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    change(&mut value);
    fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn assert_refused(out: &Output, words: &[&str], what: &str) {
    assert_eq!(out.status.code(), Some(3), "{what}: {}", stderr(out));
    let err = stderr(out);
    for word in words {
        assert!(err.contains(word), "{what}: stderr lacks {word:?}: {err}");
    }
}

#[test]
fn duplicate_key_in_proposal_is_a_parse_error_naming_the_key() {
    let dir = pack("dup-proposal");
    let proposal = dir.join("proposal.json");
    let text = fs::read_to_string(&proposal).unwrap();
    let id_line = text
        .lines()
        .find(|l| l.trim_start().starts_with("\"id\": \"rename-user-id\""))
        .expect("pack proposal has an id line")
        .to_string();
    edit_text(
        &proposal,
        &id_line,
        &format!("{id_line} \"id\": \"shadow-id\","),
    );
    let out = review(&dir);
    assert_refused(&out, &["duplicate", "`id`"], "review");
    assert!(!dir.join("review.json").exists(), "no review written");
    let out = approve(&dir);
    assert_refused(&out, &["duplicate", "`id`"], "approve");
    assert!(!dir.join("migration.json").exists(), "no migration written");
}

#[test]
fn duplicate_key_nested_in_a_rule_is_a_parse_error_naming_the_key() {
    let dir = pack("dup-nested");
    let proposal = dir.join("proposal.json");
    edit_json(&proposal, |_| {});
    edit_text(&proposal, "\"from\": ", "\"from\": \"shadow\", \"from\": ");
    let out = review(&dir);
    assert_refused(&out, &["duplicate", "`from`"], "review");
}

#[test]
fn duplicate_key_in_fixture_catalog_is_refused_at_approve() {
    let dir = pack("dup-catalog");
    edit_text(
        &dir.join("fixtures/catalog.json"),
        "\"schema_version\": \"1\",",
        "\"schema_version\": \"1\", \"schema_version\": \"1\",",
    );
    let out = approve(&dir);
    assert_refused(&out, &["duplicate", "`schema_version`"], "approve");
    assert!(!dir.join("migration.json").exists());
}

#[test]
fn duplicate_key_in_migration_is_refused_at_verify() {
    let dir = pack("dup-migration");
    let out = approve(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    let migration = dir.join("migration.json");
    let text = fs::read_to_string(&migration).unwrap();
    let reviewer_line = text
        .lines()
        .find(|l| l.trim_start().starts_with("\"reviewer\":"))
        .unwrap()
        .to_string();
    edit_text(
        &migration,
        &reviewer_line,
        &format!("{reviewer_line} \"reviewer\": \"someone-else\","),
    );
    let out = verify(&dir);
    assert_refused(&out, &["duplicate", "`reviewer`"], "verify");
    assert!(
        !dir.join("verify.json").exists(),
        "no verify report written"
    );
}

#[test]
fn unknown_top_level_proposal_field_is_refused() {
    let dir = pack("unknown-top");
    edit_json(&dir.join("proposal.json"), |v| {
        v["execute_on_load"] = json!(true);
    });
    let out = review(&dir);
    assert_refused(&out, &["execute_on_load"], "review");
    let out = approve(&dir);
    assert_refused(&out, &["execute_on_load"], "approve");
    assert!(!dir.join("migration.json").exists());
}

/// One edit applied to a parsed proposal.
type Mutation = fn(&mut Value);

#[test]
fn unknown_fields_in_rules_targets_and_parameters_are_refused() {
    let cases: [(&str, Mutation); 4] = [
        ("rule", |v| v["rules"][0]["run"] = json!("rm -rf /")),
        ("target", |v| v["rules"][0]["source"]["shell"] = json!(true)),
        ("operation", |v| {
            v["rules"][0]["source"]["operation"]["hook"] = json!("x")
        }),
        ("parameters", |v| {
            v["rules"][0]["parameters"]["eval"] = json!("1+1")
        }),
    ];
    for (name, change) in cases {
        let dir = pack(&format!("unknown-{name}"));
        edit_json(&dir.join("proposal.json"), change);
        let out = review(&dir);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{name}: unknown field accepted: {}",
            stderr(&out)
        );
    }
}

#[test]
fn unknown_field_in_catalog_item_is_refused_at_approve() {
    let dir = pack("unknown-catalog");
    edit_json(&dir.join("fixtures/catalog.json"), |v| {
        v["items"][0]["command"] = json!("curl example.invalid");
    });
    let out = approve(&dir);
    assert_refused(&out, &["command"], "approve");
    assert!(!dir.join("migration.json").exists());
}

#[test]
fn unknown_field_in_migration_approval_is_refused_at_verify() {
    let dir = pack("unknown-migration");
    let out = approve(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    edit_json(&dir.join("migration.json"), |v| {
        v["approval"]["signature"] = json!("trust-me");
    });
    let out = verify(&dir);
    assert_refused(&out, &["signature"], "verify");
}

/// Run `cfp compare` with one hand-written source contract against the pack's next contract.
fn compare_contract(name: &str, file: &str, text: &str) -> Output {
    let dir = scratch(name);
    let source = dir.join(file);
    fs::write(&source, text).unwrap();
    Command::new(bin())
        .args(["compare", "--source", p(&source), "--destination"])
        .arg(root().join("packs/rename-user-id/next.openapi.yaml"))
        .output()
        .unwrap()
}

#[test]
fn duplicate_key_in_a_json_contract_is_a_parse_error_naming_key_and_pointer() {
    let text = r#"{"openapi": "3.0.3",
 "info": {"title": "A", "version": "1", "title": "B"},
 "paths": {}}"#;
    let out = compare_contract("dup-json-contract", "base.openapi.json", text);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("duplicate key `title` at /info/title"),
        "{err}"
    );
}

#[test]
fn duplicate_key_in_a_yaml_contract_is_a_parse_error_naming_key_and_pointer() {
    let text = "openapi: 3.0.3\ninfo:\n  title: A\n  version: '1'\npaths:\n  /users:\n    post:\n      operationId: a\n      responses:\n        '200':\n          description: ok\n    post:\n      operationId: b\n      responses:\n        '200':\n          description: ok\n";
    let out = compare_contract("dup-yaml-contract", "base.openapi.yaml", text);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("duplicate key `post` at /paths/~1users/post"),
        "{err}"
    );
}

#[test]
fn yaml_contract_with_integer_keys_and_nulls_still_parses() {
    let text = "openapi: 3.0.3\ninfo:\n  title: A\n  version: 1\n  description: ~\npaths:\n  /users:\n    get:\n      operationId: a\n      responses:\n        200:\n          description: ok\n";
    let out = compare_contract("yaml-int-keys", "base.openapi.yaml", text);
    assert!(
        out.status.code() == Some(0) || out.status.code() == Some(1),
        "{}",
        stderr(&out)
    );
}

/// `approve` with a chosen `--approved-at`, on a fresh copy of the rename pack.
fn approve_at(name: &str, at: &str) -> (Output, PathBuf) {
    let dir = pack(name);
    let record = dir.join("review.json");
    let review = Command::new(bin())
        .args([
            "review",
            "--proposal",
            p(&dir.join("proposal.json")),
            "--source",
        ])
        .arg(dir.join("base.openapi.yaml"))
        .arg("--destination")
        .arg(dir.join("next.openapi.yaml"))
        .arg("--out")
        .arg(&record)
        .output()
        .unwrap();
    assert!(review.status.success(), "{}", stderr(&review));
    let migration = dir.join("migration.json");
    let out = Command::new(bin())
        .args([
            "approve",
            "--review",
            p(&record),
            "--proposal",
            p(&dir.join("proposal.json")),
        ])
        .arg("--source")
        .arg(dir.join("base.openapi.yaml"))
        .arg("--destination")
        .arg(dir.join("next.openapi.yaml"))
        .arg("--fixtures")
        .arg(dir.join("fixtures"))
        .args(["--reviewer", "local-reviewer", "--approved-at", at, "--out"])
        .arg(&migration)
        .output()
        .unwrap();
    (out, migration)
}

/// The migration schema requires `YYYY-MM-DDTHH:MM:SSZ`; `--approved-at` is
/// checked at the CLI as an RFC 3339 UTC timestamp with a real calendar date.
#[test]
fn approved_at_must_be_an_rfc3339_utc_timestamp() {
    for bad in [
        "yesterday-ish",
        "2026-13-01T00:00:00Z",
        "2026-02-29T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2026-10-30T24:00:00Z",
        "2026-10-30T12:60:00Z",
        "2026-10-30 12:00:00Z",
        "2026-10-30T12:00:00+05:30",
        "2026-10-30T12:00:00.5Z",
        "",
    ] {
        let (out, migration) = approve_at("approved-at-bad", bad);
        assert_eq!(out.status.code(), Some(3), "{bad:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("--approved-at"),
            "{bad:?}: {}",
            stderr(&out)
        );
        assert!(!migration.exists(), "{bad:?} wrote a migration");
    }
    for good in ["2028-02-29T00:00:00Z", "2026-10-30T23:59:59Z"] {
        let (out, migration) = approve_at("approved-at-good", good);
        assert!(out.status.success(), "{good}: {}", stderr(&out));
        let doc: Value = serde_json::from_str(&fs::read_to_string(migration).unwrap()).unwrap();
        assert_eq!(doc["approval"]["approved_at"], good);
    }
}

/// The exit-code row of `code` in a Markdown exit-code table.
fn exit_row(doc: &str, code: &str) -> String {
    let text = fs::read_to_string(root().join(doc)).unwrap();
    text.lines()
        .find(|l| l.starts_with(&format!("| {code} |")))
        .unwrap_or_else(|| panic!("{doc} has no exit {code} row"))
        .to_string()
}

/// A missing, unreadable or malformed catalog, or an item whose file is
/// missing, is refused at approve with exit 3, and the README and CLI doc say so.
#[test]
fn catalog_failures_exit_three_as_documented() {
    let dir = pack("catalog-missing");
    fs::remove_file(dir.join("fixtures/catalog.json")).unwrap();
    let out = approve(&dir);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let dir = pack("catalog-bad-path");
    edit_text(
        &dir.join("fixtures/catalog.json"),
        "\"request.positive.json\"",
        "\"no-such-file.json\"",
    );
    let out = approve(&dir);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    for doc in ["README.md", "docs/CLI.md"] {
        assert!(
            exit_row(doc, "3").contains("catalog"),
            "{doc}: exit 3 must name catalog failures"
        );
        assert!(
            !exit_row(doc, "1").contains("history, catalog,"),
            "{doc}: exit 1 still claims catalog failures"
        );
    }
}
