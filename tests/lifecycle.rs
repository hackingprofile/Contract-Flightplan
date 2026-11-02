//! The approval lifecycle is enforced, not described: approve needs a review
//! record that matches the proposal and contracts, history events are hash
//! chained and checked at verify, and a superseded migration cannot run.
//! These are local records, not signatures: the checks catch edits that do
//! not also recompute every digest, which is the documented boundary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use contract_flightplan::digest::canonical_digest;
use serde_json::{json, Value};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-life-{name}-{}-{}",
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

fn cfp(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn review(dir: &Path, proposal: &str, out: &str) -> Output {
    cfp(&[
        "review",
        "--proposal",
        s(&dir.join(proposal)),
        "--source",
        s(&dir.join("base.openapi.yaml")),
        "--destination",
        s(&dir.join("next.openapi.yaml")),
        "--out",
        s(&dir.join(out)),
    ])
}

fn approve_with(dir: &Path, proposal: &str, record: Option<&str>, out: &str) -> Output {
    let mut args = vec![
        "approve".to_string(),
        "--proposal".into(),
        s(&dir.join(proposal)).into(),
        "--source".into(),
        s(&dir.join("base.openapi.yaml")).into(),
        "--destination".into(),
        s(&dir.join("next.openapi.yaml")).into(),
        "--fixtures".into(),
        s(&dir.join("fixtures")).into(),
        "--reviewer".into(),
        "local-reviewer".into(),
        "--approved-at".into(),
        "2026-09-30T00:00:00Z".into(),
        "--out".into(),
        s(&dir.join(out)).into(),
    ];
    if let Some(record) = record {
        args.push("--review".into());
        args.push(s(&dir.join(record)).into());
    }
    Command::new(bin()).args(&args).output().unwrap()
}

/// Review then approve the pack's own proposal into `migration.json`.
fn approved(dir: &Path) -> PathBuf {
    let out = review(dir, "proposal.json", "review.json");
    assert!(out.status.success(), "{}", stderr(&out));
    let out = approve_with(dir, "proposal.json", Some("review.json"), "migration.json");
    assert!(out.status.success(), "{}", stderr(&out));
    dir.join("migration.json")
}

fn verify(dir: &Path, migration: &Path, proposal: &str) -> Output {
    cfp(&[
        "verify",
        "--migration",
        s(migration),
        "--proposal",
        s(&dir.join(proposal)),
        "--source",
        s(&dir.join("base.openapi.yaml")),
        "--destination",
        s(&dir.join("next.openapi.yaml")),
        "--fixtures",
        s(&dir.join("fixtures")),
    ])
}

fn verify_chain(migration: &Path) -> Output {
    cfp(&["history", "--migration", s(migration), "--verify-chain"])
}

#[test]
fn approve_without_review_record_fails() {
    let dir = pack("no-review");
    let out = approve_with(&dir, "proposal.json", None, "migration.json");
    assert_ne!(
        out.status.code(),
        Some(0),
        "approved without a review record"
    );
    assert!(stderr(&out).contains("--review"), "{}", stderr(&out));
    assert!(!dir.join("migration.json").exists());
}

#[test]
fn approve_with_mismatched_review_digest_fails() {
    let dir = pack("mismatch");
    let out = review(&dir, "proposal.json", "review.json");
    assert!(out.status.success(), "{}", stderr(&out));
    // The proposal changes after it was reviewed.
    let mut proposal = read(&dir.join("proposal.json"));
    proposal["rules"][0]["evidence"] = json!("changed after review");
    write(&dir.join("changed.proposal.json"), &proposal);
    let out = approve_with(
        &dir,
        "changed.proposal.json",
        Some("review.json"),
        "migration.json",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("review record"), "{}", stderr(&out));
    assert!(!dir.join("migration.json").exists());

    // A record for other contracts does not match either.
    let mut record = read(&dir.join("review.json"));
    record["diff"]["destination"]["digest"] = json!("sha256:0000");
    write(&dir.join("other.review.json"), &record);
    let out = approve_with(
        &dir,
        "proposal.json",
        Some("other.review.json"),
        "migration.json",
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(!dir.join("migration.json").exists());
}

#[test]
fn review_record_with_duplicate_key_or_unknown_field_is_refused() {
    let dir = pack("record-strict");
    let out = review(&dir, "proposal.json", "review.json");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = fs::read_to_string(dir.join("review.json")).unwrap();
    let kind_line = text
        .lines()
        .find(|l| l.trim_start().starts_with("\"kind\": \"review\""))
        .unwrap()
        .to_string();
    fs::write(
        dir.join("dup.review.json"),
        text.replacen(&kind_line, &format!("{kind_line} \"kind\": \"review\","), 1),
    )
    .unwrap();
    let out = approve_with(&dir, "proposal.json", Some("dup.review.json"), "m1.json");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("duplicate key `kind`"),
        "{}",
        stderr(&out)
    );

    let mut record = read(&dir.join("review.json"));
    record["approved_by"] = json!("nobody");
    write(&dir.join("extra.review.json"), &record);
    let out = approve_with(&dir, "proposal.json", Some("extra.review.json"), "m2.json");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("approved_by"), "{}", stderr(&out));
    assert!(!dir.join("m1.json").exists() && !dir.join("m2.json").exists());
}

