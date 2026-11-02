//! An approved migration cannot be changed by recomputing a digest stored in
//! the same file: the history chain commits to every bound digest (rules and
//! fixtures included), and `verify` runs only rules that equal the reviewed
//! proposal's. These are tamper-evident local records, not signatures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use contract_flightplan::approve::{load_migration, write_json, Migration};
use contract_flightplan::digest::{canonical_digest, sha256_prefixed};
use contract_flightplan::proposal::rules_digest;
use serde_json::{json, Value};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-integrity-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A copy of a shipped pack (with its shipped approval) in a scratch folder.
fn pack_copy(name: &str) -> PathBuf {
    let dir = scratch(name).join(name);
    copy_dir(&root().join("packs").join(name), &dir);
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn verify(pack: &Path, migration: &Path, proposal: &Path, out: &Path) -> Output {
    run(&[
        "verify",
        "--migration",
        s(migration),
        "--proposal",
        s(proposal),
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--fixtures",
        s(&pack.join("fixtures")),
        "--out",
        s(out),
    ])
}

fn history_check(migration: &Path) -> Output {
    run(&["history", "--migration", s(migration), "--verify-chain"])
}

/// Edit one value under the migration's rules and recompute the rules digest
/// with the library, exactly as anyone holding the file can.
fn forge_rules(migration: &Path, pointer: &str, value: Value) -> Migration {
    let mut doc = serde_json::to_value(load_migration(migration).unwrap()).unwrap();
    *doc.pointer_mut(pointer).expect("pointer") = value;
    let mut forged: Migration = serde_json::from_value(doc).unwrap();
    forged.approval.bound_digests.rules = rules_digest(&forged.rules).unwrap();
    forged
}

/// Also rewrite the approved event's digest as the digest of every bound
/// digest, and relink nothing else: everything a forger holding only the
/// migration file could recompute.
fn recompute_approved_event(forged: &mut Migration) {
    let bound = serde_json::to_value(&forged.approval.bound_digests).unwrap();
    let approved = forged
        .history
        .iter_mut()
        .rev()
        .find(|e| e.action == "approved")
        .unwrap();
    approved.digest = canonical_digest(&bound).unwrap();
}

#[test]
fn edited_rules_with_a_recomputed_rules_digest_break_the_history_chain() {
    let pack = pack_copy("default-status");
    let forged = forge_rules(
        &pack.join("approval.migration.json"),
        "/rules/0/parameters/value",
        json!("suspended-by-attacker"),
    );
    let path = pack.join("forged.migration.json");
    write_json(&path, &forged).unwrap();
    let out = history_check(&path);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("history"), "{}", stderr(&out));
}

#[test]
fn edited_fixture_with_recomputed_fixture_digests_breaks_the_history_chain() {
    let pack = pack_copy("default-status");
    let migration_path = pack.join("approval.migration.json");
    let mut forged = load_migration(&migration_path).unwrap();
    let key = forged
        .fixtures
        .keys()
        .find(|k| k.as_str() != "catalog.json")
        .unwrap()
        .clone();
    let fixture = pack.join("fixtures").join(&key);
    let mut text = fs::read_to_string(&fixture).unwrap();
    text.push('\n');
    fs::write(&fixture, &text).unwrap();
    let digest = sha256_prefixed(text.as_bytes());
    forged.fixtures.insert(key.clone(), digest.clone());
    forged.approval.bound_digests.fixtures.insert(key, digest);
    let path = pack.join("forged.migration.json");
    write_json(&path, &forged).unwrap();
    let out = history_check(&path);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("history"), "{}", stderr(&out));
}

#[test]
fn migration_rules_that_differ_from_the_reviewed_proposal_are_refused_at_verify() {
    let pack = pack_copy("default-status");
    let mut forged = forge_rules(
        &pack.join("approval.migration.json"),
        "/rules/0/parameters/value",
        json!("suspended-by-attacker"),
    );
    recompute_approved_event(&mut forged);
    let path = pack.join("forged.migration.json");
    write_json(&path, &forged).unwrap();
    let report = pack.join("verify.json");
    let out = verify(&pack, &path, &pack.join("proposal.json"), &report);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("reviewed proposal"),
        "{}",
        stderr(&out)
    );
    assert!(
        !report.exists(),
        "nothing may be written for a refused migration"
    );
}

#[test]
fn verify_requires_the_reviewed_proposal() {
    let pack = pack_copy("default-status");
    let out = run(&[
        "verify",
        "--migration",
        s(&pack.join("approval.migration.json")),
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--fixtures",
        s(&pack.join("fixtures")),
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("--proposal"), "{}", stderr(&out));
}

#[test]
fn verify_refuses_a_proposal_other_than_the_approved_one() {
    let pack = pack_copy("default-status");
    let other = root()
        .join("packs")
        .join("rename-user-id")
        .join("proposal.json");
    let report = pack.join("verify.json");
    let out = verify(
        &pack,
        &pack.join("approval.migration.json"),
        &other,
        &report,
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("reviewed proposal"),
        "{}",
        stderr(&out)
    );
    assert!(!report.exists());
}

#[test]
fn shipped_pack_verifies_with_its_reviewed_proposal() {
    let pack = pack_copy("default-status");
    let report = pack.join("verify.json");
    let out = verify(
        &pack,
        &pack.join("approval.migration.json"),
        &pack.join("proposal.json"),
        &report,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(v["stable"], true);
}
