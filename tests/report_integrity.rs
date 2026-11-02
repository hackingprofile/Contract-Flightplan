//! Reports open with the run's verdict, and untrusted text (reviewer, note,
//! catalog ids, diagnostics) cannot forge or hide it in the Markdown report,
//! the job summary or the HTML report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-report-{name}-{}-{}",
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

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A hostile reviewer string: it tries to print a passing
/// count and a stable verdict, then opens an HTML comment to hide the rest.
const FORGING_REVIEWER: &str = "a at now\n- fixtures: 7/7 passed\n- stable: yes\n<!--";
const FORGING_NOTE: &str = "note\n- stable: yes\n<script>alert(1)</script>";

struct Outputs {
    markdown: String,
    summary: String,
    html: String,
}

/// Approve the rename pack with the forging reviewer and note, make exactly
/// one of its seven fixtures fail, verify, and render every report format.
fn forged_run(name: &str, with_verify: bool) -> Outputs {
    forged_run_with(name, with_verify, FORGING_REVIEWER, FORGING_NOTE)
}

fn forged_run_with(name: &str, with_verify: bool, reviewer: &str, note: &str) -> Outputs {
    let dir = scratch(name);
    copy_dir(&root().join("packs/rename-user-id"), &dir);
    let catalog_path = dir.join("fixtures/catalog.json");
    let mut catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    let items = catalog["items"].as_array_mut().unwrap();
    assert_eq!(items.len(), 7, "the rename pack has 7 fixtures");
    let item = items
        .iter_mut()
        .find(|i| i["id"] == "request-positive")
        .unwrap();
    item["expect"] = serde_json::json!("collision");
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let source = dir.join("base.openapi.yaml");
    let dest = dir.join("next.openapi.yaml");
    let fixtures = dir.join("fixtures");
    let migration = dir.join("migration.json");
    let record = dir.join("review.json");
    let review = run(&[
        "review",
        "--proposal",
        s(&dir.join("proposal.json")),
        "--source",
        s(&source),
        "--destination",
        s(&dest),
        "--out",
        s(&record),
    ]);
    assert!(
        review.status.success(),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    let approve = run(&[
        "approve",
        "--review",
        s(&record),
        "--proposal",
        s(&dir.join("proposal.json")),
        "--source",
        s(&source),
        "--destination",
        s(&dest),
        "--fixtures",
        s(&fixtures),
        "--reviewer",
        reviewer,
        "--note",
        note,
        "--approved-at",
        "2026-09-22T00:00:00Z",
        "--out",
        s(&migration),
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
        s(&migration),
        "--proposal",
        s(&dir.join("proposal.json")),
        "--source",
        s(&source),
        "--destination",
        s(&dest),
        "--fixtures",
        s(&fixtures),
        "--out",
        s(&verify_out),
    ]);
    assert_eq!(verify.status.code(), Some(1), "one fixture must fail");
    let (md, summary, html) = (
        dir.join("report.md"),
        dir.join("summary.md"),
        dir.join("report.html"),
    );
    let json = dir.join("report.json");
    let verify_path = verify_out.to_str().unwrap().to_string();
    let proposal = dir.join("proposal.json");
    let mut args = vec![
        "report",
        "--source",
        s(&source),
        "--destination",
        s(&dest),
        "--proposal",
        s(&proposal),
        "--html",
        s(&html),
        "--json",
        s(&json),
        "--md",
        s(&md),
        "--job-summary",
        s(&summary),
    ];
    // With a verdict the report re-runs verification (migration, proposal and
    // fixtures) and checks the verify file; without one it is a diff and
    // review report whose verdict is "not run".
    if with_verify {
        args.extend([
            "--migration",
            s(&migration),
            "--fixtures",
            s(&fixtures),
            "--verify",
            &verify_path,
        ]);
    }
    let report = run(&args);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    Outputs {
        markdown: fs::read_to_string(md).unwrap(),
        summary: fs::read_to_string(summary).unwrap(),
        html: fs::read_to_string(html).unwrap(),
    }
}

/// Lines a Markdown renderer would show as a forged verdict item.
fn forged_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("- fixtures: 7/7") || t.starts_with("- stable: yes")
        })
        .collect()
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in:\n{text}"))
}

#[test]
fn job_summary_counts_failures_and_cannot_be_forged_by_the_reviewer() {
    let out = forged_run("summary", true);
    let summary = &out.summary;
    assert!(summary.contains("fixtures: 6/7 passed"), "{summary}");
    assert!(summary.contains("stable: no"), "{summary}");
    assert!(forged_lines(summary).is_empty(), "{summary}");
    assert!(!summary.contains("<!--"), "{summary}");
    assert!(!summary.contains("<script"), "{summary}");
    // The verdict comes before the approval line, and the failing id is named.
    assert!(position(summary, "stable: no") < position(summary, "approval"));
    assert!(summary.contains("request-positive"), "{summary}");
}

