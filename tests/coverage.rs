//! Fixture metadata, generated boundary cases, and the checked-in coverage
//! matrix of operations against verified outcome kinds.

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
        "cfp-coverage-{name}-{}-{}",
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

fn cfp(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn catalog_items_require_metadata() {
    let mut items = 0;
    for entry in fs::read_dir(root().join("packs")).unwrap() {
        let catalog = entry.unwrap().path().join("fixtures/catalog.json");
        if !catalog.is_file() {
            continue;
        }
        let value: Value = serde_json::from_str(&fs::read_to_string(&catalog).unwrap()).unwrap();
        for item in value["items"].as_array().unwrap() {
            let where_ = format!("{} {}", catalog.display(), item["id"]);
            assert!(
                item["description"].as_str().is_some_and(|d| d.len() >= 10),
                "{where_}: description"
            );
            assert!(
                item["version"].as_str().is_some_and(|v| !v.is_empty()),
                "{where_}: version"
            );
            items += 1;
        }
    }
    assert!(items >= 70, "{items} catalog items");

    // A catalog item without its metadata is refused at approve.
    let dir = scratch("metadata");
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    let catalog_path = dir.join("fixtures/catalog.json");
    let mut catalog: Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    catalog["items"][0]
        .as_object_mut()
        .unwrap()
        .remove("description");
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let (proposal, base, next) = (
        dir.join("proposal.json"),
        dir.join("base.openapi.yaml"),
        dir.join("next.openapi.yaml"),
    );
    let common = [
        "--proposal",
        s(&proposal),
        "--source",
        s(&base),
        "--destination",
        s(&next),
    ];
    let record = dir.join("review.json");
    let mut review = vec!["review"];
    review.extend(common);
    review.extend(["--out", s(&record)]);
    assert!(cfp(&review).status.success());
    let migration = dir.join("migration.json");
    let mut approve = vec!["approve"];
    approve.extend(common);
    let fixtures = dir.join("fixtures");
    approve.extend([
        "--fixtures",
        s(&fixtures),
        "--review",
        s(&record),
        "--reviewer",
        "r",
        "--out",
        s(&migration),
    ]);
    let out = cfp(&approve);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("description"), "{err}");
    assert!(!migration.exists());
}

#[test]
fn generated_fixtures_include_boundary_cases() {
    let dir = scratch("boundary");
    let contract = dir.join("contract.json");
    fs::write(
        &contract,
        serde_json::to_string_pretty(&json!({
            "openapi": "3.0.3",
            "info": {"title": "Boundaries", "version": "1"},
            "paths": {"/things": {"post": {
                "operationId": "createThing",
                "requestBody": {"content": {"application/json": {"schema": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {"type": "string"},
                        "count": {"type": "integer"},
                        "tags": {"type": "array", "items": {"type": "string"}}
                    }
                }}}},
                "responses": {"200": {"description": "ok"}}
            }}}
        }))
        .unwrap(),
    )
    .unwrap();
    let out = cfp(&[
        "fixtures",
        "--source",
        s(&contract),
        "--method",
        "POST",
        "--path",
        "/things",
        "--location",
        "request",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let generated: Value = serde_json::from_slice(&out.stdout).unwrap();
    let items = generated["items"].as_array().unwrap();
    let find = |id: &str| {
        items
            .iter()
            .find(|i| i["id"] == id)
            .unwrap_or_else(|| panic!("no {id}: {items:#?}"))
    };
    assert_eq!(find("generated-boundary-name-empty")["body"]["name"], "");
    assert_eq!(find("generated-boundary-count-zero")["body"]["count"], 0);
    assert_eq!(
        find("generated-boundary-count-max")["body"]["count"],
        i64::MAX
    );
    assert_eq!(
        find("generated-boundary-tags-empty")["body"]["tags"],
        json!([])
    );
    assert_eq!(
        find("generated-boundary-tags-at-limit")["body"]["tags"]
            .as_array()
            .unwrap()
            .len(),
        256
    );
    let over = find("generated-boundary-tags-over-limit");
    assert_eq!(over["body"]["tags"].as_array().unwrap().len(), 257);
    assert_eq!(over["expect"], "bound");

    // Every generated expectation is what validation actually reports.
    let doc = contract_flightplan::parse::load_openapi(&contract).unwrap();
    let ops = contract_flightplan::parse::operations(&doc).unwrap();
    let schema = &ops[0].request.as_ref().unwrap().resolved;
    for item in items {
        let actual =
            match contract_flightplan::schema::validate_instance(schema, &item["body"], "x") {
                Ok(()) => "success".to_string(),
                Err(err) => err.outcome_name().to_string(),
            };
        assert_eq!(item["expect"], actual.as_str(), "{}", item["id"]);
    }
}

fn corpus_with_matrix(name: &str) -> (Value, String) {
    let dir = scratch(name);
    let matrix = dir.join("matrix.md");
    let out = cfp(&[
        "corpus",
        "--packs",
        s(&root().join("packs")),
        "--matrix",
        s(&matrix),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        serde_json::from_slice(&out.stdout).unwrap(),
        fs::read_to_string(&matrix).unwrap(),
    )
}

#[test]
fn coverage_matrix_has_no_empty_required_cell() {
    let (report, matrix) = corpus_with_matrix("required");
    let rows = report["coverage"].as_array().expect("coverage rows");
    assert_eq!(rows.len(), 13, "one row per mapping operation: {rows:#?}");
    for row in rows {
        assert_eq!(row["missing"], json!([]), "{row:#}");
        assert!(row["counts"]["success"].as_u64().unwrap() >= 1, "{row:#}");
    }
    assert!(!matrix.contains("MISSING:"), "{matrix}");
    assert!(matrix.contains("| rename |"), "{matrix}");
}

#[test]
fn checked_in_matrix_matches_generated() {
    let (_, matrix) = corpus_with_matrix("checked-in");
    let checked_in = fs::read_to_string(root().join("docs/COVERAGE-MATRIX.md")).unwrap();
    assert_eq!(
        checked_in.replace("\r\n", "\n"),
        matrix,
        "regenerate with: cfp corpus --packs packs --matrix docs/COVERAGE-MATRIX.md"
    );
}

/// `cfp corpus` works in a fresh, uniquely named temporary folder and removes
/// it afterwards, so two runs never share files and nothing is left behind.
#[test]
fn corpus_uses_a_unique_temp_dir_and_cleans_up() {
    let tmp = std::env::temp_dir().join(format!("cfp-corpus-tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cfp"))
        .args(["corpus", "--packs"])
        .arg(root.join("packs"))
        .env("TMP", &tmp)
        .env("TEMP", &tmp)
        .env("TMPDIR", &tmp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "corpus left {left:?} in the temp folder");
    let _ = std::fs::remove_dir_all(&tmp);
}
