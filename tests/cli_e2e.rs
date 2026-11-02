use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin()).args(args).output().unwrap()
}

#[test]
fn complete_rename_path() {
    let pack = root().join("packs/rename-user-id");
    let out = std::env::temp_dir().join(format!("cfp-cli-rename-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let source = pack.join("base.openapi.yaml");
    let dest = pack.join("next.openapi.yaml");
    let proposal = pack.join("proposal.json");
    let fixtures = pack.join("fixtures");

    let compare = run(&[
        "compare",
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--out",
        out.join("diff.json").to_str().unwrap(),
    ]);
    assert!(
        compare.status.success(),
        "{}",
        String::from_utf8_lossy(&compare.stderr)
    );

    let review = run(&[
        "review",
        "--proposal",
        proposal.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--out",
        out.join("review.json").to_str().unwrap(),
    ]);
    assert!(
        review.status.success(),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );

    let approve = run(&[
        "approve",
        "--proposal",
        proposal.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--fixtures",
        fixtures.to_str().unwrap(),
        "--review",
        out.join("review.json").to_str().unwrap(),
        "--reviewer",
        "local-reviewer",
        "--approved-at",
        "2026-09-18T00:00:00Z",
        "--out",
        out.join("migration.json").to_str().unwrap(),
    ]);
    assert!(
        approve.status.success(),
        "{}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let verify = run(&[
        "verify",
        "--migration",
        out.join("migration.json").to_str().unwrap(),
        "--proposal",
        proposal.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--fixtures",
        fixtures.to_str().unwrap(),
        "--out",
        out.join("verify.json").to_str().unwrap(),
    ]);
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );

    let report = run(&[
        "report",
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--proposal",
        proposal.to_str().unwrap(),
        "--migration",
        out.join("migration.json").to_str().unwrap(),
        "--fixtures",
        fixtures.to_str().unwrap(),
        "--verify",
        out.join("verify.json").to_str().unwrap(),
        "--html",
        out.join("report.html").to_str().unwrap(),
        "--json",
        out.join("report.json").to_str().unwrap(),
    ]);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let html = fs::read_to_string(out.join("report.html")).unwrap();
    assert!(html.contains("Skip to content"));
    assert!(html.contains("Fixture outcomes"));
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
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

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-e2e-{name}-{}-{}",
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

fn write(path: &std::path::Path, text: &str) {
    fs::write(path, text).unwrap();
}

/// Source declares `id: string`; destination declares `count: integer`.
/// Renaming id -> count is well-formed but ill-typed.
fn ill_typed_pack(dir: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
    let doc = |field: &str, ty: &str| {
        format!(
            "openapi: 3.0.3\ninfo:\n  title: T\n  version: '1'\npaths:\n  /x:\n    post:\n      operationId: x\n      requestBody:\n        content:\n          application/json:\n            schema:\n              type: object\n              additionalProperties: false\n              required: [{field}]\n              properties:\n                {field}:\n                  type: {ty}\n      responses:\n        '200':\n          description: ok\n"
        )
    };
    let source = dir.join("base.openapi.yaml");
    let dest = dir.join("next.openapi.yaml");
    let proposal = dir.join("proposal.json");
    write(&source, &doc("id", "string"));
    write(&dest, &doc("count", "integer"));
    write(
        &proposal,
        r#"{
  "schema_version": "1", "kind": "proposal", "id": "ill-typed",
  "source_contract": {"path": "base.openapi.yaml"}, "destination_contract": {"path": "next.openapi.yaml"},
  "rules": [{
    "id": "r1", "operation": "rename",
    "source": {"operation": {"method": "POST", "path": "/x", "operation_id": "x"}, "location": "request", "field": "id"},
    "destination": {"operation": {"method": "POST", "path": "/x", "operation_id": "x"}, "location": "request", "field": "count"},
    "parameters": {"from": "id", "to": "count"},
    "preconditions": [], "postconditions": [], "evidence": "test",
    "lossiness": "none", "reversibility": "reversible", "fixture_refs": []
  }]
}"#,
    );
    let fixtures = dir.join("fixtures");
    fs::create_dir_all(&fixtures).unwrap();
    write(&fixtures.join("positive.json"), r#"{"id": "1"}"#);
    write(
        &fixtures.join("catalog.json"),
        r#"{"schema_version": "1", "items": [{"id": "positive", "path": "positive.json", "rule_ids": ["r1"], "expect": "success", "description": "Request with a string id.", "version": "1"}]}"#,
    );
    (source, dest, proposal)
}

#[test]
fn review_writes_diagnostics_and_exits_three_for_ill_typed_proposal() {
    let dir = scratch("illtyped");
    let (source, dest, proposal) = ill_typed_pack(&dir);
    let out = dir.join("review.json");
    let review = run(&[
        "review",
        "--proposal",
        proposal.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        review.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    let text = fs::read_to_string(&out).expect("review must still be written");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["typecheck"], "failed");
    assert!(value["blocking_diagnostics"].as_u64().unwrap() >= 1);
    let messages: Vec<String> = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("string") && m.contains("integer")),
        "{messages:?}"
    );

    let approve = run(&[
        "approve",
        "--proposal",
        proposal.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--fixtures",
        dir.join("fixtures").to_str().unwrap(),
        "--review",
        out.to_str().unwrap(),
        "--reviewer",
        "local-reviewer",
        "--out",
        dir.join("migration.json").to_str().unwrap(),
    ]);
    assert_eq!(approve.status.code(), Some(3));
    assert!(
        !dir.join("migration.json").exists(),
        "ill-typed proposals never become migrations"
    );
    assert!(String::from_utf8_lossy(&approve.stderr).contains("type-check failed"));
}