#[test]
fn markdown_report_neutralizes_untrusted_text() {
    let out = forged_run("markdown", true);
    let md = &out.markdown;
    assert!(forged_lines(md).is_empty(), "{md}");
    assert!(!md.contains("<!--"), "{md}");
    assert!(!md.contains("<script"), "{md}");
    // The reviewer is still shown, on the one line it belongs to.
    let approval_line = md
        .lines()
        .find(|l| l.starts_with("Approval state:"))
        .unwrap_or_else(|| panic!("no approval line:\n{md}"));
    assert!(approval_line.contains("7/7 passed"), "{approval_line}");
}

#[test]
fn markdown_lists_failures() {
    let out = forged_run("markdown-verdict", true);
    let md = &out.markdown;
    let verdict = position(md, "6 of 7 passed");
    assert!(verdict < position(md, "Approval state"), "{md}");
    assert!(position(md, "Stable: no") < position(md, "Approval state"));
    let failing = md
        .lines()
        .find(|l| l.starts_with("Failing fixtures:"))
        .unwrap_or_else(|| panic!("no failing-fixtures line:\n{md}"));
    assert!(failing.contains("request-positive"), "{failing}");
}

#[test]
fn html_report_opens_with_the_verdict() {
    let out = forged_run("html", true);
    let html = &out.html;
    let verdict = position(html, "id=\"verdict\"");
    assert!(verdict < position(html, "Approval state"), "{html}");
    assert!(position(html, "6 of 7 passed") < position(html, "Approval state"));
    assert!(position(html, "not stable") < position(html, "Approval state"));
    assert!(position(html, "request-positive") < position(html, "Approval state"));
    assert!(
        html.contains("href=\"#verdict\""),
        "the nav links the verdict"
    );
}

#[test]
fn html_report_shows_control_characters_in_untrusted_fields() {
    let out = forged_run("html-controls", true);
    let html = &out.html;
    let start = position(html, "Approval state");
    let line_end = html[start..].find("</p>").unwrap() + start;
    let paragraph = &html[start..line_end];
    assert!(!paragraph.contains('\n'), "raw newline in {paragraph:?}");
    assert!(paragraph.contains("\\n- fixtures: 7/7"), "{paragraph:?}");
    assert!(!html.contains("<script>alert"), "the note stays escaped");
}

#[test]
fn reports_without_a_verify_result_say_the_verdict_is_not_run() {
    let out = forged_run("no-verify", false);
    assert!(
        position(&out.markdown, "Verdict: not run") < position(&out.markdown, "Approval state")
    );
    assert!(position(&out.html, "Verdict: not run") < position(&out.html, "Approval state"));
    assert!(out.summary.contains("stable: not run"), "{}", out.summary);
    assert!(forged_lines(&out.markdown).is_empty());
    assert!(forged_lines(&out.summary).is_empty());
}

#[test]
fn html_nav_links_resolve_to_sections() {
    let pack = root().join("packs/rename-user-id");
    let report = contract_flightplan::report::build_report(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    let links: Vec<&str> = html
        .split("href=\"#")
        .skip(1)
        .map(|s| &s[..s.find('"').unwrap()])
        .collect();
    assert!(links.len() >= 7, "{links:?}");
    for id in links {
        assert!(
            html.contains(&format!("id=\"{id}\"")),
            "nav link #{id} has no target"
        );
    }
}

#[test]
fn html_has_uncertainty_section_when_notes_exist() {
    let recursive = root().join("fixtures/negatives/recursive.openapi.yaml");
    let report =
        contract_flightplan::report::build_report(&recursive, &recursive, None, None, None, None)
            .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    let start = html
        .find("id=\"uncertainty\"")
        .expect("uncertainty section");
    let section = &html[start..start + html[start..].find("</section>").unwrap()];
    assert!(section.contains("recursive_reference"), "{section}");
    assert!(
        section.contains(
            "/paths/~1nodes/post/requestBody/content/application~1json/schema/properties/child"
        ),
        "{section}"
    );
}

/// Zero-width, joiner-like and bidi format characters would let two different
/// reviewer names render identically; every report shows them as escapes.
#[test]
fn invisible_and_bidi_characters_are_shown_as_escapes() {
    let hidden = [
        '\u{200b}',
        '\u{2060}',
        '\u{feff}',
        '\u{034f}',
        '\u{061c}',
        '\u{00ad}',
        '\u{2066}',
        '\u{fe0f}',
        '\u{e0041}',
    ];
    let reviewer: String = std::iter::once("alice".to_string())
        .chain(hidden.iter().map(|c| c.to_string()))
        .collect();
    let out = forged_run_with("invisible", true, &reviewer, "note");
    for (format, text) in [
        ("markdown", &out.markdown),
        ("job summary", &out.summary),
        ("html", &out.html),
    ] {
        for c in hidden {
            assert!(
                !text.contains(c),
                "{format} carries U+{:04X} unescaped",
                c as u32
            );
        }
        assert!(text.contains("200b"), "{format} shows no escape for U+200B");
    }
}
