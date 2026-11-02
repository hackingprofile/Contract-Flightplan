//! The lossiness witness is computed by executing each rule on the pack's
//! successful fixtures, not echoed from the proposal: it lists the fields
//! actually discarded, flags rounding, computes reversibility, and reports
//! where the declaration claims more than the evidence shows.

use std::fs;
use std::path::{Path, PathBuf};

use contract_flightplan::generate::lossiness_witness;
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

/// A copy of a shipped pack (same directory name) with its rules edited.
fn edited_pack(name: &str, tag: &str, edit: impl Fn(&mut Value)) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("cfp-lossiness-{tag}-{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&root().join("packs").join(name), &dir);
    let path = dir.join("proposal.json");
    let mut proposal: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for rule in proposal["rules"].as_array_mut().unwrap() {
        edit(rule);
    }
    fs::write(&path, serde_json::to_string_pretty(&proposal).unwrap()).unwrap();
    dir
}

fn witness(pack: &Path) -> Value {
    serde_json::to_value(lossiness_witness(pack).unwrap()).unwrap()
}

fn first_rule(w: &Value) -> &Value {
    &w["rules"][0]
}

#[test]
fn lossiness_witness_lists_merge_sources() {
    let w = witness(&root().join("packs/merge-display"));
    let rule = first_rule(&w);
    assert_eq!(rule["operation"], "merge");
    assert_eq!(
        rule["discarded_fields"],
        json!(["first_name", "last_name"]),
        "{rule:#}"
    );
    assert_eq!(rule["observed_lossiness"], json!(["merged"]), "{rule:#}");
    assert_ne!(rule["computed_reversibility"], "reversible", "{rule:#}");
    // Nothing is listed that execution did not remove.
    let rename = witness(&root().join("packs/rename-user-id"));
    assert_eq!(
        first_rule(&rename)["discarded_fields"],
        json!([]),
        "a move is not a discard"
    );
    assert_eq!(first_rule(&rename)["computed_reversibility"], "reversible");
}

#[test]
fn lossiness_witness_flags_rounding() {
    let dir = edited_pack("unit-mass-grams", "rounding", |rule| {
        rule["parameters"]["factor_num"] = json!(1);
        rule["parameters"]["factor_den"] = json!(3);
        rule["parameters"]["rounding"] = json!("round");
        rule["lossiness"] = json!("none");
        rule["reversibility"] = json!("reversible");
    });
    let w = witness(&dir);
    let rule = first_rule(&w);
    assert_eq!(rule["rounded_fields"], json!(["mass"]), "{rule:#}");
    assert_eq!(rule["observed_lossiness"], json!(["rounded"]), "{rule:#}");
    assert_eq!(rule["computed_reversibility"], "irreversible", "{rule:#}");
    assert_eq!(w["consistent"], false);

    // The shipped exact conversion rounds nothing.
    let exact = witness(&root().join("packs/unit-mass-grams"));
    assert_eq!(first_rule(&exact)["rounded_fields"], json!([]));
    assert_eq!(first_rule(&exact)["computed_reversibility"], "reversible");
}

#[test]
fn lossiness_witness_reports_declaration_mismatch() {
    let dir = edited_pack("drop-legacy-flag", "mismatch", |rule| {
        rule["lossiness"] = json!("none");
        rule["reversibility"] = json!("reversible");
    });
    let w = witness(&dir);
    assert_eq!(w["consistent"], false);
    let rule = first_rule(&w);
    let rows: Vec<String> = rule["declaration_mismatch"]
        .as_array()
        .unwrap_or_else(|| panic!("no declaration_mismatch: {rule:#}"))
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    assert!(rows.iter().any(|r| r.contains("lossiness")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("reversib")), "{rows:?}");
    assert_eq!(rule["computed_reversibility"], "irreversible");

    // Every shipped pack's declaration matches what its fixtures show.
    for entry in fs::read_dir(root().join("packs")).unwrap() {
        let pack = entry.unwrap().path();
        let w = witness(&pack);
        assert_eq!(w["consistent"], true, "{}: {w:#}", pack.display());
        for rule in w["rules"].as_array().unwrap() {
            assert_eq!(rule["declaration_mismatch"], json!([]), "{rule:#}");
        }
    }
}