#[test]
fn history_chain_detects_removed_event() {
    let dir = pack("chain");
    let migration = approved(&dir);
    let doc = read(&migration);
    let history = doc["history"].as_array().unwrap();
    assert_eq!(history.len(), 2, "reviewed + approved: {history:#?}");
    assert_eq!(history[0]["action"], "reviewed");
    assert_eq!(history[1]["action"], "approved");
    assert!(history[0]["prev"].is_null());
    assert_eq!(
        history[1]["prev"].as_str().unwrap(),
        canonical_digest(&history[0]).unwrap()
    );
    let out = verify_chain(&migration);
    assert!(out.status.success(), "{}", stderr(&out));

    let mut removed = doc.clone();
    removed["history"].as_array_mut().unwrap().remove(0);
    let edited = dir.join("removed.migration.json");
    write(&edited, &removed);
    let out = verify_chain(&edited);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("history"), "{}", stderr(&out));
    let out = verify(&dir, &edited, "proposal.json");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("history"), "{}", stderr(&out));
}

#[test]
fn edited_reviewer_or_emptied_history_is_refused_at_verify() {
    let dir = pack("edited");
    let migration = approved(&dir);
    let out = verify(&dir, &migration, "proposal.json");
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = read(&migration);

    // Hand-edit the reviewer and empty the history.
    let mut both = doc.clone();
    both["approval"]["reviewer"] = json!("someone-else");
    both["history"] = json!([]);
    // Only the reviewer.
    let mut reviewer = doc.clone();
    reviewer["approval"]["reviewer"] = json!("someone-else");
    // Only an event's actor, which breaks the next event's link.
    let mut actor = doc;
    actor["history"][0]["actor"] = json!("someone-else");
    for (name, edited) in [("both", both), ("reviewer", reviewer), ("actor", actor)] {
        let path = dir.join(format!("{name}.migration.json"));
        write(&path, &edited);
        let out = verify(&dir, &path, "proposal.json");
        assert_eq!(out.status.code(), Some(1), "{name}: {}", stderr(&out));
        assert!(stderr(&out).contains("history"), "{name}: {}", stderr(&out));
    }
}

/// The pack's proposal with a changed evidence string, reviewed.
fn successor(dir: &Path) {
    let mut proposal = read(&dir.join("proposal.json"));
    proposal["rules"][0]["evidence"] = json!("successor proposal");
    write(&dir.join("next.proposal.json"), &proposal);
    let out = review(dir, "next.proposal.json", "next.review.json");
    assert!(out.status.success(), "{}", stderr(&out));
}

fn supersede(dir: &Path) -> Output {
    cfp(&[
        "supersede",
        "--migration",
        s(&dir.join("migration.json")),
        "--proposal",
        s(&dir.join("next.proposal.json")),
        "--source",
        s(&dir.join("base.openapi.yaml")),
        "--destination",
        s(&dir.join("next.openapi.yaml")),
        "--fixtures",
        s(&dir.join("fixtures")),
        "--review",
        s(&dir.join("next.review.json")),
        "--reviewer",
        "local-reviewer",
        "--approved-at",
        "2026-10-01T00:00:00Z",
        "--out",
        s(&dir.join("successor.migration.json")),
        "--archive",
        s(&dir.join("archived.migration.json")),
    ])
}

