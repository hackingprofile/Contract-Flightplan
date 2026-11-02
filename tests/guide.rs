//! `cfp guide` walks one person through compare, propose, review, approve,
//! verify and report, says who suggested the mapping, and approves nothing
//! without an explicit `y` (or `--yes`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-guide-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `cfp guide` on the rename pack with `extra` arguments and `stdin` as input.
fn guide(name: &str, extra: &[&str], stdin: &str) -> (Output, PathBuf) {
    let pack = root().join("packs/rename-user-id");
    let out_dir = scratch(name);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cfp"))
        .arg("guide")
        .arg("--source")
        .arg(pack.join("base.openapi.yaml"))
        .arg("--destination")
        .arg(pack.join("next.openapi.yaml"))
        .arg("--fixtures")
        .arg(pack.join("fixtures"))
        .arg("--out-dir")
        .arg(&out_dir)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    (child.wait_with_output().unwrap(), out_dir)
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn proposal_arg() -> String {
    root()
        .join("packs/rename-user-id/proposal.json")
        .display()
        .to_string()
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn guide_walks_every_step_and_approves_on_y() {
    let proposal = proposal_arg();
    let (out, dir) = guide(
        "yes",
        &[
            "--proposal",
            &proposal,
            "--reviewer",
            "Reviewer One",
            "--approved-at",
            "2026-10-30T09:00:00Z",
        ],
        "y\n",
    );
    let t = text(&out);
    assert_eq!(out.status.code(), Some(0), "{t}");
    for n in 1..=6 {
        assert!(t.contains(&format!("Step {n} of 6")), "step {n}: {t}");
    }
    assert!(
        t.contains("Suggested by: a person, in the proposal file proposal.json"),
        "{t}"
    );
    assert!(t.contains("Approve this mapping? [y/N]"), "{t}");
    assert!(
        t.contains("Approved by Reviewer One at 2026-10-30T09:00:00Z"),
        "{t}"
    );
    assert!(t.contains("7 of 7 example(s) passed"), "{t}");
    assert!(t.contains("Verdict: stable"), "{t}");
    let migration = json(&dir.join("migration.json"));
    assert_eq!(migration["approval"]["reviewer"], "Reviewer One");
    assert_eq!(migration["approval"]["approved_at"], "2026-10-30T09:00:00Z");
    for file in [
        "diff.json",
        "review.json",
        "verify.json",
        "report.html",
        "report.json",
        "report.md",
    ] {
        assert!(dir.join(file).is_file(), "{file} missing");
    }
}

/// The guide ends with the report path and the verdict on two lines of
/// their own; the report line carries no escaped `\n` and no verdict.
#[test]
fn guide_ends_with_the_report_path_and_the_verdict_on_separate_lines() {
    let proposal = proposal_arg();
    let (out, _dir) = guide(
        "last-lines",
        &["--proposal", &proposal, "--reviewer", "r", "--yes"],
        "",
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // (A Windows path may contain `\n` itself, as in `...\next.openapi.yaml`.)
    assert!(
        !stdout.contains("\\nVerdict"),
        "an escaped line break: {stdout}"
    );
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    let [.., report, verdict] = lines.as_slice() else {
        panic!("fewer than two lines: {stdout}");
    };
    assert!(
        report.starts_with("Report: ") && report.ends_with("report.html"),
        "{report:?}"
    );
    assert_eq!(*verdict, "Verdict: stable.");
}

#[test]
fn guide_approves_nothing_without_an_explicit_yes() {
    let proposal = proposal_arg();
    for (name, answer) in [
        ("enter", "\n"),
        ("no", "n\n"),
        ("other", "sure\n"),
        ("eof", ""),
    ] {
        let (out, dir) = guide(
            name,
            &["--proposal", &proposal, "--reviewer", "Reviewer One"],
            answer,
        );
        let t = text(&out);
        assert_eq!(out.status.code(), Some(1), "{name}: {t}");
        assert!(t.contains("Approve this mapping? [y/N]"), "{name}: {t}");
        assert!(t.contains("Not approved"), "{name}: {t}");
        assert!(!dir.join("migration.json").exists(), "{name} approved");
        assert!(!dir.join("verify.json").exists(), "{name} verified");
    }
}

#[test]
fn guide_names_the_built_in_proposer_when_no_file_is_given() {
    let (out, dir) = guide("builtin", &["--reviewer", "Reviewer One"], "n\n");
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(t.contains("Suggested by: the built-in proposer"), "{t}");
    assert!(t.contains("rename"), "{t}");
    let proposal = json(&dir.join("proposal.json"));
    assert_eq!(proposal["kind"], "proposal");
    assert!(!dir.join("migration.json").exists());
}

#[test]
fn guide_asks_for_the_reviewer_and_records_name_and_time() {
    let proposal = proposal_arg();
    let (out, dir) = guide(
        "ask-name",
        &["--proposal", &proposal],
        "Second Reviewer\ny\n",
    );
    let t = text(&out);
    assert_eq!(out.status.code(), Some(0), "{t}");
    assert!(t.contains("Your name, recorded as the reviewer:"), "{t}");
    let migration = json(&dir.join("migration.json"));
    assert_eq!(migration["approval"]["reviewer"], "Second Reviewer");
    let at = migration["approval"]["approved_at"].as_str().unwrap();
    assert!(at.len() == 20 && at.ends_with('Z'), "{at}");
}

#[test]
fn guide_without_input_needs_the_flags() {
    let proposal = proposal_arg();
    // No input and no --reviewer: nothing to record, nothing approved.
    let (out, dir) = guide("no-input", &["--proposal", &proposal], "");
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains("--reviewer"));
    assert!(!dir.join("migration.json").exists());
    // Non-interactive with the flags: --reviewer and --yes.
    let (out, dir) = guide(
        "flags",
        &["--proposal", &proposal, "--reviewer", "ci", "--yes"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(text(&out).contains("given with --yes"));
    assert_eq!(
        json(&dir.join("migration.json"))["approval"]["reviewer"],
        "ci"
    );
}
