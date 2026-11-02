//! Fixture catalogs are hostile input too: ids are names from a safe set, and
//! generated suites never build a file path from a catalog id.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use contract_flightplan::verify::load_catalog;
use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-catalog-{name}-{}-{}",
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

/// A scratch copy of a shipped pack.
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

fn edit_catalog(pack: &Path, f: impl FnOnce(&mut Value)) {
    let path = pack.join("fixtures").join("catalog.json");
    let mut catalog: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    f(&mut catalog);
    fs::write(&path, serde_json::to_string_pretty(&catalog).unwrap()).unwrap();
}

/// The id of the first catalog item expected to succeed (its output is
/// written by the generated suites).
fn first_success_id(pack: &Path) -> String {
    let path = pack.join("fixtures").join("catalog.json");
    let catalog: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    catalog["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["expect"] == "success")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn catalog_ids_outside_the_safe_set_are_refused() {
    let escape = format!("{}x", '\u{1b}');
    let long = "a".repeat(129);
    for hostile in [
        "../../escape",
        "/abs/escape",
        "a b",
        escape.as_str(),
        "",
        long.as_str(),
        "..",
        "C:escape",
        "sub/escape",
        "sub\\escape",
    ] {
        let pack = pack_copy("rename-user-id");
        edit_catalog(&pack, |c| c["items"][0]["id"] = hostile.into());
        let err = load_catalog(&pack.join("fixtures").join("catalog.json"))
            .expect_err("a hostile catalog id must be refused");
        assert!(
            err.to_string().contains("catalog item id"),
            "{hostile:?}: {err}"
        );
    }
}

/// The attack: a catalog id that walks out of the generated suite's
/// temporary folder. Generation refuses it, and nothing is written outside.
#[test]
fn generated_node_suite_cannot_write_outside_its_folder() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping: node is not installed");
        return;
    }
    let pack = pack_copy("rename-user-id");
    let target = first_success_id(&pack);
    let marker = format!("cfp-escape-{}", std::process::id());
    let escaped = std::env::temp_dir().join(format!("{marker}.after.json"));
    let _ = fs::remove_file(&escaped);
    edit_catalog(&pack, |c| {
        for item in c["items"].as_array_mut().unwrap() {
            if item["id"] == target.as_str() {
                item["id"] = format!("../{marker}").into();
            }
        }
    });
    let out_dir = pack.parent().unwrap().join("generated");
    let generate = run(&[
        "generate",
        "--kind",
        "jest",
        "--pack",
        s(&pack),
        "--out",
        s(&out_dir),
    ]);
    if generate.status.success() {
        let _ = Command::new("node")
            .arg("--test")
            .arg(out_dir.join("pack.test.js"))
            .env("CFP_PACK", &pack)
            .env("CFP_BIN", bin())
            .output()
            .unwrap();
    }
    let written = escaped.exists();
    let _ = fs::remove_file(&escaped);
    assert!(!written, "the generated suite wrote {}", escaped.display());
    assert_eq!(generate.status.code(), Some(3), "{}", stderr(&generate));
}