#[test]
fn supersede_links_previous_and_archives_it() {
    let dir = pack("supersede");
    let old_path = approved(&dir);
    let old = read(&old_path);
    successor(&dir);
    let out = supersede(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    let new = read(&dir.join("successor.migration.json"));
    assert_eq!(new["supersedes"]["id"], old["id"]);
    assert_eq!(
        new["supersedes"]["digest"].as_str().unwrap(),
        canonical_digest(&old).unwrap()
    );
    assert_eq!(new["lifecycle"], "approved");
    let out = verify(
        &dir,
        &dir.join("successor.migration.json"),
        "next.proposal.json",
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let archive = read(&dir.join("archived.migration.json"));
    assert_eq!(archive["lifecycle"], "superseded");
    let events = archive["history"].as_array().unwrap();
    assert_eq!(events.len(), 3, "{events:#?}");
    let last = &events[2];
    assert_eq!(last["action"], "superseded");
    assert_eq!(
        last["digest"].as_str().unwrap(),
        canonical_digest(&new).unwrap()
    );
    assert_eq!(
        last["prev"].as_str().unwrap(),
        canonical_digest(&events[1]).unwrap()
    );
    let out = verify_chain(&dir.join("archived.migration.json"));
    assert!(out.status.success(), "{}", stderr(&out));
    // The input migration file itself is left as it was.
    assert_eq!(read(&old_path), old);
}

#[test]
fn superseded_migration_cannot_verify() {
    let dir = pack("superseded");
    approved(&dir);
    successor(&dir);
    let out = supersede(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = verify(&dir, &dir.join("archived.migration.json"), "proposal.json");
    assert_ne!(out.status.code(), Some(0));
    assert!(stderr(&out).contains("superseded"), "{}", stderr(&out));
}

#[test]
fn no_help_text_claims_an_append_only_history() {
    for command in ["approve", "reject", "history", "verify", "supersede"] {
        let out = cfp(&[command, "--help"]);
        let help = String::from_utf8_lossy(&out.stdout).to_lowercase();
        assert!(out.status.success(), "{command} --help failed");
        assert!(!help.contains("append-only"), "{command}: {help}");
    }
}

/// Supersede revokes: the old migration is refused while its successor or
/// its archive copy sits in the same folder.
#[test]
fn a_superseded_migration_is_refused_while_its_successor_or_archive_is_present() {
    let dir = pack("revoked");
    let old_path = approved(&dir);
    successor(&dir);
    let out = supersede(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    let aside = scratch("revoked-aside");
    let (successor_file, archive_file) = (
        dir.join("successor.migration.json"),
        dir.join("archived.migration.json"),
    );
    // Both present, then only the archive, then only the successor.
    for (move_away, label) in [(None, "both"), (Some(&successor_file), "archive only")] {
        if let Some(file) = move_away {
            fs::rename(file, aside.join(file.file_name().unwrap())).unwrap();
        }
        let out = verify(&dir, &old_path, "proposal.json");
        assert_eq!(out.status.code(), Some(3), "{label}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("superseded"),
            "{label}: {}",
            stderr(&out)
        );
    }
    fs::rename(aside.join("successor.migration.json"), &successor_file).unwrap();
    fs::rename(&archive_file, aside.join("archived.migration.json")).unwrap();
    let out = verify(&dir, &old_path, "proposal.json");
    assert_eq!(
        out.status.code(),
        Some(3),
        "successor only: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("superseded"), "{}", stderr(&out));
}

/// A revival: the archive copy with its `superseded` event dropped
/// and lifecycle set back to approved has an intact chain, but it is the old
/// migration, so it is refused the same way.
#[test]
fn an_archive_copy_edited_back_to_approved_is_refused() {
    let dir = pack("revived");
    approved(&dir);
    successor(&dir);
    let out = supersede(&dir);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut revived = read(&dir.join("archived.migration.json"));
    revived["history"].as_array_mut().unwrap().pop();
    revived["lifecycle"] = json!("approved");
    let path = dir.join("revived.migration.json");
    write(&path, &revived);
    let out = verify_chain(&path);
    assert!(
        out.status.success(),
        "the chain itself cannot see a cut tail"
    );
    let out = verify(&dir, &path, "proposal.json");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("superseded"), "{}", stderr(&out));
}