#[test]
fn verify_failure_writes_report_and_sarif_carries_rule_and_region() {
    let dir = scratch("verifyfail");
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    let catalog_path = dir.join("fixtures/catalog.json");
    let mut catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    catalog["items"][0]["expect"] = serde_json::json!("collision");
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let source = dir.join("base.openapi.yaml");
    let dest = dir.join("next.openapi.yaml");
    let migration = dir.join("migration.json");
    let record = dir.join("review.json");
    let review = run(&[
        "review",
        "--proposal",
        dir.join("proposal.json").to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--out",
        record.to_str().unwrap(),
    ]);
    assert!(review.status.success());
    let approve = run(&[
        "approve",
        "--proposal",
        dir.join("proposal.json").to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--fixtures",
        dir.join("fixtures").to_str().unwrap(),
        "--review",
        record.to_str().unwrap(),
        "--reviewer",
        "local-reviewer",
        "--approved-at",
        "2026-09-22T00:00:00Z",
        "--out",
        migration.to_str().unwrap(),
    ]);
    assert!(
        approve.status.success(),
        "{}",
        String::from_utf8_lossy(&approve.stderr)
    );

    let verify_out = dir.join("verify.json");
    let verify = run(&[
        "verify",
        "--migration",
        migration.to_str().unwrap(),
        "--proposal",
        dir.join("proposal.json").to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--fixtures",
        dir.join("fixtures").to_str().unwrap(),
        "--out",
        verify_out.to_str().unwrap(),
    ]);
    assert_eq!(verify.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&verify_out).expect("verify.json written on failure"),
    )
    .unwrap();
    assert_eq!(report["stable"], false);
    assert!(report["outcomes"].as_array().unwrap().len() > 1);

    let sarif_missing = run(&[
        "report",
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--html",
        dir.join("r1.html").to_str().unwrap(),
        "--json",
        dir.join("r1.json").to_str().unwrap(),
        "--sarif",
        dir.join("r1.sarif").to_str().unwrap(),
    ]);
    assert_eq!(
        sarif_missing.status.code(),
        Some(3),
        "--sarif without a verification run must fail loudly"
    );
    assert!(!dir.join("r1.sarif").exists());
    assert!(String::from_utf8_lossy(&sarif_missing.stderr).contains("--migration"));

    let sarif_path = dir.join("report.sarif");
    let report_cmd = run(&[
        "report",
        "--source",
        source.to_str().unwrap(),
        "--destination",
        dest.to_str().unwrap(),
        "--migration",
        migration.to_str().unwrap(),
        "--proposal",
        dir.join("proposal.json").to_str().unwrap(),
        "--fixtures",
        dir.join("fixtures").to_str().unwrap(),
        "--verify",
        verify_out.to_str().unwrap(),
        "--html",
        dir.join("report.html").to_str().unwrap(),
        "--json",
        dir.join("report.json").to_str().unwrap(),
        "--md",
        dir.join("report.md").to_str().unwrap(),
        "--job-summary",
        dir.join("summary.md").to_str().unwrap(),
        "--sarif",
        sarif_path.to_str().unwrap(),
    ]);
    assert!(
        report_cmd.status.success(),
        "{}",
        String::from_utf8_lossy(&report_cmd.stderr)
    );
    let sarif: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&sarif_path).unwrap()).unwrap();
    let run0 = &sarif["runs"][0];
    let results = run0["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "one failing fixture, one result");
    assert_eq!(results[0]["ruleId"], "cfp.fixture.success");
    let rules = run0["tool"]["driver"]["rules"].as_array().unwrap();
    assert!(rules.iter().any(|r| r["id"] == "cfp.fixture.success"));
    assert_eq!(
        results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "request.positive.json"
    );
    assert_eq!(run0["invocations"][0]["executionSuccessful"], false);
    let summary = fs::read_to_string(dir.join("summary.md")).unwrap();
    assert!(summary.contains("stable: no"));
    assert!(summary.contains("Failing fixtures"));
    let html = fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(html.contains("id=\"uncertainty\""));
    // Absolute CLI paths must not be disclosed in the recorded outputs.
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["diff"]["source"]["path"], "base.openapi.yaml");
    assert_eq!(
        json["migration"]["source_contract"]["path"],
        "base.openapi.yaml"
    );
    assert!(!fs::read_to_string(&sarif_path)
        .unwrap()
        .contains(dir.to_str().unwrap()));
}

