use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use contract_flightplan::approve::{approve, write_json};
use contract_flightplan::diff::compare_paths;
use contract_flightplan::digest::{read_bounded_file, MAX_BYTES};
use contract_flightplan::error::Error;
use contract_flightplan::parse::load_openapi;
use contract_flightplan::proposal::load_proposal;
use contract_flightplan::verify::{review_proposal, verify_paths};

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

fn pack(name: &str) -> PathBuf {
    root().join("packs").join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn approve_pack(name: &str) -> (PathBuf, PathBuf) {
    let src = pack(name);
    let unique = format!(
        "cfp-it-{name}-{}-{}",
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
    );
    let tmp = std::env::temp_dir().join(unique);
    let _ = fs::remove_dir_all(&tmp);
    copy_dir(&src, &tmp);
    let migration_path = tmp.join("approval.migration.json");
    let migration = approve(
        &tmp.join("proposal.json"),
        &tmp.join("base.openapi.yaml"),
        &tmp.join("next.openapi.yaml"),
        &tmp.join("fixtures"),
        &review_record(&tmp),
        "local-reviewer",
        Some("integration"),
        Some("2026-09-18T00:00:00Z"),
    )
    .unwrap();
    write_json(&migration_path, &migration).unwrap();
    (tmp, migration_path)
}

#[test]
fn three_packs_verify() {
    for name in [
        "rename-user-id",
        "default-status",
        "enum-map-color",
        "copy-display-name",
        "drop-legacy-flag",
        "require-email",
        "safe-cast-count",
        "unit-mass-grams",
        "split-full-name",
        "merge-display",
        "array-tags",
        "page-envelope",
    ] {
        let (dir, migration) = approve_pack(name);
        let report = verify_paths(
            &migration,
            &dir.join("proposal.json"),
            &dir.join("base.openapi.yaml"),
            &dir.join("next.openapi.yaml"),
            &dir.join("fixtures"),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.stable, "{name} should be stable");
        let again = verify_paths(
            &migration,
            &dir.join("proposal.json"),
            &dir.join("base.openapi.yaml"),
            &dir.join("next.openapi.yaml"),
            &dir.join("fixtures"),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&report.outcomes).unwrap(),
            serde_json::to_value(&again.outcomes).unwrap()
        );
    }
}

#[test]
fn shipped_approval_files_verify_against_their_packs() {
    let mut seen = 0;
    for entry in fs::read_dir(root().join("packs")).unwrap() {
        let dir = entry.unwrap().path();
        let migration = dir.join("approval.migration.json");
        if !migration.is_file() {
            continue;
        }
        let report = verify_paths(
            &migration,
            &dir.join("proposal.json"),
            &dir.join("base.openapi.yaml"),
            &dir.join("next.openapi.yaml"),
            &dir.join("fixtures"),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        assert!(report.stable, "{} must be stable", dir.display());
        seen += 1;
    }
    assert!(
        seen >= 4,
        "expected the shipped approval files, found {seen}"
    );
}

#[test]
fn failing_fixture_returns_full_report_with_field_location() {
    let (dir, _) = approve_pack("rename-user-id");
    // Re-bind the approval to a catalog that expects the wrong outcome so the
    // digests match and the failure is a real outcome mismatch.
    let catalog_path = dir.join("fixtures/catalog.json");
    let mut catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    catalog["items"][0]["expect"] = serde_json::json!("collision");
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let migration = approve(
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
        &review_record(&dir),
        "local-reviewer",
        None,
        Some("2026-09-22T00:00:00Z"),
    )
    .unwrap();
    let migration_path = dir.join("rebound.migration.json");
    write_json(&migration_path, &migration).unwrap();
    let report = contract_flightplan::verify::verify_paths_report(
        &migration_path,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap();
    assert!(!report.stable);
    let failing: Vec<_> = report.outcomes.iter().filter(|o| !o.passed).collect();
    assert_eq!(failing.len(), 1);
    assert_eq!(failing[0].expect, "collision");
    assert_eq!(failing[0].actual, "success");
    assert!(
        report.outcomes.len() > 1,
        "every outcome is reported, not just the first failure"
    );
    let err = verify_paths(
        &migration_path,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::UnexpectedOutcome { .. }));

    // A real mapping failure carries the field and its position in the fixture.
    let collision = report
        .outcomes
        .iter()
        .find(|o| o.actual == "collision")
        .expect("collision fixture");
    assert_eq!(collision.field.as_deref(), Some("user_id"));
    let location = collision.location.as_ref().expect("located field");
    assert!(location.line >= 1 && location.column >= 1);
}

#[test]
fn mutating_catalog_expect_fails_verify() {
    let (dir, migration) = approve_pack("rename-user-id");
    let catalog_path = dir.join("fixtures/catalog.json");
    let mut catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    catalog["items"][0]["expect"] = serde_json::json!("collision");
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let err = verify_paths(
        &migration,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            Error::StaleDigest { .. } | Error::UnexpectedOutcome { .. }
        ),
        "{err:?}"
    );
}

#[test]
fn unapproved_proposal_cannot_verify() {
    let dir = pack("rename-user-id");
    let err = verify_paths(
        &dir.join("proposal.json"),
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Unapproved));
}

#[test]
fn injected_approval_is_rejected() {
    let err = load_proposal(&root().join("fixtures/negatives/injected-approval.proposal.json"))
        .unwrap_err();
    assert!(matches!(err, Error::InjectedApproval));
}