/// Generated suites name each output file by the fixture's position, never by
/// its id, so even an id that slipped through could not become a path.
#[test]
fn generated_suites_name_output_files_by_position() {
    let pack = root().join("packs").join("rename-user-id");
    let out_dir = scratch("by-position");
    for kind in ["jest", "pytest"] {
        let out = run(&[
            "generate",
            "--kind",
            kind,
            "--pack",
            s(&pack),
            "--out",
            s(&out_dir),
        ]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    let node = fs::read_to_string(out_dir.join("pack.test.js")).unwrap();
    assert!(!node.contains("id + '.after.json'"), "{node}");
    assert!(node.contains("'fixture-' + position + '.after.json'"));
    let python = fs::read_to_string(out_dir.join("test_pack.py")).unwrap();
    assert!(!python.contains("fixture_id + \".after.json\""), "{python}");
    assert!(python.contains("\"fixture-%d.after.json\" % position"));
}

/// Review and approve a pack directory in place; returns approve's output.
fn approve_pack(pack: &Path, migration: &Path) -> Output {
    let common: Vec<String> = vec![
        "--proposal".into(),
        s(&pack.join("proposal.json")).into(),
        "--source".into(),
        s(&pack.join("base.openapi.yaml")).into(),
        "--destination".into(),
        s(&pack.join("next.openapi.yaml")).into(),
    ];
    let record = pack.join("review.record.json");
    let mut review = vec!["review".to_string()];
    review.extend(common.iter().cloned());
    review.extend(["--out".into(), s(&record).into()]);
    let r = Command::new(bin()).args(&review).output().unwrap();
    assert!(r.status.success(), "{}", stderr(&r));
    let mut approve = vec!["approve".to_string()];
    approve.extend(common.iter().cloned());
    approve.extend([
        "--fixtures".into(),
        s(&pack.join("fixtures")).into(),
        "--review".into(),
        s(&record).into(),
        "--reviewer".into(),
        "local-reviewer".into(),
        "--approved-at".into(),
        "2026-09-30T00:00:00Z".into(),
        "--out".into(),
        s(migration).into(),
    ]);
    Command::new(bin()).args(&approve).output().unwrap()
}

fn verify_pack(pack: &Path, migration: &Path, out: &Path) -> Output {
    run(&[
        "verify",
        "--migration",
        s(migration),
        "--proposal",
        s(&pack.join("proposal.json")),
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

#[test]
fn an_empty_catalog_is_refused_at_approve() {
    let pack = pack_copy("rename-user-id");
    edit_catalog(&pack, |c| c["items"] = serde_json::json!([]));
    let migration = pack.join("migration.json");
    let out = approve_pack(&pack, &migration);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("no fixtures"), "{}", stderr(&out));
    assert!(!migration.exists());
}

#[test]
fn a_rule_without_a_fixture_is_refused_at_approve() {
    let pack = pack_copy("rename-user-id");
    edit_catalog(&pack, |c| {
        let items = c["items"].as_array_mut().unwrap();
        items.retain(|i| i["rule_ids"][0] == "rename-user-request");
    });
    let migration = pack.join("migration.json");
    let out = approve_pack(&pack, &migration);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("rename-user-response"),
        "{}",
        stderr(&out)
    );
    assert!(!migration.exists());
}

/// After approval the catalog is emptied and every digest in the migration
/// recomputed; verify still refuses to call a run without fixtures stable.
#[test]
fn verify_refuses_a_catalog_emptied_after_approval() {
    use contract_flightplan::approve::{approval_digest, load_migration, write_json};
    use contract_flightplan::digest::file_digest;
    let pack = pack_copy("rename-user-id");
    let migration = pack.join("migration.json");
    let out = approve_pack(&pack, &migration);
    assert!(out.status.success(), "{}", stderr(&out));
    let catalog = pack.join("fixtures").join("catalog.json");
    edit_catalog(&pack, |c| c["items"] = serde_json::json!([]));
    let mut forged = load_migration(&migration).unwrap();
    let digest = file_digest(&catalog).unwrap();
    forged
        .fixtures
        .insert("catalog.json".into(), digest.clone());
    forged
        .approval
        .bound_digests
        .fixtures
        .insert("catalog.json".into(), digest);
    let committed = approval_digest(&forged.approval.bound_digests).unwrap();
    for event in forged.history.iter_mut().filter(|e| e.action == "approved") {
        event.digest = committed.clone();
    }
    write_json(&migration, &forged).unwrap();
    let report = pack.join("verify.json");
    let out = verify_pack(&pack, &migration, &report);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("no fixtures"), "{}", stderr(&out));
    assert!(!report.exists());
}

/// Two catalog items with one id would be indistinguishable in reports, SARIF
/// and generated suites; the catalog is refused when it is read.
#[test]
fn duplicate_catalog_ids_are_refused() {
    let pack = pack_copy("rename-user-id");
    edit_catalog(&pack, |c| {
        for item in c["items"].as_array_mut().unwrap() {
            item["id"] = "same".into();
        }
    });
    let err = load_catalog(&pack.join("fixtures").join("catalog.json"))
        .expect_err("duplicate ids must be refused");
    assert!(err.to_string().contains("\"same\""), "{err}");
    assert!(err.to_string().contains("more than once"), "{err}");
    let migration = pack.join("migration.json");
    let out = approve_pack(&pack, &migration);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(!migration.exists());
}