#[test]
fn provider_preview_select_shows_redacted_shape_and_stays_blocked() {
    let fixture = root().join("packs/rename-user-id/fixtures/request.positive.json");
    let output = run(&[
        "provider",
        "--preview",
        "openai",
        "--select",
        fixture.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["blocked"], true);
    assert_eq!(value["outbound_request_issued"], false);
    assert_eq!(value["would_send"][0]["file"], "request.positive.json");
    assert!(value["would_send"][0]["shape"].is_object());
    let raw = fs::read_to_string(&fixture).unwrap();
    let raw_value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let preview_text = String::from_utf8_lossy(&output.stdout);
    for (_, v) in raw_value.as_object().unwrap() {
        if let Some(s) = v.as_str() {
            assert!(
                !preview_text.contains(&format!("\"{s}\"")),
                "value {s} leaked"
            );
        }
    }
}

/// The generated Node test suite actually drives the binary. Skips (loudly)
/// when `node` is not installed so the Rust suite stays runnable everywhere.
#[test]
fn generated_node_tests_pass_against_every_shipped_pack() {
    let node = Command::new("node").arg("--version").output();
    let Ok(node) = node else {
        eprintln!("skipping: node is not installed");
        return;
    };
    assert!(node.status.success());
    let bin = env!("CARGO_BIN_EXE_cfp");
    let mut checked = 0;
    for entry in fs::read_dir(root().join("packs")).unwrap() {
        let pack = entry.unwrap().path();
        if !pack.join("proposal.json").is_file() {
            continue;
        }
        let out = scratch(&format!(
            "node-{}",
            pack.file_name().unwrap().to_string_lossy()
        ));
        let generate = run(&[
            "generate",
            "--kind",
            "jest",
            "--pack",
            pack.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(
            generate.status.success(),
            "{}",
            String::from_utf8_lossy(&generate.stderr)
        );
        let test_file = out.join("pack.test.js");
        let text = fs::read_to_string(&test_file).unwrap();
        assert!(
            !text.contains(root().to_str().unwrap()),
            "generated test embeds a local path"
        );
        let result = Command::new("node")
            .arg("--test")
            .arg(&test_file)
            .env("CFP_PACK", &pack)
            .env("CFP_BIN", bin)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}:\n{}\n{}",
            pack.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        checked += 1;
    }
    assert!(checked >= 4);
}

#[test]
fn verify_proposal_exits_failure() {
    let pack = root().join("packs/rename-user-id");
    let output = run(&[
        "verify",
        "--migration",
        pack.join("proposal.json").to_str().unwrap(),
        "--proposal",
        pack.join("proposal.json").to_str().unwrap(),
        "--source",
        pack.join("base.openapi.yaml").to_str().unwrap(),
        "--destination",
        pack.join("next.openapi.yaml").to_str().unwrap(),
        "--fixtures",
        pack.join("fixtures").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn remote_ref_exits_unsupported() {
    let path = root().join("fixtures/negatives/remote-ref.openapi.yaml");
    let output = run(&[
        "compare",
        "--source",
        path.to_str().unwrap(),
        "--destination",
        path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn propose_file_validates_local_proposal() {
    let pack = root().join("packs/rename-user-id");
    let output = run(&[
        "propose",
        "--mode",
        "file",
        "--proposal",
        pack.join("proposal.json").to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("rename-user-id"));
}

#[test]
fn propose_none_does_not_generate() {
    let output = run(&["propose", "--mode", "none"]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn provider_preview_is_blocked() {
    let output = run(&["provider", "--preview", "openai"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("\"blocked\": true"));
}

#[test]
fn fixtures_and_reject_and_history() {
    let pack = root().join("packs/rename-user-id");
    let fixtures = run(&[
        "fixtures",
        "--source",
        pack.join("base.openapi.yaml").to_str().unwrap(),
        "--method",
        "POST",
        "--path",
        "/users",
        "--location",
        "request",
    ]);
    assert!(
        fixtures.status.success(),
        "{}",
        String::from_utf8_lossy(&fixtures.stderr)
    );
    let generated = String::from_utf8_lossy(&fixtures.stdout);
    assert!(generated.contains("generated-positive"));
    assert!(generated.contains("coverage"));

    let out = std::env::temp_dir().join(format!("cfp-reject-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let reject = run(&[
        "reject",
        "--proposal",
        pack.join("proposal.json").to_str().unwrap(),
        "--reviewer",
        "local-reviewer",
        "--reason",
        "unsafe mapping",
        "--out",
        out.join("rejection.json").to_str().unwrap(),
    ]);
    assert!(
        reject.status.success(),
        "{}",
        String::from_utf8_lossy(&reject.stderr)
    );
    let rejection = fs::read_to_string(out.join("rejection.json")).unwrap();
    assert!(rejection.contains("\"kind\": \"rejection\""));

    let review = run(&[
        "review",
        "--proposal",
        pack.join("proposal.json").to_str().unwrap(),
        "--source",
        pack.join("base.openapi.yaml").to_str().unwrap(),
        "--destination",
        pack.join("next.openapi.yaml").to_str().unwrap(),
        "--out",
        out.join("review.json").to_str().unwrap(),
    ]);
    assert!(review.status.success());
    let approve = run(&[
        "approve",
        "--proposal",
        pack.join("proposal.json").to_str().unwrap(),
        "--source",
        pack.join("base.openapi.yaml").to_str().unwrap(),
        "--destination",
        pack.join("next.openapi.yaml").to_str().unwrap(),
        "--fixtures",
        pack.join("fixtures").to_str().unwrap(),
        "--review",
        out.join("review.json").to_str().unwrap(),
        "--reviewer",
        "local-reviewer",
        "--approved-at",
        "2026-09-18T00:00:00Z",
        "--out",
        out.join("migration.json").to_str().unwrap(),
    ]);
    assert!(
        approve.status.success(),
        "{}",
        String::from_utf8_lossy(&approve.stderr)
    );
    let history = run(&[
        "history",
        "--migration",
        out.join("migration.json").to_str().unwrap(),
    ]);
    assert!(
        history.status.success(),
        "{}",
        String::from_utf8_lossy(&history.stderr)
    );
    let hist = String::from_utf8_lossy(&history.stdout);
    assert!(hist.contains("approved"));
}
