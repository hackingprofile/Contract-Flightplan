//! Binding checks: SARIF without placeholder links, generated
//! suites with one test per catalog item and no local paths, the `validate`
//! subcommand they use, and the review workflow's pull-request trigger.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-binding-{name}-{}-{}",
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

fn catalog_ids(pack: &Path) -> Vec<String> {
    let catalog: Value =
        serde_json::from_str(&fs::read_to_string(pack.join("fixtures/catalog.json")).unwrap())
            .unwrap();
    catalog["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn sarif_has_no_placeholder_uri() {
    let dir = scratch("sarif");
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    let catalog_path = dir.join("fixtures/catalog.json");
    let text = fs::read_to_string(&catalog_path).unwrap();
    fs::write(
        &catalog_path,
        text.replacen("\"expect\": \"success\"", "\"expect\": \"collision\"", 1),
    )
    .unwrap();
    let (base, next) = (dir.join("base.openapi.yaml"), dir.join("next.openapi.yaml"));
    let common = ["--source", s(&base), "--destination", s(&next)];
    let proposal = dir.join("proposal.json");
    let record = dir.join("review.json");
    let migration = dir.join("migration.json");
    let verify_out = dir.join("verify.json");
    let sarif = dir.join("report.sarif");
    let mut args = vec!["review", "--proposal", s(&proposal)];
    args.extend(common);
    args.extend(["--out", s(&record)]);
    assert!(cfp(&args).status.success());
    let fixtures = dir.join("fixtures");
    let mut args = vec!["approve", "--proposal", s(&proposal)];
    args.extend(common);
    args.extend([
        "--fixtures",
        s(&fixtures),
        "--review",
        s(&record),
        "--reviewer",
        "r",
        "--out",
        s(&migration),
    ]);
    assert!(cfp(&args).status.success());
    let mut args = vec![
        "verify",
        "--migration",
        s(&migration),
        "--proposal",
        s(&proposal),
    ];
    args.extend(common);
    args.extend(["--fixtures", s(&fixtures), "--out", s(&verify_out)]);
    assert_eq!(cfp(&args).status.code(), Some(1));
    let html = dir.join("r.html");
    let json = dir.join("r.json");
    let mut args = vec!["report"];
    args.extend(common);
    args.extend([
        "--migration",
        s(&migration),
        "--proposal",
        s(&proposal),
        "--fixtures",
        s(&fixtures),
        "--verify",
        s(&verify_out),
        "--html",
        s(&html),
        "--json",
        s(&json),
        "--sarif",
        s(&sarif),
    ]);
    assert!(cfp(&args).status.success());
    let text = fs::read_to_string(&sarif).unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert!(!value["runs"][0]["results"].as_array().unwrap().is_empty());
    for banned in ["example.invalid", "example.com", "http://", "file://"] {
        assert!(!text.contains(banned), "SARIF contains {banned}: {text}");
    }
    assert!(
        allowed_schema_only(&text),
        "the only absolute link is the SARIF $schema: {text}"
    );
    for result in value["runs"][0]["results"].as_array().unwrap() {
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert!(
            !uri.contains(':') && !uri.starts_with('/'),
            "{uri} is not relative"
        );
    }
}

/// The only absolute link SARIF may carry is its own format's `$schema`.
fn allowed_schema_only(text: &str) -> bool {
    text.lines()
        .filter(|l| l.contains("https://"))
        .all(|l| l.trim_start().starts_with("\"$schema\""))
}

fn generated(kind: &str, pack: &Path) -> PathBuf {
    let out = scratch(&format!("generate-{kind}"));
    let run = cfp(&[
        "generate",
        "--kind",
        kind,
        "--pack",
        s(pack),
        "--out",
        s(&out),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    out
}

fn assert_no_local_paths(text: &str, pack: &Path, file: &str) {
    let pack_text = pack.display().to_string();
    assert!(!text.contains(&pack_text), "{file} embeds the pack path");
    assert!(
        !text.contains(&pack_text.replace('\\', "\\\\")),
        "{file} embeds the pack path"
    );
    assert!(
        !text.contains(&pack_text.replace('\\', "/")),
        "{file} embeds the pack path"
    );
    let drive = text
        .as_bytes()
        .windows(3)
        .any(|w| w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/'));
    assert!(!drive, "{file} embeds a drive-letter path");
}

#[test]
fn generated_jest_has_one_test_per_catalog_item_and_no_absolute_paths() {
    let pack = root().join("packs/rename-user-id");
    let out = generated("jest", &pack);
    let ids = catalog_ids(&pack);
    for file in ["pack.test.js", "pack.jest.test.js"] {
        let text = fs::read_to_string(out.join(file)).unwrap();
        assert_no_local_paths(&text, &pack, file);
        for id in &ids {
            let call = format!("checkFixture({})", serde_json::to_string(id).unwrap());
            assert_eq!(text.matches(&call).count(), 1, "{file}: one test for {id}");
        }
        assert_eq!(
            text.matches("checkFixture(\"").count(),
            ids.len(),
            "{file}: no extra fixture tests"
        );
        assert!(
            text.contains("'validate'"),
            "{file}: success outputs are validated"
        );
    }
}

#[test]
fn generated_pytest_has_one_test_per_catalog_item_and_no_absolute_paths() {
    let pack = root().join("packs/rename-user-id");
    let out = generated("pytest", &pack);
    let ids = catalog_ids(&pack);
    let file = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "py"))
        .expect("a Python module");
    let text = fs::read_to_string(&file).unwrap();
    assert_no_local_paths(&text, &pack, "python module");
    for id in &ids {
        let call = format!("self.check_fixture({})", serde_json::to_string(id).unwrap());
        assert_eq!(text.matches(&call).count(), 1, "one test for {id}");
    }
    assert_eq!(text.matches("    def test_fixture_").count(), ids.len());
    assert!(
        text.contains("\"validate\""),
        "success outputs are validated"
    );
}

#[test]
fn validate_subcommand_accepts_positive_and_rejects_malformed() {
    let dir = scratch("validate");
    let next = root().join("packs/rename-user-id/next.openapi.yaml");
    let good = dir.join("good.json");
    let bad = dir.join("bad.json");
    fs::write(&good, r#"{"user_id": "user-1", "name": "Ada"}"#).unwrap();
    fs::write(&bad, r#"{"user_id": 7, "name": "Ada"}"#).unwrap();
    let run = |payload: &Path| {
        cfp(&[
            "validate",
            "--schema-of",
            s(&next),
            "--method",
            "POST",
            "--path",
            "/users",
            "--location",
            "request",
            "--payload",
            s(payload),
        ])
    };
    let ok = run(&good);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let value: Value = serde_json::from_slice(&ok.stdout).unwrap();
    assert_eq!(value["valid"], true);
    let refused = run(&bad);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("malformed"));
    let missing = run(&dir.join("absent.json"));
    assert_eq!(missing.status.code(), Some(3));
}

#[test]
fn review_workflow_triggers_on_pull_request() {
    let workflow = fs::read_to_string(root().join(".github/workflows/contract-flightplan.yml"))
        .expect("the review workflow");
    let on_block: Vec<&str> = workflow
        .lines()
        .skip_while(|l| !l.starts_with("on:"))
        .skip(1)
        .take_while(|l| l.starts_with(' ') || l.is_empty())
        .collect();
    assert!(
        on_block
            .iter()
            .any(|l| l.trim_start().starts_with("pull_request:")),
        "{on_block:?}"
    );
    assert!(
        on_block
            .iter()
            .any(|l| l.trim_start().starts_with("workflow_dispatch:")),
        "{on_block:?}"
    );
}