#[test]
fn unknown_mapping_operation_is_rejected() {
    let err =
        load_proposal(&root().join("fixtures/negatives/unknown-op.proposal.json")).unwrap_err();
    assert!(matches!(err, Error::UnknownMappingOperation { .. }));
}

#[test]
fn stale_rule_or_fixture_or_contract_is_rejected() {
    let (dir, migration) = approve_pack("rename-user-id");
    fs::write(dir.join("fixtures/extra.json"), "{}").unwrap();
    let err = verify_paths(
        &migration,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::StaleDigest { .. }));

    let (dir, migration) = approve_pack("rename-user-id");
    let mut yaml = fs::read_to_string(dir.join("base.openapi.yaml")).unwrap();
    yaml.push('\n');
    fs::write(dir.join("base.openapi.yaml"), yaml).unwrap();
    let err = verify_paths(
        &migration,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::StaleDigest { .. }));

    let (dir, migration_path) = approve_pack("rename-user-id");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&migration_path).unwrap()).unwrap();
    value["rules"][0]["evidence"] = serde_json::json!("tampered");
    fs::write(
        &migration_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let err = verify_paths(
        &migration_path,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::StaleDigest { .. }));
}

#[test]
fn unsupported_and_ambiguous_and_refs() {
    let negatives = root().join("fixtures/negatives");
    let diff = compare_paths(
        &negatives.join("unsupported-allof.openapi.yaml"),
        &negatives.join("unsupported-allof.openapi.yaml"),
    )
    .unwrap();
    assert!(
        diff.unsupported
            .iter()
            .any(|n| n.construct == "allOf" || n.message.contains("allOf")),
        "allOf must remain visible: {:?}",
        diff.unsupported
    );

    let err = compare_paths(
        &negatives.join("ambiguous-operation.openapi.yaml"),
        &negatives.join("ambiguous-operation.openapi.yaml"),
    )
    .unwrap_err();
    assert!(matches!(err, Error::AmbiguousOperation { .. }));

    let err = load_openapi(&negatives.join("remote-ref.openapi.yaml")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }));
    assert_eq!(err.outcome_name(), "unsupported");

    let err = load_openapi(&negatives.join("traversal-ref.openapi.yaml")).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }));

    let diff = compare_paths(
        &negatives.join("recursive.openapi.yaml"),
        &negatives.join("recursive.openapi.yaml"),
    )
    .unwrap();
    assert!(diff
        .unsupported
        .iter()
        .any(|n| n.construct.contains("recursive") || n.message.contains("cycle")));

    let json_doc = load_openapi(&negatives.join("json-contract.openapi.json")).unwrap();
    assert_eq!(json_doc.openapi_version, "3.1.0");

    let mixed = compare_paths(
        &negatives.join("mixed-media.openapi.yaml"),
        &negatives.join("mixed-media.openapi.yaml"),
    )
    .unwrap();
    assert!(
        mixed
            .unsupported
            .iter()
            .any(|n| n.construct == "media_type" && n.message.contains("xml")),
        "extra media types must stay visible: {:?}",
        mixed.unsupported
    );
}

#[test]
fn resource_bound_rejects_oversize_input() {
    let path = std::env::temp_dir().join(format!("cfp-oversize-{}.json", std::process::id()));
    let payload = vec![b'x'; MAX_BYTES + 8];
    fs::write(&path, payload).unwrap();
    let err = read_bounded_file(&path).unwrap_err();
    assert!(matches!(err, Error::Bound { .. }));
}

#[test]
fn review_explains_rules_without_approving() {
    let dir = pack("enum-map-color");
    let review = review_proposal(
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
    )
    .unwrap();
    assert_eq!(review.approval_state, "not_approved");
    assert!(review.rules.iter().any(|r| r.summary.contains("unmapped")));
}

#[test]
fn html_escapes_script_fixture() {
    let (dir, migration) = approve_pack("rename-user-id");
    let verify = verify_paths(
        &migration,
        &dir.join("proposal.json"),
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        &dir.join("fixtures"),
    )
    .unwrap();
    let report = contract_flightplan::report::build_report(
        &dir.join("base.openapi.yaml"),
        &dir.join("next.openapi.yaml"),
        Some(&dir.join("proposal.json")),
        Some(&migration),
        Some(&dir.join("fixtures")),
        Some(&verify),
    )
    .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(html.contains("href=\"#summary\""));
    assert!(html.contains("href=\"#fixtures\""));
}

#[test]
fn cli_help_lists_real_commands_only() {
    let output = Command::new(env!("CARGO_BIN_EXE_cfp"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("compare"));
    assert!(text.contains("review"));
    assert!(text.contains("approve"));
    assert!(text.contains("verify"));
    assert!(text.contains("report"));
    assert!(text.contains("propose"));
    assert!(text.contains("generate"));
    assert!(text.contains("corpus"));
}

#[test]
fn oasdiff_array_and_object_shapes_are_ingested() {
    let negatives = root().join("fixtures/negatives");
    let wrapped =
        contract_flightplan::oasdiff::ingest_oasdiff(&negatives.join("oasdiff-sample.json"))
            .unwrap();
    assert_eq!(wrapped[0].identity, "request-body-changed");
    let array = contract_flightplan::oasdiff::ingest_oasdiff(&negatives.join("oasdiff-array.json"))
        .unwrap();
    assert_eq!(array[0].identity, "request-body-changed");
    assert_eq!(array[0].summary, "POST /users request body");
}
