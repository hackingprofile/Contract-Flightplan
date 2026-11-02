//! The published schemas under schemas/v1 must be true: every shipped pack,
//! every example, and every CLI output the code produces has to validate
//! against them, and the shared rule definitions must not drift between files.

use contract_flightplan::approve::approve;
use contract_flightplan::error::Error;
use contract_flightplan::proposal::{load_proposal, parse_proposal_value};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The review record `cfp review --out` writes for a pack's proposal, saved
/// to a fresh temporary file (approve requires one).
fn review_record(pack: &Path) -> PathBuf {
    let review = contract_flightplan::verify::review_proposal(
        &pack.join("proposal.json"),
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "cfp-review-record-{}-{}.json",
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
    contract_flightplan::approve::write_json(&path, &review).unwrap();
    path
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn schema(name: &str) -> Value {
    let path = root().join(format!("schemas/v1/{name}.schema.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn validator(name: &str) -> jsonschema::Validator {
    jsonschema::validator_for(&schema(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn assert_valid(validator: &jsonschema::Validator, instance: &Value, what: &str) {
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{what} violates its schema: {errors:#?}");
}

fn load(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn valid_proposal_example_parses_and_validates() {
    let path = root().join("schemas/v1/examples/valid-proposal.json");
    load_proposal(&path).unwrap();
    assert_valid(
        &validator("proposal"),
        &load(&path),
        "valid-proposal example",
    );
}

#[test]
fn invalid_injected_approval_example_is_rejected() {
    let path = root().join("schemas/v1/examples/invalid-injected-approval.json");
    let err = load_proposal(&path).unwrap_err();
    assert!(matches!(err, Error::InjectedApproval));
    assert!(!validator("proposal").is_valid(&load(&path)));
}

#[test]
fn invalid_diff_kind_is_not_a_proposal() {
    let path = root().join("schemas/v1/examples/invalid-diff-kind.json");
    let value = load(&path);
    assert!(parse_proposal_value(&value).is_err());
    assert!(!validator("proposal").is_valid(&value));
}

#[test]
fn public_schema_files_exist_and_compile() {
    for name in ["diff", "proposal", "migration", "report"] {
        let value = schema(name);
        assert_eq!(value["properties"]["schema_version"]["const"], "1");
        validator(name);
    }
}

#[test]
fn shared_rule_definitions_do_not_drift_between_schemas() {
    let proposal = schema("proposal");
    let migration = schema("migration");
    let proposal_defs = proposal["$defs"].as_object().unwrap();
    let migration_defs = migration["$defs"].as_object().unwrap();
    for (name, def) in migration_defs {
        assert_eq!(
            proposal_defs.get(name),
            Some(def),
            "$defs/{name} differs between proposal and migration schemas"
        );
    }
    assert!(proposal_defs.contains_key("pathHint"));
}

#[test]
fn schema_operation_enum_matches_the_interpreter() {
    let value = schema("migration");
    let ops: Vec<String> = value["$defs"]["rule"]["properties"]["operation"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let mut variants = 0;
    for variant in value["$defs"]["rule"]["oneOf"].as_array().unwrap() {
        let op = variant["properties"]["operation"]["const"]
            .as_str()
            .unwrap();
        assert!(
            ops.contains(&op.to_string()),
            "variant {op} missing from enum"
        );
        variants += 1;
    }
    assert_eq!(
        variants,
        ops.len(),
        "every operation needs a parameter variant"
    );
    for op in &ops {
        let decoded = contract_flightplan::proposal::decode_mapping(op, &serde_json::json!({}));
        assert!(
            !matches!(decoded, Err(Error::UnknownMappingOperation { .. })),
            "schema lists {op} but the interpreter does not know it"
        );
    }
    for unknown in ["eval", "template", "lookup"] {
        assert!(matches!(
            contract_flightplan::proposal::decode_mapping(unknown, &serde_json::json!({})),
            Err(Error::UnknownMappingOperation { .. })
        ));
        assert!(!ops.contains(&unknown.to_string()));
    }
    let lossiness: Vec<&str> = value["$defs"]["rule"]["properties"]["lossiness"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        lossiness,
        contract_flightplan::typecheck::LOSSINESS_VOCABULARY
    );
    let reversibility: Vec<&str> = value["$defs"]["rule"]["properties"]["reversibility"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        reversibility,
        contract_flightplan::typecheck::REVERSIBILITY_VOCABULARY
    );
}

#[test]
fn every_pack_proposal_validates_against_the_published_schema() {
    let proposal_schema = validator("proposal");
    let mut seen = 0;
    for entry in std::fs::read_dir(root().join("packs")).unwrap() {
        let pack = entry.unwrap().path();
        if !pack.is_dir() {
            continue;
        }
        let path = pack.join("proposal.json");
        assert_valid(&proposal_schema, &load(&path), &path.display().to_string());
        seen += 1;
    }
    assert!(seen >= 12, "expected at least 12 packs, found {seen}");
}

#[test]
fn approve_output_validates_against_the_migration_schema() {
    let migration_schema = validator("migration");
    let pack = root().join("packs/rename-user-id");
    let migration = approve(
        &pack.join("proposal.json"),
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        &pack.join("fixtures"),
        &review_record(&pack),
        "schema-test",
        Some("schema contract"),
        Some("2026-09-22T00:00:00Z"),
    )
    .unwrap();
    let value = serde_json::to_value(&migration).unwrap();
    assert_valid(&migration_schema, &value, "approve output");
    assert!(
        !validator("proposal").is_valid(&value),
        "a migration is not a proposal"
    );
}

#[test]
fn shipped_approval_files_validate_against_the_migration_schema() {
    let migration_schema = validator("migration");
    let mut seen = 0;
    for entry in std::fs::read_dir(root().join("packs")).unwrap() {
        let path = entry.unwrap().path().join("approval.migration.json");
        if path.is_file() {
            assert_valid(&migration_schema, &load(&path), &path.display().to_string());
            seen += 1;
        }
    }
    assert!(seen >= 1);
}

#[test]
fn compare_and_report_outputs_validate_against_their_schemas() {
    let pack = root().join("packs/rename-user-id");
    let diff = contract_flightplan::diff::compare_paths(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
    )
    .unwrap();
    assert_valid(
        &validator("diff"),
        &serde_json::to_value(&diff).unwrap(),
        "compare output",
    );
    let report = contract_flightplan::report::build_report(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        Some(&pack.join("proposal.json")),
        None,
        None,
        None,
    )
    .unwrap();
    assert_valid(
        &validator("report"),
        &serde_json::to_value(&report).unwrap(),
        "report output",
    );
}

#[test]
fn schema_rejects_what_the_interpreter_rejects() {
    let proposal_schema = validator("proposal");
    let base = load(&root().join("packs/split-full-name/proposal.json"));
    assert_valid(&proposal_schema, &base, "split pack");

    let mut duplicate = base.clone();
    duplicate["rules"][0]["parameters"]["into"] = serde_json::json!(["a", "a"]);
    assert!(
        !proposal_schema.is_valid(&duplicate),
        "duplicate split targets"
    );
    assert!(parse_proposal_value(&duplicate).is_err());

    let mut unknown_op = base.clone();
    unknown_op["rules"][0]["operation"] = serde_json::json!("eval");
    assert!(!proposal_schema.is_valid(&unknown_op));
    assert!(parse_proposal_value(&unknown_op).is_err());

    let mut wrong_params = base.clone();
    wrong_params["rules"][0]["parameters"] = serde_json::json!({"from": "a", "to": "b"});
    assert!(
        !proposal_schema.is_valid(&wrong_params),
        "split with rename params"
    );
    assert!(parse_proposal_value(&wrong_params).is_err());

    let mut bad_vocab = base.clone();
    bad_vocab["rules"][0]["lossiness"] = serde_json::json!("maybe");
    assert!(!proposal_schema.is_valid(&bad_vocab));

    let mut injected = base;
    injected["approval"] = serde_json::json!({"reviewer": "sneaky"});
    assert!(!proposal_schema.is_valid(&injected));
}

fn schema_enum(schema: &Value, pointer: &str) -> Vec<String> {
    schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("diff schema has no enum at {pointer}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn serialized<T: serde::Serialize>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .map(|v| {
            serde_json::to_value(v)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn diff_schema_describes_classified_changes_and_matches_the_code() {
    use contract_flightplan::diff::{ChangeKind, Compatibility};
    let diff_schema = schema("diff");
    assert_eq!(
        schema_enum(&diff_schema, "/$defs/change/properties/kind/enum"),
        serialized(&ChangeKind::ALL)
    );
    assert_eq!(
        schema_enum(&diff_schema, "/$defs/compatibility/enum"),
        serialized(&Compatibility::ALL)
    );

    // A diff with nested classified changes validates; a body that drops its
    // compatibility or a change that drops its pointer does not.
    let pack = root().join("packs/rename-user-id");
    let diff = contract_flightplan::diff::compare_paths(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
    )
    .unwrap();
    let value = serde_json::to_value(&diff).unwrap();
    let validator = validator("diff");
    assert_valid(&validator, &value, "compare output");
    let request = "/operations/0/request";
    assert!(
        value.pointer(&format!("{request}/changes/0")).is_some(),
        "the rename pack has request changes: {value:#}"
    );
    let mut no_class = value.clone();
    no_class
        .pointer_mut(request)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("compatibility");
    assert!(!validator.is_valid(&no_class), "body without compatibility");
    let mut no_pointer = value.clone();
    no_pointer
        .pointer_mut(&format!("{request}/changes/0"))
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("pointer");
    assert!(!validator.is_valid(&no_pointer), "change without pointer");
    let mut bad_class = value;
    *bad_class
        .pointer_mut(&format!("{request}/changes/0/compatibility"))
        .unwrap() = serde_json::json!("probably_fine");
    assert!(
        !validator.is_valid(&bad_class),
        "unknown compatibility class"
    );
}

fn examples() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root().join("schemas/v1/examples")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        out.push((name, load(&path)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn interpreter_operations() -> Vec<String> {
    contract_flightplan::corpus::OPERATIONS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn every_valid_example_validates() {
    let validator = validator("proposal");
    let mut covered = Vec::new();
    for (name, value) in examples() {
        let Some(op) = name
            .strip_prefix("valid-")
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if op == "proposal" {
            continue;
        }
        assert_valid(&validator, &value, &name);
        let proposal = parse_proposal_value(&value).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(proposal.rules.len(), 1, "{name}: one rule per example");
        assert_eq!(proposal.rules[0].operation, op.replace('-', "_"), "{name}");
        covered.push(proposal.rules[0].operation.clone());
    }
    covered.sort();
    let mut expected = interpreter_operations();
    expected.sort();
    assert_eq!(covered, expected, "one valid example per operation");
}

#[test]
fn every_invalid_example_is_rejected() {
    let validator = validator("proposal");
    let mut seen = Vec::new();
    for (name, value) in examples() {
        if !name.starts_with("invalid-") {
            continue;
        }
        assert!(!validator.is_valid(&value), "{name} passes the schema");
        assert!(parse_proposal_value(&value).is_err(), "{name} parses");
        seen.push(name);
    }
    for needed in [
        "invalid-unknown-operation.json",
        "invalid-duplicate-split-targets.json",
        "invalid-missing-parameters.json",
        "invalid-wrong-parameter-type.json",
        "invalid-injected-approval.json",
    ] {
        assert!(
            seen.iter().any(|s| s == needed),
            "missing {needed}: {seen:?}"
        );
    }
}

#[test]
fn mapping_doc_examples_validate() {
    let doc = std::fs::read_to_string(root().join("docs/mapping.md")).unwrap();
    let blocks: Vec<&str> = doc
        .split("```json")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap())
        .collect();
    let validator = validator("proposal");
    let mut operations = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let rule: Value =
            serde_json::from_str(block).unwrap_or_else(|e| panic!("block {i}: {e}\n{block}"));
        let proposal = serde_json::json!({
            "schema_version": "1",
            "kind": "proposal",
            "id": "mapping-doc-example",
            "source_contract": {"path": "base.yaml"},
            "destination_contract": {"path": "next.yaml"},
            "rules": [rule]
        });
        assert_valid(&validator, &proposal, &format!("docs/mapping.md block {i}"));
        parse_proposal_value(&proposal).unwrap_or_else(|e| panic!("block {i}: {e}"));
        operations.push(
            proposal["rules"][0]["operation"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    operations.sort();
    let mut expected = interpreter_operations();
    expected.sort();
    assert_eq!(
        operations, expected,
        "one complete rule example per operation"
    );
}
