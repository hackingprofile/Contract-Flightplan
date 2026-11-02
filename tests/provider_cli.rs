//! The provider boundary from the command line: a deterministic local example
//! provider that shows its alternatives and assumptions, review and report
//! that render them, quarantine for malformed proposals, and hosted providers
//! that stay blocked after a consent preview.

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
        "cfp-provider-{name}-{}-{}",
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

fn cfp(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn contracts() -> (PathBuf, PathBuf) {
    let negatives = root().join("fixtures/negatives");
    (
        negatives.join("example-provider.base.openapi.yaml"),
        negatives.join("example-provider.next.openapi.yaml"),
    )
}

fn example_proposal() -> Output {
    let (base, next) = contracts();
    cfp(&[
        "propose",
        "--mode",
        "example",
        "--source",
        s(&base),
        "--destination",
        s(&next),
    ])
}

#[test]
fn example_local_provider_is_deterministic_across_runs() {
    let first = example_proposal();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = example_proposal();
    assert_eq!(first.stdout, second.stdout, "byte-identical across runs");
    let proposal: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(proposal["kind"], "proposal");
    assert!(
        proposal.get("approval").is_none(),
        "a proposal never carries approval"
    );
    assert_eq!(
        proposal["rules"].as_array().unwrap().len(),
        2,
        "{proposal:#}"
    );
}

#[test]
fn example_local_provider_lists_alternatives_and_assumptions() {
    let out = example_proposal();
    let proposal: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rules = proposal["rules"].as_array().unwrap();
    let pairs: Vec<(String, String)> = rules
        .iter()
        .map(|r| {
            (
                r["parameters"]["from"].as_str().unwrap().to_string(),
                r["parameters"]["to"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(pairs.contains(&("age".into(), "years".into())), "{pairs:?}");
    let id_rule = rules
        .iter()
        .find(|r| r["parameters"]["from"] == "id")
        .expect("a candidate for the removed id");
    let alternatives = proposal["alternatives"].as_array().expect("alternatives");
    let for_id: Vec<&Value> = alternatives
        .iter()
        .filter(|a| a["rule_id"] == id_rule["id"])
        .collect();
    assert_eq!(for_id.len(), 1, "{alternatives:#?}");
    assert_eq!(for_id[0]["operation"], "rename");
    assert_ne!(for_id[0]["parameters"]["to"], id_rule["parameters"]["to"]);
    assert!(for_id[0]["why"].as_str().unwrap().len() > 10);
    let assumptions = proposal["assumptions"].as_array().expect("assumptions");
    assert!(!assumptions.is_empty());
    // The provider's output is an ordinary proposal: review accepts it.
    let dir = scratch("example-review");
    let path = dir.join("proposal.json");
    fs::write(&path, &out.stdout).unwrap();
    let (base, next) = contracts();
    let review = cfp(&[
        "review",
        "--proposal",
        s(&path),
        "--source",
        s(&base),
        "--destination",
        s(&next),
    ]);
    assert!(
        review.status.success(),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
}

#[test]
fn review_renders_alternatives_and_ambiguity_nonempty_for_example_pack() {
    let dir = scratch("render");
    let path = dir.join("proposal.json");
    fs::write(&path, example_proposal().stdout).unwrap();
    let (base, next) = contracts();
    let record = dir.join("review.json");
    let out = cfp(&[
        "review",
        "--proposal",
        s(&path),
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--out",
        s(&record),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let review: Value = serde_json::from_str(&fs::read_to_string(&record).unwrap()).unwrap();
    assert!(
        !review["alternatives"].as_array().unwrap().is_empty(),
        "{review:#}"
    );
    assert!(
        !review["assumptions"].as_array().unwrap().is_empty(),
        "{review:#}"
    );
    let ambiguity = review["ambiguity"].as_array().unwrap();
    assert!(
        ambiguity.iter().any(|a| a.as_str().unwrap().contains("id")),
        "{ambiguity:#?}"
    );
    let html = dir.join("report.html");
    let out = cfp(&[
        "report",
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--proposal",
        s(&path),
        "--html",
        s(&html),
        "--json",
        s(&dir.join("report.json")),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let html = fs::read_to_string(html).unwrap();
    assert!(html.contains("Alternatives"), "report renders alternatives");
    assert!(html.contains("Assumptions"), "report renders assumptions");
    assert!(html.contains("Ambiguity"), "report renders ambiguity");
}

#[test]
fn hosted_provider_still_blocked_after_preview() {
    let fixture = root().join("packs/rename-user-id/fixtures/request.positive.json");
    let preview = cfp(&["provider", "--preview", "openai", "--select", s(&fixture)]);
    assert!(preview.status.success());
    let value: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(value["blocked"], true);
    assert_eq!(value["outbound_request_issued"], false);
    let dir = scratch("hosted");
    for mode in ["openai", "anthropic"] {
        let out_file = dir.join(format!("{mode}.json"));
        let out = cfp(&["propose", "--mode", mode, "--out", s(&out_file)]);
        assert_eq!(out.status.code(), Some(2), "{mode}");
        assert!(!out_file.exists(), "{mode} wrote a proposal");
    }
}

#[test]
fn malformed_proposal_is_quarantined_with_diagnostics() {
    let dir = scratch("quarantine");
    let quarantine = dir.join("quarantine");
    let bad = dir.join("bad.json");
    let source = fs::read_to_string(root().join("packs/rename-user-id/proposal.json")).unwrap();
    let malformed = source.replacen(
        "\"kind\": \"proposal\",",
        "\"kind\": \"proposal\", \"kind\": \"proposal\",",
        1,
    );
    assert_ne!(source, malformed);
    fs::write(&bad, &malformed).unwrap();
    let out = cfp(&[
        "propose",
        "--mode",
        "file",
        "--proposal",
        s(&bad),
        "--quarantine",
        s(&quarantine),
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut names: Vec<String> = fs::read_dir(&quarantine)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    let digest = names[0].trim_end_matches(".diagnostics.json").to_string();
    assert_eq!(digest.len(), 64, "{names:?}");
    let copy = fs::read(quarantine.join(format!("{digest}.json"))).unwrap();
    assert_eq!(
        copy,
        malformed.as_bytes(),
        "the quarantined copy is the input as given"
    );
    let diagnostics: Value = serde_json::from_str(
        &fs::read_to_string(quarantine.join(format!("{digest}.diagnostics.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(diagnostics["sha256"], format!("sha256:{digest}"));
    assert_eq!(diagnostics["file"], "bad.json");
    let text = diagnostics["diagnostics"].to_string();
    assert!(text.contains("duplicate key"), "{diagnostics:#}");

    // A valid proposal is not quarantined.
    let good_dir = dir.join("good-quarantine");
    let out = cfp(&[
        "propose",
        "--mode",
        "file",
        "--proposal",
        s(&root().join("packs/rename-user-id/proposal.json")),
        "--quarantine",
        s(&good_dir),
    ]);
    assert!(out.status.success());
    assert!(!good_dir.exists() || fs::read_dir(&good_dir).unwrap().next().is_none());
}
